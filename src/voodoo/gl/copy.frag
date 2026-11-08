// A copy between colour buffers: each pixel from the one u_offset buffer
// pixels away in u_src, the source's resolved texture, and every sample
// of it alike.

uniform sampler2D u_src;
uniform ivec2 u_offset;

out vec4 o_color;

void main() {
    o_color = texelFetch(u_src, ivec2(gl_FragCoord.xy) + u_offset, 0);
}
