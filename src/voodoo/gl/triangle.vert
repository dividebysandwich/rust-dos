// The 3dfx card's triangles: vertices in the buffer's pixels, rows from
// the top, with the values the card iterates, which are linear across the
// screen as the card's iterators are.

in vec2 a_pos;
in vec4 a_color;
in vec2 a_zw;
in vec3 a_tex0;
in vec3 a_tex1;

// The buffer's size in the card's pixels.
uniform vec2 u_size;

noperspective out vec4 v_color;
noperspective out vec2 v_zw;
noperspective out vec3 v_tex0;
noperspective out vec3 v_tex1;

void main() {
    gl_Position = vec4(a_pos / u_size * 2.0 - 1.0, 0.0, 1.0);
    v_color = a_color;
    v_zw = a_zw;
    v_tex0 = a_tex0;
    v_tex1 = a_tex1;
}
