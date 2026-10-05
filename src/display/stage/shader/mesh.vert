// The scene's meshes, already in world coordinates.

in vec3 a_position;
in vec3 a_normal;
in vec2 a_uv;

uniform mat4 u_view_projection;

out vec3 v_world;
out vec3 v_normal;
out vec2 v_uv;

void main() {
    v_world = a_position;
    v_normal = a_normal;
    v_uv = a_uv;
    gl_Position = u_view_projection * vec4(a_position, 1.0);
}
