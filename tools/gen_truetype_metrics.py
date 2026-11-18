"""Generate src/win32/gdi32/truetype.bin, the metrics and advance widths
of Windows 98 SE's TrueType faces (Arial, Times New Roman and Courier
New, each regular, bold, italic and bold italic) that the built-in GDI
lays text out with.

Two sources go into it:

- the TrueType files of Windows 98 SE (ARIAL.TTF, ARIALBD.TTF, ARIALI.TTF,
  ARIALBI.TTF, TIMES*.TTF, COUR*.TTF from C:\\WINDOWS\\FONTS): units per
  em, usWinAscent and usWinDescent, the line gap, xAvgCharWidth, the
  bounding box, the VDMX table's heights for each pixel size and each
  character's advance width in font units;
- what Windows 98 SE's GDI gives at each pixel size from 1 to 128 (lfHeight
  -1 to -128): GetCharWidthA's widths of the characters 20h to FFh in the
  character sets of the code pages below, written by
  tools/win98_font_probe.c run in a real Windows 98 SE with multilanguage
  support (its outputs FONT2.TXT and FONT4.TXT). These are the hinted
  widths, which the TrueType rasterizer gets by running each glyph's
  instructions and which differ from the scaled advance by a pixel here
  and there.

Run

    python tools/gen_truetype_metrics.py FONTS_FOLDER PROBE_OUTPUT... OUT

The output (all numbers little-endian):

    "W98T", u16 n, then the n characters (u16, ascending): ASCII and the
    upper halves of code pages 1250, 1251, 1252, 1253, 1254 and 1257
    then for each of the 12 faces (Arial, Times New Roman, Courier New;
    each regular, bold, italic, bold italic):
      u16 units per em, u16 usWinAscent, u16 usWinDescent,
      i16 external leading in font units, u16 xAvgCharWidth,
      u16 bounding box width, u16 the missing glyph's advance,
      u16 x n the characters' advances (the missing glyph's for one the
        face lacks),
      u8 x 2 x 255 the ascent and descent in pixels at the pixel sizes 1
        to 255 (VDMX's, or the scaled ones rounded where it has none),
      u8 the highest pixel size with hinted widths,
      for each pixel size from 1 up to it: u16 count, then count u16s,
        ascending by character: the characters whose hinted width isn't
        their advance scaled and rounded, each its index (bits 0-8) and
        the hinted width less the scaled one (bits 9-15, signed).
"""

import struct
import sys

FACES = [
    ("Arial", "ARIAL"), ("Arial", "ARIALBD"), ("Arial", "ARIALI"), ("Arial", "ARIALBI"),
    ("Times New Roman", "TIMES"), ("Times New Roman", "TIMESBD"),
    ("Times New Roman", "TIMESI"), ("Times New Roman", "TIMESBI"),
    ("Courier New", "COUR"), ("Courier New", "COURBD"),
    ("Courier New", "COURI"), ("Courier New", "COURBI"),
]
CODE_PAGES = ["cp1250", "cp1251", "cp1252", "cp1253", "cp1254", "cp1257"]
# The character sets the probe asks for, and their code pages.
CHARSETS = {
    0: "cp1252", 238: "cp1250", 204: "cp1251", 161: "cp1253", 162: "cp1254", 186: "cp1257",
}
MAX_PPEM = 128

# The bytes Windows 98's GDI draws with the missing glyph although the
# code page has them now: its tables of code pages 1252, 1253 and 1257
# are older ones, without Z and z with caron (1252), the horizontal bar
# (1253) and seven of 1257's accents.
MISSING = {
    "cp1252": {0x8E, 0x9E},
    "cp1253": {0xAF},
    "cp1257": {0x8D, 0x8E, 0x8F, 0x9D, 0x9E, 0xB4, 0xFF},
}


class Font:
    def __init__(self, path):
        d = self.d = open(path, "rb").read()
        n = struct.unpack_from(">H", d, 4)[0]
        self.tables = {}
        for i in range(n):
            tag, _, off, ln = struct.unpack_from(">4sIII", d, 12 + 16 * i)
            self.tables[tag.decode("latin-1")] = off
        o = self.tables["head"]
        self.upem = struct.unpack_from(">H", d, o + 18)[0]
        x_min, _, x_max, _ = struct.unpack_from(">hhhh", d, o + 36)
        self.bbox_width = x_max - x_min
        o = self.tables["hhea"]
        asc, desc, gap = struct.unpack_from(">hhh", d, o + 4)
        n_metrics = struct.unpack_from(">H", d, o + 34)[0]
        o = self.tables["maxp"]
        glyphs = struct.unpack_from(">H", d, o + 4)[0]
        o = self.tables["OS/2"]
        self.avg = struct.unpack_from(">h", d, o + 2)[0]
        self.win_ascent, self.win_descent = struct.unpack_from(">HH", d, o + 74)
        # As Windows has it: the line gap less what the Windows ascent and
        # descent take beyond the typographic ones.
        self.external = gap - ((self.win_ascent + self.win_descent) - (asc - desc))
        o = self.tables["hmtx"]
        self.advances = [
            struct.unpack_from(">H", d, o + 4 * min(g, n_metrics - 1))[0] for g in range(glyphs)
        ]
        self.cmap = self.read_cmap()
        self.vdmx = self.read_vdmx()

    def read_cmap(self):
        d = self.d
        o = self.tables["cmap"]
        n = struct.unpack_from(">H", d, o + 2)[0]
        for i in range(n):
            platform, encoding, off = struct.unpack_from(">HHI", d, o + 4 + 8 * i)
            if platform != 3 or encoding != 1:
                continue
            s = o + off
            seg = struct.unpack_from(">H", d, s + 6)[0] // 2
            ends = struct.unpack_from(">%dH" % seg, d, s + 14)
            starts = struct.unpack_from(">%dH" % seg, d, s + 16 + 2 * seg)
            deltas = struct.unpack_from(">%dh" % seg, d, s + 16 + 4 * seg)
            at = s + 16 + 6 * seg
            offsets = struct.unpack_from(">%dH" % seg, d, at)
            m = {}
            for k in range(seg):
                for c in range(starts[k], min(ends[k], 0xFFFE) + 1):
                    if offsets[k] == 0:
                        g = (c + deltas[k]) & 0xFFFF
                    else:
                        a = at + 2 * k + offsets[k] + 2 * (c - starts[k])
                        g = struct.unpack_from(">H", d, a)[0]
                        g = (g + deltas[k]) & 0xFFFF if g else 0
                    if g:
                        m[c] = g
            return m
        raise ValueError("no Unicode cmap")

    def read_vdmx(self):
        """The 1:1 ratio's {pixel size: (yMax, yMin)}."""
        d = self.d
        o = self.tables.get("VDMX")
        if o is None:
            return {}
        _, _, ratios = struct.unpack_from(">HHH", d, o)
        for i in range(ratios):
            _, x, y0, y1 = struct.unpack_from(">BBBB", d, o + 6 + 4 * i)
            if x == 0 or (x == 1 and y0 <= 1 <= y1):
                g = o + struct.unpack_from(">H", d, o + 6 + 4 * ratios + 2 * i)[0]
                recs = struct.unpack_from(">H", d, g)[0]
                out = {}
                for k in range(recs):
                    p, y_max, y_min = struct.unpack_from(">Hhh", d, g + 4 + 6 * k)
                    out[p] = (y_max, y_min)
                return out
        return {}

    def scale(self, v, ppem):
        return (v * ppem + self.upem // 2) // self.upem

    def advance(self, c):
        return self.advances[self.cmap.get(c, 0)]


def probe_widths(paths):
    """{(face, ppem, weight, italic, code page): [widths of 20h..FFh]}"""
    out = {}
    key = None
    for path in paths:
        for line in open(path, encoding="latin-1"):
            if line.startswith("F "):
                fields = line[2:].split(" h ", 1)[0].split("|")
                face, h, w, weight, italic = fields[:5]
                charset = int(fields[5]) if len(fields) > 5 else 0
                key = None
                if int(h) < 0 and int(w) == 0:
                    cp = CHARSETS[charset]
                    key = (face, -int(h), int(weight), int(italic), cp)
            elif not line.startswith("  "):
                key = None
            elif line.startswith("  W ") and key:
                out[key] = list(map(int, line[4:].split()))
    return out


def characters():
    chars = set(range(0x20, 0x7F))
    for cp in CODE_PAGES:
        for b in range(0x80, 0x100):
            try:
                chars.add(ord(bytes([b]).decode(cp)))
            except UnicodeDecodeError:
                pass
    return sorted(chars)


def main():
    folder, probes, out = sys.argv[1], sys.argv[2:-1], sys.argv[-1]
    widths = probe_widths(probes)
    chars = characters()
    index = {c: i for i, c in enumerate(chars)}
    b = bytearray(b"W98T")
    b += struct.pack("<H", len(chars))
    b += struct.pack("<%dH" % len(chars), *chars)
    for n, (face, file) in enumerate(FACES):
        f = Font("%s/%s.TTF" % (folder, file))
        b += struct.pack("<HHHhHHH", f.upem, f.win_ascent, f.win_descent, f.external,
                         f.avg, f.bbox_width, f.advances[0])
        b += struct.pack("<%dH" % len(chars), *[f.advance(c) for c in chars])
        for p in range(1, 256):
            if p in f.vdmx:
                y_max, y_min = f.vdmx[p]
                b += struct.pack("<BB", y_max, -y_min)
            else:
                b += struct.pack("<BB", f.scale(f.win_ascent, p), f.scale(f.win_descent, p))
        b.append(MAX_PPEM)
        weight, italic = (700 if n % 2 else 400), n // 2 % 2
        total = 0
        for p in range(1, MAX_PPEM + 1):
            hinted = {}
            for cp in CODE_PAGES:
                measured = widths[(face, p, weight, italic, cp)]
                for byte in range(0x20, 0x100):
                    if byte in MISSING.get(cp, ()) or byte == 0x7F:
                        continue
                    try:
                        c = ord(bytes([byte]).decode(cp))
                    except UnicodeDecodeError:
                        continue
                    w = measured[byte - 0x20]
                    if hinted.setdefault(c, w) != w:
                        print("%s %d: U+%04X is %d and %d wide" % (file, p, c, hinted[c], w))
            odd = []
            for c, w in sorted(hinted.items(), key=lambda e: index[e[0]]):
                delta = w - f.scale(f.advance(c), p)
                if delta:
                    assert index[c] < 512 and -64 <= delta < 64
                    odd.append(index[c] | (delta & 0x7F) << 9)
            b += struct.pack("<H", len(odd))
            for e in odd:
                b += struct.pack("<H", e)
            total += len(odd)
        print("%-8s %5d hinted widths" % (file, total))
    open(out, "wb").write(b)
    print("%d characters, %d bytes" % (len(chars), len(b)))


if __name__ == "__main__":
    main()
