// The cached base is already filtered; overlays remain at composed size.
uniform sampler2D u_frame;
uniform sampler2D u_overlay;
uniform bool u_overlay_enabled;

in vec2 v_uv;
out vec4 o_color;

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
    ivec2 size = textureSize(u_frame, 0);
    ivec2 at = clamp(ivec2(v_uv * vec2(size)), ivec2(0), size - 1);
    // Restore the resolve's rounding bias after the intermediate's UNORM
    // conversion, including when the final framebuffer uses dithering.
    o_color = (floor(texelFetch(u_frame, at, 0) * 255.0 + 0.5) + 0.25) / 255.0;
}
