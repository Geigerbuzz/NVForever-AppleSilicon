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
fn nvf_world_light_half(light_id: u32, P: vec3<f32>, N0: vec3<f32>, V0: vec3<f32>, R0: vec3<f32>, rough0: f32, NdotV0: f32, F0_32: vec3<f32>, F_ab32: vec2<f32>) -> vec3<f32> {
    let light = &view_bindings::clustered_lights.data[light_id];
    let light_to_frag = (*light).position_radius.xyz - P;
    let distance_square = dot(light_to_frag, light_to_frag);
    let attenuation = lighting::getDistanceAttenuation(distance_square, (*light).color_inverse_square_range.w);
    if attenuation <= 0.0 {
        return vec3(0.0);
    }
    let distance = sqrt(distance_square);
    let light_radius = (*light).position_radius.w;
    // The base layer's values come in as plain values (no pointer to LightingInput, no runtime
    // layer index: Bevy's helper indexes `layers` at run time through a pointer, which can keep
    // the whole struct in stack memory). Its sphere-light closest point, inlined, layer 0:
    let LtFdotR = max(0.0001, dot(light_to_frag, R0));
    let center_to_ray = LtFdotR * R0 - light_to_frag;
    let closest = light_to_frag + center_to_ray * saturate(light_radius * inverseSqrt(dot(center_to_ray, center_to_ray)));
    let spec = vec4(closest * inverseSqrt(dot(closest, closest)), saturate(rough0 + light_radius / (2.0 * distance)));
    let solid_angle = light_radius * light_radius / max(distance_square, 1e-4);

    let N = vec3<f16>(N0);
    let V = vec3<f16>(V0);
    let L = vec3<f16>(spec.xyz);
    let a = f16(rough0);
    let a_prime = max(f16(spec.w), 0.002h);
    let NdotV = f16(max(NdotV0, 1e-4));
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
    let F0 = vec3<f16>(F0_32);
    let F_ab = vec2<f16>(F_ab32);
    let multi = 1.0h + F0 * (1.0h / max(F_ab.x + F_ab.y, 1e-3h) - 1.0h);
    // In f32 from here: D * Vis can reach 3e7.
    var highlight = vec3<f32>(F0 * multi) * (f32(specular_intensity) * f32(D) * f32(Vis));
    let nl = f32(NdotL);
    if light_radius > 0.0 {
        highlight *= saturate(nl / max(nl + solid_angle, 1e-4));
    }
    return highlight * nl * NVF_HALF_COLOUR((*light).color_inverse_square_range.rgb) * attenuation;
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
            light_contrib = nvf_world_light_half(light_id, lighting_input.P, lighting_input.layers[0].N, lighting_input.V, lighting_input.layers[0].R, lighting_input.layers[0].roughness, lighting_input.layers[0].NdotV, mix(lighting_input.F0_dielectric, lighting_input.F0_metallic, lighting_input.metallic), lighting_input.F_ab) * (nvf_cone * nvf_cone);
        } else {
            light_contrib = lighting::spot_light(light_id, &lighting_input, enable_diffuse);
        }";
    let point_half = "        var light_contrib: vec3<f32>;
        if nvf_display {
            light_contrib = nvf_world_light_half(light_id, lighting_input.P, lighting_input.layers[0].N, lighting_input.V, lighting_input.layers[0].R, lighting_input.layers[0].roughness, lighting_input.layers[0].NdotV, mix(lighting_input.F0_dielectric, lighting_input.F0_metallic, lighting_input.metallic), lighting_input.F_ab);
        } else {
            light_contrib = lighting::point_light(light_id, &lighting_input, enable_diffuse, true);
        }";
    let body = source.replacen(HEAD, &format!("{HALF_HELPERS}{HEAD}"), 1).replace(POINT, point_half).replace(SPOT, spot_half);
    // Diagnostic: `NVF_DEBUG_LIGHT_NO_POW=1` takes the light's colour as it is (the wrong
    // tint), to measure what `nvf_light_colour`'s three pow calls per light and pixel cost.
    let colour = if std::env::var("NVF_DEBUG_LIGHT_NO_POW").is_ok_and(|v| v == "1") { "" } else { "lighting::nvf_light_colour" };
    let body = body.replace("NVF_HALF_COLOUR", colour);
    // Diagnostic: `NVF_DEBUG_LIGHT_LOAD_ONLY=1` reads each light's record (position, range,
    // colour) and returns a faint colour × attenuation, skipping the highlight's arithmetic:
    // it tells the cost of fetching the lights apart from the cost of shading them.
    let body = if std::env::var("NVF_DEBUG_LIGHT_LOAD_ONLY").is_ok_and(|v| v == "1") {
        body.replacen(
            "    let distance = sqrt(distance_square);\n",
            "    if true {\n        return (*light).color_inverse_square_range.rgb * (attenuation * 1e-4);\n    }\n    let distance = sqrt(distance_square);\n",
            1,
        )
    } else {
        body
    };
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
