// Depths of the card's pixels (red the low byte, green the high), each
// u_scale buffer pixels across: those with alpha, the others left.

uniform sampler2D u_src;
uniform float u_scale;

out vec4 o_color;

void main() {
    vec4 c = texelFetch(u_src, ivec2(gl_FragCoord.xy / u_scale), 0);
    if (c.a == 0.0) {
        discard;
    }
    gl_FragDepth = (round(c.r * 255.0) + round(c.g * 255.0) * 256.0) / 65535.0;
    o_color = vec4(0.0);
}
