// The sky behind everything: the direction through each pixel, from the
// inverse of the view's rotation and projection.

in vec2 v_uv;
out vec4 o_color;

uniform mat4 u_inverse[VIEWS];

void main() {
    vec2 ndc = vec2(v_uv.x * 2.0 - 1.0, (1.0 - v_uv.y) * 2.0 - 1.0);
    vec4 near = u_inverse[VIEW] * vec4(ndc, -1.0, 1.0);
    vec4 far = u_inverse[VIEW] * vec4(ndc, 1.0, 1.0);
    o_color = finish(tone(sky_color(far.xyz / far.w - near.xyz / near.w)), 1.0);
}
