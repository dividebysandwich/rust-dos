// A rectangle of the card's pixels (left, top, right, bottom) in a buffer
// of u_size of them, drawn as a strip of four vertices.

uniform vec4 u_rect;
uniform vec2 u_size;

void main() {
    vec2 corner = vec2(float(gl_VertexID & 1), float(gl_VertexID >> 1));
    gl_Position = vec4(mix(u_rect.xy, u_rect.zw, corner) / u_size * 2.0 - 1.0, 0.0, 1.0);
}
