//! The mach64's registers: dword indexes of block 0 (the CRTC, the
//! card's setup and the draw engine) and of block 1 (the overlay and the
//! scaler), 100h up, as ATI's register reference for the 264VT and the
//! 3D RAGE (RRG-G02700) numbers them.

pub const CRTC_H_TOTAL_DISP: usize = 0x00;
pub const CRTC_H_SYNC_STRT_WID: usize = 0x01;
pub const CRTC_V_TOTAL_DISP: usize = 0x02;
pub const CRTC_V_SYNC_STRT_WID: usize = 0x03;
pub const CRTC_VLINE_CRNT_VLINE: usize = 0x04;
pub const CRTC_OFF_PITCH: usize = 0x05;
pub const CRTC_INT_CNTL: usize = 0x06;
pub const CRTC_GEN_CNTL: usize = 0x07;
pub const OVR_CLR: usize = 0x10;
pub const OVR_WID_LEFT_RIGHT: usize = 0x11;
pub const OVR_WID_TOP_BOTTOM: usize = 0x12;
pub const CUR_CLR0: usize = 0x18;
pub const CUR_CLR1: usize = 0x19;
pub const CUR_OFFSET: usize = 0x1A;
pub const CUR_HORZ_VERT_POSN: usize = 0x1B;
pub const CUR_HORZ_VERT_OFF: usize = 0x1C;
pub const GP_IO: usize = 0x1E;
pub const GP_IO_CNTL: usize = 0x1F;
pub const SCRATCH_REG0: usize = 0x20;
pub const SCRATCH_REG1: usize = 0x21;
pub const CLOCK_CNTL: usize = 0x24;
pub const BUS_CNTL: usize = 0x28;
pub const MEM_CNTL: usize = 0x2C;
pub const MEM_VGA_WP_SEL: usize = 0x2D;
pub const MEM_VGA_RP_SEL: usize = 0x2E;
pub const DAC_REGS: usize = 0x30;
pub const DAC_CNTL: usize = 0x31;
pub const GEN_TEST_CNTL: usize = 0x34;
pub const CONFIG_CNTL: usize = 0x37;
pub const CONFIG_CHIP_ID: usize = 0x38;
pub const CONFIG_STAT0: usize = 0x39;
pub const CRC_SIG: usize = 0x3A;

pub const DST_OFF_PITCH: usize = 0x40;
pub const DST_X: usize = 0x41;
pub const DST_Y: usize = 0x42;
pub const DST_Y_X: usize = 0x43;
pub const DST_WIDTH: usize = 0x44;
pub const DST_HEIGHT: usize = 0x45;
pub const DST_HEIGHT_WIDTH: usize = 0x46;
pub const DST_X_WIDTH: usize = 0x47;
pub const DST_BRES_LNTH: usize = 0x48;
pub const DST_BRES_ERR: usize = 0x49;
pub const DST_BRES_INC: usize = 0x4A;
pub const DST_BRES_DEC: usize = 0x4B;
pub const DST_CNTL: usize = 0x4C;
pub const DST_Y_X_ALIAS: usize = 0x4D;
pub const TRAIL_BRES_ERR: usize = 0x4E;
pub const TRAIL_BRES_INC: usize = 0x4F;
pub const TRAIL_BRES_DEC: usize = 0x50;
pub const LEAD_BRES_LNTH: usize = 0x51;
pub const Z_OFF_PITCH: usize = 0x52;
pub const Z_CNTL: usize = 0x53;

pub const SRC_OFF_PITCH: usize = 0x60;
pub const SRC_X: usize = 0x61;
pub const SRC_Y: usize = 0x62;
pub const SRC_Y_X: usize = 0x63;
pub const SRC_WIDTH1: usize = 0x64;
pub const SRC_HEIGHT1: usize = 0x65;
pub const SRC_HEIGHT1_WIDTH1: usize = 0x66;
pub const SRC_X_START: usize = 0x67;
pub const SRC_Y_START: usize = 0x68;
pub const SRC_Y_X_START: usize = 0x69;
pub const SRC_WIDTH2: usize = 0x6A;
pub const SRC_HEIGHT2: usize = 0x6B;
pub const SRC_HEIGHT2_WIDTH2: usize = 0x6C;
pub const SRC_CNTL: usize = 0x6D;

/// The texture's maps, TEX_0_OFF to TEX_10_OFF; the scaler's source is
/// TEX_0_OFF, its width TEX_7_OFF and its height TEX_8_OFF.
pub const TEX_0_OFF: usize = 0x70;
pub const SCALE_WIDTH: usize = 0x77;
pub const SCALE_HEIGHT: usize = 0x78;
pub const SCALE_Y_PITCH: usize = 0x7B;
pub const SCALE_X_INC: usize = 0x7C;
pub const SCALE_Y_INC: usize = 0x7D;
pub const SCALE_VACC: usize = 0x7E;
pub const SCALE_3D_CNTL: usize = 0x7F;

pub const HOST_DATA0: usize = 0x80;
pub const HOST_DATA15: usize = 0x8F;
pub const HOST_CNTL: usize = 0x90;
pub const PAT_REG0: usize = 0xA0;
pub const PAT_REG1: usize = 0xA1;
pub const PAT_CNTL: usize = 0xA2;
pub const SC_LEFT: usize = 0xA8;
pub const SC_RIGHT: usize = 0xA9;
pub const SC_LEFT_RIGHT: usize = 0xAA;
pub const SC_TOP: usize = 0xAB;
pub const SC_BOTTOM: usize = 0xAC;
pub const SC_TOP_BOTTOM: usize = 0xAD;

pub const DP_BKGD_CLR: usize = 0xB0;
/// Also the fog colour, DP_FOG_CLR.
pub const DP_FRGD_CLR: usize = 0xB1;
pub const DP_WRITE_MSK: usize = 0xB2;
pub const DP_CHAIN_MSK: usize = 0xB3;
pub const DP_PIX_WIDTH: usize = 0xB4;
pub const DP_MIX: usize = 0xB5;
pub const DP_SRC: usize = 0xB6;
/// The 3D RAGE II's.
pub const DST_X_Y: usize = 0xBA;
pub const DST_WIDTH_HEIGHT: usize = 0xBB;
pub const DP_SET_GUI_ENGINE: usize = 0xBF;
pub const CLR_CMP_CLR: usize = 0xC0;
pub const CLR_CMP_MSK: usize = 0xC1;
pub const CLR_CMP_CNTL: usize = 0xC2;
pub const FIFO_STAT: usize = 0xC4;
pub const CONTEXT_MASK: usize = 0xC8;
pub const CONTEXT_LOAD_CNTL: usize = 0xCB;
pub const GUI_TRAJ_CNTL: usize = 0xCC;
pub const GUI_STAT: usize = 0xCE;

pub const S_X_INC2: usize = 0xD0;
pub const S_Y_INC2: usize = 0xD1;
pub const S_XY_INC2: usize = 0xD2;
pub const S_XINC_START: usize = 0xD3;
pub const S_Y_INC: usize = 0xD4;
pub const S_START: usize = 0xD5;
pub const T_X_INC2: usize = 0xD6;
pub const T_Y_INC2: usize = 0xD7;
pub const T_XY_INC2: usize = 0xD8;
pub const T_XINC_START: usize = 0xD9;
pub const T_Y_INC: usize = 0xDA;
pub const T_START: usize = 0xDB;
pub const TEX_SIZE_PITCH: usize = 0xDC;
/// The 3D RAGE II's texture palette.
pub const TEX_PALETTE: usize = 0xDF;

pub const RED_X_INC: usize = 0xF0;
pub const RED_Y_INC: usize = 0xF1;
pub const RED_START: usize = 0xF2;
pub const GREEN_X_INC: usize = 0xF3;
pub const GREEN_Y_INC: usize = 0xF4;
pub const GREEN_START: usize = 0xF5;
pub const BLUE_X_INC: usize = 0xF6;
pub const BLUE_Y_INC: usize = 0xF7;
pub const BLUE_START: usize = 0xF8;
pub const Z_X_INC: usize = 0xF9;
pub const Z_Y_INC: usize = 0xFA;
pub const Z_START: usize = 0xFB;
pub const ALPHA_X_INC: usize = 0xFC;
pub const ALPHA_Y_INC: usize = 0xFD;
pub const ALPHA_START: usize = 0xFE;

/// Block 1.
pub const OVERLAY_Y_X_START: usize = 0x100;
pub const OVERLAY_Y_X_END: usize = 0x101;
pub const OVERLAY_VIDEO_KEY_CLR: usize = 0x102;
pub const OVERLAY_VIDEO_KEY_MSK: usize = 0x103;
pub const OVERLAY_GRAPHICS_KEY_CLR: usize = 0x104;
pub const OVERLAY_GRAPHICS_KEY_MSK: usize = 0x105;
pub const OVERLAY_KEY_CNTL: usize = 0x106;
pub const OVERLAY_SCALE_INC: usize = 0x108;
pub const OVERLAY_SCALE_CNTL: usize = 0x109;
pub const SCALER_HEIGHT_WIDTH: usize = 0x10A;
/// The 3D RAGE II's scaler buffer and pitch.
pub const SCALER_BUF0_OFFSET: usize = 0x10D;
pub const SCALER_BUF_PITCH: usize = 0x10F;
pub const VIDEO_FORMAT: usize = 0x112;
pub const EXT_CRTC_GEN_CNTL_R: usize = 0x117;
pub const VMC_STATUS: usize = 0x119;
pub const BUF0_OFFSET: usize = 0x120;
pub const BUF0_PITCH: usize = 0x123;
pub const BUF1_OFFSET: usize = 0x126;
pub const BUF1_PITCH: usize = 0x129;

/// All of them: block 0, then block 1.
pub const COUNT: usize = 0x140;

/// The bits of each register that hold what is written, as it reads
/// back; the others read 0. Registers not listed keep all 32.
pub fn write_mask(reg: usize) -> u32 {
    match reg {
        CRTC_H_TOTAL_DISP => 0x00FF_01FF,
        CRTC_H_SYNC_STRT_WID => 0x003F_17FF,
        CRTC_V_TOTAL_DISP => 0x07FF_07FF,
        CRTC_V_SYNC_STRT_WID => 0x003F_07FF,
        CRTC_VLINE_CRNT_VLINE => 0x0000_07FF,
        CRTC_OFF_PITCH => 0xFFCF_FFFF,
        CRTC_GEN_CNTL => 0xFFFF_CFDF,
        OVR_WID_LEFT_RIGHT => 0x000F_000F,
        OVR_WID_TOP_BOTTOM => 0x00FF_00FF,
        CUR_OFFSET => 0x000F_FFFF,
        CUR_HORZ_VERT_POSN => 0x07FF_07FF,
        CUR_HORZ_VERT_OFF => 0x003F_003F,
        GP_IO_CNTL => 0x8000_000F,
        CLOCK_CNTL => 0x0000_3E03,
        BUS_CNTL => 0xF85F_DFFF,
        MEM_CNTL => 0x1F7F_FFFF,
        MEM_VGA_WP_SEL | MEM_VGA_RP_SEL => 0x00FF_00FF,
        DAC_CNTL => 0xBF00_E10C,
        GEN_TEST_CNTL => 0x3FFF_31B7,
        CONFIG_CNTL => 0x0008_0004,
        CONFIG_STAT0 => 0x0000_FFFF,
        CONFIG_CHIP_ID | CRC_SIG | FIFO_STAT | GUI_STAT => 0,
        0x08..=0x0F | 0x13..=0x17 | 0x1D | 0x22 | 0x23 | 0x25..=0x27 => 0,
        0x29..=0x2B | 0x2F | 0x32 | 0x33 | 0x35 | 0x36 | 0x3B..=0x3F => 0,
        DST_OFF_PITCH | SRC_OFF_PITCH | Z_OFF_PITCH => 0xFFCF_FFFF,
        DST_X => 0x1FFF,
        DST_Y | DST_HEIGHT => 0x7FFF,
        DST_WIDTH => 0xFFFF,
        DST_BRES_LNTH | LEAD_BRES_LNTH => 0xFFFF,
        DST_BRES_ERR | DST_BRES_INC | DST_BRES_DEC => 0x3_FFFF,
        TRAIL_BRES_ERR | TRAIL_BRES_INC | TRAIL_BRES_DEC => 0x3_FFFF,
        DST_CNTL => 0x0002_FFFF,
        Z_CNTL => 0x173,
        SRC_X | SRC_WIDTH1 | SRC_X_START | SRC_WIDTH2 => 0x1FFF,
        SRC_Y | SRC_HEIGHT1 | SRC_Y_START | SRC_HEIGHT2 => 0x7FFF,
        SRC_CNTL => 0x609F,
        HOST_CNTL => 0x3,
        PAT_CNTL => 0x7,
        SC_LEFT | SC_RIGHT => 0x1FFF,
        SC_TOP | SC_BOTTOM => 0x7FFF,
        DP_MIX => 0x001F_001F,
        DP_SRC => 0x0003_0707,
        CLR_CMP_CNTL => 0x0300_0007,
        TEX_SIZE_PITCH => 0xFFF,
        OVERLAY_Y_X_START | OVERLAY_Y_X_END => 0x87FF_07FF,
        OVERLAY_VIDEO_KEY_CLR..=OVERLAY_GRAPHICS_KEY_MSK => 0x00FF_FFFF,
        OVERLAY_KEY_CNTL => 0x8000_0F77,
        OVERLAY_SCALE_CNTL => 0xE000_007F,
        SCALER_HEIGHT_WIDTH => 0x03FF_03FF,
        SCALER_BUF0_OFFSET | 0x10E => 0x003F_FFFF,
        SCALER_BUF_PITCH => 0xFFF,
        VIDEO_FORMAT => 0xF00F_000F,
        BUF0_OFFSET | BUF1_OFFSET | 0x12B | 0x12C => 0x003F_FFF8,
        BUF0_PITCH | BUF1_PITCH => 0xFFE,
        EXT_CRTC_GEN_CNTL_R | VMC_STATUS => 0,
        _ => 0xFFFF_FFFF,
    }
}

/// The register's name, for the trace.
pub fn name(reg: usize) -> String {
    let known = match reg {
        CRTC_H_TOTAL_DISP => "CRTC_H_TOTAL_DISP",
        CRTC_H_SYNC_STRT_WID => "CRTC_H_SYNC_STRT_WID",
        CRTC_V_TOTAL_DISP => "CRTC_V_TOTAL_DISP",
        CRTC_V_SYNC_STRT_WID => "CRTC_V_SYNC_STRT_WID",
        CRTC_VLINE_CRNT_VLINE => "CRTC_VLINE_CRNT_VLINE",
        CRTC_OFF_PITCH => "CRTC_OFF_PITCH",
        CRTC_INT_CNTL => "CRTC_INT_CNTL",
        CRTC_GEN_CNTL => "CRTC_GEN_CNTL",
        CUR_CLR0 => "CUR_CLR0",
        CUR_CLR1 => "CUR_CLR1",
        CUR_OFFSET => "CUR_OFFSET",
        CUR_HORZ_VERT_POSN => "CUR_HORZ_VERT_POSN",
        CUR_HORZ_VERT_OFF => "CUR_HORZ_VERT_OFF",
        SCRATCH_REG0 => "SCRATCH_REG0",
        SCRATCH_REG1 => "SCRATCH_REG1",
        CLOCK_CNTL => "CLOCK_CNTL",
        BUS_CNTL => "BUS_CNTL",
        MEM_CNTL => "MEM_CNTL",
        MEM_VGA_WP_SEL => "MEM_VGA_WP_SEL",
        MEM_VGA_RP_SEL => "MEM_VGA_RP_SEL",
        DAC_REGS => "DAC_REGS",
        DAC_CNTL => "DAC_CNTL",
        GEN_TEST_CNTL => "GEN_TEST_CNTL",
        CONFIG_CNTL => "CONFIG_CNTL",
        CONFIG_CHIP_ID => "CONFIG_CHIP_ID",
        CONFIG_STAT0 => "CONFIG_STAT0",
        DST_OFF_PITCH => "DST_OFF_PITCH",
        DST_X => "DST_X",
        DST_Y => "DST_Y",
        DST_Y_X => "DST_Y_X",
        DST_WIDTH => "DST_WIDTH",
        DST_HEIGHT => "DST_HEIGHT",
        DST_HEIGHT_WIDTH => "DST_HEIGHT_WIDTH",
        DST_X_WIDTH => "DST_X_WIDTH",
        DST_BRES_LNTH => "DST_BRES_LNTH",
        DST_BRES_ERR => "DST_BRES_ERR",
        DST_BRES_INC => "DST_BRES_INC",
        DST_BRES_DEC => "DST_BRES_DEC",
        DST_CNTL => "DST_CNTL",
        TRAIL_BRES_ERR => "TRAIL_BRES_ERR",
        TRAIL_BRES_INC => "TRAIL_BRES_INC",
        TRAIL_BRES_DEC => "TRAIL_BRES_DEC",
        LEAD_BRES_LNTH => "LEAD_BRES_LNTH",
        Z_OFF_PITCH => "Z_OFF_PITCH",
        Z_CNTL => "Z_CNTL",
        SRC_OFF_PITCH => "SRC_OFF_PITCH",
        SRC_Y_X => "SRC_Y_X",
        SRC_HEIGHT1_WIDTH1 => "SRC_HEIGHT1_WIDTH1",
        SRC_Y_X_START => "SRC_Y_X_START",
        SRC_HEIGHT2_WIDTH2 => "SRC_HEIGHT2_WIDTH2",
        SRC_CNTL => "SRC_CNTL",
        SCALE_3D_CNTL => "SCALE_3D_CNTL",
        HOST_CNTL => "HOST_CNTL",
        PAT_REG0 => "PAT_REG0",
        PAT_REG1 => "PAT_REG1",
        PAT_CNTL => "PAT_CNTL",
        SC_LEFT_RIGHT => "SC_LEFT_RIGHT",
        SC_TOP_BOTTOM => "SC_TOP_BOTTOM",
        DP_BKGD_CLR => "DP_BKGD_CLR",
        DP_FRGD_CLR => "DP_FRGD_CLR",
        DP_WRITE_MSK => "DP_WRITE_MSK",
        DP_PIX_WIDTH => "DP_PIX_WIDTH",
        DP_MIX => "DP_MIX",
        DP_SRC => "DP_SRC",
        CLR_CMP_CLR => "CLR_CMP_CLR",
        CLR_CMP_MSK => "CLR_CMP_MSK",
        CLR_CMP_CNTL => "CLR_CMP_CNTL",
        FIFO_STAT => "FIFO_STAT",
        CONTEXT_LOAD_CNTL => "CONTEXT_LOAD_CNTL",
        GUI_TRAJ_CNTL => "GUI_TRAJ_CNTL",
        GUI_STAT => "GUI_STAT",
        TEX_SIZE_PITCH => "TEX_SIZE_PITCH",
        OVERLAY_SCALE_CNTL => "OVERLAY_SCALE_CNTL",
        OVERLAY_KEY_CNTL => "OVERLAY_KEY_CNTL",
        VIDEO_FORMAT => "VIDEO_FORMAT",
        HOST_DATA0..=HOST_DATA15 => return format!("HOST_DATA{}", reg - HOST_DATA0),
        _ => "",
    };
    if known.is_empty() {
        if reg >= 0x100 {
            format!("1_{:02X}", reg - 0x100)
        } else {
            format!("0_{:02X}", reg)
        }
    } else {
        known.to_string()
    }
}
