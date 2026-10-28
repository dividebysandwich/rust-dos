// A 2D operation (src/voodoo/mirror.rs's `Blit`) drawn into the card's
// memory, which is the framebuffer here: a fragment is a byte of the
// layer u_layer of the memory's texture array, and works out the pixel
// of the destination it is part of, that pixel's value and which of its
// bytes to write. The pixels are those src/win32/ddraw/blt.rs's `run`
// works out in software.

#ifdef GL_ES
precision highp usampler2D;
precision highp usampler2DArray;
#endif

// The card's memory as it was before the operation (a copy of the rows
// it reads), and a colour buffer the renderer draws, as a source.
uniform usampler2DArray u_mem;
uniform sampler2D u_buf;
// The card's memory: its size less one, where addresses wrap, and how
// its bytes lie in the layers of u_mem (src/voodoo/gl.rs's
// `MemoryLayout`): log2 of the bytes a row and of the bytes a layer.
uniform ivec3 u_mem_layout;
// The layer drawn into.
uniform int u_layer;
// The destination: its first byte, its stride and its bytes a pixel.
uniform ivec3 u_dst;
// The rectangle drawn now, and the one the source is stretched onto:
// left, top, right, bottom, in the destination's pixels.
uniform ivec4 u_clip;
uniform ivec4 u_area;
// The source: where its pixels come from (0 a fill of u_fill, 1 the
// memory, 2 the colour buffer), and in the memory its first byte, stride
// and bytes a pixel; its rectangle, as u_area.
uniform ivec4 u_src;
uniform ivec4 u_src_rect;
uniform int u_fill;
// Mirrored across and down, the conversion (0 none, 1 5-6-5 to 8-8-8,
// 2 8-8-8 to 5-6-5, 3 8-8-8 with the top byte 0), and the colour
// buffer's scale.
uniform ivec4 u_mode;
// The colour keys: lowest and highest values, and whether there is one;
// and the format's red, green and blue masks, all 0 to compare whole.
uniform ivec3 u_src_key;
uniform ivec3 u_src_masks;
uniform ivec3 u_dst_key;
uniform ivec3 u_dst_masks;

out uvec4 o_color;

uint byte_at(uint a) {
    a &= uint(u_mem_layout.x);
    uint row = uint(u_mem_layout.y);
    uint layer = uint(u_mem_layout.z);
    uint x = a & ((1u << row) - 1u);
    uint y = (a & ((1u << layer) - 1u)) >> row;
    return texelFetch(u_mem, ivec3(int(x), int(y), int(a >> layer)), 0).r;
}

// The pixel of `bpp` bytes from byte `a` on, little-endian.
uint pixel_at(uint a, int bpp) {
    uint v = 0u;
    for (int i = 0; i < 4; i++) {
        if (i < bpp) {
            v |= byte_at(a + uint(i)) << uint(8 * i);
        }
    }
    return v;
}

// Whether `v` is in the key: each channel the masks have between the
// key's low and high values of it, or with no masks the whole value.
bool keyed(uint v, ivec3 key, ivec3 masks) {
    uint low = uint(key.x);
    uint high = uint(key.y);
    if (masks == ivec3(0)) {
        return v >= low && v <= high;
    }
    for (int i = 0; i < 3; i++) {
        uint m = uint(masks[i]);
        if (m != 0u && ((v & m) < (low & m) || (v & m) > (high & m))) {
            return false;
        }
    }
    return true;
}

// 5-6-5 to 8-8-8, each channel's bits repeated below them, and back.
uint widen(uint p) {
    uint r = (p >> 11) & 31u;
    uint g = (p >> 5) & 63u;
    uint b = p & 31u;
    return (r << 3 | r >> 2) << 16 | (g << 2 | g >> 4) << 8 | (b << 3 | b >> 2);
}

uint narrow(uint p) {
    return ((p >> 19) & 31u) << 11 | ((p >> 10) & 63u) << 5 | ((p >> 3) & 31u);
}

void main() {
    ivec2 t = ivec2(gl_FragCoord.xy);
    uint a = uint(u_layer) << uint(u_mem_layout.z) | uint(t.y) << uint(u_mem_layout.y)
        | uint(t.x);
    uint start = uint(u_dst.x);
    uint stride = uint(u_dst.y);
    int bpp = u_dst.z;
    if (a < start) {
        discard;
    }
    uint off = a - start;
    int row = int(off / stride);
    uint col = off % stride;
    int x = int(col / uint(bpp));
    int k = int(col % uint(bpp));
    if (x < u_clip.x || x >= u_clip.z || row < u_clip.y || row >= u_clip.w) {
        discard;
    }
    uint value = uint(u_fill);
    if (u_src.x != 0) {
        int dw = u_area.z - u_area.x;
        int dh = u_area.w - u_area.y;
        int sw = u_src_rect.z - u_src_rect.x;
        int sh = u_src_rect.w - u_src_rect.y;
        int u = x - u_area.x;
        int v = row - u_area.y;
        if (u_mode.x != 0) {
            u = dw - 1 - u;
        }
        if (u_mode.y != 0) {
            v = dh - 1 - v;
        }
        int sx = u_src_rect.x + u * sw / dw;
        int sy = u_src_rect.y + v * sh / dh;
        if (u_src.x == 1) {
            uint from = uint(u_src.y) + uint(sy) * uint(u_src.z) + uint(sx * u_src.w);
            value = pixel_at(from, u_src.w);
        } else {
            // The buffer's pixel at the middle of its block of scale by
            // scale, as 8-8-8.
            int s = u_mode.w;
            ivec2 at = clamp(ivec2(sx, sy) * s + s / 2, ivec2(0), textureSize(u_buf, 0) - 1);
            uvec3 c = uvec3(round(texelFetch(u_buf, at, 0).rgb * 255.0));
            value = c.r << 16 | c.g << 8 | c.b;
        }
        if (u_src_key.z != 0 && keyed(value, u_src_key, u_src_masks)) {
            discard;
        }
        if (u_mode.z == 1) {
            value = widen(value);
        } else if (u_mode.z == 2) {
            value = narrow(value);
        } else if (u_mode.z == 3) {
            value &= 0xFFFFFFu;
        }
    }
    if (u_dst_key.z != 0 && !keyed(pixel_at(a - uint(k), bpp), u_dst_key, u_dst_masks)) {
        discard;
    }
    o_color = uvec4((value >> uint(8 * k)) & 255u, 0u, 0u, 0u);
}
