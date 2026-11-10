//! The card's registers in BAR 0, the bits of the ones with bits, and the
//! sizes of its three windows (chapters 2, 3 and 5 of
//! `docs/toadforce-spec.md`).

/// BAR 0, the registers: 64 KB.
pub const REGISTERS_SIZE: u32 = 0x1_0000;
/// BAR 2, the command ring: 1 MB.
pub const RING_SIZE: u32 = 0x10_0000;
/// The sizes card memory (BAR 1) can have.
pub const MEMORY_SIZES: [u32; 3] = [32 << 20, 64 << 20, 128 << 20];
/// The size it has when nothing says otherwise.
pub const DEFAULT_MEMORY: u32 = 64 << 20;

pub const ID: u32 = 0x000;
pub const LEVEL_MAX: u32 = 0x004;
pub const LEVEL: u32 = 0x008;
pub const STATUS: u32 = 0x00C;
pub const IRQ_STATUS: u32 = 0x010;
pub const IRQ_ENABLE: u32 = 0x014;
pub const RING_PUT: u32 = 0x018;
pub const RING_GET: u32 = 0x01C;
pub const ERROR: u32 = 0x020;
pub const ERROR_AT: u32 = 0x024;
pub const MEMORY: u32 = 0x028;
pub const RESET: u32 = 0x02C;
pub const MODE_ENABLE: u32 = 0x040;
pub const MODE_WIDTH: u32 = 0x044;
pub const MODE_HEIGHT: u32 = 0x048;
pub const MODE_FORMAT: u32 = 0x04C;
pub const MODE_PITCH: u32 = 0x050;
pub const MODE_START: u32 = 0x054;
pub const MODE_REFRESH: u32 = 0x058;
pub const FRAME: u32 = 0x060;
pub const SCANLINE: u32 = 0x064;
/// `CAPS[0]` to `CAPS[191]`.
pub const CAPS: u32 = 0x100;
pub const CAPS_END: u32 = 0x400;
/// `PALETTE[0]` to `PALETTE[255]`.
pub const PALETTE: u32 = 0x400;
pub const PALETTE_END: u32 = 0x800;

/// What `ID` reads: "TOAD".
pub const ID_VALUE: u32 = 0x4441_4F54;

/// `STATUS`: the display is in its vertical blank.
pub const STATUS_VBLANK: u32 = 1 << 0;
/// `STATUS`: a flip waits for the vertical blank.
pub const STATUS_FLIP_PENDING: u32 = 1 << 1;
/// `STATUS`: `ERROR` holds a rejection.
pub const STATUS_ERROR: u32 = 1 << 2;

/// `IRQ_STATUS` and `IRQ_ENABLE`: a vertical blank began.
pub const IRQ_VBLANK: u32 = 1 << 0;
/// A flip took place.
pub const IRQ_FLIP: u32 = 1 << 1;
/// `ERROR` was latched.
pub const IRQ_ERROR: u32 = 1 << 2;
pub const IRQ_ALL: u32 = IRQ_VBLANK | IRQ_FLIP | IRQ_ERROR;

/// `ERROR_AT` for a rejected register write.
pub const AT_REGISTER: u32 = 0xFFFF_FFFF;
/// `ERROR_AT` for a rejected command that didn't come through the ring:
/// one the emulator's own drivers handed the card (`ToadForce::submit`).
pub const AT_DIRECT: u32 = 0xFFFF_FFFE;

/// The highest value `RING_PUT` takes.
pub const RING_PUT_MAX: u32 = RING_SIZE - 4;

/// Why the card rejected a register write or a command: the codes of
/// `ERROR`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Error {
    /// The level in use has no such command.
    BadOpcode = 1,
    /// The length doesn't fit the opcode, is 0, or passes the end of
    /// what was submitted.
    BadLength = 2,
    /// A surface, rectangle or offset reaches outside card memory or its
    /// surface.
    BadRange = 3,
    /// An argument has a value its field doesn't define.
    BadValue = 4,
    /// A handle that isn't defined (level 1).
    BadHandle = 5,
    /// A value `RING_PUT` doesn't take.
    BadPut = 6,
    /// `MODE_ENABLE` set with a mode the card doesn't take.
    BadMode = 7,
}
