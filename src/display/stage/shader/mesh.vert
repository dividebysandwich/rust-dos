// The scene's meshes, in world coordinates already or moved there by
// u_model (the controllers and their beams). VIEWS views at once (both
// eyes, with GL_OVR_multiview2), VIEW this one.

in vec3 a_position;
in vec3 a_normal;
in vec2 a_uv;

uniform mat4 u_view_projection[VIEWS];
uniform mat4 u_model;

out vec3 v_world;
out vec3 v_normal;
out vec2 v_uv;
// The same depth in the depth program and the lit one.
invariant gl_Position;

void main() {
    vec4 world = u_model * vec4(a_position, 1.0);
    v_world = world.xyz;
    v_normal = mat3(u_model) * a_normal;
    v_uv = a_uv;
    gl_Position = u_view_projection[VIEW] * world;
}
