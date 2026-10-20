// A fastfill's colour as the card dithers it: the 5-6-5 values of a 4x4
// block of the card's pixels, by buffer row and column, widened to 8 bits
// a component as the card's picture widens them. Each card pixel is
// u_scale buffer pixels across.

uniform int u_pattern[16];
uniform float u_scale;

out vec4 o_color;

void main() {
    ivec2 pixel = ivec2(floor(gl_FragCoord.xy / u_scale));
    int v = u_pattern[((pixel.y & 3) << 2) | (pixel.x & 3)];
    int r = (v >> 11) & 31;
    int g = (v >> 5) & 63;
    int b = v & 31;
    o_color = vec4(ivec4((r << 3) | (r >> 2), (g << 2) | (g >> 4), (b << 3) | (b >> 2), 255)) / 255.0;
}
