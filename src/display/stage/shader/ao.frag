// Ambient occlusion: how much of the light from all around reaches each
// pixel past what is close by, from the view's depth, at half its size
// (McGuire et al.'s "Alchemy" estimator). AO_SAMPLES are taken around each
// pixel, on a spiral turned differently at each, which the blur after
// smooths out.

out vec4 o_color;

uniform sampler2D u_depth;
uniform mat4 u_projection;
uniform mat4 u_inverse_projection;
uniform vec2 u_size;
// How far around occluders count, in metres, and how much they darken.
uniform float u_radius;
uniform float u_strength;

// The point seen at `uv`, in the view's space.
vec3 seen(vec2 uv) {
    float d = texture(u_depth, uv).r;
    vec4 p = u_inverse_projection * vec4(vec3(uv, d) * 2.0 - 1.0, 1.0);
    return p.xyz / p.w;
}

void main() {
    vec2 texel = 1.0 / u_size;
    vec2 uv = gl_FragCoord.xy * texel;
    if (texture(u_depth, uv).r >= 1.0) {
        o_color = vec4(1.0);
        return;
    }
    vec3 p = seen(uv);
    // The surface's facing, from the neighbours on the nearer side each
    // way, so that edges don't bend it.
    vec3 left = seen(uv - vec2(texel.x, 0.0));
    vec3 right = seen(uv + vec2(texel.x, 0.0));
    vec3 below = seen(uv - vec2(0.0, texel.y));
    vec3 above = seen(uv + vec2(0.0, texel.y));
    vec3 dx = abs(right.z - p.z) < abs(p.z - left.z) ? right - p : p - left;
    vec3 dy = abs(above.z - p.z) < abs(p.z - below.z) ? above - p : p - below;
    vec3 n = normalize(cross(dx, dy));
    // The radius on the picture, and the spiral's turn at this pixel.
    // Kept to a little of the view, so that the samples stay close
    // together in memory, which keeps them fast.
    vec2 reach = u_radius * vec2(u_projection[0][0], u_projection[1][1]) * 0.5 / max(-p.z, 0.05);
    reach *= min(1.0, 0.06 / max(reach.x, reach.y));
    float turn = 6.2831853 * fract(52.9829189 * fract(dot(gl_FragCoord.xy, vec2(0.06711056, 0.00583715))));
    float radius2 = u_radius * u_radius;
    float sum = 0.0;
    for (int i = 0; i < AO_SAMPLES; i++) {
        float along = (float(i) + 0.5) / float(AO_SAMPLES);
        float angle = turn + float(i) * 2.3999632;
        vec2 at = uv + vec2(cos(angle), sin(angle)) * reach * along;
        vec3 v = seen(at) - p;
        float vv = dot(v, v);
        float near = max(1.0 - vv / radius2, 0.0);
        sum += max(dot(v, n) - 0.01 * -p.z, 0.0) / (vv + 0.001) * near;
    }
    float ao = 1.0 - u_strength * u_radius * sum / float(AO_SAMPLES);
    o_color = vec4(clamp(ao, 0.0, 1.0));
}
