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
| 1 to 8 | provisional: written before phase 1's code, corrected by the pull request that implements it, frozen when its tests pass |
| 9, level 0 | provisional, with phase 1 |
| 9, level 1 | headings only, written with phases 2, 3 and 5 |
| 9, level 2 | not started |

A frozen chapter changes only by adding what an earlier reader can't
have relied on: a new register at a reserved offset, a new command, a
new level. "Open points" at the end lists what phase 1 has to settle.

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
| 04h | command | bit 0 I/O space, bit 1 memory space, both writable; 0 after reset |
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

The three BARs decode while bit 1 of the command register is set. The
legacy VGA ports (3B0h to 3DFh) decode while bit 0 is set, and the VGA
memory at A0000h to BFFFFh while bit 1 is.

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
| 02Ch | `RESET` | W | | writing 1 resets the card (chapter 8) |
| 040h | `MODE_ENABLE` | R/W | 0 | 1: the card shows the mode below |
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
command of level 1 or above can set goes back to its default. A write of
a larger value is ignored, so a driver reads `LEVEL` back. Display
state (the `MODE` registers, `PALETTE`, a pending flip) is untouched.

`STATUS`, `FRAME` and `SCANLINE` follow the emulated clock (chapter 6).
There is no busy bit, because the card has finished every command when
the write to `RING_PUT` returns.

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

Level 0's count is 5.

## 4. Card memory (BAR 1)

`MEMORY` bytes, which is also BAR 1's size. The guest lays it out: the
card has no allocator and attaches no meaning to an offset until a
register or a command names it.

- A read returns what the last write or command put there. After a
  command that draws into a surface, a read of that surface returns the
  drawn pixels.
- A write is seen by every later command and by the screen. No command
  tells the card that memory changed.
- The first bytes of card memory are the frame buffer of the VBE modes
  (chapter 6), so a guest that uses both keeps its own data clear of
  what the VBE mode in use covers.

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
memory. The pitch needs no alignment.

| Code | Name | Pixel | Layout |
|---|---|---|---|
| 1 | `P8` | 1 byte | an index into a palette |
| 2 | `R5G6B5` | 2 bytes | red in bits 11 to 15, green 5 to 10, blue 0 to 4 |
| 3 | `X8R8G8B8` | 4 bytes | red in bits 16 to 23, green 8 to 15, blue 0 to 7; bits 24 to 31 unused |
| 4 | `A8R8G8B8` | 4 bytes | the same with alpha in bits 24 to 31 |

Level 1 adds codes for its texture and depth formats.

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
  ends exactly at the end, the guest writes 0 to `RING_PUT`.
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
command.

A header whose length is 0, or reaches past the commands submitted or
past the end of the ring, leaves no next command to find. The card
rejects it with `BAD_LENGTH` and drops the rest of what was submitted:
`RING_GET` becomes `RING_PUT`.

The first rejection while `ERROR` is 0 sets `ERROR` to its code,
`ERROR_AT` to the ring offset of the command's header (FFFFFFFFh for a
rejected register write), bit 2 of `STATUS` and bit 2 of `IRQ_STATUS`.
Later rejections change nothing until the guest writes to `ERROR`,
which clears `ERROR`, `ERROR_AT` and bit 2 of `STATUS`.

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

A correct driver never causes a rejection. The registers exist so that a
driver under development, and the emulator's log, can say what went
wrong.

## 6. Display

The card has two sources for the picture, and `MODE_ENABLE` picks one.

### `MODE_ENABLE` = 0: the VGA

The picture is what the VGA registers and the VBE mode describe, as on
the emulator's `Svga` adapter. This is the state after reset, and DOS
programs and a Windows without a driver never leave it.

A VBE mode with a linear frame buffer has its frame buffer at offset 0
of card memory. The video BIOS writes 0 to `MODE_ENABLE` in every mode
set, so a mode set through INT 10h always brings the VGA's picture back.

Bit 0 of `STATUS`, `SCANLINE` and `FRAME` follow the VGA's timing: bit 0
is set while port 3DAh reports vertical retrace, and `FRAME` counts the
retraces that began.

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
  `MODE_REFRESH` are kept and change nothing until the next write of 1
  to `MODE_ENABLE`.
- A write to `MODE_START` shows from the next frame, and cancels a
  pending flip. A value whose surface reaches outside card memory is
  rejected with `BAD_RANGE`.
- The VGA registers keep their contents and take writes, and none of it
  is shown.

A write of 0 brings the VGA's picture back as its registers describe it.
Card memory is unchanged by either write.

### Timing

With the card's mode shown, time is counted from the write that set it:

- a frame lasts 1 000 000 000 / `MODE_REFRESH` nanoseconds of the
  emulated clock, rounded down,
- it has `MODE_HEIGHT` + `MODE_HEIGHT` / 16 + 1 lines (the division
  rounded down) of equal length, the last one taking the remainder,
- `SCANLINE` is the line of the present moment, from 0,
- bit 0 of `STATUS` is set while `SCANLINE` is `MODE_HEIGHT` or more,
- when `SCANLINE` reaches `MODE_HEIGHT`, `FRAME` goes up by one, bit 0
  of `IRQ_STATUS` is set, and a pending flip takes place.

Port 3DAh reports the same vertical blank while the card's mode is
shown, so the machine has one display timing at a time.

### Flips

`FLIP` (chapter 9) changes `MODE_START` by command. Without its wait
flag the change is made at once and bit 1 of `IRQ_STATUS` is set. With
the flag, bit 1 of `STATUS` is set and `MODE_START` keeps reading the
old offset until the next vertical blank begins; then `MODE_START`
changes, bit 1 of `STATUS` clears and bit 1 of `IRQ_STATUS` is set.

The command itself never waits: the write to `RING_PUT` returns with
the flip pending. A driver that has to wait reads `STATUS`, or takes the
interrupt. A second `FLIP` while one is pending replaces it, and the
first one's surface is never shown.

## 7. Interrupt

INTA# is asserted while `IRQ_STATUS` and `IRQ_ENABLE` have a set bit in
common, and released when they don't. The bits of `IRQ_STATUS` are set
by their events whether enabled or not, and stay set until the guest
writes 1 to them.

| Bit | Set when |
|---|---|
| 0 | a vertical blank begins |
| 1 | a flip takes place |
| 2 | `ERROR` is latched |

## 8. Reset and saved states

Power-on: every register has its reset value, card memory and the ring
are zero, and the VGA is in the BIOS's start-up mode.

A reset of the machine, and a write of 1 to `RESET`, put every register
of chapter 3 at its reset value, `PALETTE` included, drop a pending
flip and return every command state to its default. Card memory, the
ring's contents and PCI configuration space keep what they had. `RESET`
leaves the VGA registers alone; a machine reset resets them as on any
VGA.

After `RESET`, a driver that left commands in the ring starts again at
offset 0, since `RING_PUT` and `RING_GET` are 0.

A saved state of the emulator holds everything this document describes.
After a load, each register, card memory, the ring, the state commands
set, a pending flip and the position in the frame are as they were at
the save, and a guest can't tell that it happened.

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
size, to each pixel of the rectangle. The rectangle has a width and
height of at least 1 and lies inside the surface, or the command is
rejected with `BAD_RANGE`.

**`BLT`, 0002h.** Length 21.

| Word | Argument |
|---|---|
| 1 to 5 | destination surface |
| 6 to 9 | destination rectangle: left, top, width, height |
| 10 to 14 | source surface |
| 15 to 18 | source rectangle |
| 19 | flags: bit 0 source colour key |
| 20 | colour key |

Copies pixels unchanged. The two surfaces have the same pixel size, or
the command is rejected with `BAD_VALUE`; the card converts no formats
at level 0. Both rectangles lie inside their surfaces.

- Equal rectangle sizes copy pixel for pixel. Where source and
  destination overlap in memory, the result is as if the whole source
  rectangle were read before any pixel is written.
- Different sizes stretch or shrink. The destination pixel at column
  `x` of `dw` and row `y` of `dh` takes the source pixel at column
  `((2x + 1) * sw) / (2 * dw)` and row `((2y + 1) * sh) / (2 * dh)`,
  divisions rounded down: the source pixel under the destination
  pixel's centre. With overlapping memory the result of a stretch is
  undefined within the destination rectangle, and nothing outside it is
  written.
- With bit 0 of the flags, a source pixel whose value equals the low
  bits of the colour key, compared over the whole pixel, leaves its
  destination pixel as it was.

**`FLIP`, 0003h.** Length 3.

| Word | Argument |
|---|---|
| 1 | offset of the surface to show |
| 2 | flags: bit 0 wait for the vertical blank |

See "Flips" in chapter 6. The surface has the current mode's size and
pitch and lies inside card memory, or the command is rejected with
`BAD_RANGE`. With `MODE_ENABLE` at 0 the flip is made at once whatever
the flag says, checked against the `MODE` registers' values.

### Level 1

Opcodes from 0010h. Written with the phases that implement them; the
command list is in "Commands of level 1" of `docs/toadforce.md`.

- State: render states, stage states, transforms, viewport, scissor,
  material, lights, clip planes, palettes.
- Surfaces: render target, clear, textures.
- Drawing: inline and from buffers, with and without indices.
- The capability entries and format codes level 1 adds.

## Open points

Phase 1 settles these and moves each into its chapter:

- **VGA memory and card memory.** The VBE frame buffer is the start of
  card memory. Whether the standard VGA modes' 256 KB (text, planar,
  mode 13h) is part of card memory too or a store of its own follows
  what the `Svga` adapter does, and chapter 4 has to say which.
- **Port 3DAh under the card's mode.** Chapter 6 gives it the card's
  timing. If the VGA code can't take its timing from the card without
  disturbing the other adapters, the port keeps the VGA's timing and
  the chapter says so.
- **The largest VBE mode**, which bounds what a guest using both has to
  keep clear at the start of card memory.
- **Blank lines.** `MODE_HEIGHT` / 16 + 1 is a round figure near what
  monitor timings of the time used (480 lines in 525, 768 in 806). A
  game that measures the retrace's length could ask for another.
- **Errors from the built-in Windows.** Its commands don't come through
  the ring, so `ERROR_AT` needs a defined value for them. A rejection
  there is a bug in the emulator's own code and should fail a debug
  build.
