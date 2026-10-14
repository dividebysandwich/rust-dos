// The rain's streaks and the snow's flakes, an instance each, a quad of
// two triangles from the vertex number: no vertex buffer. Each falls
// through a box around the eyes and comes in again at its top, so that
// there is always some around; not inside the room (`u_indoor`, which
// the fragments test: a streak's middle may be outside it, and its end
// inside).

uniform mat4 u_view_projection[VIEWS];
uniform vec3 u_eye[VIEWS];
// The box's size, and the time in seconds.
uniform vec3 u_box;
uniform float u_time;
// 1 snow, else rain.
uniform int u_snow;
out vec2 v_quad;
out float v_fade;
out vec3 v_world;

float hash(float n) {
    return fract(sin(n * 12.9898) * 43758.5453);
}

void main() {
    float id = float(gl_InstanceID);
    vec3 seed = vec3(hash(id + 0.1), hash(id + 7.3), hash(id + 19.7));
    // Rain falls fast and a little aslant; snow slowly, drifting.
    vec3 velocity = u_snow == 1
        ? vec3(0.25 * sin(u_time * 0.7 + id), -0.9 - 0.3 * seed.x, 0.25 * cos(u_time * 0.5 + id * 1.3))
        : vec3(0.8, -8.5 - 2.0 * seed.x, 0.3);
    vec3 eye = u_eye[0];
    vec3 travelled = seed * u_box + velocity * u_time;
    vec3 p = eye + mod(travelled - eye + u_box * 0.5, u_box) - u_box * 0.5;
    // Fading in at the box's edges, so that none pop.
    vec3 edge = abs(p - eye) / (u_box * 0.5);
    v_fade = 1.0 - smoothstep(0.75, 1.0, max(max(edge.x, edge.y), edge.z));
    const vec2 CORNERS[6] = vec2[](vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(1.0, 1.0), vec2(-1.0, -1.0), vec2(1.0, 1.0), vec2(-1.0, 1.0));
    vec2 corner = CORNERS[gl_VertexID];
    v_quad = corner;
    vec3 to_eye = normalize(u_eye[VIEW] - p);
    vec3 along, across;
    if (u_snow == 1) {
        across = normalize(cross(vec3(0.0, 1.0, 0.0), to_eye)) * 0.012;
        along = normalize(cross(to_eye, across)) * 0.012;
    } else {
        along = normalize(velocity) * 0.22;
        across = normalize(cross(along, to_eye)) * 0.004;
    }
    vec3 world = p + across * corner.x + along * corner.y;
    v_world = world;
    gl_Position = u_view_projection[VIEW] * vec4(world, 1.0);
}
