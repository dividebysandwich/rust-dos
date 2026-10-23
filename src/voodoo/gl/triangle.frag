// The 3dfx card's pixel pipeline for a triangle's pixel, as the software
// rasterizer has it (src/voodoo/raster.rs) in the card's integers: the
// texture units' lookups and combines, the colour and alpha combine,
// chroma key, alpha mask and test, fog, the depth value and, with
// u_dither, the dithering to 5-6-5. The depth test, blending and the write
// masks are OpenGL's own, set for each draw.
//
// The buffers have 8 bits a component. Where fbzMode bit 8 dithers and
// u_dither is on, the colour is dithered to 5-6-5 with the card's matrix
// and rounding (`tables::dither_rb`) and widened again as the card's
// picture widens its pixels, so an unblended pixel is the one the software
// rasterizer writes. The matrix is indexed by the card's pixel, so above
// 1x each cell covers u_scale by u_scale buffer pixels. OpenGL blends
// after the shader, so a blended pixel is dithered before the blend
// instead of after it, and isn't the card's: the card dithers the blend's
// result (and with fbzMode bit 19 takes the dither back out of the colour
// it reads first). The texture units' LOD dither isn't here either, as
// OpenGL picks the mipmap levels.

noperspective in vec4 v_color;
noperspective in vec2 v_zw;
noperspective in vec3 v_tex0;
noperspective in vec3 v_tex1;

out vec4 o_color;

// The registers, as their bits.
uniform int u_fbzcp;
uniform int u_fbz;
uniform int u_alpha;
uniform int u_fog;
uniform int u_color0;
uniform int u_color1;
uniform int u_chroma;
uniform int u_chroma_range;
uniform int u_zacolor;
uniform int u_fogcolor;
uniform int u_stipple;
uniform int u_yorigin;
uniform int u_fogblend[64];
uniform int u_fogdelta[64];
// Compare against zaColor in the colour pass, then write the interpolated
// depth in a stencil-masked depth pass when fbzMode bit 20 is set.
uniform int u_constant_depth;
// Buffer pixels a card pixel is across.
uniform float u_scale;
// Whether to dither as the card does where fbzMode says so
// (`voodoo_dither`).
uniform int u_dither;

// Bit 0 and 1: texture unit 0 and 1 draw; bit 2: unit 0 hands out its
// configuration (u_config) instead.
uniform int u_units;
uniform int u_config;
uniform sampler2D u_tex0;
uniform sampler2D u_tex1;
uniform int u_tmode0;
uniform int u_tmode1;
// Level 0's size in texels, and the LOD bias in levels.
uniform vec3 u_tsize0;
uniform vec3 u_tsize1;
// lodmin, lodmax, lodbias (8.8), and the detail factor's bias, maximum
// and scale.
uniform ivec3 u_tlod0;
uniform ivec3 u_tlod1;
uniform ivec3 u_tdetail0;
uniform ivec3 u_tdetail1;

bool bit(int v, int n) {
    return ((v >> n) & 1) != 0;
}

// The card's dither matrices (`tables::DITHER_MATRIX_4X4` and `_2X2`), by
// row and column.
const int DITHER4[16] = int[16](0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5);
const int DITHER2[16] = int[16](2, 10, 2, 10, 14, 6, 14, 6, 2, 10, 2, 10, 14, 6, 14, 6);

// The card's pixel the fragment is in.
ivec2 cardPixel() {
    return ivec2(floor(gl_FragCoord.xy / u_scale));
}

// A pixel's row as the card counts rows when it iterates them, before the
// Y origin flips them: what the stipple and dither patterns go by.
int iteratedRow(ivec2 pixel) {
    return bit(u_fbz, 17) ? u_yorigin - pixel.y : pixel.y;
}

// Where in the dither matrix a pixel is.
int ditherIndex(ivec2 pixel) {
    return ((iteratedRow(pixel) & 3) << 2) | (pixel.x & 3);
}

bool dithers() {
    return u_dither != 0 && bit(u_fbz, 8);
}

// A colour dithered to 5-6-5 and widened back to 8 bits a component.
ivec3 dither565(ivec3 c, int d) {
    int r = (((c.r << 1) - (c.r >> 4) + (c.r >> 7) + d) >> 1) >> 3;
    int g = (((c.g << 2) - (c.g >> 4) + (c.g >> 6) + d) >> 2) >> 2;
    int b = (((c.b << 1) - (c.b >> 4) + (c.b >> 7) + d) >> 1) >> 3;
    return ivec3((r << 3) | (r >> 2), (g << 2) | (g >> 4), (b << 3) | (b >> 2));
}

// A register's colour: red, green, blue and alpha.
ivec4 argb(int c) {
    return ivec4((c >> 16) & 255, (c >> 8) & 255, c & 255, (c >> 24) & 255);
}

// The chroma key test: the key, or a Voodoo2's range test with chromaRange
// bit 28 (each component between the key's and the range's, or outside
// with its exclusive bit; all of them, or any in union mode).
bool chromaRejects(ivec3 c) {
    ivec3 key = argb(u_chroma).rgb;
    if (!bit(u_chroma_range, 28)) {
        return c == key;
    }
    ivec3 high = argb(u_chroma_range).rgb;
    int results = 0;
    for (int i = 0; i < 3; i++) {
        bool inside = key[i] <= c[i] && c[i] <= high[i];
        if (inside != bit(u_chroma_range, 26 - i)) {
            results++;
        }
    }
    return bit(u_chroma_range, 27) ? results != 0 : results == 3;
}

// An iterated colour component as 8 bits: the integer part wraps, with the
// card's cases for -1 and 256 (CLAMPED_ARGB).
int clampedColor(float f) {
    int v = int(floor(f));
    if (bit(u_fbzcp, 28)) {
        return clamp(v, 0, 255);
    }
    v &= 0xFFF;
    if (v == 0xFFF) {
        return 0;
    }
    if (v == 0x100) {
        return 0xFF;
    }
    return v & 0xFF;
}

int clampedZ(float z) {
    int v = int(floor(z));
    if (bit(u_fbzcp, 28)) {
        return clamp(v, 0, 0xFFFF);
    }
    v &= 0xFFFFF;
    if (v == 0xFFFFF) {
        return 0;
    }
    if (v == 0x10000) {
        return 0xFFFF;
    }
    return v & 0xFFFF;
}

// W's integer part as 8 bits.
int clampedW(float w) {
    int v = (int(floor(w)) << 16) >> 16;
    if (bit(u_fbzcp, 28)) {
        return clamp(v, 0, 0xFF);
    }
    v &= 0xFFFF;
    if (v == 0xFFFF) {
        return 0;
    }
    if (v == 0x100) {
        return 0xFF;
    }
    return v & 0xFF;
}

// A 32-bit fraction as the card's 4.12 floating point: the leading zeros,
// then the inverted bits after the first one.
int float412(uint temp) {
    if ((temp & 0xFFFF0000u) == 0u) {
        return 0xFFFF;
    }
    int exp = 0;
    while (exp < 15 && (temp & (0x80000000u >> uint(exp))) == 0u) {
        exp++;
    }
    int v = (exp << 12) | int((~temp >> uint(19 - exp)) & 0xFFFu);
    return v < 0xFFFF ? v + 1 : v;
}

int wfloat(float w) {
    if (w < 0.0 || w >= 1.0) {
        return 0;
    }
    return float412(uint(w * 4294967296.0));
}

int depthValue() {
    int d;
    if (!bit(u_fbz, 3)) {
        d = clampedZ(v_zw.x);
    } else if (!bit(u_fbz, 21)) {
        d = wfloat(v_zw.y);
    } else if (v_zw.x < 0.0 || v_zw.x >= 65536.0) {
        d = 0;
    } else {
        d = float412(uint(v_zw.x * 65536.0));
    }
    if (bit(u_fbz, 16)) {
        d = clamp(d + ((u_zacolor << 16) >> 16), 0, 0xFFFF);
    }
    return d;
}

// A texture unit's texel and the level of detail it was read at (8.8).
ivec4 fetch(sampler2D tex, vec3 stw, int mode, vec3 size, ivec3 lod, out int level) {
    vec2 st = bit(mode, 0) ? stw.xy / stw.z : stw.xy;
    if (bit(mode, 3) && stw.z < 0.0) {
        st = vec2(0.0);
    }
    // How many level 0 texels a pixel of the card's spans.
    vec2 dx = dFdx(st) * u_scale;
    vec2 dy = dFdy(st) * u_scale;
    float rho = max(dot(dx, dx), dot(dy, dy));
    level = clamp(int(log2(max(rho, 1e-12)) * 128.0) + lod.z, lod.x, lod.y);
    vec4 c = texture(tex, st / size.xy, size.z);
    return ivec4(round(c * 255.0));
}

// A texture unit's combine of its texel (local) with the unit before it
// (other).
ivec4 combineTexture(int mode, ivec4 local, ivec4 other, int level, ivec3 detail) {
    ivec3 c = !bit(mode, 12) ? other.rgb : ivec3(0);
    int a = !bit(mode, 21) ? other.a : 0;
    if (bit(mode, 13)) {
        c -= local.rgb;
    }
    if (bit(mode, 22)) {
        a -= local.a;
    }
    int factor = detail.x <= level ? 0 : min(((detail.x - level) << detail.z) >> 8, detail.y);
    ivec3 b = ivec3(0);
    int select = (mode >> 14) & 7;
    if (select == 1) {
        b = local.rgb;
    } else if (select == 2) {
        b = ivec3(other.a);
    } else if (select == 3) {
        b = ivec3(local.a);
    } else if (select == 4) {
        b = ivec3(factor);
    } else if (select == 5) {
        b = ivec3(level & 0xFF);
    }
    int ba = 0;
    select = (mode >> 23) & 7;
    if (select == 1 || select == 3) {
        ba = local.a;
    } else if (select == 2) {
        ba = other.a;
    } else if (select == 4) {
        ba = factor;
    } else if (select == 5) {
        ba = level & 0xFF;
    }
    if (!bit(mode, 17)) {
        b ^= ivec3(0xFF);
    }
    if (!bit(mode, 26)) {
        ba ^= 0xFF;
    }
    c = (c * (b + 1)) >> 8;
    a = (a * (ba + 1)) >> 8;
    int add = (mode >> 18) & 3;
    if (add == 1) {
        c += local.rgb;
    } else if (add == 2) {
        c += ivec3(local.a);
    }
    if (((mode >> 27) & 3) != 0) {
        a += local.a;
    }
    ivec4 r = clamp(ivec4(c, a), 0, 255);
    if (bit(mode, 20)) {
        r.rgb ^= ivec3(0xFF);
    }
    if (bit(mode, 29)) {
        r.a ^= 0xFF;
    }
    return r;
}

// The colour combine unit on c_other and c_local.
ivec4 combineColor(ivec4 other, ivec4 local, ivec4 texel) {
    int cp = u_fbzcp;
    ivec3 c = !bit(cp, 8) ? other.rgb : ivec3(0);
    int a = !bit(cp, 17) ? other.a : 0;
    if (bit(cp, 9)) {
        c -= local.rgb;
    }
    if (bit(cp, 18)) {
        a -= local.a;
    }
    ivec3 b = ivec3(0);
    int select = (cp >> 10) & 7;
    if (select == 1) {
        b = local.rgb;
    } else if (select == 2) {
        b = ivec3(other.a);
    } else if (select == 3) {
        b = ivec3(local.a);
    } else if (select == 4) {
        b = ivec3(texel.a);
    } else if (select == 5) {
        b = texel.rgb;
    }
    int ba = 0;
    select = (cp >> 19) & 7;
    if (select == 1 || select == 3) {
        ba = local.a;
    } else if (select == 2) {
        ba = other.a;
    } else if (select == 4) {
        ba = texel.a;
    }
    if (!bit(cp, 13)) {
        b ^= ivec3(0xFF);
    }
    if (!bit(cp, 22)) {
        ba ^= 0xFF;
    }
    c = (c * (b + 1)) >> 8;
    a = (a * (ba + 1)) >> 8;
    int add = (cp >> 14) & 3;
    if (add == 1) {
        c += local.rgb;
    } else if (add == 2) {
        c += ivec3(local.a);
    }
    if (((cp >> 23) & 3) != 0) {
        a += local.a;
    }
    ivec4 r = clamp(ivec4(c, a), 0, 255);
    if (bit(cp, 16)) {
        r.rgb ^= ivec3(0xFF);
    }
    if (bit(cp, 25)) {
        r.a ^= 0xFF;
    }
    return r;
}

bool alphaTest(int a) {
    int reference = (u_alpha >> 24) & 0xFF;
    int f = (u_alpha >> 1) & 7;
    if (f == 0) {
        return false;
    } else if (f == 1) {
        return a < reference;
    } else if (f == 2) {
        return a == reference;
    } else if (f == 3) {
        return a <= reference;
    } else if (f == 4) {
        return a > reference;
    } else if (f == 5) {
        return a != reference;
    } else if (f == 6) {
        return a >= reference;
    }
    return true;
}

ivec3 fog(ivec3 c, int itera, ivec2 pixel) {
    ivec3 fc = argb(u_fogcolor).rgb;
    ivec3 f;
    if (bit(u_fog, 5)) {
        f = fc;
    } else {
        ivec3 t = !bit(u_fog, 1) ? fc : ivec3(0);
        if (!bit(u_fog, 2)) {
            t -= c;
        }
        int blend;
        int source = (u_fog >> 3) & 3;
        if (source == 0) {
            int w = wfloat(v_zw.y);
            int i = w >> 10;
            // The delta comes masked and signed (`DrawState::fogdelta`).
            int deltaval = u_fogdelta[i] * ((w >> 2) & 0xFF);
            int delta = deltaval >> 10;
            // fogMode bit 6 dithers the delta with the 4x4 matrix.
            if (dithers() && bit(u_fog, 6)) {
                delta = ((deltaval >> 6) + DITHER4[ditherIndex(pixel)]) >> 4;
            }
            blend = u_fogblend[i] + delta;
        } else if (source == 1) {
            blend = itera;
        } else if (source == 2) {
            blend = clampedZ(v_zw.x) >> 8;
        } else {
            blend = clampedW(v_zw.y);
        }
        f = (t * (blend + 1)) >> 8;
    }
    return clamp(!bit(u_fog, 2) ? c + f : f, 0, 255);
}

void main() {
    ivec4 iter = ivec4(clampedColor(v_color.r), clampedColor(v_color.g), clampedColor(v_color.b), clampedColor(v_color.a));

    // The texture units, TMU 1 first, whose output TMU 0 combines with.
    // Both are read before any pixel is dropped, for their derivatives.
    int level0 = 0;
    int level1 = 0;
    ivec4 texel0 = ivec4(0);
    ivec4 texel1 = ivec4(0);
    if (bit(u_units, 1)) {
        texel1 = fetch(u_tex1, v_tex1, u_tmode1, u_tsize1, u_tlod1, level1);
    }
    if (bit(u_units, 0)) {
        texel0 = fetch(u_tex0, v_tex0, u_tmode0, u_tsize0, u_tlod0, level0);
    }
    ivec4 texel = ivec4(0);
    if (bit(u_units, 1)) {
        texel = combineTexture(u_tmode1, texel1, texel, level1, u_tdetail1);
    }
    if (bit(u_units, 2)) {
        texel = argb(u_config);
    } else if (bit(u_units, 0)) {
        texel = combineTexture(u_tmode0, texel0, texel, level0, u_tdetail0);
    }

    // Stippling with the pattern (the rotating one draws every pixel).
    ivec2 pixel = cardPixel();
    if (bit(u_fbz, 2) && bit(u_fbz, 12)) {
        int index = ((iteratedRow(pixel) & 3) << 3) | (~pixel.x & 7);
        if (((u_stipple >> index) & 1) == 0) {
            discard;
        }
    }

    int select = u_fbzcp & 3;
    ivec4 other = select == 0 ? iter : select == 1 ? texel : select == 2 ? argb(u_color1) : ivec4(0);
    // The chroma key tests c_other before its alpha is chosen.
    if (bit(u_fbz, 1) && chromaRejects(other.rgb)) {
        discard;
    }
    select = (u_fbzcp >> 2) & 3;
    other.a = select == 0 ? iter.a : select == 1 ? texel.a : select == 2 ? argb(u_color1).a : 0;
    if (bit(u_fbz, 13) && (other.a & 1) == 0) {
        discard;
    }
    if (bit(u_alpha, 0) && !alphaTest(other.a)) {
        discard;
    }

    ivec4 color0 = argb(u_color0);
    ivec3 local;
    if (!bit(u_fbzcp, 7)) {
        local = !bit(u_fbzcp, 4) ? iter.rgb : color0.rgb;
    } else {
        local = (texel.a & 0x80) == 0 ? iter.rgb : color0.rgb;
    }
    select = (u_fbzcp >> 5) & 3;
    int alocal = select == 0 ? iter.a : select == 1 ? color0.a : select == 2 ? clampedZ(v_zw.x) & 0xFF : clampedW(v_zw.y) & 0xFF;
    ivec4 c = combineColor(other, ivec4(local, alocal), texel);
    if (bit(u_fog, 0)) {
        c.rgb = fog(c.rgb, iter.a, pixel);
    }

    int depth = u_constant_depth != 0 ? u_zacolor & 0xFFFF : depthValue();
    gl_FragDepth = float(depth) / 65535.0;
    if (dithers()) {
        int i = ditherIndex(pixel);
        c.rgb = dither565(c.rgb, bit(u_fbz, 11) ? DITHER2[i] : DITHER4[i]);
    }
    o_color = vec4(c) / 255.0;
}
