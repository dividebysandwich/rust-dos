//! The PCX2's registers: dwords from the start of BAR0, as Imagination's
//! driver source names them (`Source/pcx/hwregs.h` of the PowerVR Series1
//! release, MIT licensed), and what the driver writes to them.
//!
//! The source lacks the layer that started renders; Tomb Raider's PowerVR
//! version (TOMBPCX2.EXE) shows it:
//!
//! * At start: TMEM_SETUP, TMEM_REFRESH, a SOFTRESET pulse, the fog and
//!   divider tables, INTMASK 0 (it polls), PAGE_CTRL 300h, ISP_BASE
//!   80000h, PREC_BASE 0, IEEEFP 7, ARBMODE Dh, and 128 TLB entries.
//! * The TLB maps the 2 MB parameter space in 16 KB pages: entry n holds
//!   the physical address of page n in 4 KB units (bits 12 up).
//! * Each frame: BILINEAR, OBJECT_OFFSET (the object list's byte address
//!   in the parameter space, bit 0 set; 100000h and 180000h by turns),
//!   FOGAMOUNT, FOGCOL, CAMERA, XCLIP, PACKMODE, SOFADDR (the physical
//!   byte address of the frame buffer: the VBE linear frame buffer and
//!   its second page), LSTRIDE (bytes a line), a SOFTRESET pulse, then a
//!   write of 0 to STARTRENDER; then it reads INTSTATUS until bit 1 is
//!   set. It never writes INTSTATUS: the pulse clears it.
//! * Object pointers' plane addresses are dword addresses in the
//!   parameter space (the second frame's planes are 80000h bytes up,
//!   ISP_BASE).

pub const ID: usize = 0x000;
pub const REVISION: usize = 0x001;
pub const SOFTRESET: usize = 0x002;
/// Bit 1: end of render.
pub const INTSTATUS: usize = 0x003;
pub const INTMASK: usize = 0x004;
pub const STARTRENDER: usize = 0x005;
pub const FOGAMOUNT: usize = 0x006;
pub const OBJECT_OFFSET: usize = 0x007;
pub const PAGE_CTRL: usize = 0x008;
pub const ISP_BASE: usize = 0x00A;
pub const PREC_BASE: usize = 0x00B;
pub const TMEM_SETUP: usize = 0x00C;
pub const TMEM_REFRESH: usize = 0x00D;
pub const FOGCOL: usize = 0x00E;
pub const CAMERA: usize = 0x00F;
pub const PACKMODE: usize = 0x010;
pub const ARBMODE: usize = 0x011;
pub const LSTRIDE: usize = 0x012;
pub const SOFADDR: usize = 0x013;
pub const XCLIP: usize = 0x014;
pub const ABORTADDR: usize = 0x015;
pub const GPPORT: usize = 0x016;
pub const IEEEFP: usize = 0x018;
pub const BILINEAR: usize = 0x019;
pub const PCI21COMP: usize = 0x01B;
pub const CLKSELECT: usize = 0x01C;
pub const FASTFOG: usize = 0x01D;
pub const POWERDOWN: usize = 0x01E;
pub const MEMTEST_DATA: usize = 0x07D;
pub const MEMTEST_MODE: usize = 0x07E;
pub const MEMTEST_RES: usize = 0x07F;
/// 128 entries.
pub const FOG_TABLE: usize = 0x080;
/// The pages of the parameter space in host memory.
pub const TLB: usize = 0x100;
/// 512 entries.
pub const DIVIDER_TABLE: usize = 0x200;

/// The registers: 1024 dwords, the end of the divider table.
pub const COUNT: usize = 0x400;

/// End of render, in INTSTATUS and INTMASK.
pub const END_OF_RENDER: u32 = 1 << 1;

/// A register's name, for traces and the debugger.
pub fn name(reg: usize) -> String {
    let fixed = match reg {
        ID => "ID",
        REVISION => "REVISION",
        SOFTRESET => "SOFTRESET",
        INTSTATUS => "INTSTATUS",
        INTMASK => "INTMASK",
        STARTRENDER => "STARTRENDER",
        FOGAMOUNT => "FOGAMOUNT",
        OBJECT_OFFSET => "OBJECT_OFFSET",
        PAGE_CTRL => "PAGE_CTRL",
        ISP_BASE => "ISP_BASE",
        PREC_BASE => "PREC_BASE",
        TMEM_SETUP => "TMEM_SETUP",
        TMEM_REFRESH => "TMEM_REFRESH",
        FOGCOL => "FOGCOL",
        CAMERA => "CAMERA",
        PACKMODE => "PACKMODE",
        ARBMODE => "ARBMODE",
        LSTRIDE => "LSTRIDE",
        SOFADDR => "SOFADDR",
        XCLIP => "XCLIP",
        ABORTADDR => "ABORTADDR",
        GPPORT => "GPPORT",
        IEEEFP => "IEEEFP",
        BILINEAR => "BILINEAR",
        PCI21COMP => "PCI21COMP",
        CLKSELECT => "CLKSELECT",
        FASTFOG => "FASTFOG",
        POWERDOWN => "POWERDOWN",
        MEMTEST_DATA => "MEMTEST_DATA",
        MEMTEST_MODE => "MEMTEST_MODE",
        MEMTEST_RES => "MEMTEST_RES",
        FOG_TABLE..TLB => return format!("FOG_TABLE[{}]", reg - FOG_TABLE),
        TLB..DIVIDER_TABLE => return format!("TLB[{}]", reg - TLB),
        DIVIDER_TABLE..COUNT => return format!("DIVIDER_TABLE[{}]", reg - DIVIDER_TABLE),
        _ => return format!("reg{:03X}", reg),
    };
    fixed.to_string()
}
