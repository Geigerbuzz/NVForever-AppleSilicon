// Cloud shadows (cloud_shadows.rs; `--cloud-shadows`, off by default): the tonemapped
// picture times one coarse, world-projected mask, one texture fetch a pixel. The depth
// prepass gives each pixel's place in the world (reverse infinite z: distance = near / d),
// so the mask stays on the ground and on objects as the camera moves. An engineering
// choice of NVForever's own, not the original game's (which casts no cloud shadows).

import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput;
import nvf_assets::first_person_depth::unsqueeze;

struct CloudShadows {
    // Clip space of this frame to world space.
    world_from_clip: mat4x4<f32>,
    // x: strength now (0: none), y unused, z: world units per mask repeat, w: the lowest
    // cloud layer's ONAM offset (texture repeats).
    params: vec4<f32>,
    // x, z: the wind (unit, world XZ); y: how far it has carried the mask (game units).
    wind: vec4<f32>,
    // xyz: the camera; w: the distance beyond which the depth prepass is the sky dome.
    camera: vec4<f32>,
}

@group(0) @binding(0) var picture: texture_2d<f32>;
@group(0) @binding(1) var picture_sampler: sampler;
@group(0) @binding(2) var<uniform> shadows: CloudShadows;
@if(MULTISAMPLED) @group(0) @binding(3) var depth: texture_depth_multisampled_2d;
@if(!MULTISAMPLED) @group(0) @binding(3) var depth: texture_depth_2d;
@group(0) @binding(4) var mask: texture_2d<f32>;
@group(0) @binding(5) var mask_sampler: sampler;

@fragment
fn apply(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let dims = vec2<i32>(textureDimensions(picture));
    let p = clamp(vec2<i32>(floor(in.position.xy)), vec2<i32>(0), dims - 1);
    let src = textureLoad(picture, p, 0);
    if shadows.params.x <= 0.0 {
        return src;
    }
    let uv = (vec2<f32>(p) + 0.5) / vec2<f32>(dims);
    // An upscaler may hold the depth prepass at another size (ssao.rs); the uv picks the
    // texel then.
    let dd = vec2<f32>(textureDimensions(depth));
    let dp = clamp(vec2<i32>(uv * dd), vec2<i32>(0), vec2<i32>(dd) - 1);
    // The depth prepass's value (reverse z: 0 is the far plane); the first-person arms are
    // at the depth they were squeezed from.
    let d = unsqueeze(textureLoad(depth, dp, 0));
    if d <= 0.0 {
        return src;
    }
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    let h = shadows.world_from_clip * vec4<f32>(ndc, d, 1.0);
    let world = h.xyz / h.w;
    // The sky dome is opaque and in the prepass: no cloud shadow on the sky itself.
    if distance(world, shadows.camera.xyz) > shadows.camera.w {
        return src;
    }
    // World XZ, the lowest layer's own scroll along u, and the wind carrying it (the
    // pattern drifts downwind, as the cloud does).
    let mask_uv = world.xz / shadows.params.z
        + vec2<f32>(shadows.params.w, 0.0)
        - shadows.wind.xz * (shadows.wind.y / shadows.params.z);
    let a = textureSampleLevel(mask, mask_sampler, mask_uv, 0.0).r;
    return vec4(src.rgb * (1.0 - a * shadows.params.x), src.a);
}
