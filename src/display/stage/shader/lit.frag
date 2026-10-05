// A mesh of the scene: lit by its lights, the sky and the screen's glow,
// fading into the sky with distance; or unlit; or the screen, which shows
// the emulated picture as it is.

in vec3 v_world;
in vec3 v_normal;
in vec2 v_uv;
out vec4 o_color;

// 0 lit, 1 unlit, 2 the screen, 3 the test room's floor.
uniform int u_shading;
uniform vec4 u_base_color;
uniform int u_has_base;
uniform sampler2D u_base;
uniform vec3 u_emissive;
uniform int u_has_emissive;
uniform sampler2D u_emissive_map;
// Alpha below which the surface is cut away, or below 0 for none.
uniform float u_cutoff;

uniform vec3 u_ambient;
uniform float u_exposure;
uniform vec3 u_eye;
uniform float u_fog;
uniform int u_lights;
// xyz the position, w the kind: 0 directional, 1 point, 2 spot.
uniform vec4 u_light_pos[8];
uniform vec3 u_light_color[8];
// Where the light shines to; a spot's cone as the cosines of its inner
// and outer angle.
uniform vec3 u_light_dir[8];
uniform vec2 u_light_cone[8];

// The picture on the screen, (0, 0) its bottom left; the screen's middle,
// facing and area, for the light it gives.
uniform sampler2D u_screen;
uniform int u_has_screen;
uniform vec3 u_glow_center;
uniform vec3 u_glow_normal;
uniform float u_glow_area;

void main() {
    vec4 base = u_base_color;
    if (u_has_base == 1) {
        base *= texture(u_base, v_uv);
    }
    if (u_cutoff >= 0.0 && base.a < u_cutoff) {
        discard;
    }
    if (u_shading == 2) {
        vec3 picture = u_has_screen == 1 ? from_srgb(texture(u_screen, vec2(v_uv.x, 1.0 - v_uv.y)).rgb) : vec3(0.0);
        o_color = finish(picture, 1.0);
        return;
    }
    vec3 emissive = u_emissive;
    if (u_has_emissive == 1) {
        emissive *= texture(u_emissive_map, v_uv).rgb;
    }
    vec3 color;
    if (u_shading == 1) {
        color = base.rgb + emissive;
    } else {
        vec3 n = normalize(v_normal);
        if (!gl_FrontFacing) {
            n = -n;
        }
        vec3 albedo = base.rgb;
        if (u_shading == 3) {
            // Lines a metre apart, thin at any distance, fading away.
            vec2 g = v_world.xz;
            vec2 a = abs(fract(g - 0.5) - 0.5) / max(fwidth(g), vec2(1e-4));
            float line = (1.0 - min(min(a.x, a.y), 1.0)) * exp(-length(v_world.xz - u_eye.xz) * 0.08);
            albedo = mix(albedo, vec3(0.05, 0.07, 0.12), line * 0.8);
            emissive += vec3(0.003, 0.006, 0.014) * line;
        }
        // The sky lights from above more than from below.
        vec3 light = u_ambient * (0.65 + 0.35 * n.y);
        for (int i = 0; i < 8; i++) {
            if (i >= u_lights) {
                break;
            }
            vec3 l;
            float att = 1.0;
            if (u_light_pos[i].w == 0.0) {
                l = -u_light_dir[i];
            } else {
                vec3 to = u_light_pos[i].xyz - v_world;
                float d2 = max(dot(to, to), 1e-4);
                l = to * inversesqrt(d2);
                att = 1.0 / d2;
                if (u_light_pos[i].w == 2.0) {
                    att *= smoothstep(u_light_cone[i].y, u_light_cone[i].x, dot(-l, u_light_dir[i]));
                }
            }
            light += u_light_color[i] * max(dot(n, l), 0.0) * att;
        }
        // The screen lights what is in front of it with its average
        // colour, the smallest of the picture's mipmaps.
        if (u_has_screen == 1 && u_glow_area > 0.0) {
            vec3 average = from_srgb(textureLod(u_screen, vec2(0.5), 20.0).rgb);
            vec3 to = u_glow_center - v_world;
            float d2 = dot(to, to) + u_glow_area * 0.25;
            vec3 l = normalize(to);
            float front = max(dot(-l, u_glow_normal), 0.0);
            light += average * u_glow_area * front * max(dot(n, l), 0.0) / (3.14159 * d2);
        }
        color = albedo * light + emissive;
    }
    color = tone(color * u_exposure);
    float dist = length(v_world - u_eye);
    color = mix(color, tone(sky_color(v_world - u_eye)), 1.0 - exp(-dist * u_fog));
    o_color = finish(color, base.a);
}
