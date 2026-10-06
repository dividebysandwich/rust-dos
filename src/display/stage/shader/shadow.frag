// How far the nearest surface is, seen from a light: only the depth is
// written, where the surface isn't cut away.

in vec3 v_world;
in vec3 v_normal;
in vec2 v_uv;
out vec4 o_color;

uniform vec4 u_base_color;
uniform int u_has_base;
uniform sampler2D u_base;
uniform float u_cutoff;

void main() {
    if (u_cutoff >= 0.0) {
        float alpha = u_base_color.a * (u_has_base == 1 ? texture(u_base, v_uv).a : 1.0);
        if (alpha < u_cutoff) {
            discard;
        }
    }
    o_color = vec4(0.0);
}
