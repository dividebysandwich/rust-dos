// The light bounced around at the probes, for the screen's picture now:
// a z layer of the probes' texture (see lit.frag's `u_gi`), from the sums
// the bake left (gi.rs's `Probes::texels`), a row for each probe.
// common.glsl's `u_sky` says whether the scene has a sky.

out vec4 o_color;

uniform sampler2D u_probes;
uniform sampler2D u_glow_grid;
uniform float u_glow;
uniform vec3 u_ambient;
uniform ivec3 u_count;
uniform int u_z;

#define PATCHES (GRID_W * GRID_H)
#define GROUPS ((PATCHES + 3) / 4)

void main() {
    ivec2 at = ivec2(gl_FragCoord.xy);
    int k = at.x / u_count.x;
    int index = at.x % u_count.x + u_count.x * (at.y + u_count.y * u_z);
    vec4 tint = texelFetch(u_probes, ivec2(6 + 6 * GROUPS, index), 0);
    vec4 own = texelFetch(u_probes, ivec2(k, index), 0);
    vec3 screen = vec3(0.0);
    for (int g = 0; g < GROUPS; g++) {
        vec4 t = texelFetch(u_probes, ivec2(6 + k * GROUPS + g, index), 0);
        for (int c = 0; c < 4; c++) {
            int p = g * 4 + c;
            if (p < PATCHES) {
                screen += texelFetch(u_glow_grid, ivec2(p % GRID_W, p / GRID_W), 0).rgb * t[c];
            }
        }
    }
    // The ambient light is the sky's, which comes in where it shows; or,
    // without a sky, what fills the room, from above more than below.
    float ambient = u_sky == 1 ? own.a : (k == 2 ? 1.0 : k == 3 ? 0.3 : 0.65);
    vec3 light = own.rgb + u_ambient * ambient + screen * u_glow * tint.rgb;
    o_color = vec4(light * tint.a, tint.a);
}
