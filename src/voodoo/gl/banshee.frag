// A Banshee's or Voodoo3's picture as its video processor shows it, at
// u_scale times its size and each pixel u_across times across: the
// desktop, the overlay window over it and the hardware cursor on top, as
// src/voodoo/banshee/display.rs draws it in software. A surface is the
// card's memory (bytes in rows of 4096, from the surface's first byte
// on) or, where the OpenGL renderer draws it, the texture of one of its
// colour buffers.

// OpenGL ES has no default precision for integer samplers.
#ifdef GL_ES
precision highp usampler2D;
#endif

uniform int u_scale;
// 2 when the monitor shows each of the desktop's pixels twice across
// (a narrow mode at half the pixel clock), else 1.
uniform int u_across;
uniform usampler2D u_desk_mem;
uniform sampler2D u_desk_buf;
uniform usampler2D u_ovl_mem;
uniform sampler2D u_ovl_buf;
uniform usampler2D u_cursor;
uniform sampler2D u_clut;

// The desktop: its format, CLUT half (0 or 256), whether it bypasses the
// CLUT and where it comes from (0 memory, 1 a buffer); its stride in
// bytes (a row's, or a row of tiles'), whether it is tiled, whether
// half mode shows each line twice, and the bytes of memory it has.
uniform ivec4 u_desk;
uniform ivec4 u_desk_layout;
// The overlay likewise, its source -1 when it is off; its rectangle on
// the screen (left, top, right, bottom, inclusive) and its scaling:
// source pixels a screen pixel across and down, and the first row's
// offset, in 20-bit fractions.
uniform ivec4 u_ovl;
uniform ivec4 u_ovl_layout;
uniform ivec4 u_ovl_rect;
uniform ivec3 u_ovl_step;
// The cursor's top left corner, whether it is on and in X11 mode, and
// its two colours.
uniform ivec4 u_cur;
uniform vec3 u_cur_c0;
uniform vec3 u_cur_c1;

out vec4 o_color;

uint byte_at(usampler2D mem, uint a, uint len) {
    if (a >= len) {
        return 0u;
    }
    return texelFetch(mem, ivec2(int(a & 4095u), int(a >> 12)), 0).r;
}

// Byte x of row y of a surface (display.rs's `surface_addr`).
uint surface_addr(uint x, uint y, uint stride, bool tiled) {
    if (tiled) {
        return (y & 31u) * 128u + (y >> 5) * stride + (x & 127u) + (x >> 7) * 4096u;
    }
    return y * stride + x;
}

vec3 clut(int i) {
    return texelFetch(u_clut, ivec2(i, 0), 0).rgb;
}

// A 5-6-5 pixel's 8-bit components.
ivec3 rgb565(uint p) {
    int r = int(((p >> 8) & 0xF8u) | (p >> 13));
    int g = int(((p >> 3) & 0xFCu) | ((p >> 9) & 3u));
    int b = int(((p << 3) & 0xF8u) | ((p >> 2) & 7u));
    return ivec3(r, g, b);
}

// 8-bit components through the CLUT half at `base`, or as they are.
vec3 through(ivec3 c, int base, bool bypass) {
    if (bypass) {
        return vec3(c) / 255.0;
    }
    return vec3(clut(base + c.r).r, clut(base + c.g).g, clut(base + c.b).b);
}

// A colour buffer's texel as 8-bit components, as the 5-6-5 pixel's
// would be; past its edges, the edge's.
ivec3 buffer_texel(sampler2D buf, vec2 at) {
    ivec2 texel = clamp(ivec2(at), ivec2(0), textureSize(buf, 0) - 1);
    return ivec3(round(texelFetch(buf, texel, 0).rgb * 255.0));
}

// YCbCr to RGB, ITU-R BT.601 (display.rs's `yuv`).
vec3 yuv(int y, int u, int v) {
    y -= 16;
    u -= 128;
    v -= 128;
    ivec3 c = ivec3(298 * y + 409 * v + 128, 298 * y - 100 * u - 208 * v + 128, 298 * y + 516 * u + 128);
    return vec3(clamp(c >> 8, 0, 255)) / 255.0;
}

vec3 desktop(ivec2 p, vec2 f) {
    int format = u_desk.x;
    bool bypass = u_desk.z != 0;
    bool half_mode = u_desk_layout.z != 0;
    if (u_desk.w == 1) {
        vec2 at = half_mode ? vec2(f.x, f.y * 0.5) : f;
        return through(buffer_texel(u_desk_buf, at), u_desk.y, bypass);
    }
    uint stride = uint(u_desk_layout.x);
    bool tiled = u_desk_layout.y != 0;
    uint len = uint(u_desk_layout.w);
    uint line = uint(half_mode ? p.y / 2 : p.y);
    uint x = uint(p.x);
    if (format == 0) {
        int i = int(byte_at(u_desk_mem, surface_addr(x, line, stride, tiled), len));
        return bypass ? vec3(float(i) / 255.0) : clut(u_desk.y + i);
    }
    if (format == 1) {
        uint a = surface_addr(2u * x, line, stride, tiled);
        uint word = byte_at(u_desk_mem, a, len) | byte_at(u_desk_mem, a + 1u, len) << 8;
        return through(rgb565(word), u_desk.y, bypass);
    }
    uint a = (format == 2 ? 3u : 4u) * x;
    uint r = byte_at(u_desk_mem, surface_addr(a + 2u, line, stride, tiled), len);
    uint g = byte_at(u_desk_mem, surface_addr(a + 1u, line, stride, tiled), len);
    uint b = byte_at(u_desk_mem, surface_addr(a, line, stride, tiled), len);
    return vec3(r, g, b) / 255.0;
}

vec3 overlay(ivec2 p, vec2 f) {
    int format = u_ovl.x;
    bool bypass = u_ovl.z != 0;
    uint dudx = uint(u_ovl_step.x);
    uint dvdy = uint(u_ovl_step.y);
    uint v0 = uint(u_ovl_step.z);
    if (u_ovl.w == 1) {
        // Where the screen pixel's part is in the source, at the
        // buffer's resolution: from its left and top edges, as the
        // video processor steps from a pixel's, not from its centre.
        float scale = float(u_scale);
        vec2 from = floor(f) / scale - vec2(u_ovl_rect.xy);
        float u = from.x * float(dudx) / 1048576.0;
        float v = (float(v0) + from.y * float(dvdy)) / 1048576.0;
        return through(buffer_texel(u_ovl_buf, vec2(u, v) * scale), u_ovl.y, bypass);
    }
    uint stride = uint(u_ovl_layout.x);
    bool tiled = u_ovl_layout.y != 0;
    uint len = uint(u_ovl_layout.w);
    uint sx = (uint(p.x - u_ovl_rect.x) * dudx) >> 20;
    uint sy = (v0 + uint(p.y - u_ovl_rect.y) * dvdy) >> 20;
    if (format == 5 || format == 6) {
        // Two pixels in four bytes: YUYV or UYVY.
        uint pair = surface_addr((sx & ~1u) * 2u, sy, stride, tiled);
        int b0 = int(byte_at(u_ovl_mem, pair, len));
        int b1 = int(byte_at(u_ovl_mem, pair + 1u, len));
        int b2 = int(byte_at(u_ovl_mem, pair + 2u, len));
        int b3 = int(byte_at(u_ovl_mem, pair + 3u, len));
        bool odd = (sx & 1u) != 0u;
        if (format == 5) {
            return yuv(odd ? b2 : b0, b1, b3);
        }
        return yuv(odd ? b3 : b1, b0, b2);
    }
    if (format == 0) {
        return clut(u_ovl.y + int(byte_at(u_ovl_mem, surface_addr(sx, sy, stride, tiled), len)));
    }
    uint a = surface_addr(sx * 2u, sy, stride, tiled);
    uint word = byte_at(u_ovl_mem, a, len) | byte_at(u_ovl_mem, a + 1u, len) << 8;
    return through(rgb565(word), u_ovl.y, bypass);
}

void main() {
    // Where the fragment is on the picture before its pixels are widened,
    // which the rest goes by.
    vec2 f = gl_FragCoord.xy / vec2(float(u_across), 1.0);
    ivec2 p = ivec2(f) / u_scale;
    vec3 c = desktop(p, f);
    if (u_ovl.w >= 0 && all(greaterThanEqual(p, u_ovl_rect.xy)) && all(lessThanEqual(p, u_ovl_rect.zw))) {
        c = overlay(p, f);
    }
    ivec2 q = p - u_cur.xy;
    if (u_cur.z != 0 && all(greaterThanEqual(q, ivec2(0))) && all(lessThan(q, ivec2(64)))) {
        // Two bit planes, 8 bytes each a row.
        uint shift = uint(7 - (q.x & 7));
        uint p0 = (texelFetch(u_cursor, ivec2(q.x >> 3, q.y), 0).r >> shift) & 1u;
        uint p1 = (texelFetch(u_cursor, ivec2(8 + (q.x >> 3), q.y), 0).r >> shift) & 1u;
        if (u_cur.w != 0) {
            if (p0 != 0u) {
                c = p1 != 0u ? u_cur_c1 : u_cur_c0;
            }
        } else if (p0 == 0u) {
            c = p1 != 0u ? u_cur_c1 : u_cur_c0;
        } else if (p1 != 0u) {
            c = vec3(1.0) - c;
        }
    }
    o_color = vec4(c, 1.0);
}
