// The picture the card shows: its front buffer through the gamma table,
// a component at a time.

uniform sampler2D u_src;
uniform sampler2D u_lut;

out vec4 o_color;

void main() {
    ivec3 c = ivec3(round(texelFetch(u_src, ivec2(gl_FragCoord.xy), 0).rgb * 255.0));
    o_color = vec4(
        texelFetch(u_lut, ivec2(c.r, 0), 0).r,
        texelFetch(u_lut, ivec2(c.g, 0), 0).g,
        texelFetch(u_lut, ivec2(c.b, 0), 0).b,
        1.0);
}
