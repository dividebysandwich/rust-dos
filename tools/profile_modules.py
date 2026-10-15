#!/usr/bin/env python3
r"""Split a running profile of a Windows program by module.

Reads the profile from a rust-dos debug server (`/api/profile`) and guest
memory (`/api/memory`), names the PE image (EXE or DLL) each 64 KB block of
32-bit code belongs to, and prints the busy time by module and the callers
(`callers` in the report) by module. Windows loads every PE image on a
64 KB boundary, so a block of `bucket=65536` belongs to one image.

An image is named from its header in guest memory when Windows left the
header's page there (KERNEL32's usually is). For the others, pass the
program's own files with --files, and Windows' DLLs (WINDOWS\SYSTEM) with
--system: each file's PE header says where it loads, and the code in guest
memory is compared with the file to confirm it, or to find where Windows
moved a DLL it had to relocate. Modules from --files are marked
[program].

    python tools/profile_modules.py --host 127.0.0.1:8086 --files game/ --system system/
    python tools/profile_modules.py --files game/ --save modules.json
    python tools/profile_modules.py --map modules.json

Standard library only. See docs/profiling.md, "Profiling Windows games".
"""

import argparse
import json
import os
import struct
import sys
import urllib.error
import urllib.request
from collections import defaultdict

BLOCK = 0x10000


class Server:
    def __init__(self, host):
        self.host = host
        self.cache = {}

    def get(self, path):
        with urllib.request.urlopen(f"http://{self.host}{path}") as r:
            return json.load(r)

    def mem(self, lin, n):
        """Guest memory at a linear address, or None where a page is out."""
        key = (lin, n)
        if key not in self.cache:
            try:
                d = self.get(f"/api/memory?addr=lin:{lin:X}&len={n}&format=json")
                data = bytes.fromhex(d["hex"].replace(" ", ""))
            except (urllib.error.URLError, KeyError, ValueError):
                data = b""
            # An unmapped page reads as FF bytes.
            self.cache[key] = None if not data or data == b"\xff" * len(data) else data
        return self.cache[key]


class Image:
    """A PE image: where it loads, how big, its sections and exports name."""

    def __init__(self, name, base, size, sections=(), data=None, path=None, export=None, stamp=None):
        self.name, self.base, self.size = name, base, size
        self.sections = list(sections)  # (rva, vsize, raw offset, raw size, executable)
        self.data, self.path, self.export, self.stamp = data, path, export, stamp
        self.how = "header in memory" if path is None else "file"

    def holds(self, lin):
        return self.base <= lin < self.base + self.size

    def file_bytes(self, rva, n):
        for va, vsize, raw, rsize, _ in self.sections:
            if va <= rva < va + max(vsize, rsize) and rva - va + n <= rsize:
                at = raw + rva - va
                return self.data[at:at + n]
        return None

    def as_json(self):
        return {"name": self.name, "base": f"{self.base:08X}", "size": f"{self.size:X}", "how": self.how,
                "file": self.path}


def parse_pe(head, read=None):
    """ImageBase, SizeOfImage, TimeDateStamp, sections and the export name
    of a PE header in `head` (from offset 0). `read(rva, n)` reads the
    image for the export name, if given."""
    if head[:2] != b"MZ" or len(head) < 0x40:
        return None
    pe = struct.unpack_from("<I", head, 0x3C)[0]
    if pe + 0xF8 > len(head) or head[pe:pe + 4] != b"PE\0\0":
        return None
    nsec = struct.unpack_from("<H", head, pe + 6)[0]
    stamp = struct.unpack_from("<I", head, pe + 8)[0]
    opt = pe + 24
    if struct.unpack_from("<H", head, opt)[0] != 0x10B:  # PE32
        return None
    base, size = struct.unpack_from("<I", head, opt + 28)[0], struct.unpack_from("<I", head, opt + 56)[0]
    export_rva = struct.unpack_from("<I", head, opt + 96)[0]
    sec_at = opt + struct.unpack_from("<H", head, pe + 20)[0]
    sections = []
    for i in range(nsec):
        s = sec_at + 40 * i
        if s + 40 > len(head):
            break
        vsize, va, rsize, raw = struct.unpack_from("<IIII", head, s + 8)
        flags = struct.unpack_from("<I", head, s + 36)[0]
        sections.append((va, vsize, raw, rsize, bool(flags & 0x20000000)))
    export = None
    if read and export_rva:
        d = read(export_rva, 16)
        if d and len(d) == 16:
            name = read(struct.unpack_from("<I", d, 12)[0], 64)
            if name:
                export = name.split(b"\0")[0].decode("latin1") or None
    return base, size, stamp, sections, export


def load_files(dirs):
    images = []
    for d in dirs:
        for root, _, names in os.walk(d):
            for n in names:
                path = os.path.join(root, n)
                try:
                    data = open(path, "rb").read()
                except OSError:
                    continue
                pe = parse_pe(data[:4096])
                if not pe:
                    continue
                base, size, stamp, sections, _ = pe
                img = Image(n, base, size, sections, data, path, stamp=stamp)

                def read(rva, k, img=img):
                    return img.file_bytes(rva, k)

                img.export = parse_pe(data[:4096], read)[4]
                images.append(img)
    return images


def image_in_memory(srv, lin):
    """The image whose header is in guest memory at a 64 KB boundary at or
    below `lin`, looking back up to 16 MB."""
    base = lin & ~(BLOCK - 1)
    for _ in range(256):
        head = srv.mem(base, 0x400)
        if head and head[:2] == b"MZ":
            pe = parse_pe(head, lambda rva, n, b=base: srv.mem(b + rva, n))
            if pe and lin < base + pe[1]:
                b, size, stamp, sections, export = pe
                return Image(export or f"image@{base:08X}", base, size, sections, export=export, stamp=stamp)
            return None
        base -= BLOCK
        if base < 0:
            return None
    return None


def match_file(srv, files, lin, found):
    """A file image that holds `lin`, checked against guest memory: at the
    base its header asks for, or where Windows relocated it. `found`: the
    files already placed, which aren't looked for again."""
    code = srv.mem(lin, 32)
    for img in files:
        if img.holds(lin):
            want = img.file_bytes(lin - img.base, 32)
            if code is None or want is None:
                img.how = "file, at its preferred base (code not in memory to compare)"
                return img
            if want == code:
                img.how = "file, at its preferred base, code matches"
                return img
    if code is None:
        return None
    # Relocated: find the guest's bytes in a file's code, at an offset that
    # puts the image on a 64 KB boundary above the first 64 KB. Relocations
    # patch addresses, so look for windows of the 32 bytes, and take a base
    # two of them agree on.
    for img in files:
        if img.path in found:
            continue
        for va, vsize, raw, rsize, exe in img.sections:
            if not exe:
                continue
            sec = img.data[raw:raw + rsize]
            votes = defaultdict(int)
            for k in range(0, 32, 8):
                at = sec.find(code[k:k + 8])
                while at >= 0:
                    base = lin + k - (va + at)
                    if base % BLOCK == 0 and BLOCK <= base < 0x80000000:
                        votes[base] += 1
                    at = sec.find(code[k:k + 8], at + 1)
            for base, n in votes.items():
                if n >= 2:
                    moved = Image(img.name, base, img.size, img.sections, img.data, img.path, img.export, img.stamp)
                    moved.how = f"file, relocated from {img.base:08X}"
                    return moved
    return None


def side(mode, cs):
    if mode == "v86":
        return "v86: DOS and the BIOS"
    if mode == "real":
        return "real mode"
    if int(cs, 16) & 3 == 0:
        return "ring 0: the VMM and VxDs"
    if mode == "pm16":
        return "16-bit ring 3: Win16 (USER, GDI, KERNEL)"
    return None


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--host", default="127.0.0.1:8086", help="the debug server (default 127.0.0.1:8086)")
    ap.add_argument("--files", action="append", default=[], help="a folder with the program's EXE and DLLs (repeatable)")
    ap.add_argument("--system", action="append", default=[], help="a folder with Windows' DLLs (repeatable)")
    ap.add_argument("--map", help="a module map saved by --save, used before anything else")
    ap.add_argument("--save", help="write the modules found to this JSON file")
    ap.add_argument("--top", type=int, default=20, help="rows of each table (default 20)")
    args = ap.parse_args()

    srv = Server(args.host)
    report = srv.get("/api/profile?bucket=65536&top=100000&disasm=0")
    busy = report["busy"]["samples"] or 1
    program = load_files(args.files)
    program_paths = {img.path for img in program}
    files = program + load_files(args.system)
    known = []
    if args.map:
        for m in json.load(open(args.map)):
            img = Image(m["name"], int(m["base"], 16), int(m["size"], 16))
            img.how = m.get("how", "map")
            known.append(img)

    # The hottest instruction of each 64 KB block, where its code is most
    # likely in memory to compare with a file.
    hottest = {}
    for row in report["top"]:
        if row["mode"] == "pm32" and row.get("hottest"):
            off = int(row["hottest"].split(":")[1], 16)
            hottest.setdefault(off & ~(BLOCK - 1), off)

    def module(lin):
        for img in known:
            if img.holds(lin):
                return img
        probe = hottest.get(lin & ~(BLOCK - 1), lin)
        found = {i.path for i in known if i.path}
        img = match_file(srv, files, probe, found) or image_in_memory(srv, lin)
        if img:
            known.append(img)
        return img

    def name(addr, mode):
        cs, off = addr.split(":")
        s = side(mode, cs)
        if s:
            return s
        img = module(int(off, 16))
        if not img:
            return f"no image found at {int(off, 16) & ~(BLOCK - 1):08X}"
        return img.name + (" [program]" if img.path in program_paths else "")

    by = defaultdict(int)
    for row in report["top"]:
        by[name(row["addr"], row["mode"])] += row["samples"]
    print(f"Busy time by module ({busy} busy samples):")
    for who, n in sorted(by.items(), key=lambda x: -x[1])[:args.top]:
        print(f"  {100 * n / busy:6.2f}%  {who}")

    callers = report.get("callers", {})
    rows = callers.get("top", [])
    if rows:
        caused = defaultdict(int)
        chains = defaultdict(int)
        for row in rows:
            win32 = [name(a, "pm32") for a in row["win32"]]
            path = []
            for w in win32:
                if not path or path[-1] != w:
                    path.append(w)
            entry = row["entry"]
            via = f"{entry['mode']} {entry['via']}" if entry else "16-bit code"
            program = next((w for w in win32 if w.endswith("[program]")), None)
            caused[program or ("Win32: " + path[0] if path else "the entry's own code: " + (name(entry["addr"], entry["mode"]) if entry else "?"))] += row["samples"]
            chains[" -> ".join(path + [via, name(row["places"][0]["addr"], row["places"][0]["mode"])])] += row["samples"]
        print(f"\nRing-0 and 16-bit time by the code that caused it ({callers['busy_pct']:.2f}% of busy):")
        for who, n in sorted(caused.items(), key=lambda x: -x[1])[:args.top]:
            print(f"  {100 * n / busy:6.2f}%  {who}")
        print("\nBy chain, root first:")
        for chain, n in sorted(chains.items(), key=lambda x: -x[1])[:args.top]:
            print(f"  {100 * n / busy:6.2f}%  {chain}")

    print("\nModules:")
    for img in sorted(known, key=lambda i: i.base):
        print(f"  {img.base:08X}-{img.base + img.size - 1:08X}  {img.name:<16} {img.how}")
    if args.save:
        with open(args.save, "w") as f:
            json.dump([img.as_json() for img in sorted(known, key=lambda i: i.base)], f, indent=1)
        print(f"\nSaved {len(known)} modules to {args.save}")


if __name__ == "__main__":
    sys.exit(main())
