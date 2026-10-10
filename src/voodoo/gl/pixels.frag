// Pixels of the card's size, each u_scale buffer pixels across: those
// with alpha, the others left as they are.

uniform sampler2D u_src;
uniform float u_scale;

out vec4 o_color;

void main() {
    vec4 c = texelFetch(u_src, ivec2(gl_FragCoord.xy / u_scale), 0);
    if (c.a == 0.0) {
        discard;
    }
    o_color = vec4(c.rgb, 1.0);
}
