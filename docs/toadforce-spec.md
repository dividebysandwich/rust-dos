# ToadForce: the card's interface

What guest software can see of the ToadForce card: its PCI function, its
registers, its memory, its command ring and its interrupt. A driver is
written against this document, and the emulator's tests are written from
it. `docs/toadforce.md` says why the card is shaped this way and how the
emulator draws; where the two disagree about what a guest sees, this
document is right and the other is corrected.

The pixels a 3D command produces are defined by the software renderer
and its tests ("Defined behaviour" in `docs/toadforce.md`). This document
lists those commands and their arguments, and leaves the arithmetic
there.

## Status

| Chapter | State |
|---|---|
| 1 to 8 | frozen: implemented in `src/toadforce` and `src/bus/toadforce.rs` |
| 9, level 0 | frozen, with them |
| 9, level 1 | provisional: the state, surface and draw commands for transformed vertices are implemented in `src/toadforce/level1` and drawn by `src/toadforce/raster`. A machine's card offers the level for the emulator's own drivers; it is frozen after phase 5, and may change until then |
| 9, level 2 | not started |

A frozen chapter changes only by adding what an earlier reader can't
have relied on: a new register at a reserved offset, a new command, a
new level. "Open points" at the end lists what is still to settle.

Each rule here has a test. Those of the card alone (registers, the
ring, rejections, the display's timing, the commands) are beside the
code in `src/toadforce`; those of the card in the machine (PCI, the
windows, the screen, the interrupt line, the video BIOS, saved states)
are in `tests/video/toadforce_tests.rs`. The tests in `caps.rs` and
`command.rs` hold copies of level 0's capability table and command
list, and fail when either changes.

## 1. Conventions

- Numbers ending in `h` are hexadecimal. Bit 0 is the least significant.
- Everything is little-endian. A word is 32 bits.
- "The emulated clock" is the machine's time, which advances with the
  instructions the processor runs. Nothing a guest can read depends on
  the host's clock, speed or graphics hardware.
- Reserved bits read as 0, and what is written to them is ignored. A
  driver writes 0 to them.
- "Rejected" has one meaning throughout: the write or command changes
  nothing but the error registers (chapter 5).

### Levels

A level is a set of commands, the capability table that goes with them,
and the behaviour of both. Level 0 is display and 2D and is always
there. Level 1 is fixed-function 3D. `LEVEL_MAX` tells the highest level
this card has, and `LEVEL` selects the one in use.

A level never changes once it is released. A driver that writes 1 to
`LEVEL` gets the same capabilities, commands and pixels from every later
card. Each level contains the commands of the levels below it.

## 2. PCI configuration

One function with a type 00h header.

| Offset | Field | Value |
|---|---|---|
| 00h | vendor ID | 1234h |
| 02h | device ID | 7F01h |
| 04h | command | bit 0 I/O space, bit 1 memory space, both writable; 0003h at power-on |
| 06h | status | 0000h (no capability list) |
| 08h | revision | 00h |
| 09h | class | 03h, 00h, 00h: VGA-compatible display controller |
| 10h | BAR 0 | memory, 32-bit, 64 KB: registers |
| 14h | BAR 1 | memory, 32-bit, prefetchable, 32, 64 or 128 MB: card memory |
| 18h | BAR 2 | memory, 32-bit, 1 MB: command ring |
| 2Ch | subsystem vendor ID | 1234h |
| 2Eh | subsystem ID | 5446h |
| 30h | expansion ROM | 0, not writable |
| 3Ch | interrupt line | writable; 11 after reset |
| 3Dh | interrupt pin | 01h (INTA#) |

The other bits of the command register read as 0. The three BARs
decode while bit 1 is set. A BAR whose base is 0 decodes nothing, and
neither does one a guest puts over the machine's RAM. The legacy VGA
ports (3B0h to 3DFh) and the VGA memory at A0000h to BFFFFh decode
whatever the command register holds, as they do on the emulator's other
display adapters: a guest can't turn the VGA off there.

The emulator's BIOS doesn't enumerate PCI, so the card starts as a PCI
BIOS leaves the boot display adapter: I/O and memory decoding on, and
each BAR at a base the emulator assigns (BAR 1 where the other
adapters' VBE frame buffer is). A guest reads the BARs to find them and
may move them. The bits of a BAR below its size read as 0, which is how
a guest sizes it.

The interrupt line register is the guest's to write. The card requests
IRQ 0 to 15 as it says, and none while it holds a larger value.

The vendor and device IDs aren't registered to this card. A driver that
finds them reads `ID` and goes on only when it holds 44414F54h.

The video BIOS at C0000h is the emulator's, with VBE. It isn't an
expansion ROM of this function.

## 3. Registers (BAR 0)

Each register is one word at a word-aligned offset. A read of 1 or 2
bytes inside a register returns those bytes of its value. A write of
less than a whole aligned word is ignored. Offsets this chapter doesn't
list read as 0 and ignore writes.

| Offset | Name | Access | Reset | Meaning |
|---|---|---|---|---|
| 000h | `ID` | R | 44414F54h | "TOAD" |
| 004h | `LEVEL_MAX` | R | card's | highest level the card has |
| 008h | `LEVEL` | R/W | 0 | level in use |
| 00Ch | `STATUS` | R | | bit 0 vertical blank, bit 1 flip pending, bit 2 error latched |
| 010h | `IRQ_STATUS` | R/W1C | 0 | bit 0 vertical blank began, bit 1 flip done, bit 2 error latched |
| 014h | `IRQ_ENABLE` | R/W | 0 | the same bits |
| 018h | `RING_PUT` | R/W | 0 | byte offset after the last command written |
| 01Ch | `RING_GET` | R | 0 | byte offset the card has consumed to |
| 020h | `ERROR` | R/WC | 0 | code of the first rejection |
| 024h | `ERROR_AT` | R | 0 | where it was |
| 028h | `MEMORY` | R | card's | size of card memory in bytes |
| 02Ch | `RESET` | W | | a write with bit 0 set resets the card (chapter 8) |
| 040h | `MODE_ENABLE` | R/W | 0 | bit 0 set: the card shows the mode below |
| 044h | `MODE_WIDTH` | R/W | 0 | pixels |
| 048h | `MODE_HEIGHT` | R/W | 0 | lines |
| 04Ch | `MODE_FORMAT` | R/W | 0 | a format code (chapter 4) |
| 050h | `MODE_PITCH` | R/W | 0 | bytes from one line to the next |
| 054h | `MODE_START` | R/W | 0 | offset in card memory of the surface shown |
| 058h | `MODE_REFRESH` | R/W | 0 | frames a second |
| 060h | `FRAME` | R | 0 | vertical blanks begun since reset |
| 064h | `SCANLINE` | R | | line being shown |
| 100h to 3FCh | `CAPS[0]` to `CAPS[191]` | R | | capability table of `LEVEL` |
| 400h to 7FCh | `PALETTE[0]` to `PALETTE[255]` | R/W | 0 | colours of a `P8` mode |

"R/W1C": a 1 written to a bit clears it, a 0 leaves it. "R/WC": a write
of any value clears the register.

`LEVEL`. A write of a value up to `LEVEL_MAX` selects that level: the
commands the card accepts, the table `CAPS` shows, and every state a
command of level 1 or above can set goes back to its default, a write
of the level already in use included. A write of a larger value is
ignored, so a driver reads `LEVEL` back. Display state (the `MODE`
registers, `PALETTE`, a pending flip) is untouched.

`STATUS`, `IRQ_STATUS`, `MODE_START`, `FRAME` and `SCANLINE` follow the
emulated clock (chapter 6): a read gives their value at the moment of
the read. No read changes anything. There is no busy bit, because the
card has finished every command when the write to `RING_PUT` returns.

`IRQ_ENABLE` keeps bits 0 to 2 of what is written.

`PALETTE[n]` holds red in bits 16 to 23, green in bits 8 to 15 and blue
in bits 0 to 7. It is used only while the card shows a `P8` mode, and
it is separate from the VGA's colour registers.

### Capability table

`CAPS[0]` is the number of entries that follow it. An entry's index and
meaning are the same at every level that has it; a higher level has more
entries. Entries past the count read as 0.

| Index | Level 0's value | Meaning |
|---|---|---|
| 1 | 4096 | largest width and height of a surface in `FILL` and `BLT`, and of a mode |
| 2 | 0000001Eh | formats `FILL` and `BLT` take: bit n set for format code n |
| 3 | 0000000Eh | formats a mode takes, the same way |
| 4 | 50 | lowest `MODE_REFRESH` |
| 5 | 200 | highest `MODE_REFRESH` |

Level 0's count is 5. Level 1 has the same five and these:

| Index | Level 1's value | Meaning |
|---|---|---|
| 6 | 2048 | largest width and height of a render target |
| 7 | 0000001Ch | formats of a render target's colour surface |
| 8 | 00380000h | formats of its depth buffer |
| 9 | 0007FFFEh | formats of a texture |
| 10 | 2048 | largest width and height of a texture |
| 11 | 4 | stages that can have a texture in one draw |
| 12 | 8 | blend stages |
| 13 | 4095 | highest texture handle |
| 14 | 8 | lights |
| 15 | 6 | clip planes |
| 16 | 4 | world matrices a vertex can be blended between |
| 17 | 8 | texture coordinate sets of a vertex |
| 18 | 8 | highest `MAXANISOTROPY` |
| 19 | 64 | largest point size in pixels |
| 20 | 07FDFFFEh | stage operations: bit n set for operation n |

Level 1's count is 20.

## 4. Card memory (BAR 1)

`MEMORY` bytes, which is also BAR 1's size. The guest lays it out: the
card has no allocator and attaches no meaning to an offset until a
register or a command names it.

- A read returns what the last write or command put there. After a
  command that draws into a surface, a read of that surface returns the
  drawn pixels.
- A write is seen by every later command and by the screen. No command
  tells the card that memory changed.
- The first 400000h bytes (4 MB) of card memory are the video BIOS's:
  the frame buffer of its VBE modes, linear at BAR 1 and banked at
  A0000h, and what a VBE mode set clears. The BIOS reports 4 MB and
  gives BAR 1's address as the linear frame buffer's. A guest that uses
  both keeps its own data clear of them, or doesn't call the BIOS while
  it has data there. The BIOS never touches the rest.
- The memory of the standard VGA modes (text, the planar modes, mode
  13h) is the VGA's own and no part of card memory.

### Surfaces and formats

A surface is a rectangle of pixels in card memory, described by five
words wherever a command takes one:

| Word | Field |
|---|---|
| 0 | offset of the top left pixel |
| 1 | pitch: bytes from one line to the next |
| 2 | width in pixels |
| 3 | height in lines |
| 4 | format code |

A description is valid when the width and height are 1 to `CAPS[1]`,
the format is one the command takes, the pitch is at least the width
times the pixel size, and the last byte of the last line is inside card
memory. The pitch needs no alignment. A width, height, format or pitch
that isn't valid is rejected with `BAD_VALUE`, and a surface of valid
ones that reaches outside card memory with `BAD_RANGE`.

| Code | Name | Pixel | Layout |
|---|---|---|---|
| 1 | `P8` | 1 byte | an index into a palette |
| 2 | `R5G6B5` | 2 bytes | red in bits 11 to 15, green 5 to 10, blue 0 to 4 |
| 3 | `X8R8G8B8` | 4 bytes | red in bits 16 to 23, green 8 to 15, blue 0 to 7; bits 24 to 31 unused |
| 4 | `A8R8G8B8` | 4 bytes | the same with alpha in bits 24 to 31 |

Level 1 adds these, for textures and depth buffers. `FILL` and `BLT`
don't take them.

| Code | Name | Pixel | Layout |
|---|---|---|---|
| 5 | `X1R5G5B5` | 2 bytes | red in bits 10 to 14, green 5 to 9, blue 0 to 4 |
| 6 | `A1R5G5B5` | 2 bytes | the same with alpha in bit 15 |
| 7 | `A4R4G4B4` | 2 bytes | alpha, red, green, blue in 4 bits each, from bit 12 down |
| 8 | `A8` | 1 byte | alpha; the colour is white |
| 9 | `L8` | 1 byte | luminance; the alpha is opaque |
| 10 | `A8L8` | 2 bytes | luminance in bits 0 to 7, alpha 8 to 15 |
| 11 | `DXT1` | 8 bytes a block | 4x4 texels: two `R5G6B5` colours and 2 bits a texel, with one bit of alpha |
| 12 | `DXT2` | 16 bytes a block | 4 bits of alpha a texel, then a `DXT1` colour block; colours multiplied by alpha |
| 13 | `DXT3` | 16 bytes a block | as `DXT2`, colours not multiplied |
| 14 | `DXT4` | 16 bytes a block | two alphas and 3 bits a texel, then a `DXT1` colour block; colours multiplied by alpha |
| 15 | `DXT5` | 16 bytes a block | as `DXT4`, colours not multiplied |
| 16 | `V8U8` | 2 bytes | signed du in bits 0 to 7, dv 8 to 15 |
| 17 | `L6V5U5` | 2 bytes | signed du in bits 0 to 4, dv 5 to 9, luminance 10 to 15 |
| 18 | `X8L8V8U8` | 4 bytes | signed du in bits 0 to 7, dv 8 to 15, luminance 16 to 23 |
| 19 | `D16` | 2 bytes | depth |
| 20 | `D24X8` | 4 bytes | depth in bits 8 to 31 |
| 21 | `D24S8` | 4 bytes | depth in bits 8 to 31, stencil 0 to 7 |

An image in a block format is rows of blocks: a line of its memory is
one row, of the width divided by 4 and rounded up blocks, and it has
the height divided by 4 and rounded up of them. Its pitch is the bytes
from one row of blocks to the next.

## 5. Command ring (BAR 2)

BAR 2 is 100000h bytes of ordinary memory: the guest reads back what it
wrote, and the card never writes to it.

The guest writes commands at `RING_PUT`'s offset and upward, then
writes the offset after the last of them to `RING_PUT`. During that
write the card runs every command from `RING_GET` up to the new
`RING_PUT`, in order, and then sets `RING_GET` equal to it. When the
write returns, the commands' effects on card memory and registers are
complete.

- `RING_PUT` takes multiples of 4 from 0 to FFFFCh. Any other value is
  rejected with `BAD_PUT`, and `RING_PUT` keeps its value.
- A new value below `RING_GET` wraps: the card runs from `RING_GET` to
  the end of the ring, then from 0 to the new value.
- A command never crosses the end of the ring. The guest fills what is
  left with a `NOP` and starts the command at 0. When the last command
  ends exactly at the end, the guest writes 0 to `RING_PUT`. A command
  whose length reaches past the end is rejected as "Rejections" says of
  a length with no next command, and what the guest wrote from offset 0
  on is dropped with it.
- A write of `RING_PUT`'s current value runs nothing. One write can
  therefore submit at most FFFFCh bytes.

### Command format

Each command starts with a header word: the opcode in bits 0 to 15, and
the command's length in words, header included, in bits 16 to 31. The
arguments follow.

### Rejections

A command with an opcode the level doesn't have, a length its opcode
doesn't allow, or an argument this document calls invalid is rejected:
it draws nothing and sets no state, and the card goes on with the next
command. A command is checked in that order, and within its arguments
values before ranges, so one with several faults has the code of the
first: `BAD_OPCODE`, then `BAD_LENGTH`, then `BAD_VALUE`, then
`BAD_RANGE`, and after those what depends on the card's state,
`BAD_HANDLE` and `BAD_STATE`.

A header whose length is 0, or reaches past the commands submitted or
past the end of the ring, leaves no next command to find. The card
rejects it with `BAD_LENGTH` and drops the rest of what was submitted:
`RING_GET` becomes `RING_PUT`.

The first rejection while `ERROR` is 0 sets `ERROR` to its code,
`ERROR_AT` to where it was, bit 2 of `STATUS` and bit 2 of
`IRQ_STATUS`. Later rejections change nothing until the guest writes to
`ERROR`, which clears `ERROR`, `ERROR_AT` and bit 2 of `STATUS`.

| `ERROR_AT` | The rejection was of |
|---|---|
| 0 to FFFFCh | the command whose header is at this offset of the ring |
| FFFFFFFFh | a register write |
| FFFFFFFEh | a command that didn't come through the ring |

The last is for the emulator's own drivers. The built-in Windows hands
the card the words a guest's driver would have put in the ring, without
the ring, and the card runs them by the rules of this document. A guest
never sees the value unless such a driver has a bug, which a debug
build of the emulator stops at.

| Code | Name | Cause |
|---|---|---|
| 0 | | none |
| 1 | `BAD_OPCODE` | the level in use has no such command |
| 2 | `BAD_LENGTH` | the length doesn't fit the opcode, is 0, or passes the end |
| 3 | `BAD_RANGE` | a surface, rectangle or offset reaches outside card memory or its surface |
| 4 | `BAD_VALUE` | an argument has a value its field doesn't define |
| 5 | `BAD_HANDLE` | a handle that isn't defined (level 1) |
| 6 | `BAD_PUT` | a value `RING_PUT` doesn't take |
| 7 | `BAD_MODE` | `MODE_ENABLE` set with a mode the card doesn't take |
| 8 | `BAD_STATE` | the command needs state that isn't set: a render target, a depth buffer, a light (level 1) |

A correct driver never causes a rejection. The registers exist so that a
driver under development, and the emulator's log, can say what went
wrong.

## 6. Display

The card has two sources for the picture, and `MODE_ENABLE` picks one.

### `MODE_ENABLE` = 0: the VGA

The picture is what the VGA registers and the VBE mode describe, as on
the emulator's `Svga` adapter. This is the state after reset, and DOS
programs and a Windows without a driver never leave it.

A VBE mode has its frame buffer at offset 0 of card memory (chapter 4).
The video BIOS writes 0 to `MODE_ENABLE` before every mode set, so a
mode set through INT 10h always brings the VGA's picture back. The
built-in DOS writes it too when a program ends and the prompt's text
mode takes the screen.

Bit 0 of `STATUS`, `SCANLINE` and `FRAME` follow the VGA's timing: bit 0
is set while port 3DAh reports vertical retrace, `SCANLINE` is the line
the VGA is on, and `FRAME` counts the retraces that began, each of which
also sets bit 0 of `IRQ_STATUS`.

### `MODE_ENABLE` = 1: the card's mode

A write of 1 takes the mode from the `MODE` registers as they are at
that moment. The mode is valid when:

- `MODE_WIDTH` and `MODE_HEIGHT` are 1 to `CAPS[1]`,
- `MODE_FORMAT` is a code `CAPS[3]` has,
- `MODE_PITCH` is at least the width times the pixel size,
- `MODE_REFRESH` is `CAPS[4]` to `CAPS[5]`,
- the surface at `MODE_START` is inside card memory.

An invalid mode is rejected with `BAD_MODE`, and `MODE_ENABLE` stays as
it was. A write of 1 while it is already 1 sets the mode again from the
registers.

While the card's mode is shown:

- The picture is the surface at `MODE_START`. A `P8` pixel shows the
  colour `PALETTE` has for it.
- Writes to `MODE_WIDTH`, `MODE_HEIGHT`, `MODE_FORMAT`, `MODE_PITCH` and
  `MODE_REFRESH` are kept and read back, and change nothing until the
  next write of 1 to `MODE_ENABLE`. "The mode shown" is the values those
  registers had at that write, which the card keeps apart from them.
- A write to `MODE_START` reads back at once, shows from the next
  frame, and cancels a pending flip. A value whose surface, with the
  height, pitch and format of the mode shown, reaches outside card
  memory is rejected with `BAD_RANGE`.
- A colour written to `PALETTE` shows from the next frame.
- The VGA registers keep their contents and take writes, and none of it
  is shown.
- The window at A0000h stays what the VGA's side has it as: the VGA's
  own memory after a standard mode, and after a VBE mode the bank of
  the BIOS's part of card memory that function 4F05h selects.
- The video BIOS reports, in function 4F03h, the mode it set last,
  which is the one that comes back. Its functions that lay out a VBE
  mode's picture (4F06h, the line length, and 4F07h, the display start)
  fail with AH = 02h, since the picture on the screen isn't the BIOS's.

A write of 0 brings the VGA's picture back: what its registers
describe, and the VBE mode that was set when the card's mode took the
screen, with its display start. Both writes cancel a pending flip, and
card memory is unchanged by either.

The emulator's window shows a mode less than 640 pixels wide with each
pixel doubled across and down, as it shows the VBE modes of that size.
Nothing a guest reads depends on it.

### Timing

With the card's mode shown, `MODE_HEIGHT` and `MODE_REFRESH` below are
the mode shown's values, and all times are the emulated clock's:

- a frame has `MODE_HEIGHT` + `MODE_HEIGHT` / 16 + 1 lines (the
  division rounded down),
- a line lasts 1 000 000 000 / (`MODE_REFRESH` x the lines of a frame)
  nanoseconds, rounded down, and a frame its lines' time: up to a
  nanosecond a line less than 1 / `MODE_REFRESH` seconds,
- frames follow each other from the clock's zero, so the position in
  the frame is the clock's time modulo a frame's. A write of 1 to
  `MODE_ENABLE` changes the frame's length and doesn't start a frame,
- `SCANLINE` is the line of the present moment, from 0,
- bit 0 of `STATUS` is set while `SCANLINE` is `MODE_HEIGHT` or more,
- when `SCANLINE` reaches `MODE_HEIGHT`, `FRAME` goes up by one, bit 0
  of `IRQ_STATUS` is set, and a pending flip takes place.

`FRAME` counts through changes of mode, the card's and the VGA's alike.
The moment of a change counts no blank of its own.

Port 3DAh reports the same timing while the card's mode is shown, so
the machine has one display timing at a time: its bit 3 is bit 0 of
`STATUS`, and its bit 0 (display enable off) is set through the
vertical blank and for the last fifth of every line.

### Flips

`FLIP` (chapter 9) changes `MODE_START` by command. Without its wait
flag the change is made at once and bit 1 of `IRQ_STATUS` is set. With
the flag, bit 1 of `STATUS` is set and `MODE_START` keeps reading the
old offset until the next vertical blank begins; then `MODE_START`
changes, bit 1 of `STATUS` clears and bit 1 of `IRQ_STATUS` is set.
Either way the screen shows the surface from the frame after the
change.

The command itself never waits: the write to `RING_PUT` returns with
the flip pending. A driver that has to wait reads `STATUS`, or takes the
interrupt. A second `FLIP` while one is pending replaces it, and the
first one's surface is never shown.

## 7. Interrupt

INTA# is asserted while `IRQ_STATUS` and `IRQ_ENABLE` have a set bit in
common, and released when they don't. The bits of `IRQ_STATUS` are set
by their events whether enabled or not, and stay set until the guest
writes 1 to them. The request goes up at the event's time on the
emulated clock, on the line the interrupt line register names, and
moves with it when the guest writes another.

| Bit | Set when |
|---|---|
| 0 | a vertical blank begins |
| 1 | a flip takes place |
| 2 | `ERROR` is latched |

## 8. Reset and saved states

Power-on: every register has its reset value, card memory and the ring
are zero, and the VGA is in the BIOS's start-up mode.

A reset of the machine, and a write of 1 to `RESET`, put every register
of chapter 3 at its reset value, `PALETTE` and `FRAME` included, drop a
pending flip and return every command state to its default. With
`MODE_ENABLE` back at 0, the screen is the VGA's again, as after a write
of 0 to it. Card memory, the ring's contents and PCI configuration
space keep what they had. `RESET` leaves the VGA registers alone; a
machine reset resets them as on any VGA.

After `RESET`, a driver that left commands in the ring starts again at
offset 0, since `RING_PUT` and `RING_GET` are 0.

A saved state of the emulator holds everything this document describes.
After a load, each register, card memory, the ring, the state commands
set, the mode shown, a pending flip and the position in the frame are
as they were at the save, and a guest can't tell that it happened.

## 9. Commands

Opcodes 0000h to 000Fh are level 0's.

### Level 0

**`NOP`, 0000h.** Length 1 or more. The arguments are skipped.

**`FILL`, 0001h.** Length 11.

| Word | Argument |
|---|---|
| 1 to 5 | destination surface |
| 6, 7 | left and top of the rectangle |
| 8, 9 | its width and height |
| 10 | value |

Writes the low 8, 16 or 32 bits of the value, by the surface's pixel
size, to each pixel of the rectangle, the bits its format doesn't
define among them. The rectangle has a width and height of at least 1
and lies inside the surface, or the command is rejected with
`BAD_RANGE`.

**`BLT`, 0002h.** Length 21.

| Word | Argument |
|---|---|
| 1 to 5 | destination surface |
| 6 to 9 | destination rectangle: left, top, width, height |
| 10 to 14 | source surface |
| 15 to 18 | source rectangle |
| 19 | flags: bit 0 source colour key |
| 20 | colour key |

Copies pixels unchanged, every bit of them. The two surfaces have the
same pixel size, or the command is rejected with `BAD_VALUE`; the card
converts no formats at level 0. A set bit of the flags this table
doesn't define is rejected with `BAD_VALUE` too. Both rectangles have a
width and height of at least 1 and lie inside their surfaces, or the
command is rejected with `BAD_RANGE`.

- Equal rectangle sizes copy pixel for pixel. Where source and
  destination overlap in memory, the result is as if the whole source
  rectangle were read before any pixel is written.
- Different sizes stretch or shrink. The destination pixel at column
  `x` of `dw` and row `y` of `dh` takes the source pixel at column
  `((2x + 1) * sw) / (2 * dw)` and row `((2y + 1) * sh) / (2 * dh)`,
  divisions rounded down: the source pixel under the destination
  pixel's centre. Overlapping memory is read as for a copy: the whole
  source rectangle before any pixel is written.
- With bit 0 of the flags, a source pixel whose value equals the low
  bits of the colour key leaves its destination pixel as it was. The
  comparison covers the bits the source's format defines: bits 24 to 31
  of `X8R8G8B8` take no part, and all 32 of `A8R8G8B8` do.

**`FLIP`, 0003h.** Length 3.

| Word | Argument |
|---|---|
| 1 | offset of the surface to show |
| 2 | flags: bit 0 wait for the vertical blank |

See "Flips" in chapter 6. The surface has the height, pitch and format
of the mode shown and lies inside card memory, or the command is
rejected with `BAD_RANGE`. A set bit of the flags this table doesn't
define is rejected with `BAD_VALUE`. With `MODE_ENABLE` at 0 there is no
mode to check against or blank to wait for: the offset is inside card
memory, or the command is rejected with `BAD_RANGE`, and the flip is
made at once whatever the flag says.

### Level 1

Opcodes 0010h to 00FFh. Level 1 has level 0's commands too. This
section is provisional until the level is frozen, which waits for the
card's transform and lighting and for the games the Direct3D driver
proves the commands against. `LEVEL_MAX` reports the level before
then, for the drivers that are part of the emulator and change with
it. A driver in a guest that uses a provisional level may find it
changed by a later version of the emulator.

A number in an argument is an IEEE single-precision value in its word.
One that isn't finite is rejected with `BAD_VALUE`, and so is any
argument outside the values its table gives: a slot, a kind or a flag
word with a bit the table doesn't define. A colour of four numbers is
red, green, blue, alpha. A colour in one word has alpha in bits 24 to
31, red in 16 to 23, green in 8 to 15 and blue in 0 to 7.

After a reset or a write to `LEVEL`: every render state and stage
state has the reset value of its table below, every transform is the
identity, the viewport and the scissor rectangle are zero and the
scissor is off, the material is zero, no light is set or on, the clip
planes are zero, there is no render target, and no texture is defined.

**`SET_RENDER_STATE`, 0010h.** Length 3 or more, odd.

| Word | Argument |
|---|---|
| 1, 2 | a render state's number and its value |
| 3 on | more such pairs |

Sets each state to its value, in order. A number the table doesn't
have, or a value its state doesn't take, is rejected with `BAD_VALUE`,
and none of the command's states is set.

**`SET_STAGE_STATE`, 0011h.** Length 4 or more, even.

| Word | Argument |
|---|---|
| 1 | stage, 0 to 7 |
| 2, 3 | a stage state's number and its value |
| 4 on | more such pairs |

The same for one stage's states.

**`SET_TRANSFORM`, 0012h.** Length 18.

| Word | Argument |
|---|---|
| 1 | which: 0 to 3 world (1 to 3 for vertex blending), 4 view, 5 projection, 6 to 13 the texture coordinate sets 0 to 7 |
| 2 to 17 | the matrix, row by row; a vector is a row and multiplies from the left, as in Direct3D |

**`SET_VIEWPORT`, 0013h.** Length 7.

| Word | Argument |
|---|---|
| 1, 2 | left and top, in pixels of the render target |
| 3, 4 | width and height |
| 5, 6 | the depths of the near and far planes |

Left plus width and top plus height are at most `CAPS[6]`, and the
depths are 0 to 1 with the near one not above the far one, or the
command is rejected with `BAD_VALUE`. The viewport isn't checked
against the render target, which may change after it: a draw reaches
the pixels both have.

**`SET_SCISSOR`, 0014h.** Length 6.

| Word | Argument |
|---|---|
| 1 | 1 to limit draws to the rectangle, 0 not to |
| 2 to 5 | left, top, width, height |

Left plus width and top plus height are at most `CAPS[6]`, or the
command is rejected with `BAD_VALUE`. As with the viewport, the
rectangle isn't checked against the render target.

**`SET_MATERIAL`, 0015h.** Length 18.

| Word | Argument |
|---|---|
| 1 to 4 | diffuse colour |
| 5 to 8 | ambient colour |
| 9 to 12 | specular colour |
| 13 to 16 | emissive colour |
| 17 | specular power |

**`SET_LIGHT`, 0016h.** Length 29.

| Word | Argument |
|---|---|
| 1 | slot, 0 to 7 |
| 2 | kind: 1 point, 2 spot, 3 directional |
| 3 to 6 | diffuse colour |
| 7 to 10 | specular colour |
| 11 to 14 | ambient colour |
| 15 to 17 | position, in world space |
| 18 to 20 | direction, in world space |
| 21 | range |
| 22 | falloff |
| 23 to 25 | attenuation: constant, linear, quadratic |
| 26, 27 | theta and phi, the angles of a spot's inner and outer cones in radians |
| 28 | spot model: 0 Direct3D's (two cones and the falloff between them), 1 OpenGL's (the cosine of the angle to the power of the falloff, nothing outside phi) |

Setting a light doesn't turn it on.

**`ENABLE_LIGHT`, 0017h.** Length 3.

| Word | Argument |
|---|---|
| 1 | slot, 0 to 7 |
| 2 | 1 on, 0 off |

Turning on a slot no `SET_LIGHT` has set is rejected with `BAD_STATE`.

**`SET_CLIP_PLANE`, 0018h.** Length 6.

| Word | Argument |
|---|---|
| 1 | index, 0 to 5 |
| 2 to 5 | the plane's four coefficients, in world space |

The planes in use are those whose bits `CLIPPLANEENABLE` has.

**`SET_RENDER_TARGET`, 0020h.** Length 9.

| Word | Argument |
|---|---|
| 1 to 5 | the colour surface |
| 6 | offset of the depth buffer |
| 7 | its format, or FFFFFFFFh for none |
| 8 | its pitch |

What draws and `CLEAR` write to. The colour surface has a format of
`CAPS[7]` and a width and height up to `CAPS[6]`. The depth buffer has
a format of `CAPS[8]`, the colour surface's width and height, and a
pitch of at least its width times its pixel size. Both lie inside card
memory. A colour surface whose format word is FFFFFFFFh takes the
render target away, and the other words are ignored.

**`CLEAR`, 0021h.** Length 5 or more, in steps of 4.

| Word | Argument |
|---|---|
| 1 | flags: bit 0 colour, bit 1 depth, bit 2 stencil |
| 2 | colour |
| 3 | depth, a number from 0 to 1 |
| 4 | stencil value, 0 to 255 |
| 5 on | rectangles: left, top, width, height of each |

Writes each rectangle of the buffers the flags name, or the whole
render target when there is no rectangle. Neither the viewport nor the
scissor limits it.

- The colour is reduced to the surface's format by taking each
  channel's top bits. An `X8R8G8B8` surface gets 0 in bits 24 to 31.
- The depth is stored as the nearest whole number to it times FFFFh
  (`D16`) or FFFFFFh.
- Depth and stencil of a `D24S8` buffer are written apart: a clear of
  one leaves the other. A depth clear of `D24X8` writes 0 to bits 0 to
  7.

No flag, a flag this table doesn't define, a depth outside 0 to 1 or a
stencil value above 255 is rejected with `BAD_VALUE`; a rectangle with
no pixel or reaching outside the render target with `BAD_RANGE`; and no
render target, a depth flag without a depth buffer, or a stencil flag
without a `D24S8` buffer with `BAD_STATE`. With no render target there
is nothing to check a rectangle against, so only one with no pixel is
rejected before that `BAD_STATE`.

**`DEFINE_TEXTURE`, 0022h.** Length 11 or more, odd.

| Word | Argument |
|---|---|
| 1 | handle, 1 to `CAPS[13]` |
| 2 | format, one of `CAPS[9]` |
| 3, 4 | width and height of level 0: powers of two up to `CAPS[10]` |
| 5 | levels, 1 up to the count that ends at 1x1 |
| 6 | flags: bit 0 a cube map, bit 1 the texture has a colour key |
| 7 | offset of the palette, or FFFFFFFFh |
| 8 | colour key |
| 9, 10 | offset and pitch of level 0's image |
| 11 on | the same for each further image |

Tells the card where a texture is. The card copies nothing: it reads
the images from card memory when a draw samples them, so what the
guest writes there afterwards shows in the next draw.

- Each level has half the width and height of the one before, and at
  least 1. Images follow level by level, and a cube map has six for
  each level: the faces +x, -x, +y, -y, +z, -z. A cube map is square.
- The command has exactly the images its levels and faces need.
- An image's pitch is at least one line of it: its width times its
  pixel size, or in a block format its row of blocks.
- A `P8` texture has a palette, and no other format does. The palette
  is 256 words of card memory, each a colour in one word, alpha
  included.
- The colour key is a pixel value of the texture's format, with no
  bit set that the format doesn't define. Without flag bit 1 the word
  is ignored. A texture in a block format has no colour key.
- A handle already defined is defined anew, in the stages that have it
  too.

Values outside those above are rejected with `BAD_VALUE`, and an image
or palette that reaches outside card memory with `BAD_RANGE`.

**`FREE_TEXTURE`, 0023h.** Length 2.

| Word | Argument |
|---|---|
| 1 | handle, 1 to `CAPS[13]` |

The handle is defined no longer, and the stages that had it have no
texture. A handle that isn't defined is rejected with `BAD_HANDLE`.

**`SET_TEXTURE`, 0024h.** Length 3.

| Word | Argument |
|---|---|
| 1 | stage, 0 to 7 |
| 2 | handle, 1 to `CAPS[13]`, or 0 for no texture |

A handle that isn't defined is rejected with `BAD_HANDLE`.

**`DRAW`, 0030h.** Length 4 or more.

| Word | Argument |
|---|---|
| 1 | primitive: 1 points, 2 lines, 3 a line strip, 4 triangles, 5 a triangle strip, 6 a triangle fan |
| 2 | vertex layout |
| 3 | vertex count, 0 to 65536 |
| 4 on | the vertices, one after the other |

Draws the primitives its vertices make, with the state as it is when
the command runs. "Drawing" below says what a draw does; the rules
here are for all four draw commands.

- The vertex layout is a Direct3D flexible vertex format code, and
  says which fields a vertex has and in which order: the position,
  the point size, the diffuse colour, the specular colour, and up to
  eight sets of one to four texture coordinates. A vertex is as many
  words as its fields take. A code that names no layout, or one with a
  reserved bit set, is rejected with `BAD_VALUE`.
- Level 1's draws take vertices in screen space only (the layout's
  position is x, y, z and 1 / w). Layouts with a position for the
  card to transform are rejected with `BAD_VALUE` until the card's
  transform and lighting is specified.
- The command has exactly the words its count and layout need, or it
  is rejected with `BAD_LENGTH`. The values that set the length are
  checked before it.
- Vertices left over after the last whole primitive draw nothing: a
  third vertex of a list of lines, one or two after the last triangle.
- The state is checked last. With no render target, or with stages
  that sample more than `CAPS[11]` textures or use more than `CAPS[11]`
  sets of coordinates, the draw is rejected with `BAD_STATE`.

**`DRAW_INDEXED`, 0031h.** Length 5 or more.

| Word | Argument |
|---|---|
| 1 | primitive |
| 2 | vertex layout |
| 3 | vertex count, 0 to 65536 |
| 4 | index count, 0 to 196605 |
| 5 on | the vertices, then the indices |

The indices give the order the primitives take the vertices in. Each
is 16 bits, two to a word, the first of the two in bits 0 to 15; with
an odd count the last word's upper half is ignored. An index that
isn't below the vertex count is rejected with `BAD_RANGE`, before the
state is checked.

**`DRAW_BUFFER`, 0032h.** Length 6.

| Word | Argument |
|---|---|
| 1 | primitive |
| 2 | vertex layout |
| 3 | stride: the bytes from one vertex to the next, from the layout's size to 1024 |
| 4 | offset of the first vertex in card memory |
| 5 | vertex count, 0 to 65536 |

The vertices are read from card memory when the command runs. A
stride outside its range is rejected with `BAD_VALUE`, and vertices
that reach outside card memory with `BAD_RANGE`. The offset need not
be a multiple of four.

**`DRAW_BUFFER_INDEXED`, 0033h.** Length 8 or more.

| Word | Argument |
|---|---|
| 1 | primitive |
| 2 | vertex layout |
| 3 | stride |
| 4 | offset of the first vertex in card memory |
| 5 | vertex count, 0 to 65536 |
| 6 | offset of the indices in card memory, or FFFFFFFFh |
| 7 | index count, 0 to 196605 |
| 8 on | the indices, when word 6 is FFFFFFFFh |

`DRAW_BUFFER` with indices as `DRAW_INDEXED` has them, in the command
or in card memory, where each is two bytes. With an offset the command
is 8 words long, and indices that reach outside card memory are
rejected with `BAD_RANGE`.

#### Drawing

This section defines the pixels a draw writes. Every renderer of the
card gives these pixels; the software renderer gives them bit for bit
on every host.

**Vertices.** A field the layout lacks has its default: the diffuse
colour is white with alpha 255, the specular colour is 0, texture
coordinates are 0. A 1 / w that is 0, negative or no finite number
counts as 1. Positions are in pixels from the render target's top
left. With `PIXEL_CENTRE` 0 the centre of the top left pixel is at
(0, 0), and with 1 at (0.5, 0.5). A position is limited to 32768
pixels either side of 0 and rounded to 1/256 of a pixel. A primitive
with a position that is no finite number is not drawn.

**Primitives.** A strip's triangle i (from 0) has the vertices i,
i + 1, i + 2, and every odd one has the first two the other way round,
so all wind alike. A fan's triangle i has the vertices i + 1, i + 2
and 0. With `SHADEMODE` flat a primitive has one vertex's colours all
over, the first of its vertices in the draw's order (for a fan's
triangle i that is vertex i + 1), or the last with `FLAT_VERTEX` 1.

**Triangles.** A triangle with no area is not drawn, and neither is
one `CULLMODE` culls: clockwise on the screen with 2,
counterclockwise with 3. A triangle covers the pixels whose centres
are inside it. A centre on an edge is covered when the edge is the
triangle's top (horizontal, with the triangle below it) or on its
left, so two triangles that share an edge cover each of its pixels
once. With `FILLMODE` 2 a triangle that isn't culled is drawn as the
three lines of its edges, and with 1 as its three vertices' points.

**Lines.** A line has one pixel in each column it crosses when it is
longer along x, and one in each row otherwise: the pixel whose centre
is nearest to the line there, and of two equally near the lower or
the right one. The columns (or rows) are those whose centres lie
between the two ends, the first end's included. The last end's pixel
is drawn with `LASTPIXEL` 1 and left out with 0.

**Points.** A point is a square around its position with sides of the
vertex's point size, or `POINTSIZE` where the layout has none,
limited to `POINTSIZE_MIN` and `POINTSIZE_MAX`. It covers pixels as
two triangles would. With `POINTSPRITEENABLE` every coordinate set
goes from (0, 0) at the square's top left to (1, 1) at its bottom
right.

**Clipping.** Pixels outside the viewport's rectangle or the render
target are not drawn, nor those outside the scissor rectangle when
`SCISSORENABLE` is 1.

**Values at a pixel.** Colours, texture coordinates and fog are
interpolated to the pixel's centre with perspective: each value times
1 / w, and 1 / w itself, change linearly over the screen, and the
value is their quotient. Depth changes linearly over the screen and
is limited to 0 to 1. A colour channel is rounded to the nearest of
0 to 255.

**The order of a fragment's steps.**

1. The depth and stencil tests, when no later step can drop the
   fragment; otherwise they come after step 5, so a dropped fragment
   leaves the depth and stencil as they were.
2. The stages, from 0 up to the first whose `COLOROP` is 1. A colour
   or alpha operation that reads a texture the stage hasn't got leaves
   the fragment's colour or alpha as it was.
3. With `SPECULARENABLE`, the specular colour is added to the red,
   green and blue.
4. With `FOGENABLE`, the red, green and blue are mixed with
   `FOGCOLOR` by the fog factor f, 0 to 255: (c * f + fog * (255 - f)
   + 127) / 255. With `FOGTABLEMODE` 0 the factor is the specular
   colour's alpha. Otherwise it comes from the fragment's depth d, or
   its w with `FOG_BY_W` 1: (end - d) / (end - start) for mode 3,
   e^(-density * d) for 1, e^(-(density * d)^2) for 2.
5. The colour key and the alpha test. With `COLORKEYENABLE`, a
   fragment is dropped when a texture with a colour key has the key
   at the texel nearest to the fragment's coordinates. With
   `ALPHATESTENABLE`, it is dropped unless its alpha against
   `ALPHAREF` passes `ALPHAFUNC`.
6. With `DITHERENABLE` and an `R5G6B5` render target, the dither. The
   matrix below gives d for the pixel, by x and y modulo 4. Red and
   blue become (c + d / 2) / 8 and green (c + d / 4) / 4, each
   division rounded down and the result limited to the channel's
   bits; step 8 writes those.

   | y \ x | 0 | 1 | 2 | 3 |
   |---|---|---|---|---|
   | 0 | 0 | 8 | 2 | 10 |
   | 1 | 12 | 4 | 14 | 6 |
   | 2 | 3 | 11 | 1 | 9 |
   | 3 | 15 | 7 | 13 | 5 |
7. With `ALPHABLENDENABLE`, the blend with the pixel there.
8. The write of the channels `COLORWRITEENABLE` names. A channel
   goes to a 16-bit target rounded to the nearest value the target
   holds: (c * 31 + 127) / 255 for five bits, (c * 63 + 127) / 255
   for six. An `X8R8G8B8` target's top byte is written 0.

**Depth and stencil.** Depth is kept as 16 bits in a `D16` buffer and
24 bits in the others, where it is in bits 8 to 31 of a pixel and the
stencil value in bits 0 to 7. A fragment's depth is z times the
largest value, rounded to nearest, less `ZBIAS`, and not below 0. The
depth test compares it with the pixel's by `ZFUNC` and passes always
with `ZENABLE` 0 or no depth buffer. The stencil test, with
`STENCILENABLE` and a `D24S8` buffer, compares `STENCILREF` with the
pixel's stencil value by `STENCILFUNC`, both under `STENCILMASK`. The
stencil value then changes by `STENCILFAIL` when the stencil test
fails, `STENCILZFAIL` when it passes and the depth test fails, and
`STENCILPASS` when both pass, in the bits `STENCILWRITEMASK` names. A
fragment that passes both is drawn, and writes its depth with
`ZWRITEENABLE`.

**Textures.** A coordinate of 0 is a texture's left or top edge and 1
its right or bottom edge, so a texel's centre is half a texel in. The
nearest filter takes the texel a coordinate falls in. The linear
filter mixes the four texels around it by the distances to their
centres in 1/256 of a texel. The level of detail is half the base-2
logarithm of the larger of two numbers: the squared length, in
texels of level 0, of the step the coordinates take for one pixel to
the right, and for one pixel down. `MIPMAPLODBIAS` is added. At or
below 0 the `MAGFILTER` applies to the level `MAXMIPLEVEL` names;
above, the `MINFILTER`, on the level nearest to the level of detail
with `MIPFILTER` 1, on the two around it mixed with 2, and on the
level `MAXMIPLEVEL` names with 0. With `MIPFILTER` 1 or 2 the level
of detail is limited to `MAXMIPLEVEL` and the last level first. With
an anisotropic filter and `MAXANISOTROPY` a, the level follows the
smaller of the two numbers, though not below the larger over a
squared.

**Order between draws.** A pixel sees the primitives in the order
their commands came, and within a command in the order of its
vertices. A draw that samples a texture whose memory overlaps its
render target's, or whose colour and depth buffers overlap, reads
what the primitives before it wrote. Within one primitive it reads
what the primitive wrote to the rows above the pixel; what it reads
of the pixel's own row is not defined.

#### Render states

The numbers and values are Direct3D 8's `D3DRS` ones where Direct3D
has the state. 256 to 258 are the card's own.

| Number | Name | Values | Reset |
|---|---|---|---|
| 7 | `ZENABLE` | 0 or 1 | 0 |
| 8 | `FILLMODE` | 1 to 3 | 3 |
| 9 | `SHADEMODE` | 1 to 2 | 2 |
| 14 | `ZWRITEENABLE` | 0 or 1 | 1 |
| 15 | `ALPHATESTENABLE` | 0 or 1 | 0 |
| 16 | `LASTPIXEL` | 0 or 1 | 1 |
| 19 | `SRCBLEND` | 1 to 13, a blend factor | 2 |
| 20 | `DESTBLEND` | 1 to 13, a blend factor | 1 |
| 22 | `CULLMODE` | 1 to 3 | 3 |
| 23 | `ZFUNC` | 1 to 8, a compare function | 4 |
| 24 | `ALPHAREF` | 0 to 255 | 0 |
| 25 | `ALPHAFUNC` | 1 to 8, a compare function | 8 |
| 26 | `DITHERENABLE` | 0 or 1 | 0 |
| 27 | `ALPHABLENDENABLE` | 0 or 1 | 0 |
| 28 | `FOGENABLE` | 0 or 1 | 0 |
| 29 | `SPECULARENABLE` | 0 or 1 | 0 |
| 34 | `FOGCOLOR` | any | 0 |
| 35 | `FOGTABLEMODE` | 0 to 3 | 0 |
| 36 | `FOGSTART` | a number | 0.0 |
| 37 | `FOGEND` | a number | 1.0 |
| 38 | `FOGDENSITY` | a number | 1.0 |
| 41 | `COLORKEYENABLE` | 0 or 1 | 0 |
| 47 | `ZBIAS` | 0 to 16 | 0 |
| 48 | `RANGEFOGENABLE` | 0 or 1 | 0 |
| 52 | `STENCILENABLE` | 0 or 1 | 0 |
| 53 | `STENCILFAIL` | 1 to 8, a stencil operation | 1 |
| 54 | `STENCILZFAIL` | 1 to 8, a stencil operation | 1 |
| 55 | `STENCILPASS` | 1 to 8, a stencil operation | 1 |
| 56 | `STENCILFUNC` | 1 to 8, a compare function | 8 |
| 57 | `STENCILREF` | 0 to 255 | 0 |
| 58 | `STENCILMASK` | any | FFFFFFFFh |
| 59 | `STENCILWRITEMASK` | any | FFFFFFFFh |
| 60 | `TEXTUREFACTOR` | any | FFFFFFFFh |
| 128 | `WRAP0` | bits of Fh | 0 |
| 129 | `WRAP1` | bits of Fh | 0 |
| 130 | `WRAP2` | bits of Fh | 0 |
| 131 | `WRAP3` | bits of Fh | 0 |
| 132 | `WRAP4` | bits of Fh | 0 |
| 133 | `WRAP5` | bits of Fh | 0 |
| 134 | `WRAP6` | bits of Fh | 0 |
| 135 | `WRAP7` | bits of Fh | 0 |
| 136 | `CLIPPING` | 0 or 1 | 1 |
| 137 | `LIGHTING` | 0 or 1 | 1 |
| 139 | `AMBIENT` | any | 0 |
| 140 | `FOGVERTEXMODE` | 0 to 3 | 0 |
| 141 | `COLORVERTEX` | 0 or 1 | 1 |
| 142 | `LOCALVIEWER` | 0 or 1 | 1 |
| 143 | `NORMALIZENORMALS` | 0 or 1 | 0 |
| 145 | `DIFFUSEMATERIALSOURCE` | 0 to 2 | 1 |
| 146 | `SPECULARMATERIALSOURCE` | 0 to 2 | 2 |
| 147 | `AMBIENTMATERIALSOURCE` | 0 to 2 | 0 |
| 148 | `EMISSIVEMATERIALSOURCE` | 0 to 2 | 0 |
| 151 | `VERTEXBLEND` | 0 to 3 | 0 |
| 152 | `CLIPPLANEENABLE` | bits of 3Fh | 0 |
| 154 | `POINTSIZE` | a number | 1.0 |
| 155 | `POINTSIZE_MIN` | a number | 1.0 |
| 156 | `POINTSPRITEENABLE` | 0 or 1 | 0 |
| 157 | `POINTSCALEENABLE` | 0 or 1 | 0 |
| 158 | `POINTSCALE_A` | a number | 1.0 |
| 159 | `POINTSCALE_B` | a number | 0.0 |
| 160 | `POINTSCALE_C` | a number | 0.0 |
| 166 | `POINTSIZE_MAX` | a number | 64.0 |
| 168 | `COLORWRITEENABLE` | bits of Fh | 15 |
| 171 | `BLENDOP` | 1 to 5 | 1 |
| 256 | `PIXEL_CENTRE` | 0 or 1 | 0 |
| 257 | `FLAT_VERTEX` | 0 or 1 | 0 |
| 258 | `TWO_SIDED_LIGHTING` | 0 or 1 | 0 |
| 259 | `FOG_BY_W` | 0 or 1 | 0 |

- `ZENABLE` has no value for a W buffer.
- `SHADEMODE` is 1 flat or 2 Gouraud.
- `COLORKEYENABLE` (Direct3D 7's) drops a fragment where the texel
  nearest its coordinate in a stage's texture, in the level the sample
  reads first, has the texture's colour key. The texels around it are
  filtered with their own colours, the key's included.
- `PIXEL_CENTRE`: 0 puts pixel centres at whole coordinates (Direct3D
  7 and 8), 1 at half ones (OpenGL).
- `FLAT_VERTEX`: the vertex that gives a flat-shaded primitive its
  colour, 0 the first (Direct3D), 1 the last (OpenGL).
- `TWO_SIDED_LIGHTING`: 1 lights the back of a triangle with its
  normal turned round (OpenGL).
- `FOG_BY_W`: the distance table fog goes by, 0 a fragment's depth
  and 1 its w.

#### Stage states

Direct3D 8's `D3DTSS` numbers and values. 32 and 33 are the card's own.
The reset values are stage 0's; in stages 1 to 7 `COLOROP` and
`ALPHAOP` reset to 1 (disabled), and `TEXCOORDINDEX` to the stage's
number.

| Number | Name | Values | Reset |
|---|---|---|---|
| 1 | `COLOROP` | a stage operation | 4 |
| 2 | `COLORARG1` | a stage argument | 2 |
| 3 | `COLORARG2` | a stage argument | 1 |
| 4 | `ALPHAOP` | a stage operation | 2 |
| 5 | `ALPHAARG1` | a stage argument | 2 |
| 6 | `ALPHAARG2` | a stage argument | 1 |
| 7 | `BUMPENVMAT00` | a number | 0.0 |
| 8 | `BUMPENVMAT01` | a number | 0.0 |
| 9 | `BUMPENVMAT10` | a number | 0.0 |
| 10 | `BUMPENVMAT11` | a number | 0.0 |
| 11 | `TEXCOORDINDEX` | a set and a generation | 0 |
| 13 | `ADDRESSU` | 1 to 4 | 1 |
| 14 | `ADDRESSV` | 1 to 4 | 1 |
| 15 | `BORDERCOLOR` | any | 0 |
| 16 | `MAGFILTER` | 1 to 3 | 1 |
| 17 | `MINFILTER` | 1 to 3 | 1 |
| 18 | `MIPFILTER` | 0 to 2 | 0 |
| 19 | `MIPMAPLODBIAS` | a number | 0.0 |
| 20 | `MAXMIPLEVEL` | 0 to 11 | 0 |
| 21 | `MAXANISOTROPY` | 1 to 8 | 1 |
| 22 | `BUMPENVLSCALE` | a number | 0.0 |
| 23 | `BUMPENVLOFFSET` | a number | 0.0 |
| 24 | `TEXTURETRANSFORMFLAGS` | 0 to 4, with 256 | 0 |
| 26 | `COLORARG0` | a stage argument | 1 |
| 27 | `ALPHAARG0` | a stage argument | 1 |
| 28 | `RESULTARG` | 1 or 5 | 1 |
| 32 | `COLOR_SCALE` | 1, 2 or 4 | 1 |
| 33 | `ALPHA_SCALE` | 1, 2 or 4 | 1 |

- A stage operation is 1 to 26 but 17: `DISABLE`, `SELECTARG1`,
  `SELECTARG2`, `MODULATE`, `MODULATE2X`, `MODULATE4X`, `ADD`,
  `ADDSIGNED`, `ADDSIGNED2X`, `SUBTRACT`, `ADDSMOOTH`,
  `BLENDDIFFUSEALPHA`, `BLENDTEXTUREALPHA`, `BLENDFACTORALPHA`,
  `BLENDTEXTUREALPHAPM`, `BLENDCURRENTALPHA`, (17, `PREMODULATE`, which
  the card lacks), `MODULATEALPHA_ADDCOLOR`, `MODULATECOLOR_ADDALPHA`,
  `MODULATEINVALPHA_ADDCOLOR`, `MODULATEINVCOLOR_ADDALPHA`,
  `BUMPENVMAP`, `BUMPENVMAPLUMINANCE`, `DOTPRODUCT3`, `MULTIPLYADD`,
  `LERP`.
- A stage argument has its source in bits 0 to 3 (0 `DIFFUSE`, 1
  `CURRENT`, 2 `TEXTURE`, 3 `TFACTOR`, 4 `SPECULAR`, 5 `TEMP`), bit 4
  to take its complement and bit 5 to replicate its alpha to the colour
  channels. The two bits give OpenGL's four operands of
  `ARB_texture_env_combine`.
- `RESULTARG` is 1 (`CURRENT`) or 5 (`TEMP`).
- `TEXCOORDINDEX` has the coordinate set, 0 to 7, in bits 0 to 15, and
  in bits 16 to 31 what the coordinates are: 0 the vertex's set, 1 the
  normal, 2 the position, 3 the reflection vector, all three in camera
  space, and 4 a sphere map's.
- `TEXTURETRANSFORMFLAGS` is the count of coordinates the stage's
  texture transform gives, 0 for no transform, plus 256 to divide them
  by the last.
- `COLOR_SCALE` and `ALPHA_SCALE` multiply the stage's results by 1, 2
  or 4 before they are clamped.

## Open points

- **Blank lines.** `MODE_HEIGHT` / 16 + 1 is a round figure near what
  monitor timings of the time used (480 lines in 525, 768 in 806). A
  game that measures the retrace's length could ask for another. A
  change would be a register for the blank's lines at a reserved
  offset, whose reset value gives today's figure.
- **Stretches.** `BLT` takes the source pixel under each destination
  pixel's centre. DirectDraw's code in the built-in Windows takes the
  one under its top left corner, so that driver leaves stretches to
  that code and sends the card only what both do alike: fills, and
  copies of equal size. Which of the two a Windows 98 driver's
  DirectDraw HAL has to match, and whether level 1 gives `BLT` a flag
  for the other, is settled with a comparison against DirectDraw on a
  real Windows 98.
- **Gamma.** The card has no gamma ramp, so DirectDraw's gamma control
  does nothing on it. A ramp would be a table of registers beside
  `PALETTE`, added with the level whose games ask for it.
