// No CRT look: the frame as it is. The texture's filter, nearest or linear,
// is the `filter` setting.

uniform sampler2D u_frame;

in vec2 v_uv;
out vec4 o_color;

void main() {
    o_color = vec4(texture(u_frame, v_uv).rgb, 1.0);
}
