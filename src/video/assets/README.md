# Video ROM fonts

The fonts of IBM's EGA and VGA ROMs, which rust-dos draws text with and puts
in its video BIOS ROM for programs to read (INT 10h AH=11h AL=30h, INT 1Fh
and INT 43h).

| File | Font |
|---|---|
| `IBM_VGA_8x8.bin` | 8x8, 256 glyphs of 8 bytes: the CGA graphics modes' font, and 80x43/80x50 text |
| `IBM_EGA_8x14.bin` | 8x14, 256 glyphs of 14 bytes: the EGA's text font and the 350-line graphics modes' font |
| `IBM_VGA_8x16.bin` | 8x16, 256 glyphs of 16 bytes: the VGA's text font and the 480-line graphics modes' font |
| `IBM_EGA_9x14_alt.bin` | The glyphs that differ in 9-dot wide 14-line cells (the monochrome text mode): a character code and 14 bytes each, then a 0 |
| `IBM_VGA_9x16_alt.bin` | The same for 9-dot wide 16-line cells: a character code and 16 bytes each, then a 0 |

Each glyph row is a byte with the leftmost pixel in bit 7.

The 8x14 font and the two alternate tables were extracted from DOSBox
Staging's `src/ints/int10_memory.cpp` (`int10_font_14`,
`int10_font_14_alternate` and `int10_font_16_alternate`), which is licensed
GPL-2.0-or-later like rust-dos. That file's 8x8 and 8x16 tables are
identical to `IBM_VGA_8x8.bin` and `IBM_VGA_8x16.bin`.
