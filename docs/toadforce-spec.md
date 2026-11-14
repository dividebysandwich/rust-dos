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

## What is frozen

| Chapter | State |
|---|---|
| 1 to 8 | frozen: implemented in `src/toadforce` and `src/bus/toadforce.rs` |
| 9, level 0 | frozen, with them |
| 9, level 1 | provisional: implemented in `src/toadforce/level1` and drawn by `src/toadforce/raster`. A machine's card offers the level for the emulator's own drivers, and it may change before it is released |

There is no level above 1.

A frozen chapter changes only by adding what an earlier reader can't
have relied on: a new register at a reserved offset, a new command, a
new level.

The card's contract is with the games that exist. Nobody writes new programs
for it, so a rule here is there for the emulator's drivers and Direct3D
front ends to rely on, and where it differs from what Direct3D does the
specification says so and why. Level 1 follows measured Direct3D where a
game shows the difference or where following costs nothing; where following
would cost time for every vertex or pixel and no known game depends on it,
the card keeps the cheaper rule. "Where the card differs from Direct3D" in
chapter 9 lists the differences.

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
It has no gamma ramp.

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
  division rounded down), a round figure near the monitor timings of
  the time (480 lines in 525, 768 in 806),
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
  source rectangle before any pixel is written. DirectDraw's code in
  the built-in Windows takes the pixel under the top left corner, so
  that driver sends the card no stretches.
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
section is provisional, and may change before the level is released.
`LEVEL_MAX` reports the level before then, for the drivers that are
part of the emulator and change with it. A driver in a guest that uses
a provisional level may find it changed by a later version of the
emulator.

A number in an argument is an IEEE single-precision value in its word.
One that isn't finite is rejected with `BAD_VALUE`, and so is any
argument outside the values its table gives: a slot, a kind or a flag
word with a bit the table doesn't define. A colour of four numbers is
red, green, blue, alpha. A colour in one word has alpha in bits 24 to
31, red in 16 to 23, green in 8 to 15 and blue in 0 to 7.

The rule for numbers is for a command's own arguments. The numbers of
a vertex, in a draw command or in card memory, are taken as they are:
"Transform and lighting" and "Drawing" say what one that isn't finite
does.

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

The card has one material, which lights the back of a triangle too
under `TWO_SIDED_LIGHTING`.

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
  words as its fields take. A code is rejected with `BAD_VALUE` when
  it has bit 0 or one of bits 12 to 15 set, a position that is none of
  those the next rule names, more than eight coordinate sets, or a
  size other than 0 for a set past its count.
- A layout whose position is x, y, z and 1 / w has vertices in
  screen space, which "Drawing" takes as they are. The other
  positions (x, y, z, and x, y, z with one to three weights) and a
  normal are for the card to transform and light, as "Transform and
  lighting" says. A layout with a position in screen space and a
  normal is rejected with `BAD_VALUE`.
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

#### Transform and lighting

This section defines what the card makes of a vertex whose layout
has no 1 / w: a vertex in screen space, with its colours and texture
coordinates, which "Drawing" then draws. A vector is a row, and a
matrix of `SET_TRANSFORM` is 16 numbers, row by row: a vector times a
matrix is the vector's numbers times the matrix's columns. A position
has a fourth number of 1.

**Spaces.** The position times the world matrix is in world space, that
times the view matrix in camera space, and that times the projection matrix
in clip space, as x, y, z and w. With `VERTEXBLEND` n above 0 and a layout
with weights, the first n weights of the vertex (or all it has, if fewer)
and what they leave of 1 weigh the position through world matrices 0 to n,
one each, and the normal likewise. A normal goes to camera space by the
inverse of the world matrix times the view matrix, transposed, without the
row and column of the translation. Where those three rows and columns have
no inverse (a determinant below 1e-30 in size, or none that is a number)
every normal is (0, 0, 0), which no light lights. Direct3D goes on with the
inverse of the last such matrix it could invert, and gives zeros only on a
device that never had one; the card gives zeros every time, so that what a
draw shows doesn't depend on the draws before it. `NORMALIZENORMALS` 1
brings the normal to a length of 1 in camera space; a normal of length 0
stays. A layout without a normal has one of 0.

**The screen.** With the viewport's left, top, width, height and
depths:

- x on the screen = left + (x / w + 1) * width / 2
- y on the screen = top + (1 - y / w) * height / 2
- depth = near + z / w * (far - near)
- 1 / w is the vertex's.

**Clipping.** A primitive is cut where it crosses one of these, and what is
outside isn't drawn: w below 1/65536; z below 0; z above w; x or y on the
screen further than 16384 pixels from the render target's top left. These
are cut at with `CLIPPING` 0 as well: the state changes nothing but the clip
planes. (Measured on Direct3D 7, 8 and 9 with hardware vertex processing,
where the state changes no pixel. Direct3D's own vertex pipeline, which a
card that doesn't transform draws through, projects every vertex with the
state at 0.) With `CLIPPING` 1 a primitive is also cut at each clip plane
whose bit `CLIPPLANEENABLE` has: a plane (a, b, c, d) keeps the points whose
world-space position (x, y, z, w) has a * x + b * y + c * z + d * w of 0 or
more. Where an edge crosses, every value of the vertex is taken from the
straight line between its ends in clip space. A point is dropped whole by
its position. What is cut from a triangle is a convex polygon: filled, it
covers the pixels the triangle did inside the planes. With `FILLMODE` 2 the
parts of the triangle's own edges that are inside are drawn, and no line
along a plane; with 1 the triangle's own corners that are inside.

**Lighting.** With `LIGHTING` 0 the diffuse colour is the vertex's,
or white, and the specular colour the vertex's, or black with alpha
255. With 1 they are worked out in camera space, each channel of 0 to
1, and the vertex's own colours are material colours at most:

- diffuse = emissive + ambient * (`AMBIENT` + the sum of each
  light's ambient colour * f) + diffuse * the sum of each light's
  diffuse colour * (N . L) * f, with the alpha of the surface's
  diffuse colour
- specular = specular * the sum of each light's specular colour *
  (N . H)^power * f, with alpha 255. With `SPECULARENABLE` 0 it is
  black with alpha 255.

Emissive, ambient, diffuse and specular are the material's colours, or with
`COLORVERTEX` 1 the vertex's diffuse (1) or specular (2) colour where the
state `EMISSIVEMATERIALSOURCE`, `AMBIENTMATERIALSOURCE`,
`DIFFUSEMATERIALSOURCE` or `SPECULARMATERIALSOURCE` names one the layout
has. The sums are over the lights that are on. N is the normal. L points
from the vertex to the light, with a length of 1: for a directional light
the opposite of its direction, taken to camera space by the view matrix. H
is L plus the direction to the eye, brought to a length of 1; the direction
to the eye is from the vertex to the origin with `LOCALVIEWER` 1 and (0, 0,
-1) with 0. A light adds diffuse and specular light only where N . L is
above 0, and specular light only where N . H is above 0 too. A normal of (0,
0, 0) therefore takes a light's ambient colour and nothing else. An N . H
above 1, which a normal longer than 1 gives, counts as 1.

A power x^y here, of a specular highlight or a spot's falloff, has an x
above 0. It is 1 for a y of 0 and for an x of 1 and above, x^y for a y above
0, and for a y below 0 it is 1 / x^-y, which is above 1, and no more than
10^18. So a material power of 0 gives the whole highlight wherever N . H is
above 0, and one below 0 a highlight that grows away from the mirror's
direction, as Direct3D 8 and 9 give. Direct3D 7 gives no highlight for a
power of 0.001 or less: the card has Direct3D 8's rule, and Direct3D 7's
device turns `SPECULARENABLE` off for a lit draw with such a material, which
gives its vertices the black specular colour they have there. Direct3D's own
pipelines also drop a highlight whose (N . H)^power is below about 0.001.
The card doesn't: at a light and a material of 1 such a highlight is a
quarter of one step of a channel, and leaving it out makes the colour jump
where the card's changes evenly.

A point or spot light adds nothing, its ambient colour included, unless the
vertex is nearer to it than its range: d below the range, with d the
distance from the vertex to the light's position (taken to camera space by
the view matrix). A vertex at the light's own position has no direction to
it: it gets the light's ambient colour times f and nothing else. Its f is 1
/ (a0 + a1 * d + a2 * d * d), with a0 to a2 its attenuation; with a sum that
isn't above 0 the light adds nothing. A directional light's f is 1. A spot
light's f is multiplied by its cone: with rho the cosine of the angle
between its direction and the line from it to the vertex, 0 where rho is at
most cos(phi / 2); otherwise with spot model 0, 1 where rho is above
cos(theta / 2) and ((rho - cos(phi / 2)) / (cos(theta / 2) - cos(phi /
2)))^falloff between; with spot model 1, rho^falloff. A falloff of 0 makes
the cone 1 wherever rho is above cos(phi / 2), and one below 0 makes it
larger than 1 between the two cosines, where the light is brighter than its
colours. f is no more than 10^18.

`SET_LIGHT` takes any finite numbers, and these formulas say what each
lights, as Direct3D 8's do (Direct3D 7 refuses a light with theta above phi,
an angle below 0, a phi above pi, a range below 0 or above the square root
of the largest float, or a direction of no length, and its device refuses
them before the card sees them). Every vertex is further than a range of 0
or below, so such a light adds nothing. With theta above phi the cone is 1
wherever rho is above cos(phi / 2). The cosines are those of the angles as
they are, however large, so an angle below 0 is a cone of its size. A
direction is brought to a length of 1, and one whose length is 0 as a float
has it (the square root of the sum of the squares, which 1e-25 doesn't
reach) is no direction: a directional light with one adds its ambient colour
only, and a spot with one has a rho of 0, so it adds nothing with a phi up
to pi and lights by its cone's formula with a larger one. A light whose f
isn't a number above 0 adds nothing.

The cosines, e^x and the powers are worked out with additions,
multiplications and divisions alone, in a fixed order, so a vertex
has the same colours on every host. They are within 2 / 1000000 of
the cosine, within a millionth part of e^x and within a thousandth
part of the power.

With `TWO_SIDED_LIGHTING` 1 or 2 a triangle that shows its back has
the colours the vertices get with their normals turned round. The
back shows where the triangle is counterclockwise on the screen with
1, and clockwise with 2; a triangle with no area on the screen shows
its front. A triangle that `FILLMODE` draws as lines or points has
the colours of the side it shows. The primitives that are lines and
points have the front's colours.

Each channel is then limited to 0 to 1 and becomes c * 255 rounded to
the nearest whole number. A channel that is no number becomes 0.

**Fog.** With `FOGENABLE` 1, `FOGTABLEMODE` 0 and `FOGVERTEXMODE` not 0, the
specular colour's alpha becomes the fog factor of the vertex's distance d, 1
leaving all of the colour and 0 none. For mode 3 it is 1 where d is below
the start; otherwise 0 where d is the end or more; otherwise (end - d) /
(end - start). The questions are asked in that order, so fog that starts
where it ends, or after, leaves what is nearer than its start and fogs the
rest whole (measured on Direct3D 7's and 8's vertex pipelines). For mode 1
it is e^(-density * d) and for 2 e^(-(density * d)^2), limited to 0 to 1. d
is the size of the camera-space z, or with `RANGEFOGENABLE` 1 the distance
from the origin there. The size of z is Direct3D 8's rule; Direct3D 7 takes
z as it is, so what is behind the eye is unfogged there. In mode 1 a density
times d below 0 gives 1, where Direct3D lets the factor pass 1 and keeps the
lowest byte of the result, which fogs by a number that has nothing to do
with the distance. A distance that is no number gives 1, and so does a
factor that is none.

Without vertex fog the specular alpha is the vertex's own, and 255 for a lit
vertex and for one whose layout has no specular colour, so a draw with
`FOGENABLE` 1 and neither fog mode is fogged by what the program put there.
(Direct3D 7's `ProcessVertices` writes a lit vertex's specular colour as 0,
alpha included, with `SPECULARENABLE` 0. The card's lit vertices have 255
with either value of that state, as Direct3D 8's have with it on.) With
`FOGTABLEMODE` not 0 the vertex's factor isn't used, so the card doesn't
work it out, where Direct3D writes it into the vertex all the same.

**Texture coordinates.** The upper half of a stage's `TEXCOORDINDEX` says
what its coordinates are: 0 the vertex's set, 1 the normal in camera space,
2 the position there, 3 the reflection R = I - 2 * (N . I) * N, and 4 a
sphere map's: (R.x / m + 1/2, R.y / m + 1/2) with m = 2 * the length of
(R.x, R.y, R.z - 1), and (1/2, 1/2) where m is 0. I is the line of sight:
the position brought to a length of 1 with `LOCALVIEWER` 1, and (0, 0, 1)
with 0, with or without `LIGHTING`. N is the normal with the length it has,
1 only with `NORMALIZENORMALS`, and a normal of no length gives R = I. With
a count in `TEXTURETRANSFORMFLAGS` above 0 the coordinates are multiplied by
the stage's texture matrix, after a 1 is put behind the last of them when
they are fewer than four and zeros behind that (a set of two numbers is (u,
v, 1, 0), and what is generated is three numbers and a 1), and the first
count numbers of the result are the stage's coordinates; those not counted
are 0, and the fourth 1. Without a count the coordinates are passed on:
those the stage hasn't got are 0, but the fourth, which is then the last
number the stage has, and 1 for a stage with no numbers at all (the set of a
layout that lacks it). "Textures" says which of them a stage with 256 in
`TEXTURETRANSFORMFLAGS` divides the others by.

**Point sizes.** A vertex's size is the layout's, or `POINTSIZE`.
With `POINTSCALEENABLE` 1 it is multiplied by the viewport's height
times the square root of 1 / (A + B * d + C * d * d), with d the
vertex's distance from the origin of camera space and A to C the
states `POINTSCALE_A` to `_C`; a sum that isn't above 0 leaves it.

#### Drawing

This section defines the pixels a draw writes. The software renderer
defines them: it gives these pixels bit for bit on every host. The
OpenGL renderer gives the same pixels but where a host GPU's
arithmetic differs from the card's, and "Where the OpenGL renderer may
differ" in `docs/toadforce.md` states the bound of each such
difference.

**Vertices.** A field the layout lacks has its default: the diffuse colour
is white with alpha 255, the specular colour is 0, and a texture coordinate
a set lacks is 0, but the fourth, which is the set's last number, or 1 for a
set the layout hasn't got. A 1 / w that is 0, negative or no finite number
counts as 1. Positions are in pixels from the render target's top left. With
`PIXEL_CENTRE` 0 the centre of the top left pixel is at (0, 0), and with 1
at (0.5, 0.5). A position is limited to 32768 pixels either side of 0 and
rounded to 1/256 of a pixel. A primitive with a position that is no finite
number is not drawn.

**Primitives.** A strip's triangle i (from 0) has the vertices i, i + 1, i +
2, and every odd one has the first two the other way round, so all wind
alike. A fan's triangle i has the vertices i + 1, i + 2 and 0. With
`SHADEMODE` flat a primitive has one vertex's colours all over: the first of
its vertices in the draw's order, which is vertex i for a strip's triangle
i, vertex i + 1 for a fan's, and a line's first end, as Direct3D's
rasterisers have it (measured on Direct3D 7's and 9's); or the last with
`FLAT_VERTEX` 1. It is that vertex's diffuse colour, and the red, green and
blue of its specular colour. The lines and the points `FILLMODE` makes of a
flat triangle have the triangle's colours too. The specular colour's alpha,
which is the fog factor of a vertex, stays each vertex's own and is
interpolated, as Direct3D has it ("the specular alpha is interpolated").

**Wrapping.** `WRAP0` to `WRAP7` are for the coordinate sets 0 to 7, and bit
n of one is for coordinate n of its set (bit 0 u, bit 1 v). A line or a
triangle has a base vertex: the highest on the screen, and of two that high
the one further left; of vertices at one place, the first in the primitive.
A coordinate with its bit set is compared with the base's at each other
vertex: where it is 0.5 or more above the base's, 1 is taken from it, and
where it is 0.5 or more below, 1 is added. This is Direct3D's texture
wrapping, which takes 0 and 1 for the same place, as its software
rasterisers do it (measured on Direct3D 7's and on Direct3D 9's reference
rasteriser, whose rule for two vertices of one height this is). A difference
of exactly 0.5 moves, and a coordinate moves by 1 once: 2.25 beside a base
of 0 becomes 1.25. The two vertices that aren't the base are each brought
near it and not near each other, so the edge between them can go the longer
way round. It is done before a primitive is clipped and before its
coordinates are interpolated. A stage of a vertex the card transforms wraps
what "Texture coordinates" makes of the set its `TEXCOORDINDEX` names, by
that set's state, and takes the vertices' places on the screen before any
plane cuts the primitive; a vertex that isn't in front of the eye (a w below
1/65536) is the base only when none of the primitive's is. Points aren't
wrapped. Direct3D's rasterisers were measured on triangles; the card does
the same to a line's two ends.

**Triangles.** A triangle with no area is not drawn, and neither is one
`CULLMODE` culls: clockwise on the screen with 2, counterclockwise with 3. A
triangle covers the pixels whose centres are inside it. A centre on an edge
is covered when the edge is the triangle's top (horizontal, with the
triangle below it) or on its left, so two triangles that share an edge cover
each of its pixels once. With `FILLMODE` 2 a triangle that isn't culled is
drawn as the three lines of its edges, and with 1 as its three vertices'
points; a culled triangle has neither. (Direct3D 9's reference rasteriser
culls both so; Direct3D 7's software rasteriser culls the lines and draws
the points of a culled triangle.)

**Lines.** A line has one pixel in each column it crosses when it is
at least as long along x as along y, and one in each row otherwise:
the pixel whose centre is nearest to the line there, and of two
equally near the lower or the right one. The columns (or rows) are
those whose centres lie between the two ends. A centre exactly at the
first end counts. A centre exactly at the last end counts with
`LASTPIXEL` 1 and doesn't with 0, so a line that ends between two
centres has the same pixels with either. A line of no length is the
pixel whose centre is nearest to it with `LASTPIXEL` 1, and nothing
with 0.

**Points.** A point is a square around its position with sides of the
vertex's point size, or `POINTSIZE` where the layout has none,
limited to `POINTSIZE_MIN` and `POINTSIZE_MAX`. Each of the three
states counts as no less than 0 and no more than `CAPS[19]`, and
with a minimum above the maximum the size is the minimum. It covers
pixels as two triangles would. With `POINTSPRITEENABLE` every
coordinate set
goes from (0, 0) at the square's top left to (1, 1) at its bottom
right.

**Clipping.** Pixels outside the viewport's rectangle or the render
target are not drawn, nor those outside the scissor rectangle when
`SCISSORENABLE` is 1.

**Values at a pixel.** Colours, texture coordinates and fog are interpolated
to the pixel's centre with perspective: each value times 1 / w, and 1 / w
itself, change linearly over the screen, and the value is their quotient.
Depth changes linearly over the screen. A vertex in screen space may have a
depth outside 0 to 1, and nothing is cut by it: with the depth test on
(`ZENABLE` 1 and a depth buffer) a pixel whose depth is outside 0 to 1 isn't
drawn and changes neither depth nor stencil, and with the test off it is
drawn, its depth counting as the end of the range it is past wherever a
depth is used. (Measured on Direct3D 7, 8 and 9 cards with a program's
transformed vertices, with `CLIPPING` 1 and 0.) A screen vertex's 1 / w that
is 0, negative or no number counts as 1, so its primitive is drawn. A colour
channel is rounded to the nearest of 0 to 255. Along a line the values
change with the place along it and not across it: a pixel has those of the
point of the line nearest to its centre, which for a pixel past an end is a
point of the line continued. A point has its vertex's values at every pixel,
but for a point sprite's coordinates.

**The order of a fragment's steps.**

1. The depth and stencil tests, when no later step can drop the
   fragment; otherwise they come after step 5, so a dropped fragment
   leaves the depth and stencil as they were.
2. The stages, from 0 up to the first whose `COLOROP` is 1, as
   "Stages" says.
3. With `SPECULARENABLE`, the specular colour is added to the red,
   green and blue, each sum limited to 255.
4. With `FOGENABLE`, the red, green and blue are mixed with `FOGCOLOR` by
   the fog factor f, 0 to 255: (c * f + fog * (255 - f) + 127) / 255. With
   `FOGTABLEMODE` 0 the factor is the specular colour's alpha. Otherwise it
   comes from the fragment's depth d, or its w with `FOG_BY_W` 1, by the
   formulas of "Fog" above with the table's mode: for mode 3, 1 where d is
   below the start, otherwise 0 where d is the end or more, otherwise (end -
   d) / (end - start); e^(-density * d) for 1, e^(-(density * d)^2) for 2;
   that number is limited to 0 to 1, and f is it times 255, rounded to the
   nearest whole number. In mode 1 a density times d below 0 gives 1. With
   `FOGTABLEMODE` and `FOGVERTEXMODE` both 0, a lit vertex's specular alpha
   is 255, so nothing is fogged.
5. The colour key and the alpha test. With `COLORKEYENABLE`, a
   fragment is dropped when a texture with a colour key that a stage
   samples has the key at the texel nearest to the fragment's
   coordinates. With `ALPHATESTENABLE`, it is dropped unless its
   alpha against `ALPHAREF` passes `ALPHAFUNC`.
6. With `DITHERENABLE` and an `R5G6B5` render target, the dither. The
   matrix below gives d for the pixel, by x and y modulo 4. Red and
   blue become (c + d / 2) / 8 and green (c + d / 4) / 4, each
   division rounded down and the result limited to the channel's
   bits. The channel goes on as those bits widened to 8 by repeating
   them from the top. Step 8's rounding gives the same bits back, so
   without a blend they are what is written; with one, the blend
   takes the widened channel, and what is written is the blend's
   result as step 8 rounds it.

   | y \ x | 0 | 1 | 2 | 3 |
   |---|---|---|---|---|
   | 0 | 0 | 8 | 2 | 10 |
   | 1 | 12 | 4 | 14 | 6 |
   | 2 | 3 | 11 | 1 | 9 |
   | 3 | 15 | 7 | 13 | 5 |
7. With `ALPHABLENDENABLE`, the blend with the pixel there, as
   "Blending" says.
8. The write of the channels `COLORWRITEENABLE` names; the others
   keep the pixel's. A channel goes to a 16-bit target rounded to
   the nearest value the target holds: (c * 31 + 127) / 255 for five
   bits, (c * 63 + 127) / 255 for six. An `X8R8G8B8` target's top
   byte is written 0.

**Stages.** A stage works on two values, `CURRENT`, which starts as
the diffuse colour, and `TEMP`, which starts as 0. Its `COLOROP`
gives the red, green and blue of its result and its `ALPHAOP` the
alpha; the result goes to `CURRENT`, or to `TEMP` with `RESULTARG`
5. Both operations read the values as they were before the stage. An
argument is the colour its source has. With
bit 4 each channel c of it becomes 255 - c, and with bit 5 every
channel then becomes the alpha. In the table a, b and c are a
channel of arguments 1, 2 and 0, each 0 to 255, m(x, y) is (x * y +
127) / 255, and every division is rounded down:

| Operation | Result |
|---|---|
| 2 `SELECTARG1` | a |
| 3 `SELECTARG2` | b |
| 4 `MODULATE` | m(a, b) |
| 5 `MODULATE2X` | 2 * m(a, b) |
| 6 `MODULATE4X` | 4 * m(a, b) |
| 7 `ADD` | a + b |
| 8 `ADDSIGNED` | a + b - 128 |
| 9 `ADDSIGNED2X` | 2 * (a + b - 128) |
| 10 `SUBTRACT` | a - b |
| 11 `ADDSMOOTH` | a + b - m(a, b) |
| 12 `BLENDDIFFUSEALPHA`, 13 `BLENDTEXTUREALPHA`, 14 `BLENDFACTORALPHA`, 16 `BLENDCURRENTALPHA` | (a * t + b * (255 - t) + 127) / 255, with t the alpha of the diffuse colour, of the stage's texture, of `TEXTUREFACTOR` or of `CURRENT` |
| 15 `BLENDTEXTUREALPHAPM` | a + m(b, 255 - t), with t the alpha of the stage's texture |
| 18 `MODULATEALPHA_ADDCOLOR` | a + m(t, b) |
| 19 `MODULATECOLOR_ADDALPHA` | m(a, b) + t |
| 20 `MODULATEINVALPHA_ADDCOLOR` | m(255 - t, b) + a |
| 21 `MODULATEINVCOLOR_ADDALPHA` | m(255 - a, b) + t |
| 25 `MULTIPLYADD` | c + m(a, b) |
| 26 `LERP` | (c * a + (255 - c) * b + 127) / 255 |

In 18 to 21, t is the alpha of argument 1, complemented with its bit
4. The result is multiplied by `COLOR_SCALE` or `ALPHA_SCALE` and
limited to 0 to 255.

- `DOTPRODUCT3` (24) as a colour operation: with s the sum over red,
  green and blue of (a - 128) * (b - 128), the result is 0 where s
  is 0 or below and (4 * s + 127) / 255 otherwise, limited to 255,
  times `COLOR_SCALE` and limited to 255 again. It goes to all four
  channels, and the stage's alpha operation isn't worked out.
- 18 to 21 and 24 mix alpha into colour. As an alpha operation each
  gives argument 1, times `ALPHA_SCALE`.
- An `ALPHAOP` of 1 leaves the alpha as it was, and so does one of
  22 or 23.
- A stage samples its texture where an operation of it reads
  `TEXTURE`: as an argument the operation uses (`SELECTARG1` uses
  argument 1 alone, `SELECTARG2` argument 2, 25 and 26 all three,
  the others 1 and 2), or as the alpha of 13 and 15. A stage none of
  whose operations does samples nothing, and its texture's colour
  key drops nothing.
- An operation that reads `TEXTURE` in a stage that has no texture
  gives `CURRENT` as it is, without the scale. With `RESULTARG` 5
  that copy goes to `TEMP`.

**Bump mapping.** A stage whose `COLOROP` is `BUMPENVMAP` (22) or
`BUMPENVMAPLUMINANCE` (23) changes neither `CURRENT` nor `TEMP`, and
its alpha operation isn't worked out. It samples its texture as any
stage does, and the sample's red and green bytes, read as signed
numbers and divided by 128, are du and dv. With the stage's
`BUMPENVMAT00` to `BUMPENVMAT11`, the stage after it samples its
texture at coordinates moved by (du * mat00 + dv * mat10, du * mat01
+ dv * mat11): the offsets are added to the two coordinates once
they are divided, on a cube map to the coordinates on the face, and
the level of detail is that of the coordinates unmoved. The colour
key is looked up at the moved coordinates. With 23, the red, green
and blue of that sample are multiplied by l = the bump sample's blue
byte / 255 * `BUMPENVLSCALE` + `BUMPENVLOFFSET`, limited to 0 to 1:
each becomes c * l + 0.5, rounded down. The stage after is the next
in number. Where that stage samples no texture, or is itself a bump
stage, the offsets are dropped. A bump stage without a texture does
nothing.

**Blending.** The fragment s and the pixel d, each channel 0 to 255,
with a factor for each, fs from `SRCBLEND` and fd from `DESTBLEND`,
each 0 to 255 for each channel:

| `BLENDOP` | Result |
|---|---|
| 1 | (s * fs + d * fd + 127) / 255 |
| 2 | (s * fs - d * fd + 127) / 255 |
| 3 | (d * fd - s * fs + 127) / 255 |
| 4 | the smaller of s and d |
| 5 | the larger of s and d |

Each division is rounded down, below 0 too, and the result limited to
0 to 255. 4 and 5 use no factor. The pixel is read from the target
with each channel of five or six bits widened to 8 by repeating its
bits from the top, and with an alpha of 255 where the target has
none.

| Factor | Value |
|---|---|
| 1 `ZERO` | 0 |
| 2 `ONE` | 255 |
| 3 `SRCCOLOR`, 4 `INVSRCCOLOR` | the fragment's channel, or 255 less it |
| 5 `SRCALPHA`, 6 `INVSRCALPHA` | the fragment's alpha, or 255 less it |
| 7 `DESTALPHA`, 8 `INVDESTALPHA` | the pixel's alpha, or 255 less it |
| 9 `DESTCOLOR`, 10 `INVDESTCOLOR` | the pixel's channel, or 255 less it |
| 11 `SRCALPHASAT` | the smaller of the fragment's alpha and 255 less the pixel's, and 255 for alpha |

A `SRCBLEND` of 12 (`BOTHSRCALPHA`) blends as 5 with a `DESTBLEND` of 6, and
13 (`BOTHINVSRCALPHA`) as 6 with 5, whatever `DESTBLEND` is. A `DESTBLEND`
of 12 or 13 beside any other `SRCBLEND` is 0, as `ZERO`: Direct3D has the
two for `SRCBLEND` only, and its software rasterisers leave nothing of the
pixel for one as the destination's factor (measured on Direct3D 7's and on
Direct3D 9's reference rasteriser).

**Depth and stencil.** Depth is kept as 16 bits in a `D16` buffer and 24
bits in the others, where it is in bits 8 to 31 of a pixel and the stencil
value in bits 0 to 7. A fragment's depth is z times the largest value,
rounded to nearest, less `ZBIAS`, and not below 0. (Direct3D 7's software
rasteriser cuts the fraction off, which puts a z of 0.5 one step nearer than
half the range; the nearest value is within half a step of z everywhere.)
The depth test compares it with the pixel's by `ZFUNC` and passes always
with `ZENABLE` 0 or no depth buffer; with the test on, a fragment whose z is
outside 0 to 1 is dropped before both tests ("Values at a pixel"). The
stencil test, with `STENCILENABLE` and a `D24S8` buffer, compares
`STENCILREF` with the pixel's stencil value by `STENCILFUNC`, both under
`STENCILMASK`. The stencil value then changes by `STENCILFAIL` when the
stencil test fails, `STENCILZFAIL` when it passes and the depth test fails,
and `STENCILPASS` when both pass, in the bits `STENCILWRITEMASK` names. A
fragment that passes both is drawn, and writes its depth with
`ZWRITEENABLE`.

**Textures.** A coordinate of 0 is a texture's left or top edge and 1 its
right or bottom edge, so a texel's centre is half a texel in. A stage's
coordinates at a pixel are the first two of its set, or with 256 in
`TEXTURETRANSFORMFLAGS` those two divided by the coordinate the flags' count
names as the last, at each pixel: the second with a count of 2, and with a
count of 0 the fourth, which is the last number the vertex has for the stage
("Texture coordinates"). This is what Direct3D 9's reference rasteriser does
with the vertices it transforms; for a vertex in screen space it divides by
the last number of the set whatever the count, where the card goes by the
count. (Direct3D 7's software rasteriser never divides.) A division by 0 is
a float's: a coordinate over 0 is past the end of the card's whole numbers
on its side and stops there, and 0 over 0 is no number, which is sampled as
0. The nearest filter (1) takes the texel a coordinate falls in. The linear
filter (2) mixes the four texels around it by the distances to their centres
in 1/256 of a texel.

The level of detail is half the base-2 logarithm of the larger of two
numbers: the squared length, in texels of level 0, of the step the
coordinates take for one pixel to the right, and for one pixel down.
`MIPMAPLODBIAS` is added. At or below 0, or where it is no number,
the `MAGFILTER` applies to the level `MAXMIPLEVEL` names; above, the
`MINFILTER`, on the level nearest to the level of detail with
`MIPFILTER` 1, on the two around it mixed with 2, and on the level
`MAXMIPLEVEL` names with 0. With `MIPFILTER` 1 or 2 the level of
detail is limited to `MAXMIPLEVEL` and the last level first; a
`MAXMIPLEVEL` past the last level is the last. The nearest level is
the level of detail plus 0.5, rounded down. Of two levels the first
is the level of detail rounded down, and the next has what is left
of it, in 1/256 rounded down, as its weight. The logarithm is worked
out as the cosines of "Transform and lighting" are, and is within
2 / 100000 of the true one.

A `MINFILTER` of 3 is the anisotropic filter: it filters as the
linear one, and with `MAXANISOTROPY` a the level of detail follows
the smaller of the two numbers, though not below the larger over a
squared. The levels are used as `MIPFILTER` says, as with the other
filters. A `MAGFILTER` of 3 is the linear filter.

A coordinate c becomes a whole number of 1/256 of a texel in a level n
texels wide or high: c * n * 256, worked out in double precision, limited to
2147483520 either side of 0, less 128 for the linear filter, rounded down. A
coordinate that is no number gives 0 for the whole of that. The number's
upper bits (it divided by 256, rounded down) are the texel the nearest
filter takes, and for the linear filter the first of the two it mixes along
that direction, the next being one further; its lowest 8 bits are the weight
f of the second. `ADDRESSU` and `ADDRESSV` bring each texel into the level:
1 by its number modulo n, 2 by mirroring every other repeat, 3 by taking the
nearest edge texel, and 4 by giving `BORDERCOLOR` for a texel outside. Two
colours a and b mix channel by channel to (a * (256 - f) + b * f + 128) /
256, rounded down. The linear filter mixes the two texels of the upper row
and the two of the lower row by the weight across, and then the two results
by the weight down. Two levels mix by the same formula with the next level's
weight.

A cube map is sampled by the first three coordinates of its stage, a
direction (x, y, z), divided as above when the stage projects. The
face is that of the coordinate largest in size, x before y and y
before z among equals, and its sign, 0 counting as positive. The
coordinates on the face are (u / m + 1) / 2 and (v / m + 1) / 2 with
m the size of that largest coordinate and:

| Face | u | v |
|---|---|---|
| +x | -z | -y |
| -x | z | -y |
| +y | x | z |
| -y | x | -z |
| +z | x | -y |
| -z | -x | -y |

A direction of length 0, or one with no number for its largest
coordinate, gives face +x at (0.5, 0.5). The face is then sampled as
a texture by itself, with the stage's address modes: a filter mixes
no texels of another face. The level of detail comes from the
squared length of the step all three coordinates take for a pixel,
times the square of the face's width over 2 * m.

**Texels.** A texel is four channels of 8 bits. A field of fewer bits
is widened by repeating its bits from the top, so its largest value
becomes 255, in `R5G6B5`, `X1R5G5B5`, `A1R5G5B5`, `A4R4G4B4` and the
luminance of `L6V5U5`. One bit of alpha is 0 or 255, and a format
without alpha has 255. `A8` is white with its alpha. `L8` and `A8L8`
have their luminance as red, green and blue. A `P8` texel is its
palette word, alpha included. The bump formats have du where red is,
dv where green is and their luminance where blue is, 255 in `V8U8`,
with alpha 255. The signed fields of five bits in `L6V5U5` are
shifted up by three bits and not repeated, so -16 becomes -128 and
15 becomes 120.

A block of `DXT1` to `DXT5` has two colours of 5-6-5 bits, c0 and
c1, widened as above to a and b, and 2 bits for each texel that pick
one of four colours: a, b, and

- (2 * a + b + 1) / 3 and (a + 2 * b + 1) / 3, each division rounded
  down, in `DXT2` to `DXT5`, and in `DXT1` where the word c0 is above
  c1;
- (a + b) / 2, rounded down, and black with alpha 0, in `DXT1`
  otherwise.

`DXT1`'s other texels have alpha 255. `DXT2` and `DXT3` have 4 bits
of alpha for each texel, times 17. `DXT4` and `DXT5` have two alphas
a0 and a1 and 3 bits for each texel that pick one of eight: a0, a1,
and with a0 above a1 the six ((7 - i) * a0 + i * a1 + 3) / 7 for i
from 1 to 6, otherwise the four ((5 - i) * a0 + i * a1 + 2) / 5 for i
from 1 to 4, then 0 and 255. The colours of `DXT2` and `DXT4` are
taken as they are stored.

**Order between draws.** A pixel sees the primitives in the order
their commands came, and within a command in the order of its
vertices. A draw that samples a texture whose memory overlaps its
render target's, or whose colour and depth buffers overlap, reads
what the primitives before it wrote. Within one primitive it reads
what the primitive wrote to the rows above the pixel; what it reads
of the pixel's own row is not defined.

#### Where the card differs from Direct3D

Each rule is stated where its subject is above; these are the ones that
differ from what Direct3D was measured or documented to do, with the reason.

- A vertex at a point or spot light's own position gets the light's ambient
  colour. Direct3D adds nothing there. No known game depends on it; matching
  costs a comparison for every vertex and light.
- A spot light with no direction and a phi above pi lights by its cone's
  formula. Direct3D 8 gives such a light nothing, and Direct3D 7 refuses it.
  No known game depends on it; matching costs a comparison for every vertex
  and spot light.
- A highlight whose (N . H)^power is below about 0.001 is kept. Direct3D's
  pipelines drop it, which makes the colour jump by a quarter of a step
  where the card's changes evenly.
- A material power of 0.001 or less gives the highlight of the formula, as
  on Direct3D 8 and 9. Direct3D 7 gives none, and its device turns
  `SPECULARENABLE` off for such a draw.
- `SET_LIGHT` takes any finite numbers. Direct3D 7 refuses some, and its
  device does before the card sees them.
- Vertex fog's distance is the size of the camera-space z, as on Direct3D 8.
  Direct3D 7 takes z as it is. Its device's draws go through the same card,
  and no known game has fogged vertices behind the eye.
- Exponential fog with density times distance below 0 gives a factor of 1.
  Direct3D lets the factor pass 1 and keeps its lowest byte, which no
  program can want.
- A lit vertex's specular alpha is 255 with either value of
  `SPECULARENABLE`. Direct3D 7's `ProcessVertices` writes 0 with the state
  off.
- A world-view matrix with no inverse gives every normal as zeros. Direct3D
  goes on with the inverse of the last matrix it could invert, which makes a
  draw depend on the draws before it.
- With `CLIPPING` 0 the card still cuts the vertices it transforms at the
  near and far planes, as Direct3D's cards with hardware vertex processing
  do. Direct3D's software vertex processing projects every vertex then, and
  so does the pipeline of the built-in Windows for the draws it transforms.
- A projected stage of a vertex in screen space divides by the coordinate
  the count names, as for the vertices the card transforms. Direct3D 9's
  reference rasteriser divides by the set's last number whatever the count.
  One rule for both kinds of vertex; no known game depends on it.
- A projected stage's coordinates over 0 are a float's quotient, sampled
  past the texture's end or as 0. Direct3D 9's reference rasteriser samples
  the first texel. No known game depends on it; matching costs a comparison
  for every pixel of a projected stage.
- A culled triangle draws no points with `FILLMODE` 1, as on Direct3D 9's
  reference rasteriser. Direct3D 7's software rasteriser draws them.
- A line's two ends are wrapped as a triangle's vertices are. Direct3D's
  rasterisers were measured on triangles only.
- `COLORKEYENABLE` under a linear filter drops by the nearest texel.
  Direct3D has no one rule there ("Render states").
- `ZBIAS` is in steps of the depth buffer. Direct3D gives the state no scale
  ("Render states").
- Depth is rounded to the nearest step. Direct3D 7's software rasteriser
  cuts the fraction off.

#### Render states

The numbers and values are Direct3D 8's `D3DRS` ones where Direct3D
has the state. 256 to 259 are the card's own.

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
| 258 | `TWO_SIDED_LIGHTING` | 0 to 2 | 0 |
| 259 | `FOG_BY_W` | 0 or 1 | 0 |

- `ZENABLE` has no value for a W buffer.
- `SHADEMODE` is 1 flat or 2 Gouraud.
- `COLORKEYENABLE` (Direct3D 7's) drops a fragment where the texel nearest
  its coordinate in a stage's texture, in the level the sample reads first,
  has the texture's colour key. The texels around it are filtered with their
  own colours, the key's included, so a linear filter mixes the key's colour
  into the texels beside a keyed one. Under the nearest filter this is what
  Direct3D does. Under a linear filter Direct3D has no one rule: its
  software rasteriser filters the key's colour in, as the card does, and
  removes pixels in a dither pattern as dense as the keyed texels' weight in
  the filter; its documented reference pipeline gives a keyed texel an alpha
  of 0 before the filter and leaves the dropping to the alpha test; and a
  GPU measured through its driver removes every pixel a keyed texel has any
  weight in. The card drops by the nearest texel, the one with the largest
  weight in the filter: the edge of what it drops is within a texel of where
  each of the three puts it, and it needs no dither and no alpha test.
- `SRCBLEND` and `DESTBLEND` take Direct3D's factors 1 to 13, and
  "Blending" says what each is.
- `ZBIAS` is in steps of the depth buffer: a fragment with a bias of n is n
  steps nearer. Direct3D gives the state no scale. Its software rasteriser
  ignores the state (measured, with 16 and with 24 bits of depth), and a GPU
  measured through its driver moved a fragment about one step of a 16-bit
  buffer for each step of the state from 2 up; what the cards of the time
  did isn't known here.
- `WRAP0` to `WRAP7` have bit 0 for a set's first coordinate to bit 3
  for its fourth ("Wrapping").
- `PIXEL_CENTRE`: 0 puts pixel centres at whole coordinates (Direct3D
  7 and 8), 1 at half ones (OpenGL).
- `FLAT_VERTEX`: the vertex that gives a flat-shaded primitive its
  colour, 0 the first (Direct3D), 1 the last (OpenGL).
- `TWO_SIDED_LIGHTING`: 1 or 2 lights the back of a triangle with
  its normal turned round (OpenGL). The back is the counterclockwise
  side with 1 and the clockwise side with 2.
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
- `TEXTURETRANSFORMFLAGS` is the count of coordinates the stage's texture
  transform gives, 0 for no transform, plus 256 to divide them by the last:
  the first two (three of a cube map) by the coordinate the count ends at,
  or with a count of 0 by the fourth, which is then the last number the
  vertex has for the stage.
- `MINFILTER` 3 is the anisotropic filter, and `MAGFILTER` 3 the
  linear one ("Textures").
- `COLOR_SCALE` and `ALPHA_SCALE` multiply the stage's results by 1, 2
  or 4 before they are clamped.
