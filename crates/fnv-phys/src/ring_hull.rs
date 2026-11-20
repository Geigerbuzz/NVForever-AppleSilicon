//! The character's cone-ended shape with its support point found directly
//! (docs/spec-notes/physics.md §6, the character's shape).
//!
//! The shape is the convex hull of four rings of [`crate::character`]'s `RING_POINTS`
//! points about the vertical axis, and the two points on the axis at its ends (192 + 2).
//! parry's hull finds the point farthest along a direction by trying every corner
//! [documented: parry3d 0.31.1, `ConvexPolyhedron::local_support_point` →
//! `point_cloud_support_point`], and every sweep of a character asks for it many times per
//! triangle met: at the Strip it was two thirds of the characters' movement, about 5 ms
//! of each frame's main thread with the player and seven walking residents [observed:
//! `sample`, M2 Pro, 2026-11-20].
//!
//! On a ring of n points at angles 2πi/n, the corner farthest along a direction is the one
//! nearest the direction's own angle about the axis: one of the two either side of it.
//! [`RingHull`] tries those two on each ring and the two axis points (at most ten corners,
//! not 194), and takes the farthest, the lowest-numbered of equals, which is the corner
//! parry's search returns, the same point. A direction within a thousandth of the axis
//! (where every corner of a ring is about as far as the next) is answered by parry's
//! search. Everything else (the faces for contacts, rays, distances, mass, boxes) is
//! parry's hull's own.
use rapier3d::math::{Pose, Real, Vector};
use rapier3d::parry::bounding_volume::{Aabb, BoundingSphere};
use rapier3d::parry::mass_properties::MassProperties;
use rapier3d::parry::query::{PointProjection, PointQuery, Ray, RayCast, RayIntersection};
use rapier3d::parry::shape::{ConvexPolyhedron, FeatureId, PolygonalFeature, PolygonalFeatureMap, Shape, ShapeType, SubShapeId, SupportMap, TypedShape};

/// A cone-ended character hull (see the module notes).
#[derive(Debug, Clone)]
pub struct RingHull {
    poly: ConvexPolyhedron,
    /// Points per ring.
    n: usize,
    /// By ring, by point: the point's index among the hull's corners (`None`: not a corner).
    rings: Vec<Vec<Option<u32>>>,
    /// The axis points that are corners.
    ends: Vec<u32>,
}

impl RingHull {
    /// The hull of `points` laid out as the character builds them: the two axis points,
    /// then for each angle i (2πi/n about the axis) one point of each of `levels` rings, in
    /// ring order. The hull is made of the points in that order, as before, so it is the
    /// same polyhedron (`character::capsule`). `None` when the points are degenerate, the
    /// layout does not match, or a corner of the hull is none of them.
    pub fn new(points: &[Vector], n: usize, levels: usize) -> Option<RingHull> {
        if n < 3 || levels == 0 || points.len() != 2 + n * levels {
            return None;
        }
        let poly = ConvexPolyhedron::from_convex_hull(points)?;
        let index = |p: Vector| poly.points().iter().position(|q| q.to_array() == p.to_array()).map(|i| i as u32);
        let rings: Vec<Vec<Option<u32>>> = (0..levels).map(|j| (0..n).map(|i| index(points[2 + i * levels + j])).collect()).collect();
        let ends: Vec<u32> = points[..2].iter().filter_map(|&p| index(p)).collect();
        let known = rings.iter().flatten().flatten().count() + ends.len();
        (known == poly.points().len()).then_some(RingHull { poly, n, rings, ends })
    }

    /// The hull itself.
    pub fn polyhedron(&self) -> &ConvexPolyhedron {
        &self.poly
    }
}

impl SupportMap for RingHull {
    fn local_support_point(&self, dir: Vector) -> Vector {
        let points = self.poly.points();
        let flat = dir.x.hypot(dir.y);
        if !(flat > 1e-3 * dir.length()) {
            return self.poly.local_support_point(dir);
        }
        let step = std::f32::consts::TAU / self.n as f32;
        let at = (dir.y.atan2(dir.x) / step).floor();
        let below = (at as i64).rem_euclid(self.n as i64) as usize;
        let above = (below + 1) % self.n;
        let mut best: Option<(Real, u32)> = None;
        let mut consider = |i: u32| {
            let d = points[i as usize].dot(dir);
            // As parry's search: the farthest, the first of equals.
            if best.is_none_or(|(bd, bi)| d > bd || (d == bd && i < bi)) {
                best = Some((d, i));
            }
        };
        for ring in &self.rings {
            for k in [below, above] {
                if let Some(i) = ring[k] {
                    consider(i);
                }
            }
        }
        for &i in &self.ends {
            consider(i);
        }
        best.map_or_else(|| self.poly.local_support_point(dir), |(_, i)| points[i as usize])
    }
}

impl PolygonalFeatureMap for RingHull {
    fn local_support_feature(&self, dir: Vector, out: &mut PolygonalFeature) {
        self.poly.local_support_feature(dir, out);
    }

    fn local_support_feature_toward(&self, dir: Vector, hint: Vector, out: &mut PolygonalFeature) {
        self.poly.local_support_feature_toward(dir, hint, out);
    }

    fn is_convex_polyhedron(&self) -> bool {
        self.poly.is_convex_polyhedron()
    }
}

impl RayCast for RingHull {
    fn cast_local_ray_and_get_normal(&self, ray: &Ray, max_time_of_impact: Real, solid: bool) -> Option<RayIntersection> {
        self.poly.cast_local_ray_and_get_normal(ray, max_time_of_impact, solid)
    }
}

impl PointQuery for RingHull {
    fn project_local_point(&self, point: Vector, solid: bool) -> PointProjection {
        self.poly.project_local_point(point, solid)
    }

    fn project_local_point_and_get_feature(&self, point: Vector) -> (PointProjection, FeatureId) {
        self.poly.project_local_point_and_get_feature(point)
    }
}

impl Shape for RingHull {
    fn compute_local_aabb(&self) -> Aabb {
        self.poly.compute_local_aabb()
    }

    fn compute_local_bounding_sphere(&self) -> BoundingSphere {
        self.poly.compute_local_bounding_sphere()
    }

    fn compute_aabb(&self, position: &Pose) -> Aabb {
        self.poly.compute_aabb(position)
    }

    fn clone_dyn(&self) -> Box<dyn Shape> {
        Box::new(self.clone())
    }

    fn scale_dyn(&self, scale: Vector, num_subdivisions: u32) -> Option<Box<dyn Shape>> {
        // A scaled ring is no longer a ring of the same angles in general: parry's hull.
        self.poly.scale_dyn(scale, num_subdivisions)
    }

    fn mass_properties(&self, density: Real) -> MassProperties {
        self.poly.mass_properties(density)
    }

    fn is_convex(&self) -> bool {
        true
    }

    fn shape_type(&self) -> ShapeType {
        ShapeType::Custom
    }

    fn as_typed_shape(&self) -> TypedShape<'_> {
        TypedShape::Custom(self)
    }

    fn ccd_thickness(&self) -> Real {
        self.poly.ccd_thickness()
    }

    fn ccd_angular_thickness(&self) -> Real {
        self.poly.ccd_angular_thickness()
    }

    fn as_support_map(&self) -> Option<&dyn SupportMap> {
        Some(self as &dyn SupportMap)
    }

    fn as_polygonal_feature_map(&self) -> Option<(&dyn PolygonalFeatureMap, Real)> {
        Some((self as &dyn PolygonalFeatureMap, 0.0))
    }

    fn feature_normal_at_point(&self, subshape: SubShapeId, feature: FeatureId, point: Vector) -> Option<Vector> {
        self.poly.feature_normal_at_point(subshape, feature, point)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The character's rings as `character::capsule` builds them (radius 20, height 120,
    /// flat 4, slope 1.58, unit scale), 48 points a ring.
    fn character_like(n: usize) -> Vec<Vector> {
        let (radius, height, flat, slope) = (20.0f32, 120.0f32, 4.0f32, 1.58f32);
        let half = height / 2.0;
        let cone = slope * (radius - flat);
        let r = radius / (std::f32::consts::PI / n as f32).cos();
        let tip = flat / (std::f32::consts::PI / n as f32).cos();
        let mut points = vec![Vector::new(0.0, 0.0, -half), Vector::new(0.0, 0.0, half)];
        for i in 0..n {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            for (rr, z) in [(tip, -half), (r, cone - half), (r, half - cone), (tip, half)] {
                points.push(Vector::new(rr * a.cos(), rr * a.sin(), z));
            }
        }
        points
    }

    #[test]
    fn the_support_point_is_parrys_for_every_direction() {
        for n in [8, 48] {
            let hull = RingHull::new(&character_like(n), n, 4).expect("a hull of known corners");
            // A dense sweep of directions, the ring angles and their midpoints exactly, the
            // axis and near it, and a fixed pseudo-random set.
            let mut dirs = Vec::new();
            for i in 0..720 {
                let a = i as f32 / 720.0 * std::f32::consts::TAU;
                for z in [-1.0, -0.7, -0.2, 0.0, 0.05, 0.3, 0.9, 1.0] {
                    dirs.push(Vector::new(a.cos(), a.sin(), z));
                }
            }
            for i in 0..2 * n {
                let a = i as f32 / (2 * n) as f32 * std::f32::consts::TAU;
                dirs.push(Vector::new(a.cos(), a.sin(), 0.25));
            }
            dirs.extend([Vector::Z, -Vector::Z, Vector::new(1e-5, 0.0, 1.0), Vector::new(0.0, -2e-3, -1.0), Vector::new(3e-3, 1e-3, 1.0)]);
            let mut s: u32 = 12345;
            let mut rnd = || {
                s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                (s >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
            };
            for _ in 0..5000 {
                dirs.push(Vector::new(rnd(), rnd(), rnd()) * 37.0);
            }
            for d in dirs {
                assert_eq!(hull.local_support_point(d).to_array(), hull.polyhedron().local_support_point(d).to_array(), "n {n}, direction {d:?}");
            }
        }
    }

    #[test]
    fn a_shape_cast_meets_the_same_ground() {
        use rapier3d::parry::query::{cast_shapes, ShapeCastOptions};
        use rapier3d::parry::shape::{Cuboid, TriMesh};
        let hull = RingHull::new(&character_like(48), 48, 4).unwrap();
        let ground = TriMesh::new(
            vec![Vector::new(-500.0, -500.0, 0.0), Vector::new(500.0, -500.0, 3.0), Vector::new(500.0, 500.0, 40.0), Vector::new(-500.0, 500.0, 10.0)],
            vec![[0, 1, 2], [0, 2, 3]],
        )
        .unwrap();
        let wall = Cuboid::new(Vector::new(10.0, 200.0, 200.0));
        let opts = ShapeCastOptions { max_time_of_impact: 400.0, target_distance: 0.5, stop_at_penetration: true, compute_impact_geometry_on_penetration: true };
        for (x, y, vx, vy, vz) in [(0.0, 0.0, 0.0, 0.0, -1.0), (37.0, -12.0, 0.3, 0.1, -1.0), (-80.0, 140.0, 1.0, 0.0, -0.2)] {
            let at = Pose::from_translation(Vector::new(x, y, 150.0));
            let v = Vector::new(vx, vy, vz);
            let ours = cast_shapes(&at, v, &hull, &Pose::IDENTITY, Vector::ZERO, &ground, opts).unwrap();
            let theirs = cast_shapes(&at, v, hull.polyhedron(), &Pose::IDENTITY, Vector::ZERO, &ground, opts).unwrap();
            assert_eq!(ours.map(|h| h.time_of_impact), theirs.map(|h| h.time_of_impact));
            let wall_at = Pose::from_translation(Vector::new(x + 120.0, y, 150.0));
            let side = Vector::X;
            let ours = cast_shapes(&at, side, &hull, &wall_at, Vector::ZERO, &wall, opts).unwrap();
            let theirs = cast_shapes(&at, side, hull.polyhedron(), &wall_at, Vector::ZERO, &wall, opts).unwrap();
            assert_eq!(ours.map(|h| h.time_of_impact), theirs.map(|h| h.time_of_impact));
        }
    }
}
