//! The Voodoo Graphics' and Voodoo2's registers: their numbers (the dword
//! index in the register space), who may read and write them, the
//! alternate map of the first 64 that fbiInit3 selects, and their names
//! for the debugger. From DOSBox-X's voodoo_def.h (MAME's SST-1 emulation
//! by Aaron Giles), the Voodoo2's from MAME's voodoo_2.cpp register table.

// FBI registers.
pub const STATUS: usize = 0; // 000h
/// Voodoo2: the interrupt control.
pub const INTR_CTRL: usize = 1; // 004h
pub const VERTEX_AX: usize = 0x008 / 4;
pub const VERTEX_AY: usize = 0x00C / 4;
pub const VERTEX_BX: usize = 0x010 / 4;
pub const VERTEX_BY: usize = 0x014 / 4;
pub const VERTEX_CX: usize = 0x018 / 4;
pub const VERTEX_CY: usize = 0x01C / 4;
pub const START_R: usize = 0x020 / 4;
pub const START_G: usize = 0x024 / 4;
pub const START_B: usize = 0x028 / 4;
pub const START_Z: usize = 0x02C / 4;
pub const START_A: usize = 0x030 / 4;
pub const START_S: usize = 0x034 / 4;
pub const START_T: usize = 0x038 / 4;
pub const START_W: usize = 0x03C / 4;
pub const D_R_DX: usize = 0x040 / 4;
pub const D_G_DX: usize = 0x044 / 4;
pub const D_B_DX: usize = 0x048 / 4;
pub const D_Z_DX: usize = 0x04C / 4;
pub const D_A_DX: usize = 0x050 / 4;
pub const D_S_DX: usize = 0x054 / 4;
pub const D_T_DX: usize = 0x058 / 4;
pub const D_W_DX: usize = 0x05C / 4;
pub const D_R_DY: usize = 0x060 / 4;
pub const D_G_DY: usize = 0x064 / 4;
pub const D_B_DY: usize = 0x068 / 4;
pub const D_Z_DY: usize = 0x06C / 4;
pub const D_A_DY: usize = 0x070 / 4;
pub const D_S_DY: usize = 0x074 / 4;
pub const D_T_DY: usize = 0x078 / 4;
pub const D_W_DY: usize = 0x07C / 4;
pub const TRIANGLE_CMD: usize = 0x080 / 4;
pub const FVERTEX_AX: usize = 0x088 / 4;
pub const FVERTEX_AY: usize = 0x08C / 4;
pub const FVERTEX_BX: usize = 0x090 / 4;
pub const FVERTEX_BY: usize = 0x094 / 4;
pub const FVERTEX_CX: usize = 0x098 / 4;
pub const FVERTEX_CY: usize = 0x09C / 4;
pub const FSTART_R: usize = 0x0A0 / 4;
pub const FSTART_G: usize = 0x0A4 / 4;
pub const FSTART_B: usize = 0x0A8 / 4;
pub const FSTART_Z: usize = 0x0AC / 4;
pub const FSTART_A: usize = 0x0B0 / 4;
pub const FSTART_S: usize = 0x0B4 / 4;
pub const FSTART_T: usize = 0x0B8 / 4;
pub const FSTART_W: usize = 0x0BC / 4;
pub const FD_R_DX: usize = 0x0C0 / 4;
pub const FD_G_DX: usize = 0x0C4 / 4;
pub const FD_B_DX: usize = 0x0C8 / 4;
pub const FD_Z_DX: usize = 0x0CC / 4;
pub const FD_A_DX: usize = 0x0D0 / 4;
pub const FD_S_DX: usize = 0x0D4 / 4;
pub const FD_T_DX: usize = 0x0D8 / 4;
pub const FD_W_DX: usize = 0x0DC / 4;
pub const FD_R_DY: usize = 0x0E0 / 4;
pub const FD_G_DY: usize = 0x0E4 / 4;
pub const FD_B_DY: usize = 0x0E8 / 4;
pub const FD_Z_DY: usize = 0x0EC / 4;
pub const FD_A_DY: usize = 0x0F0 / 4;
pub const FD_S_DY: usize = 0x0F4 / 4;
pub const FD_T_DY: usize = 0x0F8 / 4;
pub const FD_W_DY: usize = 0x0FC / 4;
pub const FTRIANGLE_CMD: usize = 0x100 / 4;
pub const FBZ_COLOR_PATH: usize = 0x104 / 4;
pub const FOG_MODE: usize = 0x108 / 4;
pub const ALPHA_MODE: usize = 0x10C / 4;
pub const FBZ_MODE: usize = 0x110 / 4;
pub const LFB_MODE: usize = 0x114 / 4;
pub const CLIP_LEFT_RIGHT: usize = 0x118 / 4;
pub const CLIP_LOW_Y_HIGH_Y: usize = 0x11C / 4;
pub const NOP_CMD: usize = 0x120 / 4;
pub const FASTFILL_CMD: usize = 0x124 / 4;
pub const SWAPBUFFER_CMD: usize = 0x128 / 4;
pub const FOG_COLOR: usize = 0x12C / 4;
pub const ZA_COLOR: usize = 0x130 / 4;
pub const CHROMA_KEY: usize = 0x134 / 4;
/// Voodoo2: the chroma key's range test.
pub const CHROMA_RANGE: usize = 0x138 / 4;
/// Voodoo2: a user interrupt.
pub const USER_INTR_CMD: usize = 0x13C / 4;
pub const STIPPLE: usize = 0x140 / 4;
pub const COLOR0: usize = 0x144 / 4;
pub const COLOR1: usize = 0x148 / 4;
pub const FBI_PIXELS_IN: usize = 0x14C / 4;
pub const FBI_CHROMA_FAIL: usize = 0x150 / 4;
pub const FBI_ZFUNC_FAIL: usize = 0x154 / 4;
pub const FBI_AFUNC_FAIL: usize = 0x158 / 4;
pub const FBI_PIXELS_OUT: usize = 0x15C / 4;
pub const FOG_TABLE: usize = 0x160 / 4;
// Voodoo2: the command FIFO.
pub const CMDFIFO_BASE_ADDR: usize = 0x1E0 / 4;
pub const CMDFIFO_BUMP: usize = 0x1E4 / 4;
pub const CMDFIFO_RD_PTR: usize = 0x1E8 / 4;
pub const CMDFIFO_AMIN: usize = 0x1EC / 4;
pub const CMDFIFO_AMAX: usize = 0x1F0 / 4;
pub const CMDFIFO_DEPTH: usize = 0x1F4 / 4;
pub const CMDFIFO_HOLES: usize = 0x1F8 / 4;
pub const FBI_INIT4: usize = 0x200 / 4;
pub const V_RETRACE: usize = 0x204 / 4;
pub const BACK_PORCH: usize = 0x208 / 4;
pub const VIDEO_DIMENSIONS: usize = 0x20C / 4;
pub const FBI_INIT0: usize = 0x210 / 4;
pub const FBI_INIT1: usize = 0x214 / 4;
pub const FBI_INIT2: usize = 0x218 / 4;
pub const FBI_INIT3: usize = 0x21C / 4;
pub const H_SYNC: usize = 0x220 / 4;
pub const V_SYNC: usize = 0x224 / 4;
pub const CLUT_DATA: usize = 0x228 / 4;
pub const DAC_DATA: usize = 0x22C / 4;
pub const MAX_RGB_DELTA: usize = 0x230 / 4;
// Voodoo2.
pub const H_BORDER: usize = 0x234 / 4;
pub const V_BORDER: usize = 0x238 / 4;
pub const BORDER_COLOR: usize = 0x23C / 4;
pub const HV_RETRACE: usize = 0x240 / 4;
pub const FBI_INIT5: usize = 0x244 / 4;
pub const FBI_INIT6: usize = 0x248 / 4;
pub const FBI_INIT7: usize = 0x24C / 4;
pub const FBI_SWAP_HISTORY: usize = 0x258 / 4;
/// The triangle counter.
pub const FBI_TRIANGLES_OUT: usize = 0x25C / 4;
// The triangle setup engine.
pub const S_SETUP_MODE: usize = 0x260 / 4;
pub const S_VX: usize = 0x264 / 4;
pub const S_VY: usize = 0x268 / 4;
pub const S_ARGB: usize = 0x26C / 4;
pub const S_RED: usize = 0x270 / 4;
pub const S_GREEN: usize = 0x274 / 4;
pub const S_BLUE: usize = 0x278 / 4;
pub const S_ALPHA: usize = 0x27C / 4;
pub const S_VZ: usize = 0x280 / 4;
pub const S_WB: usize = 0x284 / 4;
pub const S_WTMU0: usize = 0x288 / 4;
pub const S_S_W0: usize = 0x28C / 4;
pub const S_T_W0: usize = 0x290 / 4;
pub const S_WTMU1: usize = 0x294 / 4;
pub const S_S_WTMU1: usize = 0x298 / 4;
pub const S_T_WTMU1: usize = 0x29C / 4;
pub const S_DRAW_TRI_CMD: usize = 0x2A0 / 4;
pub const S_BEGIN_TRI_CMD: usize = 0x2A4 / 4;
// The 2D blitter.
pub const BLT_SRC_BASE_ADDR: usize = 0x2C0 / 4;
pub const BLT_DST_BASE_ADDR: usize = 0x2C4 / 4;
pub const BLT_XY_STRIDES: usize = 0x2C8 / 4;
pub const BLT_SRC_CHROMA_RANGE: usize = 0x2CC / 4;
pub const BLT_DST_CHROMA_RANGE: usize = 0x2D0 / 4;
pub const BLT_CLIP_X: usize = 0x2D4 / 4;
pub const BLT_CLIP_Y: usize = 0x2D8 / 4;
pub const BLT_SRC_XY: usize = 0x2E0 / 4;
pub const BLT_DST_XY: usize = 0x2E4 / 4;
pub const BLT_SIZE: usize = 0x2E8 / 4;
pub const BLT_ROP: usize = 0x2EC / 4;
pub const BLT_COLOR: usize = 0x2F0 / 4;
pub const BLT_COMMAND: usize = 0x2F8 / 4;
pub const BLT_DATA: usize = 0x2FC / 4;

// TMU registers.
pub const TEXTURE_MODE: usize = 0x300 / 4;
pub const T_LOD: usize = 0x304 / 4;
pub const T_DETAIL: usize = 0x308 / 4;
pub const TEX_BASE_ADDR: usize = 0x30C / 4;
pub const TEX_BASE_ADDR_1: usize = 0x310 / 4;
pub const TEX_BASE_ADDR_2: usize = 0x314 / 4;
pub const TEX_BASE_ADDR_3_8: usize = 0x318 / 4;
pub const TREX_INIT0: usize = 0x31C / 4;
pub const TREX_INIT1: usize = 0x320 / 4;
pub const NCC_TABLE: usize = 0x324 / 4;

/// Access rights.
pub const READ: u8 = 0x01;
pub const WRITE: u8 = 0x02;
/// Writes go through the FIFO (they wait behind a pending swap on a real
/// card).
pub const FIFO: u8 = 0x08;

const R: u8 = READ;
const W: u8 = WRITE;
const RW: u8 = READ | WRITE;
const RP: u8 = READ;
const WF: u8 = WRITE | FIFO;
const RWF: u8 = READ | WRITE | FIFO;
const WPF: u8 = WRITE | FIFO;
const RWPF: u8 = READ | WRITE | FIFO;

/// Who may read and write each register of a Voodoo Graphics
/// (`voodoo_register_access`); what the Voodoo 2 added is neither.
#[rustfmt::skip]
pub const ACCESS: [u8; 0x100] = {
    let rows: [[u8; 16]; 15] = [
        // 0x000
        [RP, 0, WPF, WPF, WPF, WPF, WPF, WPF, WPF, WPF, WPF, WPF, WPF, WPF, WPF, WPF],
        // 0x040
        [WPF; 16],
        // 0x080
        [WPF, 0, WPF, WPF, WPF, WPF, WPF, WPF, WPF, WPF, WPF, WPF, WPF, WPF, WPF, WPF],
        // 0x0c0
        [WPF; 16],
        // 0x100
        [WPF, RWPF, RWPF, RWPF, RWF, RWF, RWF, RWF, WF, WF, WF, WF, WF, WF, 0, 0],
        // 0x140
        [RWF, RWF, RWF, R, R, R, R, R, WF, WF, WF, WF, WF, WF, WF, WF],
        // 0x180
        [WF; 16],
        // 0x1c0
        [WF, WF, WF, WF, WF, WF, WF, WF, 0, 0, 0, 0, 0, 0, 0, 0],
        // 0x200
        [RW, R, RW, RW, RW, RW, RW, RW, W, W, W, W, W, 0, 0, 0],
        // 0x240, 0x280, 0x2c0
        [0; 16],
        [0; 16],
        [0; 16],
        // 0x300
        [WPF, WPF, WPF, WPF, WPF, WPF, WPF, WF, WF, WF, WF, WF, WF, WF, WF, WF],
        // 0x340
        [WF; 16],
        // 0x380
        [WF, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    ];
    let mut table = [0u8; 0x100];
    let mut i = 0;
    while i < 15 * 16 {
        table[i] = rows[i / 16][i % 16];
        i += 1;
    }
    table
};

/// Who may read and write each register of a Voodoo2 (MAME's
/// `voodoo_2_device::s_register_table`): the Voodoo Graphics' and what it
/// added. The command FIFO's registers, intrCtrl and the init and video
/// registers don't go through the FIFO, so they still work while the
/// command FIFO takes the others.
pub const ACCESS_V2: [u8; 0x100] = {
    let mut table = ACCESS;
    table[INTR_CTRL] = RW;
    table[CHROMA_RANGE] = WF;
    table[USER_INTR_CMD] = WF;
    let mut i = CMDFIFO_BASE_ADDR;
    while i <= CMDFIFO_HOLES {
        table[i] = RW;
        i += 1;
    }
    table[MAX_RGB_DELTA] = W;
    table[H_BORDER] = W;
    table[V_BORDER] = W;
    table[BORDER_COLOR] = W;
    table[HV_RETRACE] = R;
    table[FBI_INIT5] = RW;
    table[FBI_INIT6] = RW;
    table[FBI_INIT7] = RW;
    table[FBI_SWAP_HISTORY] = R;
    table[FBI_TRIANGLES_OUT] = R;
    let mut i = S_SETUP_MODE;
    while i <= S_BEGIN_TRI_CMD {
        table[i] = WF;
        i += 1;
    }
    let mut i = BLT_SRC_BASE_ADDR;
    while i <= BLT_COMMAND {
        if i != 0x2DC / 4 && i != 0x2F4 / 4 {
            table[i] = RWF;
        }
        i += 1;
    }
    table[BLT_DATA] = WF;
    table
};

/// With fbiInit3 bit 0, the first 64 registers in the upper half of the
/// register space (address bit 21) are these, in the order Glide's
/// triangle setup writes them (`register_alias_map`).
#[rustfmt::skip]
pub const ALIAS: [u8; 0x40] = [
    STATUS as u8, 1, VERTEX_AX as u8, VERTEX_AY as u8,
    VERTEX_BX as u8, VERTEX_BY as u8, VERTEX_CX as u8, VERTEX_CY as u8,
    START_R as u8, D_R_DX as u8, D_R_DY as u8, START_G as u8,
    D_G_DX as u8, D_G_DY as u8, START_B as u8, D_B_DX as u8,
    D_B_DY as u8, START_Z as u8, D_Z_DX as u8, D_Z_DY as u8,
    START_A as u8, D_A_DX as u8, D_A_DY as u8, START_S as u8,
    D_S_DX as u8, D_S_DY as u8, START_T as u8, D_T_DX as u8,
    D_T_DY as u8, START_W as u8, D_W_DX as u8, D_W_DY as u8,

    TRIANGLE_CMD as u8, 0x084 / 4, FVERTEX_AX as u8, FVERTEX_AY as u8,
    FVERTEX_BX as u8, FVERTEX_BY as u8, FVERTEX_CX as u8, FVERTEX_CY as u8,
    FSTART_R as u8, FD_R_DX as u8, FD_R_DY as u8, FSTART_G as u8,
    FD_G_DX as u8, FD_G_DY as u8, FSTART_B as u8, FD_B_DX as u8,
    FD_B_DY as u8, FSTART_Z as u8, FD_Z_DX as u8, FD_Z_DY as u8,
    FSTART_A as u8, FD_A_DX as u8, FD_A_DY as u8, FSTART_S as u8,
    FD_S_DX as u8, FD_S_DY as u8, FSTART_T as u8, FD_T_DX as u8,
    FD_T_DY as u8, FSTART_W as u8, FD_W_DX as u8, FD_W_DY as u8,
];

/// The name of register `n`, for the debugger.
pub fn name(n: usize) -> &'static str {
    const NAMES: [&str; 0x80] = [
        "status", "intrCtrl", "vertexAx", "vertexAy", "vertexBx", "vertexBy", "vertexCx", "vertexCy",
        "startR", "startG", "startB", "startZ", "startA", "startS", "startT", "startW",
        "dRdX", "dGdX", "dBdX", "dZdX", "dAdX", "dSdX", "dTdX", "dWdX",
        "dRdY", "dGdY", "dBdY", "dZdY", "dAdY", "dSdY", "dTdY", "dWdY",
        "triangleCMD", "reserved084", "fvertexAx", "fvertexAy", "fvertexBx", "fvertexBy", "fvertexCx", "fvertexCy",
        "fstartR", "fstartG", "fstartB", "fstartZ", "fstartA", "fstartS", "fstartT", "fstartW",
        "fdRdX", "fdGdX", "fdBdX", "fdZdX", "fdAdX", "fdSdX", "fdTdX", "fdWdX",
        "fdRdY", "fdGdY", "fdBdY", "fdZdY", "fdAdY", "fdSdY", "fdTdY", "fdWdY",
        "ftriangleCMD", "fbzColorPath", "fogMode", "alphaMode", "fbzMode", "lfbMode", "clipLeftRight", "clipLowYHighY",
        "nopCMD", "fastfillCMD", "swapbufferCMD", "fogColor", "zaColor", "chromaKey", "chromaRange", "userIntrCMD",
        "stipple", "color0", "color1", "fbiPixelsIn", "fbiChromaFail", "fbiZfuncFail", "fbiAfuncFail", "fbiPixelsOut",
        "fogTable", "fogTable", "fogTable", "fogTable", "fogTable", "fogTable", "fogTable", "fogTable",
        "fogTable", "fogTable", "fogTable", "fogTable", "fogTable", "fogTable", "fogTable", "fogTable",
        "fogTable", "fogTable", "fogTable", "fogTable", "fogTable", "fogTable", "fogTable", "fogTable",
        "fogTable", "fogTable", "fogTable", "fogTable", "fogTable", "fogTable", "fogTable", "fogTable",
        "cmdFifoBaseAddr", "cmdFifoBump", "cmdFifoRdPtr", "cmdFifoAMin", "cmdFifoAMax", "cmdFifoDepth", "cmdFifoHoles", "reserved1fc",
    ];
    const INIT: [&str; 16] = [
        "fbiInit4", "vRetrace", "backPorch", "videoDimensions", "fbiInit0", "fbiInit1", "fbiInit2", "fbiInit3",
        "hSync", "vSync", "clutData", "dacData", "maxRgbDelta", "hBorder", "vBorder", "borderColor",
    ];
    const V2: [&str; 32] = [
        "hvRetrace", "fbiInit5", "fbiInit6", "fbiInit7", "reserved250", "reserved254", "fbiSwapHistory", "fbiTrianglesOut",
        "sSetupMode", "sVx", "sVy", "sARGB", "sRed", "sGreen", "sBlue", "sAlpha",
        "sVz", "sWb", "sWtmu0", "sS_W0", "sT_W0", "sWtmu1", "sS_Wtmu1", "sT_Wtmu1",
        "sDrawTriCMD", "sBeginTriCMD", "reserved2a8", "reserved2ac", "reserved2b0", "reserved2b4", "reserved2b8", "reserved2bc",
    ];
    const BLT: [&str; 16] = [
        "bltSrcBaseAddr", "bltDstBaseAddr", "bltXYStrides", "bltSrcChromaRange", "bltDstChromaRange", "bltClipX", "bltClipY",
        "reserved2dc", "bltSrcXY", "bltDstXY", "bltSize", "bltRop", "bltColor", "reserved2f4", "bltCommand", "bltData",
    ];
    const TMU: [&str; 9] = [
        "textureMode", "tLOD", "tDetail", "texBaseAddr", "texBaseAddr_1", "texBaseAddr_2", "texBaseAddr_3_8",
        "trexInit0", "trexInit1",
    ];
    match n {
        0..0x80 => NAMES[n],
        0x80..0x90 => INIT[n - 0x80],
        0x90..0xB0 => V2[n - 0x90],
        0xB0..0xC0 => BLT[n - 0xB0],
        0xC0..0xC9 => TMU[n - 0xC0],
        0xC9..0xE1 => "nccTable",
        _ => "reserved",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_rights_match_the_voodoo_graphics() {
        assert_eq!(ACCESS[STATUS], READ);
        assert_eq!(ACCESS[FBZ_COLOR_PATH], READ | WRITE | FIFO);
        assert_eq!(ACCESS[FBI_PIXELS_OUT], READ);
        assert_eq!(ACCESS[FBI_INIT2], READ | WRITE);
        assert_eq!(ACCESS[V_RETRACE], READ);
        assert_eq!(ACCESS[DAC_DATA], WRITE);
        assert_eq!(ACCESS[CHROMA_RANGE], 0, "Voodoo 2 only");
        assert_eq!(ACCESS[0x244 / 4], 0, "fbiInit5 is Voodoo 2 only");
        assert_eq!(ACCESS[NCC_TABLE + 23], WRITE | FIFO);
        assert_eq!(ACCESS[NCC_TABLE + 24], 0);
        assert_eq!(ALIAS[9], D_R_DX as u8);
        assert_eq!(name(FBI_INIT3), "fbiInit3");
        assert_eq!(name(T_LOD), "tLOD");
    }

    #[test]
    fn access_rights_of_the_voodoo2() {
        assert_eq!(ACCESS_V2[STATUS], READ);
        assert_eq!(ACCESS_V2[INTR_CTRL], READ | WRITE, "outside the FIFO");
        assert_eq!(ACCESS_V2[CHROMA_RANGE], WRITE | FIFO);
        assert_eq!(ACCESS_V2[CMDFIFO_RD_PTR], READ | WRITE);
        assert_eq!(ACCESS_V2[FBI_INIT7], READ | WRITE);
        assert_eq!(ACCESS_V2[S_DRAW_TRI_CMD], WRITE | FIFO);
        assert_eq!(ACCESS_V2[BLT_COMMAND], READ | WRITE | FIFO);
        assert_eq!(ACCESS_V2[BLT_DATA], WRITE | FIFO);
        assert_eq!(ACCESS_V2[0x2F4 / 4], 0);
        assert_eq!(ACCESS_V2[FBZ_COLOR_PATH], ACCESS[FBZ_COLOR_PATH]);
        assert_eq!(name(FBI_INIT7), "fbiInit7");
        assert_eq!(name(BLT_DATA), "bltData");
        assert_eq!(name(CMDFIFO_HOLES), "cmdFifoHoles");
    }
}
