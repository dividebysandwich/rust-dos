//! Glide's functions with Rust types: what GLIDE2X.DLL's exports call,
//! and what Direct3D's HAL calls to draw on the card.
//!
//! Each function takes Glide's own values (the `GR_*` constants below,
//! colours packed in the format grSstWinOpen chose, vertices in
//! `Vertex`), keeps Glide's shadow of the registers it changes in
//! `Context`, and writes the card. Bad values are left alone as Glide's
//! release library left them, without a panic.
//!
//! Drawing goes through the card's triangle setup: Glide sorts the
//! vertices by Y, culls by the sign of the area, and writes the
//! vertices and, for each parameter the combine units, depth buffer and
//! fog need, its start value and its gradients in X and Y as floats,
//! then the area to ftriangleCMD. Lines are two triangles half a pixel
//! either side of the line, gradients only along it; points are a
//! triangle a pixel wide, written as fixed point.
//!
//! Direct3D's HAL uses the same functions, and these besides:
//!
//! * `attach`: Glide's state without opening a display, for drawing
//!   into DirectDraw's surfaces on a Banshee or Voodoo3;
//! * `set_render_target` and `set_depth_buffer`: the colour and depth
//!   buffers at addresses of a Banshee's memory, with their strides;
//! * `read_rect_565`: a rectangle of a buffer, read back as 5-6-5
//!   pixels;
//! * `tex_download_level` and `tex_source` take addresses in the
//!   texture unit's memory: on a Banshee without Glide's display open,
//!   the card's memory from 0.

// The whole of Glide's interface is here for Direct3D's HAL and the
// tests, used by GLIDE2X.DLL or not.
#![allow(dead_code)]

use super::hw::{self, ALL, Card, FBI, tmu_chip};
use super::state;
use crate::cpu::Cpu;
use crate::voodoo::banshee::render3d;
use crate::voodoo::regs::*;

// --- Glide's values ---

pub const TMU0: u32 = 0;
pub const TMU1: u32 = 1;

pub const COMBINE_FUNCTION_ZERO: u32 = 0;
pub const COMBINE_FUNCTION_LOCAL: u32 = 1;
pub const COMBINE_FUNCTION_LOCAL_ALPHA: u32 = 2;
pub const COMBINE_FUNCTION_SCALE_OTHER: u32 = 3;
pub const COMBINE_FUNCTION_SCALE_OTHER_ADD_LOCAL: u32 = 4;
pub const COMBINE_FUNCTION_SCALE_OTHER_ADD_LOCAL_ALPHA: u32 = 5;
pub const COMBINE_FUNCTION_SCALE_OTHER_MINUS_LOCAL: u32 = 6;
pub const COMBINE_FUNCTION_SCALE_OTHER_MINUS_LOCAL_ADD_LOCAL: u32 = 7;
pub const COMBINE_FUNCTION_SCALE_OTHER_MINUS_LOCAL_ADD_LOCAL_ALPHA: u32 = 8;
pub const COMBINE_FUNCTION_SCALE_MINUS_LOCAL_ADD_LOCAL: u32 = 9;
pub const COMBINE_FUNCTION_SCALE_MINUS_LOCAL_ADD_LOCAL_ALPHA: u32 = 0x10;
pub const COMBINE_FUNCTION_BLEND: u32 = COMBINE_FUNCTION_SCALE_OTHER_MINUS_LOCAL_ADD_LOCAL;

pub const COMBINE_FACTOR_ZERO: u32 = 0;
pub const COMBINE_FACTOR_NONE: u32 = 0;
pub const COMBINE_FACTOR_LOCAL: u32 = 1;
pub const COMBINE_FACTOR_OTHER_ALPHA: u32 = 2;
pub const COMBINE_FACTOR_LOCAL_ALPHA: u32 = 3;
pub const COMBINE_FACTOR_TEXTURE_ALPHA: u32 = 4;
pub const COMBINE_FACTOR_LOD_FRACTION: u32 = 5;
pub const COMBINE_FACTOR_ONE: u32 = 8;
pub const COMBINE_FACTOR_ONE_MINUS_LOCAL: u32 = 9;
pub const COMBINE_FACTOR_ONE_MINUS_OTHER_ALPHA: u32 = 0xA;
pub const COMBINE_FACTOR_ONE_MINUS_LOCAL_ALPHA: u32 = 0xB;
pub const COMBINE_FACTOR_ONE_MINUS_TEXTURE_ALPHA: u32 = 0xC;
pub const COMBINE_FACTOR_ONE_MINUS_LOD_FRACTION: u32 = 0xD;

pub const COMBINE_LOCAL_ITERATED: u32 = 0;
pub const COMBINE_LOCAL_CONSTANT: u32 = 1;
pub const COMBINE_LOCAL_NONE: u32 = 1;
pub const COMBINE_LOCAL_DEPTH: u32 = 2;
pub const COMBINE_OTHER_ITERATED: u32 = 0;
pub const COMBINE_OTHER_TEXTURE: u32 = 1;
pub const COMBINE_OTHER_CONSTANT: u32 = 2;
pub const COMBINE_OTHER_NONE: u32 = 2;

pub const BLEND_ZERO: u32 = 0;
pub const BLEND_SRC_ALPHA: u32 = 1;
pub const BLEND_ONE: u32 = 4;
pub const BLEND_ONE_MINUS_SRC_ALPHA: u32 = 5;

pub const CMP_NEVER: u32 = 0;
pub const CMP_LESS: u32 = 1;
pub const CMP_ALWAYS: u32 = 7;

pub const COLORFORMAT_ARGB: u32 = 0;
pub const COLORFORMAT_ABGR: u32 = 1;
pub const COLORFORMAT_RGBA: u32 = 2;
pub const COLORFORMAT_BGRA: u32 = 3;

pub const CULL_DISABLE: u32 = 0;
pub const CULL_NEGATIVE: u32 = 1;
pub const CULL_POSITIVE: u32 = 2;

pub const DEPTHBUFFER_DISABLE: u32 = 0;
pub const DEPTHBUFFER_ZBUFFER: u32 = 1;
pub const DEPTHBUFFER_WBUFFER: u32 = 2;
pub const DEPTHBUFFER_ZBUFFER_COMPARE_TO_BIAS: u32 = 3;
pub const DEPTHBUFFER_WBUFFER_COMPARE_TO_BIAS: u32 = 4;

pub const DITHER_DISABLE: u32 = 0;
pub const DITHER_2X2: u32 = 1;
pub const DITHER_4X4: u32 = 2;

pub const FOG_DISABLE: u32 = 0;
pub const FOG_WITH_ITERATED_ALPHA: u32 = 1;
pub const FOG_WITH_TABLE: u32 = 2;
pub const FOG_WITH_ITERATED_Z: u32 = 3;
/// Not one of Glide's modes: fog by the integer part of the vertices'
/// oow (fogMode's fogZ and fogAlpha both set), for Direct3D's HAL,
/// whose depth stays Z.
pub const FOG_WITH_ITERATED_W: u32 = 4;
pub const FOG_MULT2: u32 = 0x100;
pub const FOG_ADD2: u32 = 0x200;
/// Entries of the fog table.
pub const FOG_TABLE_SIZE: usize = 64;

pub const BUFFER_FRONTBUFFER: u32 = 0;
pub const BUFFER_BACKBUFFER: u32 = 1;
pub const BUFFER_AUXBUFFER: u32 = 2;

pub const CHROMAKEY_DISABLE: u32 = 0;
pub const CHROMAKEY_ENABLE: u32 = 1;

pub const LFB_READ_ONLY: u32 = 0;
pub const LFB_WRITE_ONLY: u32 = 1;
pub const LFB_NOIDLE: u32 = 0x10;
pub const LFBWRITEMODE_565: u32 = 0;
pub const LFBWRITEMODE_555: u32 = 1;
pub const LFBWRITEMODE_1555: u32 = 2;
pub const LFBWRITEMODE_888: u32 = 4;
pub const LFBWRITEMODE_8888: u32 = 5;
pub const LFBWRITEMODE_565_DEPTH: u32 = 0xC;
pub const LFBWRITEMODE_555_DEPTH: u32 = 0xD;
pub const LFBWRITEMODE_1555_DEPTH: u32 = 0xE;
pub const LFBWRITEMODE_ZA16: u32 = 0xF;
pub const LFBWRITEMODE_ANY: u32 = 0xFF;

pub const ORIGIN_UPPER_LEFT: u32 = 0;
pub const ORIGIN_LOWER_LEFT: u32 = 1;
pub const ORIGIN_ANY: u32 = 0xFF;

pub const LOD_256: u32 = 0;
pub const LOD_1: u32 = 8;
pub const ASPECT_8X1: u32 = 0;
pub const ASPECT_1X1: u32 = 3;
pub const ASPECT_1X8: u32 = 6;

pub const MIPMAP_DISABLE: u32 = 0;
pub const MIPMAP_NEAREST: u32 = 1;
pub const MIPMAP_NEAREST_DITHER: u32 = 2;
pub const MIPMAPLEVELMASK_EVEN: u32 = 1;
pub const MIPMAPLEVELMASK_ODD: u32 = 2;
pub const MIPMAPLEVELMASK_BOTH: u32 = 3;

pub const TEXTURECLAMP_WRAP: u32 = 0;
pub const TEXTURECLAMP_CLAMP: u32 = 1;
pub const TEXTUREFILTER_POINT_SAMPLED: u32 = 0;
pub const TEXTUREFILTER_BILINEAR: u32 = 1;

pub const TEXFMT_8BIT: u32 = 0;
pub const TEXFMT_YIQ_422: u32 = 1;
pub const TEXFMT_P_8: u32 = 5;
pub const TEXFMT_16BIT: u32 = 8;
pub const TEXFMT_AYIQ_8422: u32 = 9;
pub const TEXFMT_RGB_565: u32 = 0xA;
pub const TEXFMT_ARGB_1555: u32 = 0xB;
pub const TEXFMT_ARGB_4444: u32 = 0xC;

pub const TEXTABLE_NCC0: u32 = 0;
pub const TEXTABLE_NCC1: u32 = 1;
pub const TEXTABLE_PALETTE: u32 = 2;

pub const TEXBASE_256: u32 = 0;
pub const TEXBASE_128: u32 = 1;
pub const TEXBASE_64: u32 = 2;
pub const TEXBASE_32_TO_1: u32 = 3;

pub const HINT_STWHINT: u32 = 0;
pub const HINT_ALLOW_MIPMAP_DITHER: u32 = 3;
pub const STWHINT_W_DIFF_FBI: u32 = 1;
pub const STWHINT_W_DIFF_TMU0: u32 = 2;
pub const STWHINT_ST_DIFF_TMU0: u32 = 4;
pub const STWHINT_W_DIFF_TMU1: u32 = 8;
pub const STWHINT_ST_DIFF_TMU1: u32 = 0x10;

// --- The registers' bits ---

// fbzColorPath.
const LOCALSELECT_OVERRIDE_WITH_ATEX: u32 = 1 << 7;
const CC_ZERO_OTHER: u32 = 1 << 8;
const CC_SUB_CLOCAL: u32 = 1 << 9;
const CC_REVERSE_BLEND: u32 = 1 << 13;
const CC_ADD_CLOCAL: u32 = 1 << 14;
const CC_ADD_ALOCAL: u32 = 1 << 15;
const CC_INVERT: u32 = 1 << 16;
const CCA_ZERO_OTHER: u32 = 1 << 17;
const CCA_SUB_CLOCAL: u32 = 1 << 18;
const CCA_REVERSE_BLEND: u32 = 1 << 22;
const CCA_ADD_ALOCAL: u32 = 1 << 24;
const CCA_INVERT: u32 = 1 << 25;
const PARMADJUST: u32 = 1 << 26;
const ENTEXTUREMAP: u32 = 1 << 27;
/// The bits grColorCombine sets.
const CC_BITS: u32 = 3 | 1 << 4 | 0xFF << 8 | 1 << 16 | ENTEXTUREMAP;
/// The bits grAlphaCombine sets.
const CCA_BITS: u32 = 3 << 2 | 3 << 5 | 0x1FF << 17 | ENTEXTUREMAP;

// fbzMode.
const ENRECTCLIP: u32 = 1;
const ENCHROMAKEY: u32 = 1 << 1;
const WBUFFER: u32 = 1 << 3;
const ENDEPTHBUFFER: u32 = 1 << 4;
const ZFUNC: u32 = 7 << 5;
const ENDITHER: u32 = 1 << 8;
const RGBWRMASK: u32 = 1 << 9;
const ZAWRMASK: u32 = 1 << 10;
const DITHER2X2: u32 = 1 << 11;
const DRAWBUFFER: u32 = 3 << 14;
const ENZBIAS: u32 = 1 << 16;
const YORIGIN: u32 = 1 << 17;
const ENALPHABUFFER: u32 = 1 << 18;
const DITHERSUB: u32 = 1 << 19;
const ZCOMPARE_TO_ZACOLOR: u32 = 1 << 20;

// alphaMode.
const ENALPHAFUNC: u32 = 1;
const ENALPHABLEND: u32 = 1 << 4;

// fogMode.
const ENFOGGING: u32 = 1;
const FOGADD: u32 = 1 << 1;
const FOGMULT: u32 = 1 << 2;
const FOG_ALPHA: u32 = 1 << 3;
const FOG_Z: u32 = 1 << 4;
const FOG_CONSTANT: u32 = 1 << 5;

// lfbMode.
const LFB_FORMAT: u32 = 0xF;
const LFB_WRITE_BUFFER: u32 = 3 << 4;
const LFB_READ_BUFFER: u32 = 3 << 6;
const LFB_ENPIXPIPE: u32 = 1 << 8;
const LFB_RGBALANES: u32 = 3 << 9;
const LFB_WRITE_SWAP16: u32 = 1 << 11;
const LFB_BYTESWAP: u32 = 1 << 12;
const LFB_YORIGIN: u32 = 1 << 13;

// textureMode.
const TPERSP_ST: u32 = 1;
const TMINFILTER: u32 = 1 << 1;
const TMAGFILTER: u32 = 1 << 2;
const TCLAMPW: u32 = 1 << 3;
const TLODDITHER: u32 = 1 << 4;
const TNCCSELECT: u32 = 1 << 5;
const TCLAMPS: u32 = 1 << 6;
const TCLAMPT: u32 = 1 << 7;
const TFORMAT: u32 = 0xF << 8;
const TC_ZERO_OTHER: u32 = 1 << 12;
const TC_SUB_CLOCAL: u32 = 1 << 13;
const TC_REVERSE_BLEND: u32 = 1 << 17;
const TC_ADD_CLOCAL: u32 = 1 << 18;
const TC_ADD_ALOCAL: u32 = 1 << 19;
const TC_INVERT: u32 = 1 << 20;
const TCA_ZERO_OTHER: u32 = 1 << 21;
const TCA_SUB_CLOCAL: u32 = 1 << 22;
const TCA_REVERSE_BLEND: u32 = 1 << 26;
const TCA_ADD_CLOCAL: u32 = 1 << 27;
const TCA_ADD_ALOCAL: u32 = 1 << 28;
const TCA_INVERT: u32 = 1 << 29;
const TRILINEAR: u32 = 1 << 30;
const TCOMBINE: u32 = 0x1FF << 12;
const TACOMBINE: u32 = 0x1FF << 21;
/// TC_SUB_CLOCAL | TC_ADD_CLOCAL with the LOD fraction as the factor,
/// which grTexCombine tests bit by bit for two-pass trilinear.
const TC_BLEND_LODFRAC: u32 = TC_SUB_CLOCAL | TC_ADD_CLOCAL | 5 << 14;

// tLOD.
const LODMIN: u32 = 0x3F;
const LODMAX: u32 = 0x3F << 6;
const LODBIAS: u32 = 0x3F << 12;
const LOD_ODD: u32 = 1 << 18;
const LOD_TSPLIT: u32 = 1 << 19;
const LOD_S_IS_WIDER: u32 = 1 << 20;
const LOD_ASPECT: u32 = 3 << 21;
const TMULTIBASEADDR: u32 = 1 << 24;

/// Which parameters a triangle sends (Glide's paramIndex).
const P_RGB: u32 = 1;
const P_ALPHA: u32 = 1 << 1;
const P_OOZ: u32 = 1 << 2;
const P_OOW_FBI: u32 = 1 << 3;
const P_W_TMU0: u32 = 1 << 4;
const P_ST_TMU0: u32 = 1 << 5;
const P_W_TMU1: u32 = 1 << 6;
const P_ST_TMU1: u32 = 1 << 7;

// --- Types ---

/// One texture unit's coordinates of a vertex.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TmuVertex {
    pub sow: f32,
    pub tow: f32,
    pub oow: f32,
}

/// A vertex as Glide 2 has it (GrVertex): screen X and Y (snapped by
/// the program), Z, colour and alpha 0 to 255, 1/Z scaled to 0-65535,
/// 1/W, and each texture unit's S/W, T/W in texels and its own 1/W.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vertex {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub ooz: f32,
    pub a: f32,
    pub oow: f32,
    pub tmu: [TmuVertex; 2],
}

impl Vertex {
    /// The bytes of a GrVertex: 15 floats.
    pub const SIZE: usize = 60;

    pub fn from_bytes(bytes: &[u8; Self::SIZE]) -> Self {
        let f = |i: usize| {
            f32::from_le_bytes([
                bytes[4 * i],
                bytes[4 * i + 1],
                bytes[4 * i + 2],
                bytes[4 * i + 3],
            ])
        };
        Vertex {
            x: f(0),
            y: f(1),
            z: f(2),
            r: f(3),
            g: f(4),
            b: f(5),
            ooz: f(6),
            a: f(7),
            oow: f(8),
            tmu: [
                TmuVertex {
                    sow: f(9),
                    tow: f(10),
                    oow: f(11),
                },
                TmuVertex {
                    sow: f(12),
                    tow: f(13),
                    oow: f(14),
                },
            ],
        }
    }

    pub fn to_bytes(self) -> [u8; Self::SIZE] {
        let values = [
            self.x,
            self.y,
            self.z,
            self.r,
            self.g,
            self.b,
            self.ooz,
            self.a,
            self.oow,
            self.tmu[0].sow,
            self.tmu[0].tow,
            self.tmu[0].oow,
            self.tmu[1].sow,
            self.tmu[1].tow,
            self.tmu[1].oow,
        ];
        let mut out = [0; Self::SIZE];
        for (i, v) in values.iter().enumerate() {
            out[4 * i..4 * i + 4].copy_from_slice(&v.to_le_bytes());
        }
        out
    }

    /// The parameter a triangle iterates: Glide's offset in GrVertex.
    fn param(&self, p: Param) -> f32 {
        match p {
            Param::R => self.r,
            Param::G => self.g,
            Param::B => self.b,
            Param::Ooz => self.ooz,
            Param::A => self.a,
            Param::Oow => self.oow,
            Param::S(t) => self.tmu[t].sow,
            Param::T(t) => self.tmu[t].tow,
            Param::W(t) => self.tmu[t].oow,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Param {
    R,
    G,
    B,
    Ooz,
    A,
    Oow,
    S(usize),
    T(usize),
    W(usize),
}

/// A texture as grTexSource and the downloads describe it (GrTexInfo,
/// without its data).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TexInfo {
    pub small_lod: u32,
    pub large_lod: u32,
    pub aspect: u32,
    pub format: u32,
}

impl TexInfo {
    fn valid(&self) -> bool {
        self.small_lod <= LOD_1
            && self.large_lod <= self.small_lod
            && self.aspect <= ASPECT_1X8
            && self.format <= 0xF
    }
}

/// A locked linear frame buffer: where the process sees it and its
/// stride, and the format and origin of writes (GrLfbInfo_t).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LfbInfo {
    pub ptr: u32,
    pub stride: u32,
    pub write_mode: u32,
    pub origin: u32,
}

/// A texture unit's registers as Glide last wrote them, and its
/// texture.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TmuState {
    pub texture_mode: u32,
    pub t_lod: u32,
    pub t_detail: u32,
    pub tex_base: [u32; 4],
    pub mm_mode: u32,
    pub small_lod: u32,
    pub large_lod: u32,
    pub even_odd: u32,
    pub ncc_table: u32,
}

crate::state_fields!(TmuState {
    texture_mode,
    t_lod,
    t_detail,
    tex_base,
    mm_mode,
    small_lod,
    large_lod,
    even_odd,
    ncc_table
});

/// Glide's context: the registers as it last wrote them (its shadows)
/// and its own settings.
#[derive(Clone, Debug, Default)]
pub struct Context {
    /// Open, or attached for Direct3D: the functions draw.
    pub active: bool,
    /// The size clip windows are limited to.
    pub width: u32,
    pub height: u32,
    pub fbz_color_path: u32,
    pub fog_mode: u32,
    pub alpha_mode: u32,
    pub fbz_mode: u32,
    pub lfb_mode: u32,
    pub clip_left_right: u32,
    pub clip_bottom_top: u32,
    pub fog_color: u32,
    pub za_color: u32,
    pub chroma_key: u32,
    pub color0: u32,
    pub color1: u32,
    pub tmu: [TmuState; 2],
    pub color_format: u32,
    pub cull_mode: u32,
    pub cc_requires_texture: bool,
    pub ac_requires_texture: bool,
    pub cc_requires_it_rgb: bool,
    pub ac_requires_it_alpha: bool,
    /// The texture units the texture combine needs coordinates for.
    pub tmu_mask: u32,
    pub stw_hint: u32,
    pub allow_lod_dither: bool,
    /// Iterated RGB replaced by the constant of grConstantColorValue4.
    pub delta0: bool,
    /// grConstantColorValue4's A, R, G, B.
    pub constant: [f32; 4],
    pub lfb_constant_alpha: u32,
    pub lfb_constant_depth: u32,
    /// The buffers locked for reading and for writing, or NONE.
    pub locks: [u32; 2],
    /// The buffer grRenderBuffer picked.
    pub render: u32,
    /// The clip window, for the clipping utilities.
    pub clip: [u32; 4],
    pub gamma: f32,
    /// grErrorSetCallback's function, which this library never calls.
    pub error_callback: u32,
    /// Triangles drawn and processed (grTriStats).
    pub triangles: [u32; 2],
    pub fog_table: Vec<u8>,
    /// Direct3D's buffers on a Banshee: colBufferAddr and its stride,
    /// auxBufferAddr and its stride; a color stride of 0 for Glide's
    /// own (a buffer can start at 0).
    pub target: [u32; 4],
}

crate::state_fields!(Context {
    active,
    width,
    height,
    fbz_color_path,
    fog_mode,
    alpha_mode,
    fbz_mode,
    lfb_mode,
    clip_left_right,
    clip_bottom_top,
    fog_color,
    za_color,
    chroma_key,
    color0,
    color1,
    tmu,
    color_format,
    cull_mode,
    cc_requires_texture,
    ac_requires_texture,
    cc_requires_it_rgb,
    ac_requires_it_alpha,
    tmu_mask,
    stw_hint,
    allow_lod_dither,
    delta0,
    constant,
    lfb_constant_alpha,
    lfb_constant_depth,
    locks,
    render,
    clip,
    gamma,
    error_callback,
    triangles,
    fog_table,
    target
});

/// No buffer locked.
const NONE: u32 = u32::MAX;

fn gc(cpu: &mut Cpu) -> &mut Context {
    &mut state(cpu).gc
}

/// Whether Glide may draw: the card is open or attached.
fn active(cpu: &mut Cpu) -> bool {
    gc(cpu).active && cpu.bus.voodoo.is_some()
}

/// The texture units the board has.
fn tmus(cpu: &Cpu) -> u32 {
    hw::board(cpu).map_or(0, |b| b.tmus)
}

pub fn tmu_ok(cpu: &Cpu, tmu: u32) -> bool {
    tmu < tmus(cpu)
}

// --- The board, opening and closing ---

/// The board the machine has (grSstQueryHardware), or None without a
/// 3dfx card.
pub fn query(cpu: &Cpu) -> Option<hw::Board> {
    hw::board(cpu)
}

/// The size of each of grSstWinOpen's resolutions.
pub fn resolution_size(resolution: u32) -> Option<(u32, u32)> {
    const SIZES: [(u32, u32); 16] = [
        (320, 200),
        (320, 240),
        (400, 256),
        (512, 384),
        (640, 200),
        (640, 350),
        (640, 400),
        (640, 480),
        (800, 600),
        (960, 720),
        (856, 480),
        (512, 256),
        (1024, 768),
        (1280, 1024),
        (1600, 1200),
        (400, 300),
    ];
    SIZES.get(resolution as usize).copied()
}

/// The rate of each of grSstWinOpen's refresh rates, in Hz.
pub fn refresh_hz(refresh: u32) -> u32 {
    const RATES: [u32; 9] = [60, 70, 72, 75, 80, 90, 100, 85, 120];
    RATES.get(refresh as usize).copied().unwrap_or(60)
}

/// Open the card's display for the running process (grSstWinOpen):
/// `width` x `height` at `refresh` Hz, colours passed in
/// `color_format`, Y from `origin`, with `colors` colour buffers and
/// `auxes` auxiliary (depth or alpha) buffers. False if another process
/// has the card, or the board can't.
#[allow(clippy::too_many_arguments)]
pub fn open(
    cpu: &mut Cpu,
    width: u32,
    height: u32,
    refresh: u32,
    color_format: u32,
    origin: u32,
    colors: u32,
    auxes: u32,
) -> bool {
    let pid = super::super::user32::current_pid(cpu);
    let owner = state(cpu).owner;
    if owner != 0 && owner != pid {
        return false;
    }
    if owner == pid && state(cpu).screen.open {
        close(cpu);
    }
    let colors = colors.clamp(2, 3);
    if !hw::open_display(cpu, width, height, refresh, colors, auxes.min(1)) {
        return false;
    }
    state(cpu).owner = pid;
    super::gu::tex_mem_reset(cpu);
    init_state(cpu, width, height, color_format, origin);
    // A clean picture to start from.
    for buffer in [BUFFER_FRONTBUFFER, BUFFER_BACKBUFFER] {
        render_buffer(cpu, buffer);
        let mask = gc(cpu).fbz_mode;
        set_fbz_mode(cpu, mask | RGBWRMASK | ZAWRMASK);
        buffer_clear(cpu, 0, 0, 0);
        set_fbz_mode(cpu, mask);
    }
    render_buffer(cpu, BUFFER_BACKBUFFER);
    true
}

/// Glide's state without a display of its own, for Direct3D drawing
/// into DirectDraw's surfaces (`set_render_target`): clip windows as
/// big as `width` x `height`.
pub fn attach(cpu: &mut Cpu, width: u32, height: u32) -> bool {
    if cpu.bus.voodoo.is_none() {
        return false;
    }
    init_state(cpu, width, height, COLORFORMAT_ARGB, ORIGIN_UPPER_LEFT);
    true
}

/// The state grSstWinOpen leaves.
fn init_state(cpu: &mut Cpu, width: u32, height: u32, color_format: u32, origin: u32) {
    let tmus = tmus(cpu);
    *gc(cpu) = Context {
        active: true,
        width,
        height,
        color_format: color_format & 3,
        locks: [NONE; 2],
        render: BUFFER_BACKBUFFER,
        gamma: 1.0,
        fog_table: vec![0; FOG_TABLE_SIZE],
        ..Context::default()
    };
    set_fbz_mode(cpu, ENRECTCLIP | ENZBIAS | 1 << 14);
    hints(cpu, HINT_ALLOW_MIPMAP_DITHER, 0);
    sst_origin(cpu, origin);
    alpha_blend_function(cpu, BLEND_ONE, BLEND_ZERO, BLEND_ONE, BLEND_ZERO);
    alpha_test_function(cpu, CMP_ALWAYS);
    alpha_test_reference_value(cpu, 0);
    chromakey_mode(cpu, CHROMAKEY_DISABLE);
    constant_color_value(cpu, u32::MAX);
    clip_window(cpu, 0, 0, width, height);
    set_delta0(cpu, false);
    color_combine(
        cpu,
        COMBINE_FUNCTION_SCALE_OTHER,
        COMBINE_FACTOR_ONE,
        COMBINE_LOCAL_ITERATED,
        COMBINE_OTHER_ITERATED,
        false,
    );
    alpha_combine(
        cpu,
        COMBINE_FUNCTION_SCALE_OTHER,
        COMBINE_FACTOR_ONE,
        COMBINE_LOCAL_NONE,
        COMBINE_OTHER_CONSTANT,
        false,
    );
    color_mask(cpu, true, false);
    cull_mode(cpu, CULL_DISABLE);
    depth_bias_level(cpu, 0);
    depth_mask(cpu, false);
    depth_buffer_mode(cpu, DEPTHBUFFER_DISABLE);
    depth_buffer_function(cpu, CMP_LESS);
    dither_mode(cpu, DITHER_4X4);
    fog_mode(cpu, FOG_DISABLE);
    fog_color_value(cpu, 0);
    let gamma = if hw::banshee(cpu) { 1.0 } else { 1.7 };
    gamma_correction_value(cpu, gamma);
    for tmu in 0..tmus {
        // A Voodoo Graphics' first texture units can't download 8-bit
        // texels in sequence; Glide uses it where it can, this library
        // never.
        let t = &mut gc(cpu).tmu[tmu as usize];
        t.mm_mode = MIPMAP_NEAREST;
        t.small_lod = LOD_1;
        t.large_lod = LOD_1;
        t.even_odd = MIPMAPLEVELMASK_BOTH;
        tex_clamp_mode(cpu, tmu, TEXTURECLAMP_CLAMP, TEXTURECLAMP_CLAMP);
        tex_detail_control(cpu, tmu, 0, 1, 1.0);
        tex_filter_mode(
            cpu,
            tmu,
            TEXTUREFILTER_POINT_SAMPLED,
            TEXTUREFILTER_POINT_SAMPLED,
        );
        tex_lod_bias_value(cpu, tmu, 0.0);
        tex_mipmap_mode(cpu, tmu, MIPMAP_DISABLE, false);
        tex_combine(
            cpu,
            tmu,
            COMBINE_FUNCTION_ZERO,
            COMBINE_FACTOR_NONE,
            COMBINE_FUNCTION_ZERO,
            COMBINE_FACTOR_NONE,
            false,
            false,
        );
    }
    lfb_constant_alpha(cpu, 0xFF);
    lfb_constant_depth(cpu, 0);
}

/// Close the card (grSstWinClose): the screen back as it was.
pub fn close(cpu: &mut Cpu) {
    hw::close_display(cpu);
    let s = state(cpu);
    s.owner = 0;
    s.gc.active = false;
    super::gu::tex_mem_reset(cpu);
}

/// The width and height of the display Glide opened.
pub fn screen_size(cpu: &mut Cpu) -> (u32, u32) {
    let g = gc(cpu);
    (g.width, g.height)
}

// --- Writing registers ---

fn set_fbz_color_path(cpu: &mut Cpu, value: u32) {
    let old = gc(cpu).fbz_color_path;
    // Into or out of texturing: a nop, as Glide writes.
    if (old ^ value) & ENTEXTUREMAP != 0 {
        hw::write(cpu, ALL, NOP_CMD, 0);
    }
    gc(cpu).fbz_color_path = value;
    hw::write(cpu, ALL, FBZ_COLOR_PATH, value);
}

fn set_fbz_mode(cpu: &mut Cpu, value: u32) {
    gc(cpu).fbz_mode = value;
    hw::write(cpu, ALL, FBZ_MODE, value);
}

fn set_alpha_mode(cpu: &mut Cpu, value: u32) {
    gc(cpu).alpha_mode = value;
    hw::write(cpu, ALL, ALPHA_MODE, value);
}

fn set_fog_mode_reg(cpu: &mut Cpu, value: u32) {
    gc(cpu).fog_mode = value;
    hw::write(cpu, ALL, FOG_MODE, value);
}

fn set_lfb_mode(cpu: &mut Cpu, value: u32) {
    gc(cpu).lfb_mode = value;
    hw::write(cpu, ALL, LFB_MODE, value);
}

fn set_za_color(cpu: &mut Cpu, value: u32) {
    gc(cpu).za_color = value;
    hw::write(cpu, ALL, ZA_COLOR, value);
}

fn set_texture_mode(cpu: &mut Cpu, tmu: u32, value: u32) {
    gc(cpu).tmu[tmu as usize].texture_mode = value;
    hw::write(cpu, tmu_chip(tmu), TEXTURE_MODE, value);
}

fn set_t_lod(cpu: &mut Cpu, tmu: u32, value: u32) {
    gc(cpu).tmu[tmu as usize].t_lod = value;
    hw::write(cpu, tmu_chip(tmu), T_LOD, value);
}

/// A colour packed as `format` says, as the card takes it: ARGB.
pub fn to_argb(format: u32, color: u32) -> u32 {
    let [b0, b1, b2, b3] = color.to_le_bytes();
    let (a, r, g, b) = match format & 3 {
        COLORFORMAT_ARGB => (b3, b2, b1, b0),
        COLORFORMAT_ABGR => (b3, b0, b1, b2),
        COLORFORMAT_RGBA => (b0, b3, b2, b1),
        _ => (b0, b1, b2, b3),
    };
    u32::from_be_bytes([a, r, g, b])
}

fn swizzle(cpu: &mut Cpu, color: u32) -> u32 {
    to_argb(gc(cpu).color_format, color)
}

// --- Combine units ---

/// The combine function's bits: zero other, subtract local, add local,
/// add local alpha.
fn function_bits(function: u32) -> Option<(bool, bool, bool, bool)> {
    Some(match function {
        COMBINE_FUNCTION_ZERO => (true, false, false, false),
        COMBINE_FUNCTION_LOCAL => (true, false, true, false),
        COMBINE_FUNCTION_LOCAL_ALPHA => (true, false, false, true),
        COMBINE_FUNCTION_SCALE_OTHER => (false, false, false, false),
        COMBINE_FUNCTION_SCALE_OTHER_ADD_LOCAL => (false, false, true, false),
        COMBINE_FUNCTION_SCALE_OTHER_ADD_LOCAL_ALPHA => (false, false, false, true),
        COMBINE_FUNCTION_SCALE_OTHER_MINUS_LOCAL => (false, true, false, false),
        COMBINE_FUNCTION_SCALE_OTHER_MINUS_LOCAL_ADD_LOCAL => (false, true, true, false),
        COMBINE_FUNCTION_SCALE_OTHER_MINUS_LOCAL_ADD_LOCAL_ALPHA => (false, true, false, true),
        COMBINE_FUNCTION_SCALE_MINUS_LOCAL_ADD_LOCAL => (true, true, true, false),
        COMBINE_FUNCTION_SCALE_MINUS_LOCAL_ADD_LOCAL_ALPHA => (true, true, false, true),
        _ => return None,
    })
}

/// The colour combine unit (grColorCombine).
pub fn color_combine(
    cpu: &mut Cpu,
    function: u32,
    factor: u32,
    local: u32,
    other: u32,
    invert: bool,
) {
    let Some((zero, sub, add, add_alpha)) = function_bits(function) else {
        return;
    };
    let mut path = gc(cpu).fbz_color_path & !CC_BITS;
    path |= PARMADJUST;
    if factor & 8 == 0 {
        path |= CC_REVERSE_BLEND;
    }
    let factor = factor & 7;
    let g = gc(cpu);
    g.cc_requires_texture =
        factor == COMBINE_FACTOR_TEXTURE_ALPHA || other == COMBINE_OTHER_TEXTURE;
    g.cc_requires_it_rgb = local == COMBINE_LOCAL_ITERATED || other == COMBINE_OTHER_ITERATED;
    path |= factor << 10 | (local & 1) << 4 | (other & 3);
    if invert {
        path |= CC_INVERT;
    }
    path |= if zero { CC_ZERO_OTHER } else { 0 }
        | if sub { CC_SUB_CLOCAL } else { 0 }
        | if add { CC_ADD_CLOCAL } else { 0 }
        | if add_alpha { CC_ADD_ALOCAL } else { 0 };
    let g = gc(cpu);
    if g.cc_requires_texture || g.ac_requires_texture {
        path |= ENTEXTUREMAP;
    }
    set_fbz_color_path(cpu, path);
}

/// The alpha combine unit (grAlphaCombine).
pub fn alpha_combine(
    cpu: &mut Cpu,
    function: u32,
    factor: u32,
    local: u32,
    other: u32,
    invert: bool,
) {
    let Some((zero, sub, _, _)) = function_bits(function) else {
        return;
    };
    let mut path = gc(cpu).fbz_color_path & !CCA_BITS;
    if factor & 8 == 0 {
        path |= CCA_REVERSE_BLEND;
    }
    let factor = factor & 7;
    let g = gc(cpu);
    g.ac_requires_texture =
        factor == COMBINE_FACTOR_TEXTURE_ALPHA || other == COMBINE_OTHER_TEXTURE;
    g.ac_requires_it_alpha = local == COMBINE_LOCAL_ITERATED || other == COMBINE_OTHER_ITERATED;
    path |= factor << 19 | (local & 3) << 5 | (other & 3) << 2;
    if invert {
        path |= CCA_INVERT;
    }
    // The alpha unit adds its local alpha for "local" and "add local"
    // alike.
    let adds = !matches!(
        function,
        COMBINE_FUNCTION_ZERO
            | COMBINE_FUNCTION_SCALE_OTHER
            | COMBINE_FUNCTION_SCALE_OTHER_MINUS_LOCAL
    );
    path |= if zero { CCA_ZERO_OTHER } else { 0 }
        | if sub { CCA_SUB_CLOCAL } else { 0 }
        | if adds { CCA_ADD_ALOCAL } else { 0 };
    let g = gc(cpu);
    if g.cc_requires_texture || g.ac_requires_texture {
        path |= ENTEXTUREMAP;
    }
    set_fbz_color_path(cpu, path);
}

/// Whether the texture's alpha takes the place of the iterated colour
/// as the local colour (grAlphaControlsITRGBLighting).
pub fn alpha_controls_itrgb_lighting(cpu: &mut Cpu, enable: bool) {
    let path = gc(cpu).fbz_color_path & !LOCALSELECT_OVERRIDE_WITH_ATEX;
    let bit = if enable {
        LOCALSELECT_OVERRIDE_WITH_ATEX
    } else {
        0
    };
    set_fbz_color_path(cpu, path | bit);
}

/// Iterated RGB replaced by grConstantColorValue4's colour, as the
/// utility library's ITRGB_DELTA0 modes ask.
pub fn set_delta0(cpu: &mut Cpu, on: bool) {
    gc(cpu).delta0 = on;
    if on {
        let [_, r, g, b] = gc(cpu).constant;
        for (reg, v) in [(FSTART_R, r), (FSTART_G, g), (FSTART_B, b)] {
            hw::write_f(cpu, ALL, reg, v);
        }
        for reg in [FD_R_DX, FD_G_DX, FD_B_DX, FD_R_DY, FD_G_DY, FD_B_DY] {
            hw::write_f(cpu, ALL, reg, 0.0);
        }
    }
}

/// A texture unit's combine unit (grTexCombine).
#[allow(clippy::too_many_arguments)]
pub fn tex_combine(
    cpu: &mut Cpu,
    tmu: u32,
    rgb_function: u32,
    rgb_factor: u32,
    alpha_function: u32,
    alpha_factor: u32,
    rgb_invert: bool,
    alpha_invert: bool,
) {
    if !tmu_ok(cpu, tmu) {
        return;
    }
    let (Some(rgb), Some(alpha)) = (function_bits(rgb_function), function_bits(alpha_function))
    else {
        return;
    };
    let t = gc(cpu).tmu[tmu as usize];
    let mut mode = t.texture_mode & !(TCOMBINE | TACOMBINE);
    let mut lod = t.t_lod & !LOD_ODD;
    let bit = 1 << tmu;
    let mut needs = false;
    mode |= (rgb_factor & 7) << 14;
    if rgb_factor & 8 == 0 {
        mode |= TC_REVERSE_BLEND;
    }
    if matches!(
        rgb_factor & 7,
        COMBINE_FACTOR_LOCAL | COMBINE_FACTOR_LOCAL_ALPHA
    ) {
        needs = true;
    }
    mode |= (alpha_factor & 7) << 23;
    if alpha_factor & 8 == 0 {
        mode |= TCA_REVERSE_BLEND;
    }
    if matches!(
        alpha_factor & 7,
        COMBINE_FACTOR_LOCAL | COMBINE_FACTOR_LOCAL_ALPHA
    ) {
        needs = true;
    }
    if rgb_invert {
        mode |= TC_INVERT;
    }
    if alpha_invert {
        mode |= TCA_INVERT;
    }
    let (zero, sub, add, add_alpha) = rgb;
    mode |= if zero { TC_ZERO_OTHER } else { 0 }
        | if sub { TC_SUB_CLOCAL } else { 0 }
        | if add { TC_ADD_CLOCAL } else { 0 }
        | if add_alpha { TC_ADD_ALOCAL } else { 0 };
    let (zero, sub, add, add_alpha) = alpha;
    mode |= if zero { TCA_ZERO_OTHER } else { 0 }
        | if sub { TCA_SUB_CLOCAL } else { 0 }
        | if add { TCA_ADD_CLOCAL } else { 0 }
        | if add_alpha { TCA_ADD_ALOCAL } else { 0 };
    // Any function but zero and "scale other" uses the unit's texel.
    let uses = |f: u32| !matches!(f, COMBINE_FUNCTION_ZERO | COMBINE_FUNCTION_SCALE_OTHER);
    needs |= uses(rgb_function) || uses(alpha_function);
    let g = gc(cpu);
    g.tmu_mask = (g.tmu_mask & !bit) | if needs { bit } else { 0 };
    // The odd pass of two-pass trilinear: Glide tests the bits loosely.
    if mode & TRILINEAR != 0
        && mode & TC_ZERO_OTHER != 0
        && mode & TC_BLEND_LODFRAC != 0
        && mode & TC_REVERSE_BLEND == 0
    {
        lod |= LOD_ODD;
    }
    lod |= even_odd_bits(t.even_odd);
    set_texture_mode(cpu, tmu, mode);
    set_t_lod(cpu, tmu, lod);
}

/// tLOD's split bits for the levels a texture has.
fn even_odd_bits(even_odd: u32) -> u32 {
    match even_odd {
        MIPMAPLEVELMASK_EVEN => LOD_TSPLIT,
        MIPMAPLEVELMASK_ODD => LOD_TSPLIT | LOD_ODD,
        _ => 0,
    }
}

/// tLOD's aspect bits.
fn aspect_bits(aspect: u32) -> u32 {
    match aspect {
        0..=2 => (3 - aspect) << 21 | LOD_S_IS_WIDER,
        _ => (aspect.min(6) - 3) << 21,
    }
}

/// tLOD's minimum and maximum LOD, in its 4.2 format.
fn lod_min_max(min: u32, max: u32) -> u32 {
    (min.min(8) << 2) | (max.min(8) << 2) << 6
}

// --- Blending, tests, buffers' masks ---

/// The blend factors (grAlphaBlendFunction): alpha's only one or zero.
pub fn alpha_blend_function(cpu: &mut Cpu, rgb_sf: u32, rgb_df: u32, alpha_sf: u32, alpha_df: u32) {
    let alpha_sf = if matches!(alpha_sf, BLEND_ONE | BLEND_ZERO) {
        alpha_sf
    } else {
        BLEND_ONE
    };
    let alpha_df = if matches!(alpha_df, BLEND_ONE | BLEND_ZERO) {
        alpha_df
    } else {
        BLEND_ZERO
    };
    let mut mode = gc(cpu).alpha_mode & !(0xFFFF << 8 | ENALPHABLEND);
    if (rgb_sf, rgb_df, alpha_sf, alpha_df) != (BLEND_ONE, BLEND_ZERO, BLEND_ONE, BLEND_ZERO) {
        mode |= ENALPHABLEND;
    }
    mode |= (rgb_sf & 0xF) << 8 | (rgb_df & 0xF) << 12 | alpha_sf << 16 | alpha_df << 20;
    set_alpha_mode(cpu, mode);
}

pub fn alpha_test_function(cpu: &mut Cpu, function: u32) {
    let mut mode = gc(cpu).alpha_mode & !(7 << 1 | ENALPHAFUNC);
    if function != CMP_ALWAYS {
        mode |= (function & 7) << 1 | ENALPHAFUNC;
    }
    set_alpha_mode(cpu, mode);
}

pub fn alpha_test_reference_value(cpu: &mut Cpu, value: u32) {
    let mode = gc(cpu).alpha_mode & !(0xFF << 24);
    set_alpha_mode(cpu, mode | (value & 0xFF) << 24);
}

pub fn chromakey_mode(cpu: &mut Cpu, mode: u32) {
    let fbz = gc(cpu).fbz_mode & !ENCHROMAKEY;
    let bit = if mode == CHROMAKEY_ENABLE {
        ENCHROMAKEY
    } else {
        0
    };
    set_fbz_mode(cpu, fbz | bit);
}

pub fn chromakey_value(cpu: &mut Cpu, color: u32) {
    let color = swizzle(cpu, color);
    gc(cpu).chroma_key = color;
    hw::write(cpu, ALL, CHROMA_KEY, color);
}

/// The clip window (grClipWindow): pixels from `min` up to `max`,
/// within the screen.
pub fn clip_window(cpu: &mut Cpu, minx: u32, miny: u32, maxx: u32, maxy: u32) {
    let g = gc(cpu);
    let maxx = maxx.min(g.width);
    let maxy = maxy.min(g.height);
    let minx = minx.min(maxx);
    let miny = miny.min(maxy);
    let lr = minx << 16 | maxx;
    let bt = miny << 16 | maxy;
    g.clip_left_right = lr;
    g.clip_bottom_top = bt;
    g.clip = [minx, miny, maxx, maxy];
    hw::write(cpu, ALL, CLIP_LEFT_RIGHT, lr);
    hw::write(cpu, ALL, CLIP_LOW_Y_HIGH_Y, bt);
}

/// Writes of colour and of alpha (grColorMask); alpha only without a
/// depth buffer, and not with three colour buffers.
pub fn color_mask(cpu: &mut Cpu, rgb: bool, alpha: bool) {
    let mut fbz = gc(cpu).fbz_mode & !RGBWRMASK;
    if rgb {
        fbz |= RGBWRMASK;
    }
    let triple = state(cpu).screen.color_buffers == 3;
    if fbz & ENDEPTHBUFFER == 0 && !triple {
        fbz &= !(ENALPHABUFFER | ZAWRMASK);
        if alpha {
            fbz |= ENALPHABUFFER | ZAWRMASK;
        }
    }
    set_fbz_mode(cpu, fbz);
}

/// The constant colour (grConstantColorValue): color0 and color1.
pub fn constant_color_value(cpu: &mut Cpu, color: u32) {
    let color = swizzle(cpu, color);
    let g = gc(cpu);
    g.color0 = color;
    g.color1 = color;
    hw::write(cpu, ALL, COLOR0, color);
    hw::write(cpu, ALL, COLOR1, color);
}

/// grConstantColorValue4: the colour delta 0 modes iterate.
pub fn constant_color_value4(cpu: &mut Cpu, a: f32, r: f32, g: f32, b: f32) {
    gc(cpu).constant = [a, r, g, b];
    if gc(cpu).delta0 {
        set_delta0(cpu, true);
    }
}

pub fn cull_mode(cpu: &mut Cpu, mode: u32) {
    gc(cpu).cull_mode = mode.min(2);
}

pub fn depth_bias_level(cpu: &mut Cpu, level: i16) {
    let za = gc(cpu).za_color & !0xFFFF;
    set_za_color(cpu, za | level as u16 as u32);
}

pub fn depth_buffer_function(cpu: &mut Cpu, function: u32) {
    let fbz = gc(cpu).fbz_mode & !ZFUNC;
    set_fbz_mode(cpu, fbz | (function & 7) << 5);
}

pub fn depth_buffer_mode(cpu: &mut Cpu, mode: u32) {
    let mut fbz = gc(cpu).fbz_mode & !(ENDEPTHBUFFER | WBUFFER | ENZBIAS | ZCOMPARE_TO_ZACOLOR);
    fbz |= match mode {
        DEPTHBUFFER_ZBUFFER => ENDEPTHBUFFER | ENZBIAS,
        DEPTHBUFFER_WBUFFER => ENDEPTHBUFFER | WBUFFER | ENZBIAS,
        DEPTHBUFFER_ZBUFFER_COMPARE_TO_BIAS => ENDEPTHBUFFER | ZCOMPARE_TO_ZACOLOR,
        DEPTHBUFFER_WBUFFER_COMPARE_TO_BIAS => ENDEPTHBUFFER | WBUFFER | ZCOMPARE_TO_ZACOLOR,
        _ => 0,
    };
    set_fbz_mode(cpu, fbz);
}

pub fn depth_mask(cpu: &mut Cpu, enable: bool) {
    let fbz = gc(cpu).fbz_mode & !ZAWRMASK;
    set_fbz_mode(cpu, fbz | if enable { ZAWRMASK } else { 0 });
}

/// Blending, the alpha test, chroma key, depth bias and buffering and fog
/// all off (grDisableAllEffects).
pub fn disable_all_effects(cpu: &mut Cpu) {
    alpha_blend_function(cpu, BLEND_ONE, BLEND_ZERO, BLEND_ONE, BLEND_ZERO);
    alpha_test_function(cpu, CMP_ALWAYS);
    chromakey_mode(cpu, CHROMAKEY_DISABLE);
    depth_bias_level(cpu, 0);
    depth_buffer_mode(cpu, DEPTHBUFFER_DISABLE);
    fog_mode(cpu, FOG_DISABLE);
}

pub fn dither_mode(cpu: &mut Cpu, mode: u32) {
    let mut fbz = gc(cpu).fbz_mode & !(ENDITHER | DITHER2X2 | DITHERSUB);
    fbz |= match mode {
        DITHER_2X2 => ENDITHER | DITHER2X2,
        DITHER_4X4 => ENDITHER | DITHERSUB,
        _ => 0,
    };
    set_fbz_mode(cpu, fbz);
}

pub fn fog_mode(cpu: &mut Cpu, mode: u32) {
    let mut fog =
        gc(cpu).fog_mode & !(ENFOGGING | FOGADD | FOGMULT | FOG_ALPHA | FOG_Z | FOG_CONSTANT);
    fog |= match mode & 0xFF {
        FOG_WITH_ITERATED_ALPHA => ENFOGGING | FOG_ALPHA,
        FOG_WITH_ITERATED_Z => ENFOGGING | FOG_Z,
        FOG_WITH_ITERATED_W => ENFOGGING | FOG_Z | FOG_ALPHA,
        FOG_WITH_TABLE => ENFOGGING,
        _ => 0,
    };
    if mode & FOG_MULT2 != 0 {
        fog |= FOGMULT;
    }
    if mode & FOG_ADD2 != 0 {
        fog |= FOGADD;
    }
    set_fog_mode_reg(cpu, fog);
}

pub fn fog_color_value(cpu: &mut Cpu, color: u32) {
    let color = swizzle(cpu, color);
    gc(cpu).fog_color = color;
    hw::write(cpu, ALL, FOG_COLOR, color);
}

/// The fog table: 64 levels, by W from `gu::fog_table_index_to_w`.
/// Each register has two entries and their deltas to the next.
pub fn fog_table(cpu: &mut Cpu, table: &[u8; FOG_TABLE_SIZE]) {
    gc(cpu).fog_table = table.to_vec();
    for i in 0..FOG_TABLE_SIZE / 2 {
        let e0 = table[2 * i];
        let e1 = table[2 * i + 1];
        let next = table.get(2 * i + 2).copied().unwrap_or(e1);
        let d0 = e1.wrapping_sub(e0).wrapping_shl(2);
        let d1 = next.wrapping_sub(e1).wrapping_shl(2);
        let value = (e1 as u32) << 24 | (d1 as u32) << 16 | (e0 as u32) << 8 | d0 as u32;
        hw::write(cpu, FBI, FOG_TABLE + i, value);
    }
}

/// The gamma table (grGammaCorrectionValue).
pub fn gamma_correction_value(cpu: &mut Cpu, gamma: f32) {
    gc(cpu).gamma = gamma;
    hw::set_gamma(cpu, gamma);
}

/// Y up from the bottom of the screen, or down from its top
/// (grSstOrigin).
pub fn sst_origin(cpu: &mut Cpu, origin: u32) {
    let fbz = gc(cpu).fbz_mode & !YORIGIN;
    let bit = if origin == ORIGIN_LOWER_LEFT {
        YORIGIN
    } else {
        0
    };
    set_fbz_mode(cpu, fbz | bit);
}

/// grHints: which vertex coordinates differ between the units, and
/// whether mipmaps may be dithered.
pub fn hints(cpu: &mut Cpu, kind: u32, value: u32) {
    match kind {
        HINT_STWHINT => gc(cpu).stw_hint = value,
        HINT_ALLOW_MIPMAP_DITHER => gc(cpu).allow_lod_dither = value != 0,
        _ => {}
    }
}

/// The buffer drawing goes to (grRenderBuffer): front or back.
pub fn render_buffer(cpu: &mut Cpu, buffer: u32) {
    if buffer > BUFFER_BACKBUFFER {
        return;
    }
    gc(cpu).render = buffer;
    let fbz = gc(cpu).fbz_mode & !DRAWBUFFER;
    set_fbz_mode(cpu, fbz | buffer << 14);
    if hw::banshee(cpu) && state(cpu).screen.open && gc(cpu).target[1] == 0 {
        let addr = buffer_addr(cpu, buffer);
        hw::write(cpu, ALL, render3d::COL_BUFFER_ADDR, addr);
    }
}

/// A Banshee's buffer Glide's front, back or auxiliary buffer is now.
fn buffer_addr(cpu: &mut Cpu, buffer: u32) -> u32 {
    let s = &state(cpu).screen;
    match buffer {
        BUFFER_FRONTBUFFER => s.color[s.front as usize % 3],
        BUFFER_BACKBUFFER => s.color[s.back as usize % 3],
        _ => s.aux,
    }
}

/// The constant alpha and depth of linear frame buffer writes without
/// them.
pub fn lfb_constant_alpha(cpu: &mut Cpu, alpha: u32) {
    gc(cpu).lfb_constant_alpha = alpha & 0xFF;
}

pub fn lfb_constant_depth(cpu: &mut Cpu, depth: u32) {
    gc(cpu).lfb_constant_depth = depth & 0xFFFF;
}

/// The order of the colour components of linear frame buffer writes.
pub fn lfb_write_color_format(cpu: &mut Cpu, format: u32) {
    let mode = gc(cpu).lfb_mode & !LFB_RGBALANES;
    set_lfb_mode(cpu, mode | (format & 3) << 9);
}

/// Byte and word swapping of linear frame buffer writes.
pub fn lfb_write_color_swizzle(cpu: &mut Cpu, swizzle_bytes: bool, swap_words: bool) {
    let mut mode = gc(cpu).lfb_mode & !(LFB_WRITE_SWAP16 | LFB_BYTESWAP);
    if swizzle_bytes {
        mode |= LFB_BYTESWAP;
    }
    if swap_words {
        mode |= LFB_WRITE_SWAP16;
    }
    set_lfb_mode(cpu, mode);
}

// --- Textures ---

/// The texels of a mipmap level `lod` of `aspect`: its width and height.
pub fn level_size(aspect: u32, lod: u32) -> (u32, u32) {
    let big = 256 >> lod.min(8);
    if aspect <= ASPECT_1X1 {
        (big, (big >> (3 - aspect)).max(1))
    } else {
        ((big >> (aspect.min(6) - 3)).max(1), big)
    }
}

/// Bytes a texel of `format` takes.
fn texel_bytes(format: u32) -> u32 {
    if format >= TEXFMT_16BIT { 2 } else { 1 }
}

/// The bytes level `lod` takes in texture memory: four texels at the
/// least.
fn level_bytes(aspect: u32, lod: u32, format: u32) -> u32 {
    let (w, h) = level_size(aspect, lod);
    (w * h).max(4) * texel_bytes(format)
}

/// Whether level `lod` is among the levels `even_odd` keeps.
fn level_kept(lod: u32, even_odd: u32) -> bool {
    let bit = if lod & 1 != 0 {
        MIPMAPLEVELMASK_ODD
    } else {
        MIPMAPLEVELMASK_EVEN
    };
    even_odd & bit != 0
}

/// The bytes of the levels kept from `from` up to, not with, `to`.
fn levels_bytes(from: u32, to: u32, aspect: u32, format: u32, even_odd: u32) -> u32 {
    (from..to)
        .filter(|&l| even_odd == MIPMAPLEVELMASK_BOTH || level_kept(l, even_odd))
        .map(|l| level_bytes(aspect, l, format))
        .sum()
}

/// The texture memory a texture takes (grTexTextureMemRequired),
/// rounded up to the alignment of texture addresses: 8 bytes, 16 on a
/// Banshee.
pub fn tex_texture_mem_required(cpu: &Cpu, even_odd: u32, info: &TexInfo) -> u32 {
    if !info.valid() || even_odd == 0 || even_odd > MIPMAPLEVELMASK_BOTH {
        return 0;
    }
    let bytes = levels_bytes(
        info.large_lod,
        info.small_lod + 1,
        info.aspect,
        info.format,
        even_odd,
    );
    let align = if hw::banshee(cpu) { 16 } else { 8 };
    bytes.next_multiple_of(align)
}

/// The lowest and highest address a texture may start at.
pub fn tex_min_address(_cpu: &mut Cpu, _tmu: u32) -> u32 {
    0
}

pub fn tex_max_address(cpu: &mut Cpu, tmu: u32) -> u32 {
    let (_, size) = hw::texture_memory(cpu, tmu);
    size.saturating_sub(8)
}

/// The card's texBaseAddr for a texture whose largest level `info`
/// says is at `start` of texture unit `tmu`'s memory: where level 0
/// would be.
fn tex_base_reg(
    cpu: &mut Cpu,
    tmu: u32,
    start: u32,
    large_lod: u32,
    info: &TexInfo,
    even_odd: u32,
) -> u32 {
    let (base, _) = hw::texture_memory(cpu, tmu);
    let before = levels_bytes(0, large_lod, info.aspect, info.format, even_odd);
    let addr = base.wrapping_add(start).wrapping_sub(before);
    if hw::banshee(cpu) {
        addr & 0xFF_FFF0
    } else {
        (addr >> 3) & 0x7_FFFF
    }
}

/// The texture texture unit `tmu` maps (grTexSource), at `start` of its
/// memory.
pub fn tex_source(cpu: &mut Cpu, tmu: u32, start: u32, even_odd: u32, info: &TexInfo) {
    if !tmu_ok(cpu, tmu) || !info.valid() || even_odd == 0 || even_odd > 3 {
        return;
    }
    let base = tex_base_reg(cpu, tmu, start, info.large_lod, info, even_odd);
    let t = &mut gc(cpu).tmu[tmu as usize];
    t.small_lod = info.small_lod;
    t.large_lod = info.large_lod;
    t.even_odd = even_odd;
    t.tex_base[0] = base;
    let mode = (t.texture_mode & !TFORMAT) | info.format << 8 | TPERSP_ST | TCLAMPW;
    let small = if t.mm_mode == MIPMAP_DISABLE {
        info.large_lod
    } else {
        info.small_lod
    };
    let mut lod = t.t_lod & !(LODMIN | LODMAX | LOD_ASPECT | LOD_TSPLIT | LOD_ODD | LOD_S_IS_WIDER);
    lod |= lod_min_max(info.large_lod, small) | even_odd_bits(even_odd) | aspect_bits(info.aspect);
    hw::write(cpu, tmu_chip(tmu), TEX_BASE_ADDR, base);
    set_texture_mode(cpu, tmu, mode);
    set_t_lod(cpu, tmu, lod);
}

/// Separate base addresses for levels 256, 128, 64 and 32 to 1
/// (grTexMultibase).
pub fn tex_multibase(cpu: &mut Cpu, tmu: u32, enable: bool) {
    if !tmu_ok(cpu, tmu) {
        return;
    }
    let lod = gc(cpu).tmu[tmu as usize].t_lod & !TMULTIBASEADDR;
    set_t_lod(cpu, tmu, lod | if enable { TMULTIBASEADDR } else { 0 });
}

/// The base address of one range of levels (grTexMultibaseAddress).
pub fn tex_multibase_address(
    cpu: &mut Cpu,
    tmu: u32,
    range: u32,
    start: u32,
    even_odd: u32,
    info: &TexInfo,
) {
    if !tmu_ok(cpu, tmu) || range > TEXBASE_32_TO_1 || info.aspect > ASPECT_1X8 {
        return;
    }
    let lod = [0, 1, 2, 3][range as usize];
    let base = tex_base_reg(cpu, tmu, start, lod, info, even_odd);
    let reg = [
        TEX_BASE_ADDR,
        TEX_BASE_ADDR_1,
        TEX_BASE_ADDR_2,
        TEX_BASE_ADDR_3_8,
    ][range as usize];
    gc(cpu).tmu[tmu as usize].tex_base[range as usize] = base;
    hw::write(cpu, tmu_chip(tmu), reg, base);
}

pub fn tex_filter_mode(cpu: &mut Cpu, tmu: u32, min: u32, mag: u32) {
    if !tmu_ok(cpu, tmu) {
        return;
    }
    let mut mode = gc(cpu).tmu[tmu as usize].texture_mode & !(TMINFILTER | TMAGFILTER);
    if min == TEXTUREFILTER_BILINEAR {
        mode |= TMINFILTER;
    }
    if mag == TEXTUREFILTER_BILINEAR {
        mode |= TMAGFILTER;
    }
    set_texture_mode(cpu, tmu, mode);
}

pub fn tex_clamp_mode(cpu: &mut Cpu, tmu: u32, s: u32, t: u32) {
    if !tmu_ok(cpu, tmu) {
        return;
    }
    let mut mode = gc(cpu).tmu[tmu as usize].texture_mode & !(TCLAMPS | TCLAMPT);
    if s == TEXTURECLAMP_CLAMP {
        mode |= TCLAMPS;
    }
    if t == TEXTURECLAMP_CLAMP {
        mode |= TCLAMPT;
    }
    set_texture_mode(cpu, tmu, mode);
}

/// The LOD bias, in quarters (grTexLodBiasValue).
pub fn tex_lod_bias_value(cpu: &mut Cpu, tmu: u32, bias: f32) {
    if !tmu_ok(cpu, tmu) {
        return;
    }
    let quarters = ((bias + 0.125) / 0.25) as i32 as u32 & 0x3F;
    let lod = gc(cpu).tmu[tmu as usize].t_lod & !LODBIAS;
    set_t_lod(cpu, tmu, lod | quarters << 12);
}

/// The detail texture's bias, scale and maximum (grTexDetailControl).
pub fn tex_detail_control(cpu: &mut Cpu, tmu: u32, lod_bias: i32, scale: u32, max: f32) {
    if !tmu_ok(cpu, tmu) {
        return;
    }
    let max = (max.clamp(0.0, 1.0) * 255.0) as u32;
    let detail = ((lod_bias as u32) & 0x3F) << 8 | (max & 0xFF) | (scale & 7) << 14;
    gc(cpu).tmu[tmu as usize].t_detail = detail;
    hw::write(cpu, tmu_chip(tmu), T_DETAIL, detail);
}

/// Mipmapping (grTexMipMapMode): off, nearest or dithered between
/// levels, and blending between levels for trilinear.
pub fn tex_mipmap_mode(cpu: &mut Cpu, tmu: u32, mode: u32, lod_blend: bool) {
    if !tmu_ok(cpu, tmu) {
        return;
    }
    let dither_ok = gc(cpu).allow_lod_dither;
    let t = gc(cpu).tmu[tmu as usize];
    let mut lod = t.t_lod & !(LODMIN | LODMAX | LOD_ODD);
    let mut texture_mode = t.texture_mode & !(TLODDITHER | TRILINEAR);
    match mode {
        MIPMAP_DISABLE => lod |= lod_min_max(t.large_lod, t.large_lod),
        MIPMAP_NEAREST | MIPMAP_NEAREST_DITHER => {
            if mode == MIPMAP_NEAREST_DITHER && dither_ok {
                texture_mode |= TLODDITHER;
            }
            lod |= lod_min_max(t.large_lod, t.small_lod);
        }
        _ => return,
    }
    gc(cpu).tmu[tmu as usize].mm_mode = mode;
    if lod_blend {
        texture_mode |= TRILINEAR;
        if texture_mode & TC_ZERO_OTHER != 0
            && texture_mode & TC_BLEND_LODFRAC != 0
            && texture_mode & TC_REVERSE_BLEND == 0
        {
            lod |= LOD_ODD;
        }
    }
    lod |= even_odd_bits(t.even_odd);
    set_t_lod(cpu, tmu, lod);
    set_texture_mode(cpu, tmu, texture_mode);
}

/// Which of the two NCC tables decodes YIQ textures (grTexNCCTable).
pub fn tex_ncc_table(cpu: &mut Cpu, tmu: u32, table: u32) {
    if !tmu_ok(cpu, tmu) {
        return;
    }
    gc(cpu).tmu[tmu as usize].ncc_table = table & 1;
    let mode = gc(cpu).tmu[tmu as usize].texture_mode & !TNCCSELECT;
    let bit = if table & 1 != 0 { TNCCSELECT } else { 0 };
    set_texture_mode(cpu, tmu, mode | bit);
}

/// An NCC table, as GuNccTable's 12 packed registers.
pub fn tex_download_ncc_table(cpu: &mut Cpu, tmu: u32, which: u32, packed: &[u32; 12]) {
    if !tmu_ok(cpu, tmu) || which > 1 {
        return;
    }
    for (i, &v) in packed.iter().enumerate() {
        hw::write(cpu, tmu_chip(tmu), NCC_TABLE + 12 * which as usize + i, v);
    }
}

/// Entries `start` to `end` of the palette of 8-bit paletted textures,
/// 0RGB each.
pub fn tex_download_palette(cpu: &mut Cpu, tmu: u32, palette: &[u32; 256], start: u32, end: u32) {
    if !tmu_ok(cpu, tmu) {
        return;
    }
    for i in start.min(255)..=end.min(255) {
        let value = 0x8000_0000 | (i & 0xFE) << 23 | (palette[i as usize] & 0xFF_FFFF);
        hw::write(cpu, tmu_chip(tmu), NCC_TABLE + 4 + (i & 7) as usize, value);
    }
}

/// Rows `t0` to `t1` of mipmap level `lod` of a texture whose largest
/// level `large_lod` is at `start` of texture unit `tmu`'s memory
/// (grTexDownloadMipMapLevelPartial). `data` has the rows from `t0`.
#[allow(clippy::too_many_arguments)]
pub fn tex_download_level(
    cpu: &mut Cpu,
    tmu: u32,
    start: u32,
    lod: u32,
    large_lod: u32,
    aspect: u32,
    format: u32,
    even_odd: u32,
    data: &[u8],
    t0: u32,
    t1: u32,
) {
    if !tmu_ok(cpu, tmu)
        || lod > LOD_1
        || large_lod > lod
        || aspect > ASPECT_1X8
        || format > 0xF
        || even_odd == 0
        || even_odd > 3
        || !level_kept(lod, even_odd)
    {
        return;
    }
    let (w, h) = level_size(aspect, lod);
    let t1 = t1.min(h - 1);
    if t0 > t1 {
        return;
    }
    let bpp = texel_bytes(format);
    let row = w * bpp;
    let texel = |t: u32, at: u32| -> u8 {
        let i = (t - t0) as usize * row as usize + at as usize;
        data.get(i).copied().unwrap_or(0)
    };
    let info = TexInfo {
        small_lod: LOD_1,
        large_lod,
        aspect,
        format,
    };
    if hw::banshee(cpu) {
        let (base, len) = hw::texture_memory(cpu, tmu);
        // Past the texture memory there is no texture to write.
        if start >= len {
            return;
        }
        let level = base
            .wrapping_add(start)
            .wrapping_add(levels_bytes(large_lod, lod, aspect, format, even_odd));
        let first = level + t0 * row;
        let end = level + (t1 + 1) * row;
        let port_base = first & 0xFF_FFF0;
        hw::write(cpu, tmu_chip(0), TEX_BASE_ADDR, port_base);
        let mut at = first & !3;
        while at < end {
            let mut bytes = hw::memory_read(cpu, at).unwrap_or(0).to_le_bytes();
            for (i, byte) in bytes.iter_mut().enumerate() {
                let a = at + i as u32;
                if a >= first && a < end {
                    let offset = a - level;
                    *byte = texel(offset / row, offset % row);
                }
            }
            hw::texture_port_write(cpu, at - port_base, u32::from_le_bytes(bytes));
            at += 4;
        }
        let shadow = gc(cpu).tmu[0].tex_base[0];
        hw::write(cpu, tmu_chip(0), TEX_BASE_ADDR, shadow);
        return;
    }
    // A Voodoo Graphics or Voodoo2: the unit's registers for this
    // texture while its rows go to the texture space, then Glide's.
    let base = tex_base_reg(cpu, tmu, start, large_lod, &info, even_odd);
    let chip = tmu_chip(tmu);
    hw::write(cpu, chip, TEX_BASE_ADDR, base);
    hw::write(cpu, chip, TEXTURE_MODE, format << 8);
    let lod_reg = lod_min_max(large_lod, LOD_1) | even_odd_bits(even_odd) | aspect_bits(aspect);
    hw::write(cpu, chip, T_LOD, lod_reg);
    // Four 8-bit texels or two 16-bit ones a dword.
    let per = 4 / bpp;
    for t in t0..=t1 {
        let mut s = 0;
        while s < w.max(1) {
            let mut bytes = [0u8; 4];
            // A row narrower than a dword: zeros after it.
            for (i, byte) in bytes.iter_mut().enumerate() {
                let at = s * bpp + i as u32;
                if at < row {
                    *byte = texel(t, at);
                }
            }
            let offset = lod << 17 | t << 9 | s << 1;
            hw::texture_write(cpu, tmu, offset, u32::from_le_bytes(bytes));
            s += per;
        }
    }
    let t = gc(cpu).tmu[tmu as usize];
    hw::write(cpu, chip, TEX_BASE_ADDR, t.tex_base[0]);
    hw::write(cpu, chip, TEXTURE_MODE, t.texture_mode);
    hw::write(cpu, chip, T_LOD, t.t_lod);
}

/// All levels of a texture from `data`, largest first
/// (grTexDownloadMipMap).
pub fn tex_download_mipmap(
    cpu: &mut Cpu,
    tmu: u32,
    start: u32,
    even_odd: u32,
    info: &TexInfo,
    data: &[u8],
) {
    if !info.valid() {
        return;
    }
    let mut at = 0usize;
    for lod in info.large_lod..=info.small_lod {
        let (w, h) = level_size(info.aspect, lod);
        let len = (w * h * texel_bytes(info.format)) as usize;
        let level = data.get(at..).unwrap_or(&[]);
        tex_download_level(
            cpu,
            tmu,
            start,
            lod,
            info.large_lod,
            info.aspect,
            info.format,
            even_odd,
            level,
            0,
            h - 1,
        );
        at += len;
    }
}

// --- Drawing ---

/// Which parameters triangles send, from the combine units, the depth
/// buffer, fog and the hints (Glide's paramIndex).
fn params(g: &Context) -> u32 {
    let mut p = 0;
    if g.fbz_color_path & ENTEXTUREMAP != 0 {
        p |= P_OOW_FBI;
        if g.tmu_mask & 1 != 0 {
            p |= P_W_TMU0 | P_ST_TMU0;
        }
        if g.tmu_mask & 2 != 0 {
            p |= P_W_TMU1 | P_ST_TMU1;
        }
    }
    if g.cc_requires_it_rgb && !g.delta0 {
        p |= P_RGB;
    }
    if g.ac_requires_it_alpha {
        p |= P_ALPHA;
    }
    // Alpha's local select is Z.
    if g.fbz_color_path & (3 << 5) == 2 << 5 {
        p |= P_OOZ;
    }
    if g.fbz_mode & ENDEPTHBUFFER != 0 {
        p |= if g.fbz_mode & WBUFFER != 0 {
            P_OOW_FBI
        } else {
            P_OOZ
        };
    }
    if g.fog_mode & ENFOGGING != 0 {
        p |= if g.fog_mode & (FOG_Z | FOG_ALPHA) == FOG_Z | FOG_ALPHA {
            P_OOW_FBI
        } else if g.fog_mode & FOG_Z != 0 {
            P_OOZ
        } else if g.fog_mode & FOG_ALPHA != 0 {
            P_ALPHA
        } else {
            P_OOW_FBI
        };
    }
    let hint = g.stw_hint;
    if p & P_W_TMU0 != 0 && hint & STWHINT_W_DIFF_TMU0 == 0 {
        p &= !P_W_TMU0;
    }
    if p & (P_ST_TMU0 | P_ST_TMU1) == P_ST_TMU0 | P_ST_TMU1 && hint & STWHINT_ST_DIFF_TMU1 == 0 {
        p &= !P_ST_TMU1;
    }
    if p & P_W_TMU1 != 0 && hint & STWHINT_W_DIFF_TMU1 == 0 {
        p &= !P_W_TMU1;
    }
    p
}

/// The parameter registers a triangle writes, each with the chips it
/// goes to: (parameter, start register, chips).
fn param_list(p: u32) -> Vec<(Param, usize, u32)> {
    let mut list = Vec::with_capacity(12);
    if p & P_RGB != 0 {
        list.push((Param::R, FSTART_R, ALL));
        list.push((Param::G, FSTART_G, ALL));
        list.push((Param::B, FSTART_B, ALL));
    }
    if p & P_OOZ != 0 {
        list.push((Param::Ooz, FSTART_Z, ALL));
    }
    if p & P_ALPHA != 0 {
        list.push((Param::A, FSTART_A, ALL));
    }
    if p & P_ST_TMU0 != 0 {
        list.push((Param::S(0), FSTART_S, ALL));
        list.push((Param::T(0), FSTART_T, ALL));
    }
    if p & P_OOW_FBI != 0 {
        list.push((Param::Oow, FSTART_W, ALL));
    }
    // Units 0 and on, and 1 and on.
    if p & P_W_TMU0 != 0 {
        list.push((Param::W(0), FSTART_W, 0xE));
    }
    if p & P_ST_TMU1 != 0 {
        list.push((Param::S(1), FSTART_S, 0xC));
        list.push((Param::T(1), FSTART_T, 0xC));
    }
    if p & P_W_TMU1 != 0 {
        list.push((Param::W(1), FSTART_W, 0xC));
    }
    list
}

/// The start register's gradients: in X 8 registers on, in Y 16.
const DX: usize = 8;
const DY: usize = 16;

/// A float's order as Glide compares Y: by its bits, negative ones
/// turned round.
fn y_key(y: f32) -> i32 {
    let i = y.to_bits() as i32;
    if i < 0 { i ^ 0x7FFF_FFFF } else { i }
}

/// Draw a triangle (grDrawTriangle). False if it wasn't drawn: no area,
/// culled, or Glide isn't open.
pub fn draw_triangle(cpu: &mut Cpu, a: &Vertex, b: &Vertex, c: &Vertex) -> bool {
    if !active(cpu) {
        return false;
    }
    gc(cpu).triangles[1] = gc(cpu).triangles[1].wrapping_add(1);
    let mut cull = gc(cpu).cull_mode;
    let (ay, by, cy) = (y_key(a.y), y_key(b.y), y_key(c.y));
    // Sorted by Y as Glide sorts them; each odd reordering turns the
    // culling test round.
    let (va, vb, vc) = if ay < by {
        if by > cy {
            if ay < cy {
                cull ^= 1;
                (a, c, b)
            } else {
                (c, a, b)
            }
        } else {
            (a, b, c)
        }
    } else if by < cy {
        if ay < cy {
            cull ^= 1;
            (b, a, c)
        } else {
            (b, c, a)
        }
    } else {
        cull ^= 1;
        (c, b, a)
    };
    let dx_ab = va.x - vb.x;
    let dx_bc = vb.x - vc.x;
    let dy_ab = va.y - vb.y;
    let dy_bc = vb.y - vc.y;
    let area = dx_ab * dy_bc - dx_bc * dy_ab;
    if area.to_bits() & 0x7FFF_FFFF == 0 {
        return false;
    }
    if gc(cpu).cull_mode != CULL_DISABLE {
        let negative = area.to_bits() >> 31;
        if negative == cull & 1 {
            return false;
        }
    }
    let ooa = 1.0 / area;
    for (reg, v) in [
        (FVERTEX_AX, va.x),
        (FVERTEX_AY, va.y),
        (FVERTEX_BX, vb.x),
        (FVERTEX_BY, vb.y),
        (FVERTEX_CX, vc.x),
        (FVERTEX_CY, vc.y),
    ] {
        hw::write_f(cpu, ALL, reg, v);
    }
    let (dx_bc, dy_ab, dx_ab, dy_bc) = (dx_bc * ooa, dy_ab * ooa, dx_ab * ooa, dy_bc * ooa);
    let p = params(gc(cpu));
    for (param, reg, chips) in param_list(p) {
        let (pa, pb, pc) = (va.param(param), vb.param(param), vc.param(param));
        let d_ab = pa - pb;
        let d_bc = pb - pc;
        hw::write_f(cpu, chips, reg, pa);
        hw::write_f(cpu, chips, reg + DX, d_ab * dy_bc - d_bc * dy_ab);
        hw::write_f(cpu, chips, reg + DY, d_bc * dx_ab - d_ab * dx_bc);
    }
    hw::write_f(cpu, ALL, FTRIANGLE_CMD, area);
    gc(cpu).triangles[0] = gc(cpu).triangles[0].wrapping_add(1);
    true
}

/// Draw a line (grDrawLine): two triangles half a pixel either side of
/// it, from the lower Y to the higher.
pub fn draw_line(cpu: &mut Cpu, a: &Vertex, b: &Vertex) {
    if !active(cpu) {
        return;
    }
    let (a, b) = if b.y - a.y < 0.0 { (b, a) } else { (a, b) };
    let ady = b.y - a.y;
    let dx = b.x - a.x;
    let adx = dx.abs();
    let list = param_list(params(gc(cpu)));
    if adx >= ady {
        if adx == 0.0 {
            return;
        }
        let m = 1.0 / dx;
        hw::write_f(cpu, ALL, FVERTEX_AX, a.x);
        hw::write_f(cpu, ALL, FVERTEX_BX, b.x);
        hw::write_f(cpu, ALL, FVERTEX_CX, b.x);
        hw::write_f(cpu, ALL, FVERTEX_AY, a.y - 0.5);
        hw::write_f(cpu, ALL, FVERTEX_BY, b.y - 0.5);
        hw::write_f(cpu, ALL, FVERTEX_CY, b.y + 0.5);
        for &(param, reg, chips) in &list {
            let pa = a.param(param);
            hw::write_f(cpu, chips, reg, pa);
            hw::write_f(cpu, chips, reg + DX, (b.param(param) - pa) * m);
            hw::write_f(cpu, chips, reg + DY, 0.0);
        }
        hw::write_f(cpu, ALL, FTRIANGLE_CMD, dx);
        hw::write_f(cpu, ALL, FVERTEX_BX, a.x);
        hw::write_f(cpu, ALL, FVERTEX_BY, a.y + 0.5);
        hw::write_f(cpu, ALL, FTRIANGLE_CMD, -dx);
    } else {
        let m = 1.0 / ady;
        hw::write_f(cpu, ALL, FVERTEX_AY, a.y);
        hw::write_f(cpu, ALL, FVERTEX_BY, b.y);
        hw::write_f(cpu, ALL, FVERTEX_CY, b.y);
        hw::write_f(cpu, ALL, FVERTEX_AX, a.x - 0.5);
        hw::write_f(cpu, ALL, FVERTEX_BX, b.x - 0.5);
        hw::write_f(cpu, ALL, FVERTEX_CX, b.x + 0.5);
        for &(param, reg, chips) in &list {
            let pa = a.param(param);
            hw::write_f(cpu, chips, reg, pa);
            hw::write_f(cpu, chips, reg + DX, 0.0);
            hw::write_f(cpu, chips, reg + DY, (b.param(param) - pa) * m);
        }
        hw::write(cpu, ALL, TRIANGLE_CMD, u32::MAX);
        hw::write_f(cpu, ALL, FVERTEX_BX, a.x + 0.5);
        hw::write_f(cpu, ALL, FVERTEX_BY, a.y);
        hw::write(cpu, ALL, TRIANGLE_CMD, 1);
    }
}

/// Draw a point (grDrawPoint): a triangle from the centre of the pixel
/// the vertex is in, its parameters the vertex's.
pub fn draw_point(cpu: &mut Cpu, p: &Vertex) {
    if !active(cpu) {
        return;
    }
    // 12.4 fixed point: the pixel, and half a pixel. The triangle's
    // far corners are at the pixel's far edge, 2048 for pixel 2047,
    // which the card's registers take as -2048: the pixel is at most
    // 2046.
    let fixed = |v: f32| -> u32 {
        let v = if v.is_finite() {
            v.clamp(-2048.0, 2046.0)
        } else {
            0.0
        };
        ((v.floor() as i32 * 16) + 8) as u32 & 0xFFFF
    };
    let (x, y) = (fixed(p.x), fixed(p.y));
    hw::write(cpu, ALL, VERTEX_AX, x);
    hw::write(cpu, ALL, VERTEX_AY, y);
    hw::write(cpu, ALL, VERTEX_BX, (x + 8) & 0xFFFF);
    hw::write(cpu, ALL, VERTEX_BY, y);
    hw::write(cpu, ALL, VERTEX_CY, (y + 8) & 0xFFFF);
    hw::write(cpu, ALL, VERTEX_CX, (x + 8) & 0xFFFF);
    for (param, reg, chips) in param_list(params(gc(cpu))) {
        hw::write_f(cpu, chips, reg, p.param(param));
    }
    hw::write(cpu, ALL, TRIANGLE_CMD, 1);
}

/// Draw a convex polygon as a fan of triangles from its first vertex
/// (grDrawPolygonVertexList, and the planar ones).
pub fn draw_polygon(cpu: &mut Cpu, vertices: &[Vertex]) {
    for i in 1..vertices.len().saturating_sub(1) {
        draw_triangle(cpu, &vertices[0], &vertices[i], &vertices[i + 1]);
    }
}

// --- Buffers ---

/// Clear the buffers drawing goes to (grBufferClear): colour, alpha and
/// depth where their writes are on, within the clip window.
pub fn buffer_clear(cpu: &mut Cpu, color: u32, alpha: u32, depth: u32) {
    if !active(cpu) {
        return;
    }
    let g = gc(cpu).clone();
    if g.fbz_mode & RGBWRMASK != 0 {
        let color = to_argb(g.color_format, color);
        hw::write(cpu, ALL, COLOR1, color);
    }
    let mut za = g.za_color;
    if g.fbz_mode & (ENALPHABUFFER | ZAWRMASK) == ENALPHABUFFER | ZAWRMASK {
        za = (za & 0x00FF_FFFF) | (alpha & 0xFF) << 24;
        hw::write(cpu, ALL, ZA_COLOR, za);
    }
    if g.fbz_mode & (ENDEPTHBUFFER | ZAWRMASK) == ENDEPTHBUFFER | ZAWRMASK {
        za = (za & !0xFFFF) | (depth & 0xFFFF);
        hw::write(cpu, ALL, ZA_COLOR, za);
    }
    hw::write(cpu, ALL, FASTFILL_CMD, 1);
    hw::write(cpu, ALL, COLOR1, g.color1);
    hw::write(cpu, ALL, ZA_COLOR, g.za_color);
}

/// Show the back buffer (grBufferSwap): at once with interval 0, or
/// after `interval` vertical retraces.
pub fn buffer_swap(cpu: &mut Cpu, interval: u32) {
    if !active(cpu) {
        return;
    }
    let interval = interval.min(255);
    let cmd = if interval > 0 {
        (interval - 1) << 1 | 1
    } else {
        0
    };
    if !hw::banshee(cpu) {
        hw::write(cpu, ALL, SWAPBUFFER_CMD, cmd);
        return;
    }
    if !state(cpu).screen.open {
        return;
    }
    // The overlay shows the back buffer, which is the front one now.
    let back = buffer_addr(cpu, BUFFER_BACKBUFFER);
    hw::write(cpu, ALL, render3d::SWAP_PENDING, 1);
    hw::write(cpu, ALL, render3d::LEFT_OVERLAY_BUF, back);
    hw::write(cpu, ALL, SWAPBUFFER_CMD, cmd);
    let s = &mut state(cpu).screen;
    let n = s.color_buffers.max(2);
    s.front = s.back;
    s.back = (s.back + 1) % n;
    let render = gc(cpu).render;
    render_buffer(cpu, render);
}

/// Swaps waiting for their retrace (grBufferNumPending).
pub fn buffer_num_pending(cpu: &Cpu) -> u32 {
    (sst_status(cpu) >> 28) & 7
}

/// The card's status register (grSstStatus).
pub fn sst_status(cpu: &Cpu) -> u32 {
    if cpu.bus.voodoo.is_none() {
        return 0;
    }
    hw::read(cpu, STATUS)
}

/// Whether the card is busy (grSstIsBusy).
pub fn sst_is_busy(cpu: &Cpu) -> bool {
    sst_status(cpu) & (1 << 9) != 0
}

/// Whether the card is in its vertical retrace (grSstVRetraceOn).
pub fn sst_vretrace_on(cpu: &Cpu) -> bool {
    cpu.bus.voodoo.is_some() && sst_status(cpu) & (1 << 6) == 0
}

/// The line the card's display is on (grSstVideoLine).
pub fn sst_video_line(cpu: &Cpu) -> u32 {
    if cpu.bus.voodoo.is_none() {
        return 0;
    }
    if hw::banshee(cpu) {
        return cpu
            .bus
            .voodoo_read_32(crate::voodoo::banshee::VID_IN_STATUS_CURRENT_LINE)
            & 0x7FF;
    }
    hw::read(cpu, V_RETRACE) & 0x1FFF
}

// --- The linear frame buffer ---

/// Pixels a row of the 3D engine's linear frame buffer has.
fn lfb_row_pixels(cpu: &Cpu) -> u32 {
    if hw::banshee(cpu) { 2048 } else { 1024 }
}

/// Lock a buffer for reading or writing (grLfbLock): where the process
/// sees it, or None. Writes are in `write_mode`, through the pixel
/// pipeline if `pipeline` says, from `origin`.
pub fn lfb_lock(
    cpu: &mut Cpu,
    kind: u32,
    buffer: u32,
    write_mode: u32,
    origin: u32,
    pipeline: bool,
) -> Option<LfbInfo> {
    if !active(cpu) || !state(cpu).screen.open || buffer > BUFFER_AUXBUFFER {
        return None;
    }
    let kind = kind & !LFB_NOIDLE;
    if kind > LFB_WRITE_ONLY || gc(cpu).locks[kind as usize] != NONE {
        return None;
    }
    let origin = if origin == ORIGIN_LOWER_LEFT {
        ORIGIN_LOWER_LEFT
    } else {
        ORIGIN_UPPER_LEFT
    };
    let banshee = hw::banshee(cpu);
    let info = if kind == LFB_READ_ONLY {
        let mut mode = gc(cpu).lfb_mode & !(LFB_READ_BUFFER | LFB_YORIGIN);
        mode |= buffer << 6;
        if origin == ORIGIN_LOWER_LEFT {
            mode |= LFB_YORIGIN;
        }
        set_lfb_mode(cpu, mode);
        if banshee {
            let addr = buffer_addr(cpu, buffer);
            let view = hw::view(cpu)?;
            LfbInfo {
                ptr: view + hw::aperture_offset(cpu, addr),
                stride: 4096,
                write_mode,
                origin,
            }
        } else {
            LfbInfo {
                ptr: hw::view(cpu)?,
                stride: 2048,
                write_mode,
                origin,
            }
        }
    } else {
        let write_mode = match (buffer, write_mode) {
            (BUFFER_AUXBUFFER, LFBWRITEMODE_ANY | LFBWRITEMODE_ZA16) => LFBWRITEMODE_ZA16,
            (BUFFER_AUXBUFFER, _) => return None,
            (_, LFBWRITEMODE_ANY) => LFBWRITEMODE_565,
            (_, LFBWRITEMODE_ZA16) => return None,
            (_, m @ (0..=2 | 4 | 5 | 0xC..=0xE)) => m,
            _ => return None,
        };
        let g = gc(cpu).clone();
        // The constant depth and alpha of writes without them, and no
        // depth bias.
        let za = g.lfb_constant_depth | g.lfb_constant_alpha << 24;
        hw::write(cpu, ALL, ZA_COLOR, za);
        let mut fbz = g.fbz_mode & !ENZBIAS;
        let mut mode = g.lfb_mode & !(LFB_WRITE_BUFFER | LFB_YORIGIN | LFB_FORMAT | LFB_ENPIXPIPE);
        let target = if buffer == BUFFER_AUXBUFFER {
            0
        } else {
            buffer
        };
        mode |= target << 4 | write_mode;
        if origin == ORIGIN_LOWER_LEFT {
            mode |= LFB_YORIGIN;
        }
        if pipeline {
            mode |= LFB_ENPIXPIPE;
            fbz = (fbz & !YORIGIN)
                | if origin == ORIGIN_LOWER_LEFT {
                    YORIGIN
                } else {
                    0
                };
        }
        set_lfb_mode(cpu, mode);
        hw::write(cpu, ALL, FBZ_MODE, fbz);
        let wide = matches!(write_mode, 4 | 5 | 0xC..=0xE);
        if banshee {
            let addr = buffer_addr(cpu, buffer);
            if write_mode == LFBWRITEMODE_565 && !pipeline && origin == ORIGIN_UPPER_LEFT {
                let view = hw::view(cpu)?;
                LfbInfo {
                    ptr: view + hw::aperture_offset(cpu, addr),
                    stride: 4096,
                    write_mode,
                    origin,
                }
            } else {
                if buffer != BUFFER_AUXBUFFER {
                    hw::write(cpu, ALL, render3d::COL_BUFFER_ADDR, addr);
                }
                LfbInfo {
                    ptr: hw::pipe_view(cpu)?,
                    stride: if wide { 8192 } else { 4096 },
                    write_mode,
                    origin,
                }
            }
        } else {
            LfbInfo {
                ptr: hw::view(cpu)?,
                stride: if wide { 4096 } else { 2048 },
                write_mode,
                origin,
            }
        }
    };
    gc(cpu).locks[kind as usize] = buffer;
    Some(info)
}

/// Unlock a locked buffer (grLfbUnlock): the depth bias and the buffer
/// drawn into come back. False if it wasn't locked.
pub fn lfb_unlock(cpu: &mut Cpu, kind: u32, buffer: u32) -> bool {
    let kind = (kind & !LFB_NOIDLE).min(2);
    if kind > LFB_WRITE_ONLY || gc(cpu).locks[kind as usize] != buffer || buffer == NONE {
        return false;
    }
    gc(cpu).locks[kind as usize] = NONE;
    let g = gc(cpu).clone();
    hw::write(cpu, ALL, ZA_COLOR, g.za_color);
    hw::write(cpu, ALL, FBZ_MODE, g.fbz_mode);
    if kind == LFB_WRITE_ONLY && hw::banshee(cpu) {
        let render = g.render;
        render_buffer(cpu, render);
    }
    true
}

/// Read a rectangle of a buffer through the 3D engine's linear frame
/// buffer: 16-bit pixels, row after row.
fn read_lfb_rect(cpu: &mut Cpu, x: u32, y: u32, w: u32, h: u32) -> Vec<u16> {
    let row_pixels = lfb_row_pixels(cpu);
    let mut out = Vec::with_capacity((w * h) as usize);
    for row in y..y + h {
        for col in x..x + w {
            let pixel = row * row_pixels + col;
            let dword = hw::lfb_read(cpu, (pixel * 2) & !3);
            out.push((dword >> (16 * (pixel & 1))) as u16);
        }
    }
    out
}

/// A rectangle of a buffer (grLfbReadRegion): 5-6-5 pixels of a colour
/// buffer, or the auxiliary buffer's depth or alpha, from the top left.
pub fn lfb_read_region(
    cpu: &mut Cpu,
    buffer: u32,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
) -> Option<Vec<u16>> {
    if !active(cpu) || buffer > BUFFER_AUXBUFFER {
        return None;
    }
    let g = gc(cpu).clone();
    let mode = (g.lfb_mode & !(LFB_READ_BUFFER | LFB_YORIGIN)) | buffer << 6;
    hw::write(cpu, ALL, LFB_MODE, mode);
    let banshee = hw::banshee(cpu) && state(cpu).screen.open && g.target[1] == 0;
    if banshee && buffer != BUFFER_AUXBUFFER {
        let addr = buffer_addr(cpu, buffer);
        hw::write(cpu, ALL, render3d::COL_BUFFER_ADDR, addr);
    }
    let pixels = read_lfb_rect(cpu, x, y, w, h);
    hw::write(cpu, ALL, LFB_MODE, g.lfb_mode);
    if banshee && buffer != BUFFER_AUXBUFFER {
        render_buffer(cpu, g.render);
    }
    Some(pixels)
}

/// Write a rectangle of `format` pixels to a buffer (grLfbWriteRegion):
/// `data` rows `stride` bytes apart, 16-bit formats two bytes a pixel,
/// the others four.
#[allow(clippy::too_many_arguments)]
pub fn lfb_write_region(
    cpu: &mut Cpu,
    buffer: u32,
    x: u32,
    y: u32,
    format: u32,
    w: u32,
    h: u32,
    stride: u32,
    data: &[u8],
) -> bool {
    if !active(cpu) || buffer > BUFFER_AUXBUFFER {
        return false;
    }
    let valid = match buffer {
        BUFFER_AUXBUFFER => format == LFBWRITEMODE_ZA16,
        _ => matches!(format, 0..=2 | 4 | 5 | 0xC..=0xE),
    };
    if !valid {
        return false;
    }
    let g = gc(cpu).clone();
    let target = if buffer == BUFFER_AUXBUFFER {
        0
    } else {
        buffer
    };
    let mode = (g.lfb_mode & !(LFB_WRITE_BUFFER | LFB_YORIGIN | LFB_FORMAT | LFB_ENPIXPIPE))
        | target << 4
        | format;
    hw::write(cpu, ALL, LFB_MODE, mode);
    let za = g.lfb_constant_depth | g.lfb_constant_alpha << 24;
    hw::write(cpu, ALL, ZA_COLOR, za);
    let banshee = hw::banshee(cpu) && state(cpu).screen.open && g.target[1] == 0;
    if banshee && buffer != BUFFER_AUXBUFFER {
        let addr = buffer_addr(cpu, buffer);
        hw::write(cpu, ALL, render3d::COL_BUFFER_ADDR, addr);
    }
    let row_pixels = lfb_row_pixels(cpu);
    let wide = matches!(format, 4 | 5 | 0xC..=0xE);
    let byte = |at: usize| data.get(at).copied().unwrap_or(0);
    for row in 0..h {
        let src = (row * stride) as usize;
        let line = (y + row) * row_pixels;
        for col in 0..w {
            let pixel = line + x + col;
            if wide {
                let at = src + 4 * col as usize;
                let v = u32::from_le_bytes([byte(at), byte(at + 1), byte(at + 2), byte(at + 3)]);
                hw::lfb_write(cpu, pixel * 4, v, u32::MAX);
            } else {
                let at = src + 2 * col as usize;
                let v = u16::from_le_bytes([byte(at), byte(at + 1)]) as u32;
                let shift = 16 * (pixel & 1);
                hw::lfb_write(cpu, (pixel * 2) & !3, v << shift, 0xFFFF << shift);
            }
        }
    }
    hw::write(cpu, ALL, LFB_MODE, g.lfb_mode);
    hw::write(cpu, ALL, ZA_COLOR, g.za_color);
    if banshee && buffer != BUFFER_AUXBUFFER {
        render_buffer(cpu, g.render);
    }
    true
}

// --- Direct3D's extensions ---

/// Draw into the 16-bit buffer at `addr` of a Banshee's memory, rows
/// `stride` apart: colBufferStride's value, bytes a row (bit 15 clear)
/// or tiles a row of tiles (bit 15 set). Clip windows as big as
/// `width` x `height`. `stride` 0 gives drawing back to Glide's
/// buffers.
pub fn set_render_target(cpu: &mut Cpu, addr: u32, stride: u32, width: u32, height: u32) {
    if !hw::banshee(cpu) {
        return;
    }
    let g = gc(cpu);
    g.target[0] = addr;
    g.target[1] = stride;
    if stride == 0 {
        let render = g.render;
        render_buffer(cpu, render);
        let s = &state(cpu).screen;
        if s.open {
            let tiled = 1 << 15 | s.tiles;
            hw::write(cpu, ALL, render3d::COL_BUFFER_STRIDE, tiled);
        }
        return;
    }
    g.width = width;
    g.height = height;
    hw::write(cpu, ALL, render3d::COL_BUFFER_ADDR, addr);
    hw::write(cpu, ALL, render3d::COL_BUFFER_STRIDE, stride);
}

/// The 16-bit depth buffer at `addr` of a Banshee's memory, rows
/// `stride` apart as for `set_render_target`.
pub fn set_depth_buffer(cpu: &mut Cpu, addr: u32, stride: u32) {
    if !hw::banshee(cpu) {
        return;
    }
    let g = gc(cpu);
    g.target[2] = addr;
    g.target[3] = stride;
    hw::write(cpu, ALL, render3d::AUX_BUFFER_ADDR, addr);
    hw::write(cpu, ALL, render3d::AUX_BUFFER_STRIDE, stride);
}

/// A rectangle of the 16-bit buffer at `addr` of a Banshee's memory, rows
/// `stride` apart as for `set_render_target`, as 5-6-5 pixels row after
/// row. On a Voodoo Graphics or Voodoo2, `addr` is Glide's buffer
/// (BUFFER_FRONTBUFFER, BUFFER_BACKBUFFER) and `stride` is unused.
pub fn read_rect_565(
    cpu: &mut Cpu,
    addr: u32,
    stride: u32,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
) -> Vec<u16> {
    if cpu.bus.voodoo.is_none() {
        return Vec::new();
    }
    let g = gc(cpu).clone();
    if !hw::banshee(cpu) {
        let mode = (g.lfb_mode & !(LFB_READ_BUFFER | LFB_YORIGIN)) | (addr.min(1)) << 6;
        hw::write(cpu, ALL, LFB_MODE, mode);
        let pixels = read_lfb_rect(cpu, x, y, w, h);
        hw::write(cpu, ALL, LFB_MODE, g.lfb_mode);
        return pixels;
    }
    let old = [
        hw::read(cpu, render3d::COL_BUFFER_ADDR),
        hw::read(cpu, render3d::COL_BUFFER_STRIDE),
    ];
    hw::write(cpu, ALL, render3d::COL_BUFFER_ADDR, addr);
    hw::write(cpu, ALL, render3d::COL_BUFFER_STRIDE, stride);
    hw::write(
        cpu,
        ALL,
        LFB_MODE,
        g.lfb_mode & !(LFB_READ_BUFFER | LFB_YORIGIN),
    );
    let pixels = read_lfb_rect(cpu, x, y, w, h);
    hw::write(cpu, ALL, LFB_MODE, g.lfb_mode);
    hw::write(cpu, ALL, render3d::COL_BUFFER_ADDR, old[0]);
    hw::write(cpu, ALL, render3d::COL_BUFFER_STRIDE, old[1]);
    pixels
}

/// Which board it is, for Direct3D's capabilities.
pub fn card(cpu: &Cpu) -> Option<Card> {
    hw::board(cpu).map(|b| b.card)
}

// --- grGlideGetState and grGlideSetState ---

/// Bytes of GrState.
pub const STATE_SIZE: usize = 312;

/// Glide's state as grGlideGetState keeps it for the program.
pub fn get_state(cpu: &mut Cpu) -> [u8; STATE_SIZE] {
    let g = gc(cpu).clone();
    let mut words: Vec<u32> = vec![
        g.fbz_color_path,
        g.fog_mode,
        g.alpha_mode,
        g.fbz_mode,
        g.lfb_mode,
        g.clip_left_right,
        g.clip_bottom_top,
        g.fog_color,
        g.za_color,
        g.chroma_key,
        g.color0,
        g.color1,
        g.color_format,
        g.cull_mode,
        g.cc_requires_texture as u32
            | (g.ac_requires_texture as u32) << 1
            | (g.cc_requires_it_rgb as u32) << 2
            | (g.ac_requires_it_alpha as u32) << 3
            | (g.allow_lod_dither as u32) << 4
            | (g.delta0 as u32) << 5,
        g.tmu_mask,
        g.stw_hint,
        g.lfb_constant_alpha,
        g.lfb_constant_depth,
        g.render,
    ];
    words.extend(g.constant.iter().map(|f| f.to_bits()));
    words.extend(g.clip);
    for t in &g.tmu {
        words.extend([t.texture_mode, t.t_lod, t.t_detail]);
        words.extend(t.tex_base);
        words.extend([t.mm_mode, t.small_lod, t.large_lod, t.even_odd, t.ncc_table]);
    }
    let mut out = [0u8; STATE_SIZE];
    for (i, w) in words.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&w.to_le_bytes());
    }
    out
}

/// Glide's state back from what `get_state` gave, and the card's
/// registers with it.
pub fn set_state(cpu: &mut Cpu, bytes: &[u8; STATE_SIZE]) {
    if !active(cpu) {
        return;
    }
    let w = |i: usize| {
        u32::from_le_bytes([
            bytes[4 * i],
            bytes[4 * i + 1],
            bytes[4 * i + 2],
            bytes[4 * i + 3],
        ])
    };
    let g = gc(cpu);
    [
        g.fbz_color_path,
        g.fog_mode,
        g.alpha_mode,
        g.fbz_mode,
        g.lfb_mode,
        g.clip_left_right,
        g.clip_bottom_top,
        g.fog_color,
        g.za_color,
        g.chroma_key,
        g.color0,
        g.color1,
    ] = std::array::from_fn(w);
    g.color_format = w(12) & 3;
    g.cull_mode = w(13).min(2);
    let flags = w(14);
    g.cc_requires_texture = flags & 1 != 0;
    g.ac_requires_texture = flags & 2 != 0;
    g.cc_requires_it_rgb = flags & 4 != 0;
    g.ac_requires_it_alpha = flags & 8 != 0;
    g.allow_lod_dither = flags & 16 != 0;
    g.delta0 = flags & 32 != 0;
    g.tmu_mask = w(15) & 3;
    g.stw_hint = w(16);
    g.lfb_constant_alpha = w(17) & 0xFF;
    g.lfb_constant_depth = w(18) & 0xFFFF;
    g.render = w(19).min(1);
    g.constant = std::array::from_fn(|i| f32::from_bits(w(20 + i)));
    g.clip = std::array::from_fn(|i| w(24 + i));
    for t in 0..2 {
        let at = 28 + 12 * t;
        let s = &mut g.tmu[t];
        s.texture_mode = w(at);
        s.t_lod = w(at + 1);
        s.t_detail = w(at + 2);
        s.tex_base = std::array::from_fn(|i| w(at + 3 + i));
        s.mm_mode = w(at + 7);
        s.small_lod = w(at + 8).min(8);
        s.large_lod = w(at + 9).min(8);
        s.even_odd = w(at + 10) & 3;
        s.ncc_table = w(at + 11) & 1;
    }
    let g = g.clone();
    for (reg, v) in [
        (FBZ_COLOR_PATH, g.fbz_color_path),
        (FOG_MODE, g.fog_mode),
        (ALPHA_MODE, g.alpha_mode),
        (FBZ_MODE, g.fbz_mode),
        (LFB_MODE, g.lfb_mode),
        (CLIP_LEFT_RIGHT, g.clip_left_right),
        (CLIP_LOW_Y_HIGH_Y, g.clip_bottom_top),
        (FOG_COLOR, g.fog_color),
        (ZA_COLOR, g.za_color),
        (CHROMA_KEY, g.chroma_key),
        (COLOR0, g.color0),
        (COLOR1, g.color1),
    ] {
        hw::write(cpu, ALL, reg, v);
    }
    for tmu in 0..tmus(cpu) {
        let t = g.tmu[tmu as usize];
        let chip = tmu_chip(tmu);
        hw::write(cpu, chip, TEXTURE_MODE, t.texture_mode);
        hw::write(cpu, chip, T_LOD, t.t_lod);
        hw::write(cpu, chip, T_DETAIL, t.t_detail);
        for (reg, v) in [
            TEX_BASE_ADDR,
            TEX_BASE_ADDR_1,
            TEX_BASE_ADDR_2,
            TEX_BASE_ADDR_3_8,
        ]
        .into_iter()
        .zip(t.tex_base)
        {
            hw::write(cpu, chip, reg, v);
        }
    }
    if g.delta0 {
        set_delta0(cpu, true);
    }
    render_buffer(cpu, g.render);
}
