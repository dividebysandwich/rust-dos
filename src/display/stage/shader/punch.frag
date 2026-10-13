// How much of the real world shows through a window over the keyboard
// (passthrough): alpha 1 inside the window, falling to 0 over the feather
// around it. Blended to take that much of the colour and alpha away.

in vec3 v_world;
in vec3 v_normal;
in vec2 v_uv;
out vec4 o_color;

// The window's frame: world to its own metres, x across, z deep.
uniform mat4 u_local;
uniform vec2 u_half;
uniform float u_feather;

void main() {
    vec2 p = abs((u_local * vec4(v_world, 1.0)).xz);
    vec2 outside = max(p - u_half, 0.0);
    float through = 1.0 - smoothstep(0.0, u_feather, length(outside));
    o_color = vec4(0.0, 0.0, 0.0, through);
}
