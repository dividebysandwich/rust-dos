// A raindrop's streak or a snowflake, lit by the sky (and a lightning
// flash, which is in the sky's light).

in vec2 v_quad;
in float v_fade;
out vec4 o_color;

uniform int u_snow;
// The light from all around, and how much falls (0 to 1).
uniform vec3 u_ambient;
uniform float u_amount;

void main() {
    float shape;
    vec3 color;
    if (u_snow == 1) {
        shape = smoothstep(1.0, 0.3, length(v_quad));
        color = u_ambient * 1.6 + vec3(0.01);
    } else {
        shape = (1.0 - abs(v_quad.x)) * smoothstep(-1.0, 0.2, v_quad.y) * 0.35;
        color = u_ambient * 1.2 + vec3(0.006, 0.007, 0.009);
    }
    float alpha = shape * v_fade;
    if (alpha < 0.004) {
        discard;
    }
    o_color = finish(tone(color), alpha);
}
