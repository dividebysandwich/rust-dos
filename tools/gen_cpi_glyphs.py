"""Generate src/video/assets/CPIDOS_glyphs.bin, the glyphs the built-in
Windows draws for the characters of its code pages that the VGA's ROM
font (code page 437) doesn't have.

The glyphs come from the DISPLAY fonts of FreeDOS's CPIDOS 3.0a by Henrique
Peron (GPL 2.0 or later), https://gitlab.com/FreeDOS/base/cpidos: the
UPX-packed CPI files EGA.CPX, EGA2.CPX, EGA3.CPX and EGA5.CPX in its CPI
folder. The characters of their code pages come from the Unicode
Consortium's mapping files (MAPPINGS/VENDORS/MICSFT/PC/CPxxx.TXT and
WINDOWS/CP125x.TXT), the same files tools/gen_code_pages.py reads. Run

    python tools/gen_cpi_glyphs.py CPI_FOLDER MAPPINGS_FOLDER OUT

The output is a list of glyphs sorted by character: each the character
(16 bits, little-endian), then its 8x8, 8x14 and 8x16 glyphs (8, 14 and 16
bytes, the leftmost pixel in bit 7). CPIDOS draws its 8x14 and 8x16
glyphs a row lower than the ROM's; a glyph whose top row is empty is
moved up a row to sit on the ROM's baseline.
"""

import os
import struct
import sys

# The code pages whose characters get glyphs: the built-in Windows'.
WANTED = [1250, 1251, 1252, 1253, 1254, 1257, 737, 775, 850, 852, 855, 857, 866, 869]

# The CPIDOS files and code pages the glyphs are taken from, in the order
# they are looked in.
SOURCES = [
    ("EGA.CPX", 850),
    ("EGA.CPX", 852),
    ("EGA.CPX", 857),
    ("EGA.CPX", 858),
    ("EGA2.CPX", 775),
    ("EGA3.CPX", 866),
    ("EGA3.CPX", 855),
    ("EGA5.CPX", 737),
    ("EGA5.CPX", 869),
]

# Characters the built-in Windows draws from glyphs of its own: the ROM
# font's control-range paragraph and section signs, and its copyright and
# registered signs (font.rs's EXTRA_GLYPHS).
OWN = {0xB6, 0xA7, 0xA9, 0xAE}


def nrv2b_le16(src, start, out_len):
    """UPX's NRV2B decompression, with the 16-bit bit buffer of its DOS
    COM programs."""
    dst = bytearray()
    ilen = start
    bb = 0

    def getbit():
        nonlocal bb, ilen
        bb = (bb * 2) & 0xFFFFFFFF
        if bb & 0xFFFF:
            return (bb >> 16) & 1
        bb = (src[ilen] + src[ilen + 1] * 256) * 2 + 1
        ilen += 2
        return (bb >> 16) & 1

    last = 1
    while True:
        while getbit():
            dst.append(src[ilen])
            ilen += 1
        m_off = 1
        while True:
            m_off = (m_off * 2 + getbit()) & 0xFFFFFFFF
            if getbit():
                break
        if m_off == 2:
            m_off = last
        else:
            m_off = ((m_off - 3) * 256 + src[ilen]) & 0xFFFFFFFF
            ilen += 1
            if m_off == 0xFFFFFFFF:
                break
            m_off += 1
            last = m_off
        m_len = getbit()
        m_len = m_len * 2 + getbit()
        if m_len == 0:
            m_len = 1
            while True:
                m_len = m_len * 2 + getbit()
                if getbit():
                    break
            m_len += 2
        m_len += m_off > 0xD00
        p = len(dst) - m_off
        for i in range(m_len + 1):
            dst.append(dst[p + i])
    assert len(dst) == out_len, (len(dst), out_len)
    return bytes(dst)


def unpack(data):
    """The CPI file a CPX file holds."""
    at = data.index(b"UPX!")
    u_len, _ = struct.unpack_from("<HH", data, at + 16)
    return nrv2b_le16(data, at + 22, u_len)


def fonts(cpi):
    """{code page: {height: 256 * height bytes}} of a DISPLAY CPI file."""
    assert cpi[0] == 0xFF and cpi[1:8] == b"FONT   ", cpi[:8]
    (fih,) = struct.unpack_from("<I", cpi, 0x13)
    (n,) = struct.unpack_from("<H", cpi, fih)
    at = fih + 2
    out = {}
    for _ in range(n):
        _, nxt, _ = struct.unpack_from("<HIH", cpi, at)
        cp = struct.unpack_from("<H", cpi, at + 16)[0]
        (cpih,) = struct.unpack_from("<I", cpi, at + 24)
        _, nfonts, _ = struct.unpack_from("<HHH", cpi, cpih)
        p = cpih + 6
        sizes = {}
        for _ in range(nfonts):
            h, w, _, _, count = struct.unpack_from("<BBBBH", cpi, p)
            p += 6
            ln = count * h * ((w + 7) // 8)
            sizes[h] = cpi[p : p + ln]
            p += ln
        out[cp] = sizes
        # Some files' last links are wrong: the entries follow each other.
        good = 0 < nxt < len(cpi) and cpi[nxt : nxt + 2] == bytes([28, 0])
        at = nxt if good else p
    return out


def mapping(folder, cp):
    """{byte: character} of the code page `cp`."""
    sub = "WINDOWS" if cp >= 1250 else "PC"
    name = f"CP{cp}.TXT"
    path = os.path.join(folder, sub, name)
    if not os.path.exists(path):
        path = os.path.join(folder, name)
    table = {}
    for line in open(path, encoding="latin-1"):
        line = line.split("#")[0].strip()
        if not line.startswith("0x"):
            continue
        parts = line.split()
        if len(parts) > 1:
            table[int(parts[0], 16)] = int(parts[1], 16)
    return table


def main(cpi_folder, map_folder, out_path):
    pages = {}
    for cp in WANTED + [437]:
        pages[cp] = mapping(map_folder, cp)
    # Code page 858 is 850 with the euro sign at D5h.
    pages[858] = dict(mapping(map_folder, 850))
    pages[858][0xD5] = 0x20AC
    for _, cp in SOURCES:
        if cp not in pages:
            pages[cp] = mapping(map_folder, cp)
    rom = set(pages[437].values())
    wanted = set()
    for cp in WANTED:
        wanted |= {c for b, c in pages[cp].items() if b >= 0x80}
    wanted -= rom | OWN
    loaded = {}
    for name, _ in SOURCES:
        if name not in loaded:
            data = open(os.path.join(cpi_folder, name), "rb").read()
            loaded[name] = fonts(unpack(data))
    out = bytearray()
    missing = []
    for c in sorted(wanted):
        found = None
        for name, cp in SOURCES:
            byte = next((b for b, u in pages[cp].items() if u == c), None)
            if byte is not None:
                found = (loaded[name][cp], byte)
                break
        if found is None:
            missing.append(c)
            continue
        sizes, byte = found
        out += struct.pack("<H", c)
        for h in (8, 14, 16):
            rows = bytearray(sizes[h][byte * h : byte * h + h])
            if h != 8 and rows[0] == 0:
                rows = rows[1:] + b"\0"
            out += rows
    open(out_path, "wb").write(out)
    print(f"{len(out) // 40} glyphs, {len(out)} bytes")
    if missing:
        print("no glyph:", " ".join(f"U+{c:04X}" for c in missing))


if __name__ == "__main__":
    main(*sys.argv[1:4])
