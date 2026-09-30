//! An Epson ESC/P 2 printer, as DOSBox-X's virtual printer (printer.cpp)
//! has it: the control codes and ESC and FS commands of the LQ printers,
//! text in the typefaces, pitches, sizes and styles they select, 8- and
//! 24-pin bit images, and colour. It prints onto a `Page`, and pages it
//! fed out wait in `done` for the output to take them.
//!
//! Some things differ from DOSBox-X: the ESC ( commands it doesn't know
//! are skipped with their data, tab stops take all their positions, a
//! tab goes to the next stop rather than the last, FS commands that are
//! ESC ones do what those do, ESC @ leaves the page as it is, and the
//! dots of every bit image mode have the size of its density.

use super::font::{Face, Fonts, Style};
use super::page::{BLACK, FULL, Page};
use std::path::PathBuf;

const STYLE_PROP: u16 = 0x01;
const STYLE_CONDENSED: u16 = 0x02;
const STYLE_BOLD: u16 = 0x04;
const STYLE_DOUBLESTRIKE: u16 = 0x08;
const STYLE_DOUBLEWIDTH: u16 = 0x10;
const STYLE_ITALICS: u16 = 0x20;
const STYLE_UNDERLINE: u16 = 0x40;
const STYLE_SUPERSCRIPT: u16 = 0x80;
const STYLE_SUBSCRIPT: u16 = 0x100;
const STYLE_STRIKETHROUGH: u16 = 0x200;
const STYLE_OVERSCORE: u16 = 0x400;
const STYLE_DOUBLEWIDTHONELINE: u16 = 0x800;
const STYLE_DOUBLEHEIGHT: u16 = 0x1000;

const SCORE_NONE: u8 = 0x00;
const SCORE_SINGLE: u8 = 0x01;
const SCORE_DOUBLE: u8 = 0x02;
const SCORE_SINGLEBROKEN: u8 = 0x05;
const SCORE_DOUBLEBROKEN: u8 = 0x06;

/// The commands' numbers past the ESC ones: FS commands, ESC ( ones,
/// ESC C NUL, and data to skip.
const FS_COMMAND: u16 = 0x800;
const ESC_PAREN_COMMAND: u16 = 0x200;
const ESC_PAGE_INCHES: u16 = 0x100;
const ESC_SKIP_VARIABLE: u16 = 0x101;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Quality {
    Draft,
    Lq,
}

/// A bit image being received (ESC K, L, Y, Z and *, FS Z).
#[derive(Clone, Copy, Debug, Default)]
struct BitImage {
    /// Dots per inch across and down.
    horiz: u16,
    vert: u16,
    /// Bytes a column (1, 3 or 6).
    column_bytes: usize,
    /// Bytes still to come.
    remaining: usize,
    column: [u8; 6],
    read: usize,
}

/// The international character sets of ESC R: what they print for
/// # $ @ [ \ ] ^ ` { | } ~. Those DOSBox-X doesn't have print as the USA's.
const INTERNATIONAL: [[u16; 12]; 15] = {
    const USA: [u16; 12] = [0x23, 0x24, 0x40, 0x5b, 0x5c, 0x5d, 0x5e, 0x60, 0x7b, 0x7c, 0x7d, 0x7e];
    let mut sets = [USA; 15];
    sets[1] = [0x23, 0x24, 0xe0, 0xba, 0xe7, 0xa7, 0x5e, 0x60, 0xe9, 0xf9, 0xe8, 0xa8]; // France
    sets[2] = [0x23, 0x24, 0xa7, 0xc4, 0xd6, 0xdc, 0x5e, 0x60, 0xe4, 0xf6, 0xfc, 0xdf]; // Germany
    sets[3] = [0xa3, 0x24, 0x40, 0x5b, 0x5c, 0x5d, 0x5e, 0x60, 0x7b, 0x7c, 0x7d, 0x7e]; // UK
    sets[14] = [0x23, 0x24, 0xa7, 0xc4, 0x27, 0x22, 0xb6, 0x60, 0xa9, 0xae, 0x2020, 0x2122]; // Legal
    sets
};
const INTERNATIONAL_CODES: [usize; 12] = [0x23, 0x24, 0x40, 0x5b, 0x5c, 0x5d, 0x5e, 0x60, 0x7b, 0x7c, 0x7d, 0x7e];

/// The code pages of ESC ( t, by their number there. Only code page 437
/// prints as itself; the others print as it.
const CODEPAGES: [u16; 15] = [0, 437, 932, 850, 851, 853, 855, 860, 863, 865, 852, 857, 862, 864, 866];

pub struct Escp {
    fonts: Fonts,
    dpi: u32,
    page: Page,
    /// Pages fed out, for the output to take.
    pub done: Vec<Page>,
    /// Commands it didn't know, for the log.
    pub unknown: Vec<String>,
    default_width: f64,
    default_height: f64,
    color: u8,
    /// The print head, in inches from the sheet's top left corner.
    cur_x: f64,
    cur_y: f64,
    esc_cmd: u16,
    esc_seen: bool,
    fs_seen: bool,
    num_param: usize,
    needed_param: usize,
    params: [u8; 20],
    variable_length: u16,
    variable_count: u16,
    style: u16,
    cpi: f64,
    actcpi: f64,
    score: u8,
    top_margin: f64,
    bottom_margin: f64,
    right_margin: f64,
    left_margin: f64,
    page_width: f64,
    page_height: f64,
    line_spacing: f64,
    horiz_tabs: [f64; 32],
    num_horiz_tabs: usize,
    vert_tabs: [f64; 16],
    /// 255: none set since the reset.
    num_vert_tabs: usize,
    cur_char_table: usize,
    quality: Quality,
    typeface: u8,
    extra_intra_space: f64,
    pub auto_feed: bool,
    bit_image: BitImage,
    dens_k: u8,
    dens_l: u8,
    dens_y: u8,
    dens_z: u8,
    cur_map: [char; 256],
    char_tables: [u16; 4],
    /// The unit of ESC/P 2's commands, in inches (negative: theirs).
    defined_unit: f64,
    multipoint: bool,
    multi_point_size: f64,
    multicpi: f64,
    /// Horizontal motion index, in inches (negative: from the pitch).
    hmi: f64,
    /// 255: the top bit as it comes; 0 or 1: always that.
    msb: u8,
    num_print_as_char: u16,
    font: Style,
}

impl Escp {
    /// A printer with paper `width` by `height` inches in it, printing at
    /// `dpi`.
    pub fn new(width: f64, height: f64, dpi: u32, fontpath: Option<PathBuf>) -> Self {
        let mut escp = Self {
            fonts: Fonts::new(fontpath, dpi),
            dpi,
            page: Page::new(width, height, dpi),
            done: Vec::new(),
            unknown: Vec::new(),
            default_width: width,
            default_height: height,
            color: BLACK,
            cur_x: 0.0,
            cur_y: 0.0,
            esc_cmd: 0,
            esc_seen: false,
            fs_seen: false,
            num_param: 0,
            needed_param: 0,
            params: [0; 20],
            variable_length: 0,
            variable_count: 0,
            style: 0,
            cpi: 10.0,
            actcpi: 10.0,
            score: SCORE_NONE,
            top_margin: 0.0,
            bottom_margin: height,
            right_margin: width,
            left_margin: 0.0,
            page_width: width,
            page_height: height,
            line_spacing: 1.0 / 6.0,
            horiz_tabs: [0.0; 32],
            num_horiz_tabs: 0,
            vert_tabs: [0.0; 16],
            num_vert_tabs: 255,
            cur_char_table: 1,
            quality: Quality::Lq,
            typeface: 2,
            extra_intra_space: 0.0,
            auto_feed: false,
            bit_image: BitImage::default(),
            dens_k: 0,
            dens_l: 1,
            dens_y: 2,
            dens_z: 3,
            cur_map: crate::video::CP437,
            char_tables: [0, 437, 437, 437],
            defined_unit: -1.0,
            multipoint: false,
            multi_point_size: 0.0,
            multicpi: 0.0,
            hmi: -1.0,
            msb: 255,
            num_print_as_char: 0,
            font: Style { face: Face::Courier, h_points: 10.5, v_points: 10.5, italic: false },
        };
        escp.reset();
        escp
    }

    /// Back to the settings the printer is switched on with (ESC @, and
    /// INIT on the port). What is printed stays on the page.
    pub fn reset(&mut self) {
        self.color = BLACK;
        self.cur_x = 0.0;
        self.esc_seen = false;
        self.fs_seen = false;
        self.esc_cmd = 0;
        self.num_param = 0;
        self.needed_param = 0;
        self.top_margin = 0.0;
        self.left_margin = 0.0;
        self.right_margin = self.default_width;
        self.page_width = self.default_width;
        self.bottom_margin = self.default_height;
        self.page_height = self.default_height;
        self.line_spacing = 1.0 / 6.0;
        self.cpi = 10.0;
        self.cur_char_table = 1;
        self.style = 0;
        self.extra_intra_space = 0.0;
        self.bit_image.remaining = 0;
        self.dens_k = 0;
        self.dens_l = 1;
        self.dens_y = 2;
        self.dens_z = 3;
        self.char_tables = [0, 437, 437, 437];
        self.defined_unit = -1.0;
        self.multipoint = false;
        self.multi_point_size = 0.0;
        self.multicpi = 0.0;
        self.hmi = -1.0;
        self.msb = 255;
        self.num_print_as_char = 0;
        self.typeface = 2;
        self.select_codepage(self.char_tables[self.cur_char_table]);
        self.update_font();
        // Tab stops every eight characters.
        for (i, tab) in self.horiz_tabs.iter_mut().enumerate() {
            *tab = i as f64 * 8.0 / self.cpi;
        }
        self.num_horiz_tabs = 32;
        self.num_vert_tabs = 255;
    }

    /// Whether nothing is printed on the page in the printer.
    pub fn is_blank(&self) -> bool {
        self.page.is_blank()
    }

    /// Feed the page out if anything is printed on it (the eject button,
    /// and the end of a job).
    pub fn form_feed(&mut self) {
        let print = !self.page.is_blank();
        self.new_page(print, true);
    }

    fn select_codepage(&mut self, cp: u16) {
        // Code page 437 (and the italic table, 0), whatever was asked for.
        let _ = cp;
        self.cur_map = crate::video::CP437;
    }

    fn pix_x(&self) -> i64 {
        (self.cur_x * self.dpi as f64 + 0.5).floor() as i64
    }

    fn pix_y(&self) -> i64 {
        (self.cur_y * self.dpi as f64 + 0.5).floor() as i64
    }

    /// Pick the font and pitch the style, pitch and size ask for.
    fn update_font(&mut self) {
        let mut horiz = 10.5;
        let mut vert = 10.5;
        let style = self.style;
        if !self.multipoint {
            self.actcpi = self.cpi;
            if style & STYLE_CONDENSED == 0 {
                horiz *= 10.0 / self.cpi;
                vert *= 10.0 / self.cpi;
            }
            if style & STYLE_PROP == 0 {
                if self.cpi == 10.0 && style & STYLE_CONDENSED != 0 {
                    self.actcpi = 17.14;
                    horiz *= 10.0 / 17.14;
                }
                if self.cpi == 12.0 && style & STYLE_CONDENSED != 0 {
                    self.actcpi = 20.0;
                    horiz *= 10.0 / 20.0;
                    vert *= 10.0 / 12.0;
                }
            } else if style & STYLE_CONDENSED != 0 {
                horiz /= 2.0;
            }
            if style & (STYLE_DOUBLEWIDTH | STYLE_DOUBLEWIDTHONELINE) != 0 {
                self.actcpi /= 2.0;
                horiz *= 2.0;
            }
            if style & STYLE_DOUBLEHEIGHT != 0 {
                vert *= 2.0;
            }
        } else {
            self.actcpi = self.multicpi;
            horiz = self.multi_point_size;
            vert = self.multi_point_size;
        }
        if style & (STYLE_SUPERSCRIPT | STYLE_SUBSCRIPT) != 0 {
            horiz *= 2.0 / 3.0;
            vert *= 2.0 / 3.0;
            self.actcpi /= 2.0 / 3.0;
        }
        let face = match self.typeface {
            1 => Face::Sans,
            2 => Face::Courier,
            4 => Face::Script,
            5 | 6 => Face::Ocr,
            _ => Face::Roman,
        };
        let italic = style & STYLE_ITALICS != 0 || self.char_tables[self.cur_char_table] == 0;
        self.font = Style { face, h_points: horiz, v_points: vert, italic };
    }

    fn param16(&self, i: usize) -> u16 {
        u16::from_le_bytes([self.params[i], self.params[i + 1]])
    }

    /// The next line, and the next page past the bottom margin.
    fn line_feed(&mut self) {
        self.cur_x = self.left_margin;
        self.cur_y += self.line_spacing;
        if self.cur_y > self.bottom_margin - 0.0001 {
            self.new_page(true, false);
        }
    }

    fn end_one_line_double_width(&mut self) {
        if self.style & STYLE_DOUBLEWIDTHONELINE != 0 {
            self.style &= !STYLE_DOUBLEWIDTHONELINE;
            self.update_font();
        }
    }

    /// Take a byte for a command, if it is part of one. False for a
    /// byte to print.
    fn command(&mut self, ch: u8) -> bool {
        if self.esc_cmd == ESC_SKIP_VARIABLE && self.variable_length != 0 {
            self.variable_count += 1;
            if self.variable_count >= self.variable_length {
                self.esc_cmd = 0;
                self.needed_param = 0;
                self.num_param = 0;
                self.variable_length = 0;
                self.variable_count = 0;
            }
            return true;
        }

        if self.esc_seen || self.fs_seen {
            self.esc_cmd = ch as u16;
            if self.fs_seen {
                self.esc_cmd |= FS_COMMAND;
            }
            self.esc_seen = false;
            self.fs_seen = false;
            self.num_param = 0;
            self.needed_param = match self.esc_cmd {
                0x02 | 0x0a | 0x0c | 0x0e | 0x0f | 0x23 | 0x30 | 0x31 | 0x32 | 0x34 | 0x35 | 0x36 | 0x37 | 0x38
                | 0x39 | 0x3c | 0x3d | 0x3e | 0x40 | 0x45 | 0x46 | 0x47 | 0x48 | 0x4d | 0x4f | 0x50 | 0x54 | 0x5e
                | 0x67 | 0x834 | 0x835 | 0x846 | 0x852 => 0,
                0x19 | 0x20 | 0x21 | 0x2b | 0x2d | 0x2f | 0x33 | 0x41 | 0x43 | 0x49 | 0x4a | 0x4e | 0x51 | 0x52
                | 0x53 | 0x55 | 0x57 | 0x61 | 0x66 | 0x68 | 0x69 | 0x6a | 0x6b | 0x6c | 0x70 | 0x72 | 0x73 | 0x74
                | 0x77 | 0x78 | 0x7e | 0x832 | 0x833 | 0x841 | 0x843 | 0x845 | 0x849 | 0x853 | 0x856 => 1,
                0x24 | 0x3f | 0x4b | 0x4c | 0x59 | 0x5a | 0x5c | 0x63 | 0x65 | 0x85a => 2,
                0x2a | 0x58 => 3,
                0x5b => 7,
                // Vertical tabs (ESC b's channel first), horizontal tabs:
                // the stops follow up to a NUL.
                0x62 | 0x42 => {
                    self.num_vert_tabs = 0;
                    return true;
                }
                0x44 => {
                    self.num_horiz_tabs = 0;
                    return true;
                }
                0x25 | 0x26 | 0x3a => {
                    self.note("user-defined characters");
                    self.esc_cmd = 0;
                    return true;
                }
                0x28 => return true,
                _ => {
                    let what = format!("{} {:02X}h", if self.fs_command() { "FS" } else { "ESC" }, self.esc_cmd & 0xFF);
                    self.note(&what);
                    self.esc_cmd = 0;
                    return true;
                }
            };
            if self.needed_param > 0 {
                return true;
            }
        }

        // ESC ( commands: their second byte, then a length word first in
        // their parameters.
        if self.esc_cmd == b'(' as u16 {
            self.esc_cmd = ESC_PAREN_COMMAND | ch as u16;
            self.needed_param = match self.esc_cmd {
                0x242 | 0x25e => 2,
                0x255 => 3,
                0x243 | 0x256 | 0x276 => 4,
                0x274 | 0x22d => 5,
                0x263 => 6,
                _ => {
                    let what = format!("ESC ( {:02X}h", ch);
                    self.note(&what);
                    self.esc_cmd = ESC_SKIP_VARIABLE;
                    2
                }
            };
            return true;
        }

        // ESC b's channel.
        if self.esc_cmd == 0x62 {
            self.esc_cmd = 0x42;
            return true;
        }
        if self.esc_cmd == 0x42 {
            let at = ch as f64 * self.line_spacing;
            if ch == 0 || (self.num_vert_tabs > 0 && self.vert_tabs[self.num_vert_tabs - 1] > at) {
                self.esc_cmd = 0;
            } else if self.num_vert_tabs < 16 {
                self.vert_tabs[self.num_vert_tabs] = at;
                self.num_vert_tabs += 1;
            }
            return true;
        }
        if self.esc_cmd == 0x44 {
            let at = ch as f64 / self.cpi;
            if ch == 0 || (self.num_horiz_tabs > 0 && self.horiz_tabs[self.num_horiz_tabs - 1] > at) {
                self.esc_cmd = 0;
            } else if self.num_horiz_tabs < 32 {
                self.horiz_tabs[self.num_horiz_tabs] = at;
                self.num_horiz_tabs += 1;
            }
            return true;
        }

        if self.num_param < self.needed_param {
            self.params[self.num_param] = ch;
            self.num_param += 1;
            if self.num_param < self.needed_param {
                return true;
            }
        }

        if self.esc_cmd != 0 {
            let cmd = self.esc_cmd;
            self.esc_cmd = 0;
            self.execute(cmd);
            return true;
        }

        self.control(ch)
    }

    fn fs_command(&self) -> bool {
        self.esc_cmd & FS_COMMAND != 0
    }

    fn note(&mut self, what: &str) {
        if self.unknown.len() < 64 {
            self.unknown.push(what.to_string());
        }
    }

    /// Carry out the ESC, ESC ( or FS command `cmd` with its parameters.
    fn execute(&mut self, cmd: u16) {
        let p = self.params;
        let on = |v: u8| v == 1 || v == b'1';
        let off = |v: u8| v == 0 || v == b'0';
        // The FS commands that are ESC ones.
        let cmd = match cmd {
            0x832 => 0x32,
            0x833 => 0x2b,
            0x834 => 0x34,
            0x835 => 0x35,
            0x841 => 0x41,
            0x843 => 0x6b,
            0x849 => 0x74,
            0x856 => 0x77,
            c => c,
        };
        match cmd {
            0x02 | 0x2f | 0x3c | 0x55 | 0x61 | 0x73 | 0x36 | 0x37 | 0x38 | 0x39 | 0x7e | 0x69 => {}
            0x0e => {
                // Double width for the line (ESC SO).
                if !self.multipoint {
                    self.hmi = -1.0;
                    self.style |= STYLE_DOUBLEWIDTHONELINE;
                    self.update_font();
                }
            }
            0x0f => self.condensed(),
            0x19 => {
                // Paper loading and ejecting (ESC EM): R ejects.
                if p[0] == b'R' {
                    self.new_page(true, false);
                }
            }
            0x20 => {
                // Space between characters (ESC SP).
                if !self.multipoint {
                    self.extra_intra_space = p[0] as f64 / if self.quality == Quality::Draft { 120.0 } else { 180.0 };
                    self.hmi = -1.0;
                    self.update_font();
                }
            }
            0x21 => {
                // Master select (ESC !).
                self.cpi = if p[0] & 0x01 != 0 { 12.0 } else { 10.0 };
                self.style &= 0xFF80;
                for (bit, style) in [
                    (0x02, STYLE_PROP),
                    (0x04, STYLE_CONDENSED),
                    (0x08, STYLE_BOLD),
                    (0x10, STYLE_DOUBLESTRIKE),
                    (0x20, STYLE_DOUBLEWIDTH),
                    (0x40, STYLE_ITALICS),
                ] {
                    if p[0] & bit != 0 {
                        self.style |= style;
                    }
                }
                if p[0] & 0x80 != 0 {
                    self.score = SCORE_SINGLE;
                    self.style |= STYLE_UNDERLINE;
                }
                self.hmi = -1.0;
                self.multipoint = false;
                self.update_font();
            }
            0x23 => self.msb = 255,
            0x24 => {
                // Absolute horizontal position (ESC $), in 1/60 inch.
                let unit = if self.defined_unit < 0.0 { 1.0 / 60.0 } else { self.defined_unit };
                let x = self.left_margin + self.param16(0) as f64 * unit;
                if x <= self.right_margin {
                    self.cur_x = x;
                }
            }
            0x85a => self.setup_bit_image(40, self.param16(0)),
            0x2a => self.setup_bit_image(p[0], self.param16(1)),
            0x2b => self.line_spacing = p[0] as f64 / 360.0,
            0x2d => {
                // Underline (ESC -).
                if off(p[0]) {
                    self.style &= !STYLE_UNDERLINE;
                }
                if on(p[0]) {
                    self.style |= STYLE_UNDERLINE;
                    self.score = SCORE_SINGLE;
                }
                self.update_font();
            }
            0x30 => self.line_spacing = 1.0 / 8.0,
            0x31 => self.line_spacing = 7.0 / 72.0,
            0x32 => self.line_spacing = 1.0 / 6.0,
            0x33 => self.line_spacing = p[0] as f64 / 180.0,
            0x34 => {
                self.style |= STYLE_ITALICS;
                self.update_font();
            }
            0x35 => {
                self.style &= !STYLE_ITALICS;
                self.update_font();
            }
            0x3d => self.msb = 0,
            0x3e => self.msb = 1,
            0x3f => {
                // Reassign a bit image mode (ESC ?).
                match p[0] {
                    b'K' => self.dens_k = p[1],
                    b'L' => self.dens_l = p[1],
                    b'Y' => self.dens_y = p[1],
                    b'Z' => self.dens_z = p[1],
                    _ => {}
                }
            }
            0x40 => self.reset(),
            0x41 => self.line_spacing = p[0] as f64 / 60.0,
            0x43 => {
                // Page length in lines (ESC C n), or in inches (ESC C NUL n).
                if p[0] != 0 {
                    self.page_height = p[0] as f64 * self.line_spacing;
                    self.bottom_margin = self.page_height;
                } else {
                    self.needed_param = 1;
                    self.num_param = 0;
                    self.esc_cmd = ESC_PAGE_INCHES;
                }
            }
            ESC_PAGE_INCHES => {
                self.page_height = p[0] as f64;
                self.bottom_margin = self.page_height;
                self.top_margin = 0.0;
            }
            0x45 => {
                self.style |= STYLE_BOLD;
                self.update_font();
            }
            0x46 => {
                self.style &= !STYLE_BOLD;
                self.update_font();
            }
            0x47 => self.style |= STYLE_DOUBLESTRIKE,
            0x48 => self.style &= !STYLE_DOUBLESTRIKE,
            0x4a => {
                // Advance n/180 inch (ESC J).
                self.cur_y += p[0] as f64 / 180.0;
                if self.cur_y > self.bottom_margin {
                    self.new_page(true, false);
                }
            }
            0x4b => self.setup_bit_image(self.dens_k, self.param16(0)),
            0x4c => self.setup_bit_image(self.dens_l, self.param16(0)),
            0x59 => self.setup_bit_image(self.dens_y, self.param16(0)),
            0x5a => self.setup_bit_image(self.dens_z, self.param16(0)),
            0x4d => self.pitch(12.0),
            0x50 => self.pitch(10.0),
            0x67 => self.pitch(15.0),
            0x4e => {
                // Bottom margin (ESC N), in lines.
                self.top_margin = 0.0;
                self.bottom_margin = p[0] as f64 * self.line_spacing;
            }
            0x4f => {
                self.top_margin = 0.0;
                self.bottom_margin = self.page_height;
            }
            0x51 => self.right_margin = (p[0] as f64 - 1.0) / self.cpi,
            0x52 => {
                // International character set (ESC R).
                if p[0] <= 13 || p[0] == 64 {
                    let set = if p[0] == 64 { 14 } else { p[0] as usize };
                    for (code, &u) in INTERNATIONAL_CODES.iter().zip(&INTERNATIONAL[set]) {
                        self.cur_map[*code] = char::from_u32(u as u32).unwrap_or('?');
                    }
                }
            }
            0x53 => {
                if off(p[0]) {
                    self.style |= STYLE_SUBSCRIPT;
                }
                if on(p[0]) {
                    self.style |= STYLE_SUPERSCRIPT;
                }
                self.update_font();
            }
            0x54 => {
                self.style &= !(STYLE_SUPERSCRIPT | STYLE_SUBSCRIPT);
                self.update_font();
            }
            0x57 => {
                // Double width (ESC W).
                if !self.multipoint {
                    self.hmi = -1.0;
                    if off(p[0]) {
                        self.style &= !STYLE_DOUBLEWIDTH;
                    }
                    if on(p[0]) {
                        self.style |= STYLE_DOUBLEWIDTH;
                    }
                    self.update_font();
                }
            }
            0x58 => {
                // Pitch and point (ESC X).
                self.multipoint = true;
                if self.multicpi == 0.0 {
                    self.multicpi = self.cpi;
                }
                if p[0] == 1 {
                    self.style |= STYLE_PROP;
                } else if p[0] >= 5 {
                    self.multicpi = 360.0 / p[0] as f64;
                }
                if self.multi_point_size == 0.0 {
                    self.multi_point_size = 10.5;
                }
                if self.param16(1) > 0 {
                    self.multi_point_size = self.param16(1) as f64 / 2.0;
                }
                self.update_font();
            }
            0x5c => {
                // Relative horizontal position (ESC \).
                let moved = self.param16(0) as i16 as f64;
                let unit = if self.defined_unit >= 0.0 {
                    self.defined_unit
                } else if self.quality == Quality::Draft {
                    1.0 / 120.0
                } else {
                    1.0 / 180.0
                };
                self.cur_x += moved * unit;
            }
            0x63 => {
                // Horizontal motion index (ESC c).
                self.hmi = self.param16(0) as f64 / 360.0;
                self.extra_intra_space = 0.0;
            }
            0x846 => self.line_spacing = self.line_spacing.abs(),
            0x6a => {
                // Reverse feed n/216 inch (ESC j).
                self.cur_y = (self.cur_y - self.param16(0) as f64 / 216.0).max(self.top_margin);
            }
            0x6b => {
                // Typeface (ESC k).
                if p[0] <= 11 || p[0] == 30 || p[0] == 31 {
                    self.typeface = p[0];
                }
                self.update_font();
            }
            0x6c => {
                self.left_margin = (p[0] as f64 - 1.0).max(0.0) / self.cpi;
                if self.cur_x < self.left_margin {
                    self.cur_x = self.left_margin;
                }
            }
            0x70 => {
                // Proportional (ESC p).
                if off(p[0]) {
                    self.style &= !STYLE_PROP;
                }
                if on(p[0]) {
                    self.style |= STYLE_PROP;
                    self.quality = Quality::Lq;
                }
                self.multipoint = false;
                self.hmi = -1.0;
                self.update_font();
            }
            0x72 => {
                // Colour (ESC r): 0 black, 1 magenta, 2 cyan, 3 violet,
                // 4 yellow, 5 red, 6 green.
                self.color = if p[0] == 0 || p[0] > 6 { BLACK } else { p[0] << 5 };
            }
            0x74 => {
                // Character table (ESC t).
                if p[0] < 4 {
                    self.cur_char_table = p[0] as usize;
                }
                if (48..=51).contains(&p[0]) {
                    self.cur_char_table = (p[0] - 48) as usize;
                }
                self.select_codepage(self.char_tables[self.cur_char_table]);
                self.update_font();
            }
            0x77 => {
                // Double height (ESC w).
                if !self.multipoint {
                    if off(p[0]) {
                        self.style &= !STYLE_DOUBLEHEIGHT;
                    }
                    if on(p[0]) {
                        self.style |= STYLE_DOUBLEHEIGHT;
                    }
                    self.update_font();
                }
            }
            0x78 => {
                // Draft or letter quality (ESC x).
                if off(p[0]) {
                    self.quality = Quality::Draft;
                    self.style |= STYLE_CONDENSED;
                }
                if on(p[0]) {
                    self.quality = Quality::Lq;
                    self.style &= !STYLE_CONDENSED;
                }
                self.hmi = -1.0;
                self.update_font();
            }
            0x274 => {
                // Assign a character table (ESC ( t).
                if p[2] < 4 && p[3] < 15 {
                    self.char_tables[p[2] as usize] = CODEPAGES[p[3] as usize];
                    if p[2] as usize == self.cur_char_table {
                        self.select_codepage(self.char_tables[self.cur_char_table]);
                        self.update_font();
                    }
                }
            }
            0x22d => {
                // Line and score (ESC ( -).
                self.style &= !(STYLE_UNDERLINE | STYLE_STRIKETHROUGH | STYLE_OVERSCORE);
                self.score = p[4];
                if self.score != 0 {
                    match p[3] {
                        1 => self.style |= STYLE_UNDERLINE,
                        2 => self.style |= STYLE_STRIKETHROUGH,
                        3 => self.style |= STYLE_OVERSCORE,
                        _ => {}
                    }
                }
                self.update_font();
            }
            0x242 | ESC_SKIP_VARIABLE => {
                // Bar codes (ESC ( B), and the commands not known: their
                // data is skipped.
                if cmd == 0x242 {
                    self.note("bar codes");
                }
                self.variable_length = self.param16(0);
                self.variable_count = 0;
                self.needed_param = 0;
                self.num_param = 0;
                if self.variable_length > 0 {
                    self.esc_cmd = ESC_SKIP_VARIABLE;
                }
            }
            0x243 => {
                // Page length in the unit (ESC ( C).
                if self.defined_unit > 0.0 {
                    self.page_height = self.param16(2) as f64 * self.defined_unit;
                    self.bottom_margin = self.page_height;
                    self.top_margin = 0.0;
                }
            }
            0x255 => {
                // The unit (ESC ( U), in 1/3600 inch.
                if p[2] > 0 {
                    self.defined_unit = p[2] as f64 / 3600.0;
                }
            }
            0x256 => {
                // Absolute vertical position (ESC ( V).
                let unit = if self.defined_unit < 0.0 { 1.0 / 360.0 } else { self.defined_unit };
                let y = self.top_margin + self.param16(2) as f64 * unit;
                if y > self.bottom_margin {
                    self.new_page(true, false);
                } else {
                    self.cur_y = y;
                }
            }
            0x25e => self.num_print_as_char = self.param16(0),
            0x263 => {
                // Page format (ESC ( c): top and bottom margins.
                if self.defined_unit > 0.0 {
                    let top = self.param16(2) as f64 * self.defined_unit;
                    let bottom = self.param16(4) as f64 * self.defined_unit;
                    if top < bottom {
                        if top < self.page_height {
                            self.top_margin = top;
                        }
                        if bottom < self.page_height {
                            self.bottom_margin = bottom;
                        }
                        if self.top_margin > self.cur_y {
                            self.cur_y = self.top_margin;
                        }
                    }
                }
            }
            0x276 => {
                // Relative vertical position (ESC ( v).
                let unit = if self.defined_unit < 0.0 { 1.0 / 360.0 } else { self.defined_unit };
                let y = self.cur_y + self.param16(2) as i16 as f64 * unit;
                if y > self.top_margin {
                    if y > self.bottom_margin {
                        self.new_page(true, false);
                    } else {
                        self.cur_y = y;
                    }
                }
            }
            _ => {
                let what = if (ESC_PAREN_COMMAND..FS_COMMAND).contains(&cmd) {
                    format!("ESC ( {:02X}h", cmd & 0xFF)
                } else if cmd >= FS_COMMAND {
                    format!("FS {:02X}h", cmd & 0xFF)
                } else {
                    format!("ESC {:02X}h", cmd)
                };
                self.note(&what);
            }
        }
    }

    fn condensed(&mut self) {
        if !self.multipoint && self.cpi != 15.0 {
            self.hmi = -1.0;
            self.style |= STYLE_CONDENSED;
            self.update_font();
        }
    }

    fn pitch(&mut self, cpi: f64) {
        self.cpi = cpi;
        self.hmi = -1.0;
        self.multipoint = false;
        self.update_font();
    }

    /// A control code: whether it was one.
    fn control(&mut self, ch: u8) -> bool {
        match ch {
            // NUL, BEL, DC1 (select), DC3 (deselect), CAN (cancel line).
            0x00 | 0x07 | 0x11 | 0x13 | 0x18 => true,
            0x08 => {
                // Backspace.
                let x = self.cur_x - if self.hmi > 0.0 { self.hmi } else { 1.0 / self.actcpi };
                if x >= self.left_margin {
                    self.cur_x = x;
                }
                true
            }
            0x09 => {
                // Horizontal tab: to the next stop.
                let next = self.horiz_tabs[..self.num_horiz_tabs].iter().copied().find(|&t| t > self.cur_x + 1e-9);
                if let Some(x) = next
                    && x < self.right_margin
                {
                    self.cur_x = x;
                }
                true
            }
            0x0b => {
                // Vertical tab.
                if self.num_vert_tabs == 0 {
                    self.cur_x = self.left_margin;
                } else if self.num_vert_tabs == 255 {
                    self.line_feed();
                } else {
                    match self.vert_tabs[..self.num_vert_tabs].iter().copied().find(|&t| t > self.cur_y + 1e-9) {
                        Some(y) if y <= self.bottom_margin => self.cur_y = y,
                        _ => self.new_page(true, false),
                    }
                }
                self.end_one_line_double_width();
                true
            }
            0x0c => {
                self.end_one_line_double_width();
                self.new_page(true, true);
                true
            }
            0x0d => {
                self.cur_x = self.left_margin;
                if self.auto_feed {
                    self.end_one_line_double_width();
                    self.line_feed();
                }
                true
            }
            0x0a => {
                self.end_one_line_double_width();
                self.line_feed();
                true
            }
            0x0e => {
                if !self.multipoint {
                    self.hmi = -1.0;
                    self.style |= STYLE_DOUBLEWIDTHONELINE;
                    self.update_font();
                }
                true
            }
            0x0f => {
                self.condensed();
                true
            }
            0x12 => {
                // Cancel condensed (DC2).
                self.hmi = -1.0;
                self.style &= !STYLE_CONDENSED;
                self.update_font();
                true
            }
            0x14 => {
                // Cancel double width for the line (DC4).
                self.hmi = -1.0;
                self.style &= !STYLE_DOUBLEWIDTHONELINE;
                self.update_font();
                true
            }
            0x1b => {
                self.esc_seen = true;
                true
            }
            0x1c => {
                self.fs_seen = true;
                true
            }
            _ => false,
        }
    }

    /// Feed the page out (`save`, into `done`) and start a new one, the
    /// head at its top and, if `reset_x`, at the left margin.
    fn new_page(&mut self, save: bool, reset_x: bool) {
        let blank = Page::new(self.default_width, self.default_height, self.dpi);
        let page = std::mem::replace(&mut self.page, blank);
        if save {
            self.done.push(page);
        }
        if reset_x {
            self.cur_x = self.left_margin;
        }
        self.cur_y = self.top_margin;
    }

    /// Print `ch`: a character, part of a command, or bit image data.
    pub fn print(&mut self, ch: u8) {
        if self.bit_image.remaining > 0 {
            self.print_bit_image(ch);
            return;
        }
        let ch = match self.msb {
            0 => ch & 0x7F,
            1 => ch | 0x80,
            _ => ch,
        };
        if self.num_print_as_char > 0 {
            self.num_print_as_char -= 1;
        } else if self.command(ch) {
            return;
        }
        let ch = if ch == 0x01 { 0x20 } else { ch };
        let c = self.cur_map[ch as usize];
        // Lines and blocks fill the character's cell and the line, and
        // join up; letters are the font's.
        let glyph = match c {
            '\u{2500}'..='\u{259F}' if self.style & STYLE_PROP == 0 => {
                let pitch = if self.hmi > 0.0 { self.hmi } else { 1.0 / self.actcpi };
                let dpi = self.dpi as f64;
                self.fonts.cell(c, pitch * dpi, self.line_spacing.abs() * dpi, self.font.italic)
            }
            _ => None,
        }
        .unwrap_or_else(|| self.fonts.glyph(&self.font, c));
        // Bold and double-strike print again a dot or more over, in dots
        // of 360 dpi as DOSBox-X has them, but never on the same dots.
        let over = |dots: f64| (dots * self.dpi as f64 / 360.0).round().max(1.0) as i64;
        let dot = over(1.0);
        let pen_x = self.pix_x() + glyph.left;
        let mut pen_y = self.pix_y() + glyph.top;
        if self.style & STYLE_SUBSCRIPT != 0 && glyph.width > 0 {
            pen_y += (glyph.coverage.len() / glyph.width / 2) as i64;
        }
        let mut strikes = vec![(0, 0), (dot, 0)];
        if self.style & STYLE_DOUBLESTRIKE != 0 {
            strikes.extend([(0, dot), (dot, dot)]);
        }
        if self.style & STYLE_BOLD != 0 {
            strikes.extend([(over(2.0), 0), (over(3.0), 0)]);
        }
        for (dx, dy) in strikes {
            self.page.blit(pen_x + dx, pen_y + dy, glyph.width, &glyph.coverage, self.color);
        }

        let line_start = self.pix_x();
        let mut advance = if self.style & STYLE_PROP != 0 {
            glyph.advance / self.dpi as f64
        } else if self.hmi < 0.0 {
            1.0 / self.actcpi
        } else {
            self.hmi
        };
        advance += self.extra_intra_space;
        self.cur_x += advance;

        if self.score != SCORE_NONE && self.style & (STYLE_UNDERLINE | STYLE_STRIKETHROUGH | STYLE_OVERSCORE) != 0 {
            let height = self.fonts.height(&self.font).floor() as i64;
            let double = matches!(self.score, SCORE_DOUBLE | SCORE_DOUBLEBROKEN);
            let broken = matches!(self.score, SCORE_SINGLEBROKEN | SCORE_DOUBLEBROKEN);
            let gap = over(5.0);
            let y = if self.style & STYLE_UNDERLINE != 0 {
                self.pix_y() + (height as f64 * 0.9) as i64
            } else if self.style & STYLE_STRIKETHROUGH != 0 {
                self.pix_y() + (height as f64 * 0.45) as i64
            } else {
                self.pix_y() - if double { gap } else { 0 }
            };
            let end = self.pix_x();
            self.page.line(line_start, end, y, broken, self.color);
            if double {
                self.page.line(line_start, end, y + gap, broken, self.color);
            }
        }

        // Past the right margin, the next character goes on the next line.
        if self.cur_x + advance > self.right_margin {
            self.line_feed();
        }
    }

    fn setup_bit_image(&mut self, density: u8, columns: u16) {
        let (horiz, vert, bytes) = match density {
            0 => (60, 60, 1),
            1 | 2 => (120, 60, 1),
            3 => (60, 240, 1),
            4 => (80, 60, 1),
            6 => (90, 60, 1),
            32 => (60, 180, 3),
            33 => (120, 180, 3),
            38 => (90, 180, 3),
            39 => (180, 180, 3),
            40 => (360, 180, 3),
            71 => (180, 360, 6),
            72 | 73 => (360, 360, 6),
            _ => {
                self.note(&format!("bit image density {}", density));
                let b = self.bit_image;
                if b.column_bytes == 0 { (60, 60, 1) } else { (b.horiz, b.vert, b.column_bytes) }
            }
        };
        self.bit_image = BitImage {
            horiz,
            vert,
            column_bytes: bytes,
            remaining: columns as usize * bytes,
            column: [0; 6],
            read: 0,
        };
    }

    fn print_bit_image(&mut self, ch: u8) {
        let b = &mut self.bit_image;
        b.column[b.read] = ch;
        b.read += 1;
        b.remaining -= 1;
        if b.read < b.column_bytes {
            return;
        }
        b.read = 0;
        let b = *b;
        // Each pin's dot covers its share of an inch at the density.
        let size_x = (self.dpi / b.horiz as u32).max(1) as i64;
        let size_y = (self.dpi / b.vert as u32).max(1) as i64;
        let x = self.pix_x();
        let mut pin = 0;
        for &byte in &b.column[..b.column_bytes] {
            for bit in (0..8).rev() {
                if byte >> bit & 1 != 0 {
                    let y = ((self.cur_y + pin as f64 / b.vert as f64) * self.dpi as f64 + 0.5).floor() as i64;
                    for dy in 0..size_y {
                        for dx in 0..size_x {
                            self.page.ink(x + dx, y + dy, FULL, self.color);
                        }
                    }
                }
                pin += 1;
            }
        }
        self.cur_x += 1.0 / b.horiz as f64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A printer on Letter paper at 90 dpi, with the ROM font.
    fn printer() -> Escp {
        let mut escp = Escp::new(8.5, 11.0, 90, Some(PathBuf::from("/nonexistent")));
        escp.fonts = Fonts::new(None, 90);
        escp
    }

    fn send(escp: &mut Escp, bytes: &[u8]) {
        for &b in bytes {
            escp.print(b);
        }
    }

    fn inked(page: &Page, x0: usize, y0: usize, x1: usize, y1: usize) -> usize {
        (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).filter(|&(x, y)| page.dots[y * page.width + x] & FULL != 0).count()
    }

    #[test]
    fn text_prints_and_form_feed_ejects() {
        let mut escp = printer();
        send(&mut escp, b"Hello\r\n");
        assert!(!escp.is_blank());
        assert!(escp.done.is_empty());
        // Ten characters an inch: five letters take half an inch.
        assert_eq!(escp.cur_x, 0.0);
        assert!((escp.cur_y - 1.0 / 6.0).abs() < 1e-9);
        send(&mut escp, b"\x0c");
        assert_eq!(escp.done.len(), 1);
        assert!(escp.is_blank());
        let page = &escp.done[0];
        assert!(inked(page, 0, 0, 45, 15) > 50);
        assert_eq!(inked(page, 50, 0, 90, 15), 0);
        // Nothing more on the page: the eject button feeds nothing.
        escp.form_feed();
        assert_eq!(escp.done.len(), 1);
    }

    #[test]
    fn lines_run_on_to_the_next_page() {
        let mut escp = printer();
        // 66 lines of 1/6 inch fill 11 inches.
        for _ in 0..66 {
            send(&mut escp, b"x\r\n");
        }
        assert_eq!(escp.done.len(), 1);
        send(&mut escp, b"x");
        escp.form_feed();
        assert_eq!(escp.done.len(), 2);
    }

    #[test]
    fn long_lines_wrap_at_the_right_margin() {
        let mut escp = printer();
        // Right margin at column 10 (ESC Q 11).
        send(&mut escp, b"\x1bQ\x0b");
        send(&mut escp, b"0123456789AB");
        assert!((escp.cur_y - 1.0 / 6.0).abs() < 1e-9);
        assert!((escp.cur_x - 0.2).abs() < 1e-9);
    }

    #[test]
    fn bit_images_put_their_dots() {
        let mut escp = printer();
        // ESC * 0 (60 dpi), 2 columns: the top pin, then all eight.
        send(&mut escp, b"\x1b*\x00\x02\x00\x80\xff");
        let page = &escp.page;
        // A 60 dpi dot is one 90 dpi dot.
        assert_eq!(inked(page, 0, 0, 1, 12), 1);
        assert_eq!(inked(page, 1, 0, 2, 12), 0);
        assert_eq!(inked(page, 2, 0, 3, 12), 8);
        assert!((escp.cur_x - 2.0 / 60.0).abs() < 1e-9);
        // The data isn't printed as characters.
        assert_eq!(inked(page, 3, 0, 90, 20), 0);
    }

    #[test]
    fn unknown_esc_paren_commands_are_skipped_with_their_data() {
        let mut escp = printer();
        send(&mut escp, b"\x1b(Q\x03\x00ABC");
        assert!(escp.is_blank());
        assert_eq!(escp.unknown, vec!["ESC ( 51h".to_string()]);
        send(&mut escp, b"D");
        assert!(!escp.is_blank());
    }

    #[test]
    fn tabs_go_to_the_next_stop() {
        let mut escp = printer();
        send(&mut escp, b"\x1bD\x04\x08\x0c\x00");
        send(&mut escp, b"a\t");
        assert!((escp.cur_x - 0.4).abs() < 1e-9);
        send(&mut escp, b"\t");
        assert!((escp.cur_x - 0.8).abs() < 1e-9);
        assert!(!escp.is_blank());
    }

    #[test]
    fn styles_and_pitches() {
        let mut escp = printer();
        // Condensed 10 cpi is 17.14 cpi, double width halves it.
        send(&mut escp, b"\x0f");
        assert!((escp.actcpi - 17.14).abs() < 1e-9);
        send(&mut escp, b"\x12\x1bW1");
        assert!((escp.actcpi - 5.0).abs() < 1e-9);
        send(&mut escp, b"\x1bW0\x1bM");
        assert_eq!(escp.actcpi, 12.0);
        // Master select: bold italic underlined.
        send(&mut escp, b"\x1b!\xc8");
        assert!(escp.style & STYLE_BOLD != 0 && escp.style & STYLE_ITALICS != 0 && escp.style & STYLE_UNDERLINE != 0);
        assert!(escp.font.italic);
        send(&mut escp, b"\x1b@");
        assert_eq!(escp.style, 0);
        assert_eq!(escp.actcpi, 10.0);
    }

    #[test]
    fn colour_prints_in_its_ink() {
        let mut escp = printer();
        send(&mut escp, b"\x1br\x02X");
        assert!(!escp.page.is_gray());
    }
}
