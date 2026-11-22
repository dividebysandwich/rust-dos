"""Write the Rust crates' part of THIRD-PARTY-NOTICES: the licence
texts of every crate that the shipped programs are built from.

    python tools/third_party_notices.py          # rewrite the file
    python tools/third_party_notices.py --check  # fail if it is stale

Run it from anywhere in the checkout after a dependency changes, and at
release time. It reads `cargo metadata` of the three builds that ship
(the program and its crates, the libretro core and the web build), so
cargo must be able to read every dependency's manifest (it downloads
them when needed).

A crate counts when a shipped package reaches it through normal
dependencies, for any target: build and dev dependencies, which don't
end up in the programs, are left out. Crates of the workspace itself
(path dependencies) are rust-dos's own code, whose notices are in the
hand-written part of the file.

For each crate the script picks, of the licences its `license`
expression allows, the alternative with the most permissive licences,
and takes their texts from the licence files the crate ships, which it
recognises by their contents. A crate that ships no file for a licence
gets the licence's standard text with its authors as the copyright
holders (MIT, BSD, ISC), or a pointer to the full text that another
crate's notice carries (Apache-2.0, MPL-2.0). NOTICE files always go
in, as Apache-2.0 asks. Crates with the same text share one entry.

Everything in THIRD-PARTY-NOTICES after the MARKER line is replaced;
the part above it is kept as written.
"""

import json
import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
NOTICES = os.path.join(ROOT, "THIRD-PARTY-NOTICES")
MANIFESTS = ["Cargo.toml", "libretro/Cargo.toml", "web/Cargo.toml"]
MARKER = "=" * 72 + "\n6. RUST CRATES"
RULE = "-" * 72

# The licences in the order they are preferred where a crate offers a
# choice: those that ask least of a binary distribution first.
PREFERENCE = [
    "MIT-0", "0BSD", "CC0-1.0", "Unlicense", "MIT", "Zlib", "ISC",
    "BSD-2-Clause", "BSD-3-Clause", "BSL-1.0", "Unicode-3.0",
    "bzip2-1.0.6", "CDLA-Permissive-2.0", "Apache-2.0", "MPL-2.0",
]

# What a licence file's text says, by licence: the first that matches.
SIGNATURES = [
    ("Apache-2.0", r"Apache License\s+Version 2\.0"),
    ("MPL-2.0", r"Mozilla Public License,? (Version|v\.?) ?2\.0"),
    ("LGPL-2.1-or-later", r"GNU LESSER GENERAL PUBLIC LICENSE"),
    ("Unlicense", r"This is free and unencumbered software"),
    ("BSL-1.0", r"Boost Software License"),
    ("CDLA-Permissive-2.0", r"Community Data License Agreement"),
    ("Unicode-3.0", r"UNICODE LICENSE V3|Unicode, Inc\."),
    ("CC0-1.0", r"CC0 1\.0 Universal|Creative Commons Legal Code"),
    ("bzip2-1.0.6", r"bzip2/libbzip2"),
    ("MIT-0", r"^(?![\s\S]*above copyright notice)[\s\S]*"
              r"Permission is hereby granted, free of charge"),
    ("MIT", r"Permission is hereby granted, free of charge"),
    ("BSD-3-Clause", r"Redistribution and use in source and binary forms"
                     r"[\s\S]*(Neither the name|names of its)"),
    ("BSD-2-Clause", r"Redistribution and use in source and binary forms"),
    ("Zlib", r"This software is provided ['\"]?as-is['\"]?"),
    ("0BSD", r"Permission to use, copy, modify, and/or distribute"
             r"[\s\S]*with or without fee is hereby granted\.?\s+THE"),
    ("ISC", r"Permission to use, copy, modify,? and/?or distribute"),
]

MIT = """\
Copyright (c) {holders}

Permission is hereby granted, free of charge, to any person obtaining a
copy of this software and associated documentation files (the
"Software"), to deal in the Software without restriction, including
without limitation the rights to use, copy, modify, merge, publish,
distribute, sublicense, and/or sell copies of the Software, and to
permit persons to whom the Software is furnished to do so, subject to
the following conditions:

The above copyright notice and this permission notice shall be included
in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS
OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT,
TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE
SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
"""

BSD_3 = """\
Copyright (c) {holders}
All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are
met:

1. Redistributions of source code must retain the above copyright
   notice, this list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright
   notice, this list of conditions and the following disclaimer in the
   documentation and/or other materials provided with the distribution.

3. Neither the name of the copyright holder nor the names of its
   contributors may be used to endorse or promote products derived from
   this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS
IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED
TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A
PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED
TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR
PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF
LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING
NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
"""

ISC = """\
Copyright (c) {holders}

Permission to use, copy, modify, and/or distribute this software for any
purpose with or without fee is hereby granted, provided that the above
copyright notice and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
"""

TEMPLATES = {"MIT": MIT, "BSD-3-Clause": BSD_3, "ISC": ISC}
# Licences whose full text another crate's entry carries, given by
# reference where a crate ships no file of its own.
LONG = ("Apache-2.0", "MPL-2.0")

LICENCE_FILE = re.compile(
    r"^(licen[cs]e|copying|unlicense|copyright|notice)", re.IGNORECASE)


def metadata(manifest):
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked",
         "--manifest-path", os.path.join(ROOT, manifest)],
        check=True, capture_output=True, text=True, encoding="utf-8")
    return json.loads(out.stdout)


def shipped_crates():
    """The registry and git crates the shipped packages reach through
    normal dependencies, by (name, version)."""
    crates = {}
    for manifest in MANIFESTS:
        meta = metadata(manifest)
        packages = {p["id"]: p for p in meta["packages"]}
        nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
        seen = set()
        stack = list(meta["workspace_members"])
        while stack:
            node = stack.pop()
            if node in seen:
                continue
            seen.add(node)
            for dep in nodes[node]["deps"]:
                if any(k["kind"] is None for k in dep["dep_kinds"]):
                    stack.append(dep["pkg"])
        for node in seen:
            package = packages[node]
            if package["source"] is not None:
                crates[(package["name"], package["version"])] = package
    return crates


def alternatives(expression):
    """An SPDX expression as the sets of licences that satisfy it (OR of
    ANDs). `/` is old Cargo's OR; WITH exceptions are dropped, as the
    licence's text is what a notice carries."""
    text = expression.replace("/", " OR ")
    tokens = re.findall(r"\(|\)|[^\s()]+", text)
    pos = 0

    def parse_or():
        nonlocal pos
        result = parse_and()
        while pos < len(tokens) and tokens[pos].upper() == "OR":
            pos += 1
            result = result + parse_and()
        return result

    def parse_and():
        nonlocal pos
        result = parse_atom()
        while pos < len(tokens) and tokens[pos].upper() == "AND":
            pos += 1
            right = parse_atom()
            result = [a | b for a in result for b in right]
        return result

    def parse_atom():
        nonlocal pos
        token = tokens[pos]
        pos += 1
        if token == "(":
            result = parse_or()
            pos += 1  # ")"
        else:
            result = [frozenset([token])]
        if pos < len(tokens) and tokens[pos].upper() == "WITH":
            pos += 2
        return result

    return parse_or()


def rank(licence):
    return PREFERENCE.index(licence) if licence in PREFERENCE else 100


def classify(text):
    for licence, pattern in SIGNATURES:
        if re.search(pattern, text, re.IGNORECASE):
            return licence
    return None


def read(path):
    with open(path, "rb") as f:
        data = f.read()
    text = data.decode("utf-8", errors="replace").replace("\r\n", "\n")
    lines = [line.rstrip() for line in text.split("\n")]
    while lines and not lines[0]:
        lines.pop(0)
    while lines and not lines[-1]:
        lines.pop()
    return "\n".join(lines) + "\n"


def holders(package):
    authors = [re.sub(r"\s*<[^>]*>", "", a).strip()
               for a in package.get("authors") or []]
    authors = [a for a in authors if a]
    return ", ".join(authors) or f"the {package['name']} authors"


def notices(package):
    """The texts a crate's notice is made of, and the licences picked."""
    directory = os.path.dirname(package["manifest_path"])
    files = {}
    for name in sorted(os.listdir(directory)):
        path = os.path.join(directory, name)
        if LICENCE_FILE.match(name) and os.path.isfile(path):
            files[name] = read(path)
    licence_file = package.get("license_file")
    if licence_file:
        path = os.path.join(directory, licence_file)
        if os.path.isfile(path):
            files[os.path.basename(licence_file)] = read(path)
    kinds = {name: classify(text) for name, text in files.items()}
    shipped = set(kinds.values())

    expression = package.get("license") or "NOASSERTION"
    options = alternatives(expression)
    # Prefer an alternative the crate ships every text of, then the most
    # permissive.
    best = min(options, key=lambda o: (
        not o <= shipped, max(rank(x) for x in o), len(o), sorted(o)))

    texts = []
    for licence in sorted(best, key=rank):
        found = [files[n] for n, k in kinds.items() if k == licence]
        if found:
            texts.extend(found)
        elif licence in TEMPLATES:
            texts.append(TEMPLATES[licence].format(holders=holders(package)))
        elif licence in LONG:
            texts.append(f"Copyright (c) {holders(package)}\n\nLicensed "
                         f"under {licence}; its full text is in\nthis "
                         f"file's entries for other crates.\n")
        else:
            unknown = [files[n] for n, k in kinds.items() if k is None]
            if unknown:
                texts.extend(unknown)
            else:
                texts.append(f"Copyright (c) {holders(package)}\n\n"
                             f"Licensed under {licence}.\n")
    for name, text in files.items():
        if name.upper().startswith("NOTICE") and text not in texts:
            texts.append(text)
    return " AND ".join(sorted(best, key=rank)), texts


def crates_part():
    entries = {}
    for (name, version), package in sorted(shipped_crates().items()):
        licence, texts = notices(package)
        key = "\n".join(texts)
        entry = entries.setdefault(key, {"crates": [], "licences": set()})
        entry["crates"].append(f"{name} {version}")
        entry["licences"].add(licence)
    out = [MARKER + "\n" + "=" * 72 + "\n"]
    out.append(
        "The programs are built with the Rust crates below, from crates.io.\n"
        "Each entry names the crates and the licence rust-dos uses them\n"
        "under (where a crate offers a choice, the one picked), then the\n"
        "licence text with its copyright notice. This part is written by\n"
        "tools/third_party_notices.py.\n")
    for key, entry in sorted(entries.items(),
                             key=lambda e: e[1]["crates"][0].lower()):
        out.append(RULE)
        out.append("Crates: " + ", ".join(entry["crates"]))
        out.append("Licence: " + " / ".join(sorted(entry["licences"])))
        out.append("")
        out.append(key.rstrip("\n"))
        out.append("")
    return "\n".join(out)


def main():
    check = "--check" in sys.argv[1:]
    with open(NOTICES, encoding="utf-8", newline="") as f:
        current = f.read()
    at = current.find(MARKER)
    if at < 0:
        sys.exit(f"{NOTICES} has no line starting the crates part")
    wanted = current[:at] + crates_part()
    if check:
        if wanted != current:
            sys.exit("THIRD-PARTY-NOTICES is out of date: run "
                     "python tools/third_party_notices.py")
        return
    with open(NOTICES, "w", encoding="utf-8", newline="\n") as f:
        f.write(wanted)


if __name__ == "__main__":
    main()
