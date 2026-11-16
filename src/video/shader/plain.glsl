// No CRT look: the frame as it is, scaled as `filter` says. The window
// and the web page filter the texture linearly for both values; the
// libretro core draws it at its own size with `u_sharp` 0.
//
// With `u_sharp` 1 (`filter=sharp`) each texel covers the same number of
// screen pixels, whole or not. A screen pixel inside a texel samples the
// texel's centre, where linear filtering gives that texel alone, and one
// across the edge between two texels mixes them in the share of its
// width each covers. That blend is less than one screen pixel wide, and
// at a whole-number scale no screen pixel crosses an edge, so the picture
// is what nearest sampling gives. With `u_sharp` 0 the texture is
// sampled where the pixel is (`filter=linear`, and the layer).

uniform sampler2D u_frame;
uniform vec2 u_source;
uniform vec2 u_output;
uniform int u_sharp;

in vec2 v_uv;
out vec4 o_color;

void main() {
    vec2 uv = v_uv;
    if (u_sharp != 0) {
        // Screen pixels per texel. Below 1 the picture is made smaller,
        // and plain linear sampling, which the remap gives there, does it.
        vec2 scale = max(u_output / u_source, vec2(1.0));
        vec2 texel = v_uv * u_source;
        vec2 base = floor(texel);
        // The distance from the texel's centre, in texels, and how far
        // from it a screen pixel stays wholly inside the texel.
        vec2 off = texel - base - 0.5;
        vec2 inner = 0.5 - 0.5 / scale;
        off = (off - clamp(off, -inner, inner)) * scale;
        uv = (base + 0.5 + off) / u_source;
    }
    o_color = vec4(texture(u_frame, uv).rgb, 1.0);
}
