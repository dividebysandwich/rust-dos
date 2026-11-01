"""Write the export ordinals of Windows 98 SE's DLLs for the built-in ones.

    python -I tools/win98_ordinals.py <folder with the DLLs> src/win32/ordinals

reads the export directory of each DLL the built-in Windows has (copied
out of a Windows 98 SE installation's WINDOWS\\SYSTEM) and writes
<name>.txt into the output folder: one export a line, its ordinal and its
name, or "-" for one Windows exports by ordinal only. src/win32/ordinals.rs
reads them. The DLLs themselves stay out of the repository.
"""

import os
import struct
import sys

# The built-in DLLs Windows 98 SE has. GLIDE2X (3dfx's) and D3D8
# (DirectX 8) aren't part of it.
DLLS = [
    "ADVAPI32.DLL", "ADVPACK.DLL", "COMCTL32.DLL", "COMDLG32.DLL",
    "DDRAW.DLL", "DINPUT.DLL", "DMBAND.DLL", "DMCOMPOS.DLL", "DMIME.DLL",
    "DMLOADER.DLL", "DMSTYLE.DLL", "DMUSIC.DLL", "DPLAYX.DLL", "DSOUND.DLL",
    "GDI32.DLL", "IMAGEHLP.DLL", "KERNEL32.DLL", "LZ32.DLL", "MPR.DLL",
    "MSIMG32.DLL", "MSVCRT.DLL", "NTDLL.DLL", "OLE32.DLL", "OLEAUT32.DLL",
    "OPENGL32.DLL", "SHELL32.DLL", "USER32.DLL", "VERSION.DLL", "WINMM.DLL",
    "WINSPOOL.DRV", "WNASPI32.DLL", "WOW32.DLL", "WS2_32.DLL", "WSOCK32.DLL",
]


def exports(data):
    """The (ordinal, name or None) of each export of the PE image."""
    pe = struct.unpack_from("<I", data, 0x3C)[0]
    sections = struct.unpack_from("<H", data, pe + 6)[0]
    optional = struct.unpack_from("<H", data, pe + 20)[0]
    directory = struct.unpack_from("<I", data, pe + 24 + 96)[0]
    table = []
    for i in range(sections):
        at = pe + 24 + optional + 40 * i
        size, rva, raw_size, raw = struct.unpack_from("<IIII", data, at + 8)
        table.append((rva, max(size, raw_size), raw))

    def offset(rva):
        for start, size, raw in table:
            if start <= rva < start + size:
                return rva - start + raw
        raise ValueError(f"RVA {rva:X} is in no section")

    if directory == 0:
        return []
    at = offset(directory)
    base, functions, names, function_table, name_table, ordinal_table = (
        struct.unpack_from("<IIIIII", data, at + 16)
    )
    named = {}
    for i in range(names):
        name_rva = struct.unpack_from("<I", data, offset(name_table) + 4 * i)[0]
        index = struct.unpack_from("<H", data, offset(ordinal_table) + 2 * i)[0]
        start = offset(name_rva)
        named[index] = data[start : data.index(b"\0", start)].decode("ascii")
    out = []
    for i in range(functions):
        if struct.unpack_from("<I", data, offset(function_table) + 4 * i)[0]:
            out.append((base + i, named.get(i)))
    return out


def main():
    source, target = sys.argv[1], sys.argv[2]
    files = {name.upper(): name for name in os.listdir(source)}
    for dll in DLLS:
        if dll not in files:
            sys.exit(f"{dll} isn't in {source}")
        with open(os.path.join(source, files[dll]), "rb") as f:
            data = f.read()
        lines = [f"{o} {n or '-'}\n" for o, n in exports(data)]
        out = os.path.join(target, dll.lower().replace(".", "_") + ".txt")
        with open(out, "w", newline="\n") as f:
            f.writelines(lines)
        print(f"{dll}: {len(lines)} exports")


if __name__ == "__main__":
    main()
