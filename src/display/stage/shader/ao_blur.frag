// The ambient occlusion smoothed along `u_step` (a texel across or down),
// over neighbours at much the same depth only, so that it doesn't spread
// over edges.

out vec4 o_color;

uniform sampler2D u_ao;
uniform sampler2D u_depth;
uniform mat4 u_inverse_projection;
uniform vec2 u_size;
uniform vec2 u_step;

// How far from the eye the point seen at `uv` is.
float distance_at(vec2 uv) {
    vec4 p = u_inverse_projection * vec4(0.0, 0.0, texture(u_depth, uv).r * 2.0 - 1.0, 1.0);
    return -p.z / p.w;
}

void main() {
    vec2 uv = gl_FragCoord.xy / u_size;
    float here = distance_at(uv);
    float sum = 0.0;
    float weights = 0.0;
    for (int i = -4; i <= 4; i++) {
        vec2 at = uv + u_step * float(i);
        float w = exp(-float(i * i) / 8.0) * exp(-abs(distance_at(at) - here) / (0.03 * here + 0.01));
        sum += texture(u_ao, at).r * w;
        weights += w;
    }
    o_color = vec4(sum / weights);
}
