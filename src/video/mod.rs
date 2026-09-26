use crate::bus::Bus;
use crate::cpu::Cpu;

pub mod adapter;
pub mod bios;
pub mod cga;
pub mod composite;
pub mod crt;
pub mod echo;
pub mod hercules;
pub mod modes;
pub mod mono;
pub mod overlay;
pub mod palette;
pub mod pixels;
pub mod shader;
pub mod tandy;
pub mod text;
pub mod vbe;
pub mod vga;

pub const SCREEN_WIDTH: u32 = 640;
pub const SCREEN_HEIGHT: u32 = 400;

/// Code page 437 → Unicode: the characters of the VGA font, for text-mode
/// screen dumps and for typing characters of other keyboard layouts.
pub const CP437: [char; 256] = [
    ' ', '☺', '☻', '♥', '♦', '♣', '♠', '•', '◘', '○', '◙', '♂', '♀', '♪', '♫', '☼',
    '►', '◄', '↕', '‼', '¶', '§', '▬', '↨', '↑', '↓', '→', '←', '∟', '↔', '▲', '▼',
    ' ', '!', '"', '#', '$', '%', '&', '\'', '(', ')', '*', '+', ',', '-', '.', '/',
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', ':', ';', '<', '=', '>', '?',
    '@', 'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O',
    'P', 'Q', 'R', 'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z', '[', '\\', ']', '^', '_',
    '`', 'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o',
    'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z', '{', '|', '}', '~', '⌂',
    'Ç', 'ü', 'é', 'â', 'ä', 'à', 'å', 'ç', 'ê', 'ë', 'è', 'ï', 'î', 'ì', 'Ä', 'Å',
    'É', 'æ', 'Æ', 'ô', 'ö', 'ò', 'û', 'ù', 'ÿ', 'Ö', 'Ü', '¢', '£', '¥', '₧', 'ƒ',
    'á', 'í', 'ó', 'ú', 'ñ', 'Ñ', 'ª', 'º', '¿', '⌐', '¬', '½', '¼', '¡', '«', '»',
    '░', '▒', '▓', '│', '┤', '╡', '╢', '╖', '╕', '╣', '║', '╗', '╝', '╜', '╛', '┐',
    '└', '┴', '┬', '├', '─', '┼', '╞', '╟', '╚', '╔', '╩', '╦', '╠', '═', '╬', '╧',
    '╨', '╤', '╥', '╙', '╘', '╒', '╓', '╫', '╪', '┘', '┌', '█', '▄', '▌', '▐', '▀',
    'α', 'ß', 'Γ', 'π', 'Σ', 'σ', 'µ', 'τ', 'Φ', 'Θ', 'Ω', 'δ', '∞', 'φ', 'ε', '∩',
    '≡', '±', '≥', '≤', '⌠', '⌡', '÷', '≈', '°', '∙', '·', '√', 'ⁿ', '²', '■', ' ',
];

// Memory Map Addresses
pub const ADDR_VGA_GRAPHICS: usize = 0xA0000;
pub const ADDR_VGA_TEXT: usize = 0xB8000;
pub const SIZE_GRAPHICS: usize = 0x10000; // 64KB A0000..AFFFF window (covers modes 13h, 12h, etc.)
pub const SIZE_TEXT: usize = 32 * 1024; // 32kB to cover CGA modes too
pub const BDA_CURSOR_POS: usize = 0x0450; // Base for Page 0. Page n = 0x450 + n*2
pub const BDA_CURSOR_MODE: usize = 0x0460;
pub const MAX_COLS: u8 = 80;
pub const MAX_ROWS: u8 = 25;

// The IBM EGA and VGA ROM fonts (see assets/README.md).
static FONT_8X16: &[u8] = include_bytes!("assets/IBM_VGA_8x16.bin");
static FONT_8X14: &[u8] = include_bytes!("assets/IBM_EGA_8x14.bin");
static FONT_8X8: &[u8] = include_bytes!("assets/IBM_VGA_8x8.bin");
/// The glyphs that differ in a 9-dot wide character cell: a character
/// code, then its glyph, for each; a 0 ends the table.
pub static FONT_9X14_ALTERNATE: &[u8] = include_bytes!("assets/IBM_EGA_9x14_alt.bin");
pub static FONT_9X16_ALTERNATE: &[u8] = include_bytes!("assets/IBM_VGA_9x16_alt.bin");

/// The VGA's 8x16 font: 256 CP437 glyphs of 16 bytes, one per row, the
/// leftmost pixel in bit 7.
pub fn font_8x16() -> &'static [u8] {
    FONT_8X16
}

/// The EGA's 8x14 font, laid out like `font_8x16`.
pub fn font_8x14() -> &'static [u8] {
    FONT_8X14
}

/// The VGA's 8x8 font, laid out like `font_8x16`.
pub fn font_8x8() -> &'static [u8] {
    FONT_8X8
}

#[derive(PartialEq, Clone, Copy, Debug)]
pub enum VideoMode {
    Text40x25 = 0x00,
    Text40x25Color = 0x01,
    Text80x25 = 0x02,
    Text80x25Color = 0x03,
    Cga320x200Color = 0x04,
    #[allow(dead_code)]
    Cga320x200 = 0x05, // I can't be bothered and just treat it as Color too
    Cga640x200 = 0x06,
    /// The monochrome text mode of the MDA and the Hercules card: 80x25 in
    /// 9x14 cells at B0000h.
    Mono80x25 = 0x07,
    /// The Tandy 1000's and PCjr's 16 colours at 160x200 and 320x200, two
    /// pixels a byte, and 4 colours at 640x200 in pairs of bytes (see
    /// tandy.rs).
    Tandy160x200x16 = 0x08,
    Tandy320x200x16 = 0x09,
    Tandy640x200x4 = 0x0A,
    Ega320x200 = 0x0D,  // EGA planar, 16 colors
    Ega640x200 = 0x0E,  // EGA planar, 16 colors
    /// The EGA's monochrome graphics: 640x350 in planes 0 and 2, the video
    /// and the intensity.
    Ega640x350Mono = 0x0F,
    Ega640x350 = 0x10,  // EGA planar, 16 colors
    /// The VGA's 640x480 in two colors: plane 0.
    Vga640x480Mono = 0x11,
    Vga640x480 = 0x12,  // VGA planar, 16 colors
    Graphics320x200 = 0x13,
    /// The Hercules card's 720x348 graphics, which programs set with its
    /// registers; the BIOS knows nothing of it. Never written to the BIOS
    /// data area.
    HercGraphics = 0xFE,
    /// A VESA mode: which one, and its size, are in `Bus::vbe`. Never
    /// written to the BIOS data area.
    Vesa = 0xFF,
}

impl VideoMode {
    /// A text mode.
    pub fn is_text(self) -> bool {
        matches!(
            self,
            VideoMode::Text40x25 | VideoMode::Text40x25Color | VideoMode::Text80x25 | VideoMode::Text80x25Color | VideoMode::Mono80x25
        )
    }

    pub fn is_planar(self) -> bool {
        matches!(
            self,
            VideoMode::Ega320x200
                | VideoMode::Ega640x200
                | VideoMode::Ega640x350
                | VideoMode::Ega640x350Mono
                | VideoMode::Vga640x480
                | VideoMode::Vga640x480Mono
        )
    }

    /// Dimensions for each mode in pixels (width, height).
    pub fn dimensions(self) -> (usize, usize) {
        match self {
            VideoMode::Text40x25 | VideoMode::Text40x25Color => (320, 200),
            VideoMode::Text80x25 | VideoMode::Text80x25Color => (640, 400),
            VideoMode::Cga320x200Color | VideoMode::Cga320x200 => (320, 200),
            VideoMode::Cga640x200 => (640, 200),
            VideoMode::Mono80x25 => (720, 350),
            VideoMode::Tandy160x200x16 => (160, 200),
            VideoMode::Tandy320x200x16 => (320, 200),
            VideoMode::Tandy640x200x4 => (640, 200),
            VideoMode::HercGraphics => hercules::GRAPHICS_SIZE,
            VideoMode::Ega320x200 => (320, 200),
            VideoMode::Ega640x200 => (640, 200),
            VideoMode::Ega640x350 | VideoMode::Ega640x350Mono => (640, 350),
            VideoMode::Vga640x480 | VideoMode::Vga640x480Mono => (640, 480),
            VideoMode::Graphics320x200 => (320, 200),
            // The mode's size is in `Bus::vbe`; see `Bus::display_size`.
            VideoMode::Vesa => (640, 480),
        }
    }
}

/// A picture as the screen shows it: RGB24 pixels, row by row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

impl Frame {
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height, rgb: vec![0; (width * height * 3) as usize] }
    }

    /// Make the frame `width` x `height`. True if that changed its size;
    /// it is black then.
    pub fn resize(&mut self, width: u32, height: u32) -> bool {
        if (width, height) == (self.width, self.height) {
            return false;
        }
        *self = Self::new(width, height);
        true
    }
}

/// The size of the picture of the current video mode: 640x400 for the
/// text and CGA modes, and for the others their size, doubled in each
/// direction where it is small (mode 13h's 320x200 is 640x400, mode 12h
/// 640x480, a 320x240 "mode X" 640x480).
pub fn frame_size(bus: &Bus) -> (u32, u32) {
    match bus.video_mode {
        VideoMode::Graphics320x200
        | VideoMode::Ega320x200
        | VideoMode::Ega640x200
        | VideoMode::Ega640x350
        | VideoMode::Ega640x350Mono
        | VideoMode::Vga640x480
        | VideoMode::Vga640x480Mono => {
            let (width, rows) = bus.vga.graphics_size();
            let width = if width < 400 { width * 2 } else { width };
            let rows = if rows < 300 { rows * 2 } else { rows };
            (width as u32, rows as u32)
        }
        VideoMode::Vesa => bus.vbe.frame_size().unwrap_or((SCREEN_WIDTH, SCREEN_HEIGHT)),
        VideoMode::HercGraphics => {
            let (width, height, ..) = bus.vga.herc_graphics_shape();
            (width as u32, height as u32)
        }
        // Text: its characters across, and the scanlines the CRTC shows
        // (the EGA's 350, the VGA's 400; the CGA's 200 scanned twice).
        _ => match text::geometry(bus) {
            Some(g) if !bus.vga.adapter.cga_like() => {
                let lines = bus.vga.peek_timing().display.clamp(200, 600);
                ((g.cols * g.cell_w()) as u32, lines)
            }
            _ => (SCREEN_WIDTH, SCREEN_HEIGHT),
        },
    }
}

/// Draw the rows the VGA marked dirty into `frame`, which must be
/// `frame_size` big.
pub fn render_screen(frame: &mut Frame, bus: &Bus) {
    // Re-render only the rows the VGA has marked dirty since the last call.
    // For the common case of a shell prompt blinking or one line of output,
    // this is one or two character rows out of 25 — orders of magnitude less
    // work than re-rendering the whole screen.
    let width = frame.width as usize;
    let y_min = bus.vga.dirty_y_min.min(frame.height) as usize;
    let y_max = bus.vga.dirty_y_max.min(frame.height) as usize;
    if y_min >= y_max {
        return;
    }

    // Black-fill just the dirty band. Renderers either fully cover this band
    // or leave a sub-row gap that we want to appear black.
    let row_bytes = width * 3;
    frame.rgb[y_min * row_bytes..y_max * row_bytes].fill(0);
    let canvas = &mut frame.rgb[..];

    // The Tandy's and PCjr's video shows system memory.
    if bus.vga.adapter.gate_array() {
        match text::geometry(bus) {
            Some(geometry) => text::render(canvas, width, bus, &geometry, y_min, y_max, bus.display_mem()),
            None => tandy::render_graphics(canvas, bus),
        }
        return;
    }

    match bus.video_mode {
        VideoMode::Graphics320x200 => render_graphics_mode(canvas, width, &bus.vga.vram_graphics, bus),
        VideoMode::Cga320x200Color | VideoMode::Cga320x200 => {
            render_cga_mode4(canvas, &bus.vga.vram_text, bus)
        }
        VideoMode::Cga640x200 => render_cga_mode6(canvas, &bus.vga.vram_text, bus),
        // Text renderers honour the dirty row band so a single-line shell
        // update only repaints those 16 scanlines instead of the full 80x25.
        VideoMode::Text80x25
        | VideoMode::Text80x25Color
        | VideoMode::Text40x25
        | VideoMode::Text40x25Color
        | VideoMode::Mono80x25 => {
            if let Some(geometry) = text::geometry(bus) {
                text::render(canvas, width, bus, &geometry, y_min, y_max, &bus.vga.vram_text);
            }
        }
        VideoMode::HercGraphics => hercules::render_graphics(canvas, width, &bus.vga),
        // 16-color planar modes (0Dh, 0Eh, 10h, 12h), at the size the CRTC
        // registers give them.
        VideoMode::Ega320x200
        | VideoMode::Ega640x200
        | VideoMode::Ega640x350
        | VideoMode::Ega640x350Mono
        | VideoMode::Vga640x480
        | VideoMode::Vga640x480Mono => render_planar(canvas, width, &bus.vga.vram_graphics, bus),
        VideoMode::Vesa => render_vbe(canvas, width, y_min, y_max, bus),
        // Only the gate array has them, which is drawn above.
        VideoMode::Tandy160x200x16 | VideoMode::Tandy320x200x16 | VideoMode::Tandy640x200x4 => {}
    }
}

/// VESA modes: rows of pixels in linear video memory from the display
/// start on, 8-bit through the DAC, or direct color.
fn render_vbe(canvas: &mut [u8], canvas_w: usize, y_min: usize, y_max: usize, bus: &Bus) {
    let vbe = &bus.vbe;
    let Some(mode) = vbe.mode else {
        return;
    };
    let colors: Vec<(u8, u8, u8)> = (0..=255u8).map(|i| bus.vga.get_rgb(i & bus.vga.dac_mask)).collect();
    let vram = &vbe.vram;
    let wrap = vbe::VRAM_SIZE - 1;
    let pixel = |off: usize| -> (u8, u8, u8) {
        let byte = |i: usize| vram[(off + i) & wrap];
        let word = || byte(0) as u16 | (byte(1) as u16) << 8;
        let five = |v: u16| ((v << 3) | (v >> 2)) as u8;
        match mode.bpp {
            8 => colors[byte(0) as usize],
            15 => {
                let v = word();
                (five(v >> 10 & 31), five(v >> 5 & 31), five(v & 31))
            }
            16 => {
                let v = word();
                let g = v >> 5 & 63;
                (five(v >> 11), ((g << 2) | (g >> 4)) as u8, five(v & 31))
            }
            _ => (byte(2), byte(1), byte(0)),
        }
    };
    let scale = vbe.scale() as usize;
    let bytes = mode.bytes_per_pixel();
    let row_bytes = canvas_w * 3;
    for fy in y_min..y_max.min(mode.height as usize * scale) {
        let dst = fy * row_bytes;
        // Doubled rows copy the one above.
        if fy % scale != 0 && fy > y_min {
            canvas.copy_within(dst - row_bytes..dst, dst);
            continue;
        }
        let row = vbe.latched_start as usize + fy / scale * vbe.pitch as usize;
        for x in 0..mode.width as usize {
            let rgb = pixel(row + x * bytes);
            for dx in 0..scale {
                let i = dst + (x * scale + dx) * 3;
                canvas[i] = rgb.0;
                canvas[i + 1] = rgb.1;
                canvas[i + 2] = rgb.2;
            }
        }
    }
}

/// Renderer for the planar 16-color VGA/EGA modes (0Dh, 0Eh, 10h, 12h and
/// variants programs make of them, such as 640x240 with doubled scanlines).
///
/// Reads pixels out of the 4-plane memory layout and runs each 4-bit pixel
/// through the Attribute Controller palette, then the DAC, scaling the
/// picture to the canvas.
fn render_planar(canvas: &mut [u8], canvas_w: usize, vram: &[u8], bus: &Bus) {
    let (width, rows) = bus.vga.graphics_size();
    // The planes the Color Plane Enable register lets through, and the
    // colour of each pixel value.
    let planes = bus.vga.attribute_regs[0x12] & 0x0F;
    let colors: [(u8, u8, u8); 16] = std::array::from_fn(|pixel| bus.vga.attribute_rgb(pixel as u8 & planes));
    // CRTC Offset (index 0x13) holds bytes-per-scanline / 2 (i.e. words
    // per row). Fall back to width/8 if the game never touched it.
    let offset_reg = bus.vga.crtc_regs[0x13] as usize;
    let bytes_per_row = if offset_reg != 0 { offset_reg * 2 } else { width / 8 };
    let base = planar_base_offset(bus);
    let split = bus.vga.split_row();

    let pan = bus.vga.pixel_panning();
    let split_pan = if bus.vga.attribute_regs[0x10] & 0x20 != 0 { 0 } else { pan };

    let canvas_h = canvas.len() / (canvas_w * 3);
    let row_bytes = canvas_w * 3;
    let mut last_y = usize::MAX;
    for ty in 0..canvas_h {
        let y = ty * rows / canvas_h;
        let dst = ty * row_bytes;
        if y == last_y {
            canvas.copy_within(dst - row_bytes..dst, dst);
            continue;
        }
        last_y = y;
        // Rows past the split screen's start show VRAM from address 0.
        let (row_base, row, pan) = if y >= split { (0, y - split, split_pan) } else { (base, y, pan) };
        for tx in 0..canvas_w {
            let x = tx * width / canvas_w;
            let rgb = colors[planar_pixel(vram, bytes_per_row, x + pan, row, row_base) as usize];
            let idx = dst + tx * 3;
            canvas[idx] = rgb.0;
            canvas[idx + 1] = rgb.1;
            canvas[idx + 2] = rgb.2;
        }
    }
}

/// Byte offset into each plane where the display controller begins scanning.
/// We read the value the CRTC latched at the last vertical retrace rather
/// than the raw register — games that page-flip mid-frame expect the
/// display to ignore pending writes and only update at vretrace.
fn planar_base_offset(bus: &Bus) -> usize {
    bus.vga.latched_start_addr
}

/// The 4-bit value of pixel (`x`, `y`): a bit from each plane.
fn planar_pixel(vram: &[u8], bytes_per_row: usize, x: usize, y: usize, base: usize) -> u8 {
    // Plane space wraps at 64 KiB; games with smaller back buffers rely on
    // that so page flips near the top of VRAM don't walk into garbage.
    let byte_offset = (base + y * bytes_per_row + (x / 8)) & 0xFFFF;
    let bit_pos = 7 - (x % 8) as u8;
    let mut pixel: u8 = 0;
    for plane in 0..4 {
        let bit = (vram[plane * 65536 + byte_offset] >> bit_pos) & 1;
        pixel |= bit << plane;
    }
    pixel
}

/// 256-color modes: mode 13h and the unchained "mode X" family (320x240,
/// 360x480, ...), whatever size the CRTC registers give them, scaled to
/// the canvas, `canvas_w` pixels wide.
pub fn render_graphics_mode(canvas: &mut [u8], canvas_w: usize, vram: &[u8], bus: &Bus) {
    // The CRTC scans the 4 planes in parallel: pixel x of a row is in plane
    // x % 4 at Start Address + row * stride + x / 4. With Chain 4 (plain
    // mode 13h) that is where CPU address y * 320 + x lands. Unchained
    // "mode X" games draw into several pages and flip between them with
    // the Start Address.
    let (width, rows) = bus.vga.graphics_size();
    let start = bus.vga.latched_start_addr;
    let stride = match bus.vga.crtc_regs[0x13] {
        0 => 80,
        words => words as usize * 2,
    };
    // Rows past the split screen's start show VRAM from address 0, and
    // unpanned when the Attribute Mode Control register says so.
    let split = bus.vga.split_row();
    let pan = bus.vga.pixel_panning();
    let split_pan = if bus.vga.attribute_regs[0x10] & 0x20 != 0 { 0 } else { pan };
    // VGA hardware ANDs each pixel with the PEL mask before the DAC lookup.
    let colors: Vec<(u8, u8, u8)> = (0..=255u8).map(|i| bus.vga.get_rgb(i & bus.vga.dac_mask)).collect();
    let canvas_h = canvas.len() / (canvas_w * 3);
    let row_bytes = canvas_w * 3;
    let mut last_y = usize::MAX;
    for ty in 0..canvas_h {
        let y = ty * rows / canvas_h;
        let dst = ty * row_bytes;
        if y == last_y {
            canvas.copy_within(dst - row_bytes..dst, dst);
            continue;
        }
        last_y = y;
        let (row, pan) = if y >= split { ((y - split) * stride, split_pan) } else { (start + y * stride, pan) };
        for tx in 0..canvas_w {
            let px = tx * width / canvas_w + pan;
            let plane = px & 3;
            let offset = (row + (px >> 2)) & 0xFFFF;
            let color_idx = vram.get(plane * 65536 + offset).copied().unwrap_or(0);
            let rgb = colors[color_idx as usize];
            let idx = dst + tx * 3;
            canvas[idx] = rgb.0;
            canvas[idx + 1] = rgb.1;
            canvas[idx + 2] = rgb.2;
        }
    }
}

/// Where CGA graphics start in memory, and whether they show at all: a
/// CGA's 6845 starts at its Start Address (counted in words) and Mode
/// Control bit 3 turns the picture off; a VGA starts at 0.
fn cga_start(bus: &Bus) -> Option<usize> {
    match bus.vga.adapter {
        adapter::Adapter::Cga if !bus.vga.cga_video_enabled() => None,
        adapter::Adapter::Cga => Some(bus.vga.latched_start_addr * 2),
        _ => Some(0),
    }
}

/// CGA mode 4/5 (320x200, 4 colors): two bits a pixel, even rows at 0000h
/// and odd rows at 2000h. A VGA takes the four colors from palette
/// registers 0-3, which INT 10h AH=0Bh sets for the background and the
/// palette; a CGA from its Color Select register.
fn render_cga_mode4(canvas: &mut [u8], vram: &[u8], bus: &Bus) {
    let Some(start) = cga_start(bus) else { return };
    if let Some(decoder) = bus.vga.composite_decoder() {
        // On a composite monitor: each pixel is two of the card's 640
        // samples a line, and the background is the border too.
        let indices = bus.vga.cga_indices_4();
        let border = bus.vga.cga_color & 0x0F;
        render_cga_composite(canvas, vram, start, decoder, border, |byte, samples| {
            for p in 0..4 {
                let color = indices[((byte >> (6 - p * 2)) & 3) as usize];
                samples[p * 2] = color;
                samples[p * 2 + 1] = color;
            }
        });
        return;
    }
    let colors: [(u8, u8, u8); 4] = match bus.vga.adapter {
        adapter::Adapter::Cga => bus.vga.cga_colors_4(),
        _ => std::array::from_fn(|pixel| bus.vga.attribute_rgb(pixel as u8)),
    };

    for y in 0..200 {
        // Determine memory offset based on interleave
        let bank_offset = if y % 2 == 0 { 0 } else { 0x2000 };
        let line_offset = bank_offset + ((y / 2) * 80);

        for byte_idx in 0..80 {
            let byte = vram[bank_offset + ((line_offset - bank_offset + start + byte_idx) & 0x1FFF)];

            // 4 pixels per byte (2 bits each)
            for p in 0..4 {
                // High bits are leftmost pixel
                let shift = 6 - (p * 2);
                let rgb = colors[((byte >> shift) & 0x03) as usize];

                let x = (byte_idx * 4) + p;

                // Scale 2x2
                for dy in 0..2 {
                    for dx in 0..2 {
                        let target_x = x * 2 + dx;
                        let target_y = y * 2 + dy;
                        let idx = (target_y * SCREEN_WIDTH as usize + target_x) * 3;
                        if idx + 2 < canvas.len() {
                            canvas[idx] = rgb.0;
                            canvas[idx + 1] = rgb.1;
                            canvas[idx + 2] = rgb.2;
                        }
                    }
                }
            }
        }
    }
}

/// CGA mode 6 (640x200, 2 colors): a bit a pixel, interleaved like mode
/// 4, in palette registers 0 and 1 (on a CGA black and the Color Select
/// register's color).
fn render_cga_mode6(canvas: &mut [u8], vram: &[u8], bus: &Bus) {
    let Some(start) = cga_start(bus) else { return };
    if let Some(decoder) = bus.vga.composite_decoder() {
        // On a composite monitor: a sample a pixel, in the Color Select
        // register's colour on black; the artifact colours come from the
        // patterns.
        let fg = bus.vga.cga_color & 0x0F;
        render_cga_composite(canvas, vram, start, decoder, 0, |byte, samples| {
            for (p, sample) in samples.iter_mut().enumerate() {
                *sample = if (byte >> (7 - p)) & 1 != 0 { fg } else { 0 };
            }
        });
        return;
    }
    let [bg, fg] = match bus.vga.adapter {
        adapter::Adapter::Cga => bus.vga.cga_colors_2(),
        _ => [bus.vga.attribute_rgb(0), bus.vga.attribute_rgb(1)],
    };

    for y in 0..200 {
        let bank_offset = if y % 2 == 0 { 0 } else { 0x2000 };
        let line_offset = bank_offset + ((y / 2) * 80);

        for byte_idx in 0..80 {
            let byte = vram[bank_offset + ((line_offset - bank_offset + start + byte_idx) & 0x1FFF)];

            // 8 pixels per byte (1 bit each)
            for p in 0..8 {
                let shift = 7 - p;
                let on = (byte >> shift) & 0x01 == 1;
                let rgb = if on { fg } else { bg };

                let x = (byte_idx * 8) + p;

                // Scale 1x horizontal, 2x vertical (to get 640x400)
                for dy in 0..2 {
                    let target_y = y * 2 + dy;
                    let idx = (target_y * SCREEN_WIDTH as usize + x) * 3;
                    if idx + 2 < canvas.len() {
                        canvas[idx] = rgb.0;
                        canvas[idx + 1] = rgb.1;
                        canvas[idx + 2] = rgb.2;
                    }
                }
            }
        }
    }
}

/// The CGA's graphics through the composite decoder: each of the 200 lines
/// (interleaved like modes 4 to 6) as the 640 samples `samples` makes of
/// each of its 80 bytes (8 a byte), decoded to 640 pixels and drawn twice
/// for 400 lines.
fn render_cga_composite(
    canvas: &mut [u8],
    vram: &[u8],
    start: usize,
    decoder: &composite::Decoder,
    border: u8,
    samples: impl Fn(u8, &mut [u8]),
) {
    let width = SCREEN_WIDTH as usize;
    let mut line = [0u8; 640];
    let mut rgb = [0u8; 640 * 3];
    for y in 0..200 {
        let bank_offset = if y % 2 == 0 { 0 } else { 0x2000 };
        for byte_idx in 0..80 {
            let byte = vram[bank_offset + ((y / 2 * 80 + start + byte_idx) & 0x1FFF)];
            samples(byte, &mut line[byte_idx * 8..byte_idx * 8 + 8]);
        }
        decoder.decode_line(&line, border, &mut rgb);
        for dy in 0..2 {
            let row = (y * 2 + dy) * width * 3;
            if row + rgb.len() <= canvas.len() {
                canvas[row..row + rgb.len()].copy_from_slice(&rgb);
            }
        }
    }
}

// Prints a character and advances cursor, handling scrolling
pub fn print_char(bus: &mut Bus, ascii: u8) {
    match ascii {
        0x0D => {
            // Carriage Return (\r)
            bus.cursor_x = 0;
        }
        0x0A => {
            // Line Feed (\n)
            bus.cursor_y += 1;
        }
        0x08 => {
            // Backspace
            if bus.cursor_x > 0 {
                bus.cursor_x -= 1;
                // Visually clear the character
                let offset = (bus.cursor_y * 80 + bus.cursor_x) * 2;
                bus.text_mem_mut()[offset] = 0x20; // Space
                bus.mark_text_dirty(offset, offset + 2);
            }
        }
        _ => {
            // Print standard character
            let offset = (bus.cursor_y * 80 + bus.cursor_x) * 2;
            let text = bus.text_mem_mut();
            text[offset] = ascii;
            text[offset + 1] = 0x07; // Light Gray Attribute
            bus.cursor_x += 1;
            bus.mark_text_dirty(offset, offset + 2);
        }
    }

    // Handle Line Wrap
    if bus.cursor_x >= 80 {
        bus.cursor_x = 0;
        bus.cursor_y += 1;
    }

    // Handle Scrolling, using the row count from BDA so 80x43 / 80x50 modes
    // get proper scroll behaviour rather than being clamped to 25.
    let rows = bus.text_rows();
    if bus.cursor_y >= rows {
        bus.scroll_up();
        bus.cursor_y = rows - 1;
    }
    // The BIOS's cursor follows, as `print_cells` keeps it.
    let (col, row) = (bus.cursor_x as u8, bus.cursor_y as u8);
    bus.write_8(0x0450, col);
    bus.write_8(0x0451, row);
}

pub fn print_string(cpu: &mut Cpu, s: &str) {
    print_cells(cpu, s.chars().map(|c| c as u8), 0x07);
}

/// Print code page 437 characters in the colors of `attr`, as
/// `print_string` prints in light gray.
pub fn print_cp437(cpu: &mut Cpu, text: &[u8], attr: u8) {
    print_cells(cpu, text.iter().copied(), attr);
}

fn print_cells(cpu: &mut Cpu, text: impl Iterator<Item = u8>, attr: u8) {
    // A built-in command's output redirected to a file.
    if let Some(captured) = cpu.stdout_capture.as_mut() {
        captured.extend(text);
        return;
    }
    let mut col = cpu.bus.cursor_x;
    let mut row = cpu.bus.cursor_y;
    let max_cols = 80;
    let max_rows = cpu.bus.text_rows();
    // Text VRAM range written directly below, for the dirty-rect renderer
    let mut touched: Option<(usize, usize)> = None;
    let mut scrolled = false;
    let mut touch = |offset: usize| {
        touched = Some(match touched {
            Some((lo, hi)) => (lo.min(offset), hi.max(offset + 2)),
            None => (offset, offset + 2),
        });
    };

    for c in text {
        match c {
            b'\r' => {
                col = 0;
            }
            b'\n' => {
                row += 1;
            }
            0x08 => {
                // Backspace
                if col > 0 {
                    col -= 1;
                    // Visual Erase (Space + Light Gray)
                    let offset = (row * max_cols + col) * 2;
                    let text = cpu.bus.text_mem_mut();
                    if offset + 1 < text.len() {
                        text[offset] = 0x20;
                        text[offset + 1] = 0x07;
                        touch(offset);
                    }
                }
            }
            _ => {
                // Printable Character
                let offset = (row * max_cols + col) * 2;
                let text = cpu.bus.text_mem_mut();
                if offset + 1 < text.len() {
                    text[offset] = c;
                    text[offset + 1] = attr;
                    touch(offset);
                }
                col += 1;
            }
        }

        // Handle Wrapping
        if col >= max_cols {
            col = 0;
            row += 1;
        }

        // Handle Scrolling
        if row >= max_rows {
            // Scroll Up Logic (Direct Memory Move)
            let row_size = max_cols * 2;
            let screen_size = max_rows * row_size;

            // Shift everything up by one row
            let text = cpu.bus.text_mem_mut();
            text.copy_within(row_size..screen_size, 0);

            // Clear bottom row: spaces in light grey.
            for (i, byte) in text[screen_size - row_size..screen_size].iter_mut().enumerate() {
                *byte = if i % 2 == 0 { 0x20 } else { 0x07 };
            }

            row = max_rows - 1;
            scrolled = true;
        }
    }

    // Update Internal Bus State
    cpu.bus.cursor_x = col;
    cpu.bus.cursor_y = row;

    // Update BIOS Data Area (BDA)
    // The Assembly Shell reads [0x0450] to know where to print the next prompt.
    // If we don't update this, the shell will print over our output.
    cpu.bus.write_8(0x0450, col as u8);
    cpu.bus.write_8(0x0451, row as u8);

    if scrolled {
        cpu.bus.vga.mark_dirty_full();
    } else if let Some((start, end)) = touched {
        cpu.bus.mark_text_dirty(start, end);
    }
}

/// Saved as its number.
impl crate::savestate::State for VideoMode {
    fn save(&self, w: &mut crate::savestate::Writer) {
        (*self as u8).save(w);
    }
    fn load(&mut self, r: &mut crate::savestate::Reader) -> crate::savestate::Result<()> {
        use VideoMode::*;
        const ALL: &[VideoMode] = &[
            Text40x25, Text40x25Color, Text80x25, Text80x25Color, Cga320x200Color, Cga320x200, Cga640x200,
            Mono80x25, Tandy160x200x16, Tandy320x200x16, Tandy640x200x4, Ega320x200, Ega640x200, Ega640x350Mono,
            Ega640x350, Vga640x480Mono, Vga640x480, Graphics320x200, HercGraphics, Vesa,
        ];
        let mut number = 0u8;
        number.load(r)?;
        *self = ALL
            .iter()
            .copied()
            .find(|&m| m as u8 == number)
            .ok_or_else(|| crate::savestate::StateError::Invalid(format!("video mode {:X}h", number)))?;
        Ok(())
    }
}
