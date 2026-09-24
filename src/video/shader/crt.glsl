// The CRT looks. Before this come the look's #defines (see shader.rs):
//   MASK           0 none, 1 aperture grille, 2 slot mask
//   CURVED         1 for a curved tube with rounded corners, else 0
//   BEAM_MIN/MAX   the beam's width (sigma, in scanlines) at black and white
//   EDGE           how wide the step between two pixels is, in frame pixels
//   MASK_STRENGTH  how much of the light the mask takes, 0 to 1
//   SLOT_GAP       the light left in the gaps of the slot mask
//   GLOW           how much light spreads around bright parts
//   CURVATURE      how far the tube bends, across and down
//   OVERSCAN       how much of the picture the bezel covers
//   CORNER         the corners' radius, in picture heights
//   VIGNETTE       how much darker the edges are
// The light is mixed linearly and encoded again at the end.

uniform sampler2D u_frame;
// The frame's size in pixels. A frame row is a scanline: VGA double-scans
// the 200-line modes, and the frame has them doubled.
uniform vec2 u_source;
// The picture's size on the screen, in pixels.
uniform vec2 u_output;
// 1 for a colour tube's mask, 0 for a monochrome tube, which has none.
uniform float u_mask;

in vec2 v_uv;
out vec4 o_color;

// How far a screen pixel takes in light, as a Gaussian's sigma in pixels.
const float PIXEL_BLUR = 0.5;

vec3 decode(vec3 c) {
    return pow(c, vec3(2.2));
}

vec3 encode(vec3 c) {
    return pow(clamp(c, 0.0, 1.0), vec3(1.0 / 2.2));
}

// A frame pixel; black above and below the frame.
vec3 texel(float x, float row) {
    if (row < 0.0 || row >= u_source.y) {
        return vec3(0.0);
    }
    return decode(texelFetch(u_frame, ivec2(int(clamp(x, 0.0, u_source.x - 1.0)), int(row)), 0).rgb);
}

// A frame row at x: sharp steps between its pixels, EDGE frame pixels wide
// but never less than a screen pixel.
vec3 row_at(float row, float x) {
    float fx = x - 0.5;
    float x0 = floor(fx);
    float w = max(EDGE, u_source.x / u_output.x);
    float t = clamp((fx - x0 - 0.5) / w + 0.5, 0.0, 1.0);
    return mix(texel(x0, row), texel(x0 + 1.0, row), t);
}

// The light of a scanline of colour c, d scanlines from its middle, as a
// screen pixel sees it. Bright lines are wider than dark ones. The pixel
// takes in light from `blur` scanlines around its middle (a Gaussian too),
// which makes the beam wider and lower but keeps its light: lines too fine
// for the screen come out smooth instead of as moire.
vec3 beam(float d, vec3 c, float blur) {
    vec3 s = mix(vec3(BEAM_MIN), vec3(BEAM_MAX), sqrt(c));
    vec3 w = sqrt(s * s + blur * blur);
    vec3 z = d / w;
    return c * (s / w) * exp(-0.5 * z * z);
}

// The scanlines at pos, in frame pixels: the line there and its
// neighbours. The lines are moved by half a screen pixel so that at a whole
// number of screen pixels per line one of them is the line's middle;
// otherwise at 2x both would be halfway to the dark gap. Between whole
// numbers the lines drift across the pixels and would beat into moire
// waves, which the pixels' blur keeps faint. With fewer than two screen
// pixels per line they fade to the plain picture.
vec3 scanlines(vec2 pos) {
    float rows_per_pixel = u_source.y / u_output.y;
    float y = pos.y - 0.5 * rows_per_pixel;
    float n = floor(y + 0.5);
    float d = y - n;
    float blur = PIXEL_BLUR * rows_per_pixel;
#if !CURVED
    // At a whole number of screen pixels per line nothing beats, and the
    // lines stay sharp. (A curved tube has a little more or less anywhere.)
    float pixels = 1.0 / rows_per_pixel;
    blur *= smoothstep(0.0, 0.03, abs(pixels - floor(pixels + 0.5)));
#endif
    vec3 here = row_at(n, pos.x);
    vec3 sum = beam(d + 1.0, row_at(n - 1.0, pos.x), blur) + beam(d, here, blur)
        + beam(d - 1.0, row_at(n + 1.0, pos.x), blur);
    // A white line gives as much light as a white row.
    vec3 lit = sum / (BEAM_MAX * 2.5066);
    return mix(here, lit, smoothstep(1.25, 2.0, 1.0 / rows_per_pixel));
}

#if MASK != 0
// The phosphors at the screen pixel f: stripes of red, green and blue, a
// screen pixel each or more on big screens. The mask keeps the mean
// brightness, and fades out where a frame pixel is less than two screen
// pixels wide, too small for it.
vec3 mask(vec2 f) {
    float pixels = u_output.x / u_source.x;
    float s = MASK_STRENGTH * u_mask * smoothstep(1.25, 2.0, pixels);
    float w = max(1.0, floor(pixels / 3.0 + 0.5));
    int x = int(f.x / w);
    int phase = x - 3 * (x / 3);
    vec3 m = vec3(phase == 0, phase == 1, phase == 2);
    float lit = 1.0 / 3.0;
#if MASK == 2
    // The slots: every fourth row of a triad is dark, staggered between
    // neighbouring triads.
    int y = int(f.y / w) + 2 * ((x / 3) - 2 * (x / 6));
    if (y - 4 * (y / 4) == 0) {
        m *= SLOT_GAP;
    }
    lit *= 1.0 - 0.25 * (1.0 - SLOT_GAP);
#endif
    return mix(vec3(1.0), m, s) / (1.0 - s * (1.0 - lit));
}
#endif

// The light bright parts spread around them: a small mipmap of the frame,
// eight frame pixels a texel, blurred once more with a 3x3 tent.
vec3 glow(vec2 t) {
    vec2 r = 8.0 / u_source;
    vec3 g = vec3(0.0);
    for (int y = -1; y <= 1; y++) {
        for (int x = -1; x <= 1; x++) {
            float weight = (x == 0 ? 2.0 : 1.0) * (y == 0 ? 2.0 : 1.0);
            g += weight * decode(textureLod(u_frame, t + vec2(float(x), float(y)) * r, 3.0).rgb);
        }
    }
    return g / 16.0;
}

void main() {
#if CURVED
    // Where the curved glass shows the picture (Shader::warp in shader.rs
    // is the same), and the tube's rounded edge, smoothed over a pixel.
    vec2 c = (v_uv * 2.0 - 1.0) * OVERSCAN;
    c *= 1.0 + CURVATURE * c.yx * c.yx;
    vec2 t = c * 0.5 + 0.5;
    vec2 q = (abs(t - 0.5) - 0.5) * vec2(u_output.x / u_output.y, 1.0) + CORNER;
    float edge = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - CORNER;
    float inside = clamp(0.5 - edge / max(fwidth(edge), 1e-6), 0.0, 1.0);
    float vignette = pow(clamp(16.0 * t.x * t.y * (1.0 - t.x) * (1.0 - t.y), 1e-6, 1.0), VIGNETTE);
    float shade = inside * vignette;
#else
    vec2 t = v_uv;
    float shade = 1.0;
#endif
    vec3 light = scanlines(t * u_source);
#if MASK != 0
    light *= mask(gl_FragCoord.xy);
#endif
    light += GLOW * glow(t);
    o_color = vec4(encode(light * shade), 1.0);
}
