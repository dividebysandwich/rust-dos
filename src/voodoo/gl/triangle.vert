// The 3dfx card's triangles: vertices in the buffer's pixels, rows from
// the top, with the values the card iterates, which are linear across the
// screen as the card's iterators are.

in vec2 a_pos;
in vec4 a_color;
in vec2 a_zw;
in vec3 a_tex0;
in vec3 a_tex1;
// Each texture unit's level of detail without perspective, the
// triangle's.
in vec2 a_lod;
// Each unit's gradients of S, T and W across the screen and down it.
in vec3 a_dx0;
in vec3 a_dy0;
in vec3 a_dx1;
in vec3 a_dy1;

// The buffer's size in the card's pixels.
uniform vec2 u_size;

noperspective out vec4 v_color;
noperspective out vec2 v_zw;
noperspective out vec3 v_tex0;
noperspective out vec3 v_tex1;
flat out vec2 v_lod;
flat out vec3 v_dx0;
flat out vec3 v_dy0;
flat out vec3 v_dx1;
flat out vec3 v_dy1;

void main() {
    gl_Position = vec4(a_pos / u_size * 2.0 - 1.0, 0.0, 1.0);
    v_color = a_color;
    v_zw = a_zw;
    v_tex0 = a_tex0;
    v_tex1 = a_tex1;
    v_lod = a_lod;
    v_dx0 = a_dx0;
    v_dy0 = a_dy0;
    v_dx1 = a_dx1;
    v_dy1 = a_dy1;
}
