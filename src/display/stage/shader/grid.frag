// The picture's colours in GRID_W by GRID_H patches, the light the screen
// gives: each the average over its patch, row 0 the top, in linear light,
// its alpha how much it takes over from the colours before. Column GRID_W
// is the whole picture's average.

out vec4 o_color;

uniform sampler2D u_screen;
uniform float u_take;

void main() {
    vec2 grid = vec2(float(GRID_W), float(GRID_H));
    // The patch's middle, (0, 0) the picture's top left, and the mipmap
    // with about two texels across it.
    vec2 middle = gl_FragCoord.xy / grid;
    vec2 size = vec2(textureSize(u_screen, 0));
    if (gl_FragCoord.x > grid.x) {
        // The smallest mipmap.
        vec3 all = from_srgb(textureLod(u_screen, vec2(0.5), 20.0).rgb);
        o_color = vec4(all, u_take);
        return;
    }
    float level = max(log2(max(size.x / grid.x, size.y / grid.y)) - 1.0, 0.0);
    vec3 sum = vec3(0.0);
    for (int i = 0; i < 4; i++) {
        vec2 at = middle + (vec2(float(i & 1), float(i >> 1)) - 0.5) * 0.5 / grid;
        sum += from_srgb(textureLod(u_screen, vec2(at.x, 1.0 - at.y), level).rgb);
    }
    o_color = vec4(sum / 4.0, u_take);
}
