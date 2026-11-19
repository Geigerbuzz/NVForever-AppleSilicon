//! Half-precision world lights (opt-in, `NVF_HALF_LIGHTS=1`): in a world view the clustered
//! point and spot lights' highlight is worked out in f16 where its values are small, and in f32
//! where they are not.
//!
//! Why: Metal GPU counters on the Strip at night, native 3024x1964 (2026-11-19) — ALU limiter
//! 45%, fragment occupancy 33%, F32 utilisation 11.4% against F16 0.8%: the main pass is
//! arithmetic-bound, all in f32. Apple GPUs run f16 at twice the rate with half the registers
//! [documented: Apple, "Metal Shading Language Specification" and "Optimize Metal Performance
//! for Apple silicon Macs" (WWDC 2020): use half precision where possible].
//!
//! What a world view's light is (the display edits, `display_shader.rs`, and lights.rs
//! `FRESNEL_EDIT`/`SPECULAR_EDITS`): Bevy's diffuse is 0 there (the display sum carries the
//! diffuse light), Fresnel is F0, and what is left is the GGX highlight with Bevy's sphere-light
//! fix and multiscatter compensation, times the light's colour as linear light
//! (`nvf_light_colour`) and the game's attenuation. This function is that, term for term:
//!
//! - f32: the light-to-fragment vector, distance, attenuation, the sphere light's closest point
//!   (`compute_specular_layer_values_for_point_light`), its solid angle, and the result.
//! - f16: N·L, N·H, the roughness remap, D_GGX (with |N×H|² for 1-(N·H)², which keeps the peak
//!   of a glossy highlight where 1-(N·H)² cancels in f16 [documented: Google Filament,
//!   "Physically Based Rendering in Filament", §4.4.2 "GGX in mediump"]), Smith-GGX correlated
//!   visibility (held below an f16-safe ceiling so a grazing angle cannot overflow to infinity
//!   and give NaN), F0 and the multiscatter factor.
//!
//! Light textures (cookies, `LIGHT_TEXTURES`) are not used by world lights; this path leaves
//! them out. Views without the world mark (the minigame views) keep Bevy's f32 light.

/// Helper functions, put in front of `apply_pbr_lighting` (after the display helpers).
pub(crate) const HALF_HELPERS: &str = "// NVForever (lights/half_shader.rs): a world view's point light highlight, f16 where safe.
@if(!PREPASS_FRAGMENT)
fn nvf_world_light_half(light_id: u32, input: ptr<function, lighting::LightingInput>) -> vec3<f32> {
    let light = &view_bindings::clustered_lights.data[light_id];
    let P = (*input).P;
    let light_to_frag = (*light).position_radius.xyz - P;
    let distance_square = dot(light_to_frag, light_to_frag);
    let attenuation = lighting::getDistanceAttenuation(distance_square, (*light).color_inverse_square_range.w);
    if attenuation <= 0.0 {
        return vec3(0.0);
    }
    let distance = sqrt(distance_square);
    let light_radius = (*light).position_radius.w;
    let spec = lighting::compute_specular_layer_values_for_point_light(input, 0u, (*input).V, light_to_frag, light_radius, distance);
    let solid_angle = light_radius * light_radius / max(distance_square, 1e-4);

    let N = vec3<f16>((*input).layers[0].N);
    let V = vec3<f16>((*input).V);
    let L = vec3<f16>(spec.xyz);
    let a = f16((*input).layers[0].roughness);
    let a_prime = max(f16(spec.w), 0.002h);
    let NdotV = f16(max((*input).layers[0].NdotV, 1e-4));
    let H = normalize(L + V);
    let NdotL = saturate(dot(N, L));
    let NdotH = saturate(dot(N, H));
    let normalization = a / a_prime;
    let specular_intensity = normalization * normalization;
    let inv = 1.0h - a;
    let r = mix(a, a_prime, 1.0h - inv * inv * inv * inv);
    // D_GGX with |N x H|^2 = 1 - (N.H)^2.
    let NxH = cross(N, H);
    let ah = NdotH * r;
    let k = r / max(dot(NxH, NxH) + ah * ah, 1e-4h);
    let D = min(k * k * 0.3183099h, 30000.0h);
    // Smith-GGX correlated visibility, held below an f16-safe ceiling.
    let r2 = r * r;
    let lambda_v = NdotL * sqrt((NdotV - r2 * NdotV) * NdotV + r2);
    let lambda_l = NdotV * sqrt((NdotL - r2 * NdotL) * NdotL + r2);
    let Vis = min(0.5h / max(lambda_v + lambda_l, 1e-4h), 1000.0h);
    let F0 = vec3<f16>(mix((*input).F0_dielectric, (*input).F0_metallic, (*input).metallic));
    let F_ab = vec2<f16>((*input).F_ab);
    let multi = 1.0h + F0 * (1.0h / max(F_ab.x + F_ab.y, 1e-3h) - 1.0h);
    // In f32 from here: D * Vis can reach 3e7.
    var highlight = vec3<f32>(F0 * multi) * (f32(specular_intensity) * f32(D) * f32(Vis));
    let nl = f32(NdotL);
    if light_radius > 0.0 {
        highlight *= saturate(nl / max(nl + solid_angle, 1e-4));
    }
    return highlight * nl * lighting::nvf_light_colour((*light).color_inverse_square_range.rgb) * attenuation;
}
";

/// `source` (bevy_pbr's `pbr_functions` after the display edits) with the f16 world light in
/// both light loops of `apply_pbr_lighting`, or `None` when an anchor is missing.
pub(crate) fn half_lights_patched(source: &str) -> Option<String> {
    const HEAD: &str = "@if(!PREPASS_FRAGMENT)\nfn nvf_display_lighting() -> bool {";
    const POINT: &str = "        let light_contrib = lighting::point_light(light_id, &lighting_input, enable_diffuse, true);";
    const SPOT: &str = "        let light_contrib = lighting::spot_light(light_id, &lighting_input, enable_diffuse);";
    if source.matches(HEAD).count() != 1 || source.matches(POINT).count() != 1 || source.matches(SPOT).count() != 1 {
        return None;
    }
    // The spot light's cone, as Bevy's spot_light and `nvf_spot_display` work it out.
    let spot_half = "        var light_contrib: vec3<f32>;
        if nvf_display {
            let nvf_spot = &view_bindings::clustered_lights.data[light_id];
            var nvf_spot_dir = vec3<f32>((*nvf_spot).light_custom_data.x, 0.0, (*nvf_spot).light_custom_data.y);
            nvf_spot_dir.y = sqrt(max(0.0, 1.0 - nvf_spot_dir.x * nvf_spot_dir.x - nvf_spot_dir.z * nvf_spot_dir.z));
            if ((*nvf_spot).flags & mesh_view_types::POINT_LIGHT_FLAGS_SPOT_LIGHT_Y_NEGATIVE) != 0u {
                nvf_spot_dir.y = -nvf_spot_dir.y;
            }
            let nvf_cd = dot(-nvf_spot_dir, normalize((*nvf_spot).position_radius.xyz - in.world_position.xyz));
            let nvf_cone = saturate(nvf_cd * (*nvf_spot).light_custom_data.z + (*nvf_spot).light_custom_data.w);
            light_contrib = nvf_world_light_half(light_id, &lighting_input) * (nvf_cone * nvf_cone);
        } else {
            light_contrib = lighting::spot_light(light_id, &lighting_input, enable_diffuse);
        }";
    let point_half = "        var light_contrib: vec3<f32>;
        if nvf_display {
            light_contrib = nvf_world_light_half(light_id, &lighting_input);
        } else {
            light_contrib = lighting::point_light(light_id, &lighting_input, enable_diffuse, true);
        }";
    let body = source.replacen(HEAD, &format!("{HALF_HELPERS}{HEAD}"), 1).replace(POINT, point_half).replace(SPOT, spot_half);
    Some(with_enable_after_imports(&body, "enable f16;"))
}

/// `source` with `directive` after its leading `import` statements: WESL takes imports
/// first, then directives, then declarations.
fn with_enable_after_imports(source: &str, directive: &str) -> String {
    let mut out = String::with_capacity(source.len() + directive.len() + 1);
    let mut in_import = false;
    let mut placed = false;
    for line in source.split_inclusive('\n') {
        if !placed {
            let t = line.trim();
            if in_import {
                if t.ends_with(';') {
                    in_import = false;
                }
            } else if t.starts_with("import ") || (t.starts_with('@') && t.contains(" import ")) {
                in_import = !t.ends_with(';');
            } else if !(t.is_empty() || t.starts_with("//")) {
                out.push_str(directive);
                out.push('\n');
                placed = true;
            }
        }
        out.push_str(line);
    }
    if !placed {
        out.push_str(directive);
        out.push('\n');
    }
    out
}
