// GPU xBRZ at 2x. Each fragment computes its source pixel's 2x2 block
// from the surrounding texels and selects the output texel. Channels
// stay at whole numbers from 0 to 255 to preserve integer blend rounding.

uniform sampler2D u_frame;
uniform vec2 u_source;
uniform sampler2D u_overlay;
uniform bool u_overlay_enabled;

in vec2 v_uv;
out vec4 o_color;

const float CENTER_BIAS = 4.0;
const float DIR_THRESHOLD = 3.6;
const float STEEP_THRESHOLD = 2.2;
const float EQUAL_TOLERANCE = 30.0;

// The blend types of a corner: none, normal, dominant.
const int BLEND_NONE = 0;
const int BLEND_NORMAL = 1;
const int BLEND_DOMINANT = 2;

// The 3x3 cell that a rotated kernel reads for a cell, at [cell * 4 +
// rotation]. Cells are a to i, row by row, as 0 to 8.
const int ROT[36] = int[36](
    0, 0, 0, 0,
    1, 3, 7, 5,
    2, 0, 6, 8,
    3, 7, 5, 1,
    4, 4, 4, 4,
    5, 1, 3, 7,
    6, 8, 2, 0,
    7, 5, 1, 3,
    8, 2, 0, 6
);

// The colour in a texel, outside the frame transparent black.
vec4 texel(ivec2 p) {
    ivec2 size = ivec2(u_source);
    if (p.x < 0 || p.y < 0 || p.x >= size.x || p.y >= size.y) {
        return vec4(0.0);
    }
    return floor(texelFetch(u_frame, p, 0) * 255.0 + 0.5);
}

// Quantized YCbCr distance: channel differences are halved, truncated,
// then cut to steps of 8 and doubled.
float dist(vec4 p, vec4 q) {
    const float K_B = 0.0593;
    const float K_R = 0.2627;
    const float K_G = 1.0 - K_B - K_R;
    const float SCALE_B = 0.5 / (1.0 - K_B);
    const float SCALE_R = 0.5 / (1.0 - K_R);

    vec3 d = p.rgb - q.rgb;
    vec3 half_diff = sign(d) * floor(abs(d) * 0.5);
    vec3 c = 16.0 * floor(half_diff / 8.0);
    float y = K_R * c.r + K_G * c.g + K_B * c.b;
    float cb = SCALE_B * (c.b - y);
    float cr = SCALE_R * (c.r - y);
    float d_ycc = sqrt(y * y + cb * cb + cr * cr);
    float a1 = p.a / 255.0;
    float a2 = q.a / 255.0;
    if (a1 < a2) {
        return a1 * d_ycc + 255.0 * (a2 - a1);
    }
    return a2 * d_ycc + 255.0 * (a1 - a2);
}

bool same(vec4 p, vec4 q) {
    return dist(p, q) < EQUAL_TOLERANCE;
}

// The whole-number quotient of two whole numbers. A GPU's division can
// be a little off an exact one, so the quotient is checked against the
// products and moved by one where it is.
vec3 whole_div(vec3 num, float den) {
    vec3 q = floor(num / den);
    q += step(den * (q + 1.0), num);
    q -= step(num + 1.0, den * q);
    return q;
}

// `front` as m parts of n, and `back` as the rest, in whole channels.
vec4 gradient(vec4 front, vec4 back, float m, float n) {
    float weight_front = front.a * m;
    float weight_back = back.a * (n - m);
    float weight_sum = weight_front + weight_back;
    if (weight_sum == 0.0) {
        return vec4(0.0);
    }
    vec3 rgb = whole_div(front.rgb * weight_front + back.rgb * weight_back, weight_sum);
    float alpha = whole_div(vec3(weight_sum), n).x;
    return vec4(rgb, alpha);
}

// Classify blends at the corner between q, q + (1, 0), q + (0, 1) and
// q + (1, 1). Return (top_left, top_right, bottom_left, bottom_right)
// in x, y, z, w.
ivec4 corners(ivec2 q) {
    vec4 b = texel(q + ivec2(0, -1));
    vec4 c = texel(q + ivec2(1, -1));
    vec4 e = texel(q + ivec2(-1, 0));
    vec4 f = texel(q);
    vec4 g = texel(q + ivec2(1, 0));
    vec4 h = texel(q + ivec2(2, 0));
    vec4 i = texel(q + ivec2(-1, 1));
    vec4 j = texel(q + ivec2(0, 1));
    vec4 k = texel(q + ivec2(1, 1));
    vec4 l = texel(q + ivec2(2, 1));
    vec4 n = texel(q + ivec2(0, 2));
    vec4 o = texel(q + ivec2(1, 2));

    ivec4 r = ivec4(BLEND_NONE);
    if (f == g && j == k) {
        return r;
    }
    if (f == j && g == k) {
        return r;
    }

    float jg = dist(i, f) + dist(f, c) + dist(n, k) + dist(k, h)
        + CENTER_BIAS * dist(j, g);
    float fk = dist(e, j) + dist(j, o) + dist(b, g) + dist(g, l)
        + CENTER_BIAS * dist(f, k);

    if (jg < fk) {
        int mode = DIR_THRESHOLD * jg < fk ? BLEND_DOMINANT : BLEND_NORMAL;
        if (f != g && f != j) {
            r.x = mode;
        }
        if (k != j && k != g) {
            r.w = mode;
        }
    } else if (fk < jg) {
        int mode = DIR_THRESHOLD * fk < jg ? BLEND_DOMINANT : BLEND_NORMAL;
        if (j != f && j != k) {
            r.z = mode;
        }
        if (g != f && g != k) {
            r.y = mode;
        }
    }
    return r;
}

// The corner blends of source pixel p. At the left boundary, reuse the
// left kernel's upper-right blend and leave the lower-left blend empty.
ivec4 blend_of(ivec2 p) {
    ivec4 here = corners(p);
    ivec4 left = corners(p + ivec2(-1, 0));
    ivec4 above = corners(p + ivec2(0, -1));
    ivec4 blend;
    blend.x = p.x == 0 ? left.y : corners(p + ivec2(-1, -1)).w;
    blend.y = above.z;
    blend.z = p.x == 0 ? BLEND_NONE : left.y;
    blend.w = here.x;
    return blend;
}

// Rotate the corner blends with the pixel neighbourhood.
ivec4 rotate_blend(ivec4 b, int rot) {
    if (rot == 1) {
        return ivec4(b.z, b.x, b.w, b.y);
    }
    if (rot == 2) {
        return ivec4(b.w, b.z, b.y, b.x);
    }
    if (rot == 3) {
        return ivec4(b.y, b.w, b.x, b.z);
    }
    return b;
}

// A cell of the 3x3 neighbourhood n, as the rotation rot sees it.
vec4 cell(vec4 n[9], int name, int rot) {
    return n[ROT[name * 4 + rot]];
}

// The 2x2 block cell for the logical cell (i, j) of rotation rot.
int block_index(int i, int j, int rot) {
    if (rot == 1) {
        return (1 - j) * 2 + i;
    }
    if (rot == 2) {
        return (1 - i) * 2 + (1 - j);
    }
    if (rot == 3) {
        return j * 2 + (1 - i);
    }
    return i * 2 + j;
}

// Blend `col` into logical cell (i, j) of rotation rot.
void mix(inout vec4 blk[4], int rot, int i, int j, vec4 col, float m, float n) {
    int k = block_index(i, j, rot);
    blk[k] = gradient(col, blk[k], m, n);
}

// Blend one rotation of the pixel.
void blend_rotation(vec4 n[9], ivec4 blend, int rot, inout vec4 blk[4]) {
    ivec4 r = rotate_blend(blend, rot);
    if (r.w == BLEND_NONE) {
        return;
    }

    vec4 b = cell(n, 1, rot);
    vec4 c = cell(n, 2, rot);
    vec4 d = cell(n, 3, rot);
    vec4 e = cell(n, 4, rot);
    vec4 f = cell(n, 5, rot);
    vec4 g = cell(n, 6, rot);
    vec4 h = cell(n, 7, rot);
    vec4 i = cell(n, 8, rot);

    bool line;
    if (r.w == BLEND_DOMINANT) {
        line = true;
    } else if (r.y != BLEND_NONE && !same(e, g)) {
        line = false;
    } else if (r.z != BLEND_NONE && !same(e, c)) {
        line = false;
    } else if (!same(e, i) && same(g, h) && same(h, i) && same(i, f) && same(f, c)) {
        line = false;
    } else {
        line = true;
    }

    vec4 col = dist(e, f) <= dist(e, h) ? f : h;

    if (!line) {
        mix(blk, rot, 1, 1, col, 21.0, 100.0);
        return;
    }

    float fg = dist(f, g);
    float hc = dist(h, c);
    bool shallow = STEEP_THRESHOLD * fg <= hc && !same(e, g) && !same(d, g);
    bool steep = STEEP_THRESHOLD * hc <= fg && !same(e, c) && !same(b, c);

    if (shallow && steep) {
        mix(blk, rot, 1, 0, col, 1.0, 4.0);
        mix(blk, rot, 0, 1, col, 1.0, 4.0);
        mix(blk, rot, 1, 1, col, 5.0, 6.0);
    } else if (shallow) {
        mix(blk, rot, 1, 0, col, 1.0, 4.0);
        mix(blk, rot, 1, 1, col, 3.0, 4.0);
    } else if (steep) {
        mix(blk, rot, 0, 1, col, 1.0, 4.0);
        mix(blk, rot, 1, 1, col, 3.0, 4.0);
    } else {
        mix(blk, rot, 1, 1, col, 1.0, 2.0);
    }
}

void main() {
    if (u_overlay_enabled) {
        ivec2 size = textureSize(u_overlay, 0);
        ivec2 at = clamp(ivec2(v_uv * vec2(size)), ivec2(0), size - 1);
        vec4 overlay = texelFetch(u_overlay, at, 0);
        if (overlay.a > 0.0) {
            o_color = overlay;
            return;
        }
    }
    ivec2 o = ivec2(floor(v_uv * u_source * 2.0));
    ivec2 p = o / 2;

    vec4 n[9];
    n[0] = texel(p + ivec2(-1, -1));
    n[1] = texel(p + ivec2(0, -1));
    n[2] = texel(p + ivec2(1, -1));
    n[3] = texel(p + ivec2(-1, 0));
    n[4] = texel(p);
    n[5] = texel(p + ivec2(1, 0));
    n[6] = texel(p + ivec2(-1, 1));
    n[7] = texel(p + ivec2(0, 1));
    n[8] = texel(p + ivec2(1, 1));

    vec4 blk[4] = vec4[4](n[4], n[4], n[4], n[4]);
    ivec4 blend = blend_of(p);
    for (int rot = 0; rot < 4; rot++) {
        blend_rotation(n, blend, rot, blk);
    }
    // A quarter of a level more than the whole number, so that the
    // conversion to 8 bits lands on it whether it rounds or truncates.
    o_color = (blk[(o.y - 2 * p.y) * 2 + (o.x - 2 * p.x)] + 0.25) / 255.0;
}
