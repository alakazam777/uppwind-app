# -*- coding: utf-8 -*-
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Uppwind
"""Uppwind Link -- check a built file before it is published (read-only, no network).

    python -I dev/release_check.py companion/target/release/uppwind-companion.exe [more files…]

For every file: no absolute path of the machine that built it (C:\\Users\\…, /Users/…, a cargo registry or rustup
folder, another drive's folder), in ASCII or UTF-16; for a Windows program, also: DLLs loaded from System32 only
(DependentLoadFlags 0x800), no VCRUNTIME140 / api-ms-win-crt import (the C runtime is inside), a REPRO debug entry
(reproducible build). Prints the SHA-256 of each file. Exit code 1 if anything is wrong. A .pdb is always refused:
it is never published.
"""
import hashlib
import re
import struct
import sys

PATTERNS = [
    (re.compile(rb"[A-Za-z]:\\\\?Users\\\\?[^\\\x00\"']+", re.I), "a user folder"),
    (re.compile(rb"\\\.cargo\\\\?registry|/\.cargo/registry", re.I), "the cargo registry"),
    (re.compile(rb"\\\.rustup\\\\?toolchains|/\.rustup/toolchains", re.I), "a rustup toolchain"),
    (re.compile(rb"\b[D-Z]:\\\\?[A-Za-z0-9_.-]+\\\\?", re.I), "a folder on another drive"),
    # a macOS (or Linux) home folder: /Users/<name>/…, /home/<name>/… (/Users/Shared is not anyone's)
    (re.compile(rb"/(?:Users/(?!Shared/)|home/)[A-Za-z0-9_.-]+/"), "a user folder"),
]


def utf16_to_ascii(data):
    """The UTF-16LE strings of the file, as bytes (ASCII letters only), for the same patterns."""
    out = bytearray()
    for m in re.finditer(rb"(?:[\x20-\x7e]\x00){6,}", data):
        out += m.group(0)[::2] + b"\n"
    return bytes(out)


def path_hits(data):
    hits = []
    for blob in (data, utf16_to_ascii(data)):
        for rx, what in PATTERNS:
            for m in rx.finditer(blob):
                s = m.group(0)[:120].decode("ascii", "replace")
                if what == "a folder on another drive" and re.match(r"^[A-Za-z]:\\\\?(Windows|Program Files)", s, re.I):
                    continue
                hits.append("%s: %s" % (what, s))
    return sorted(set(hits))


def pe_checks(data):
    """→ list of problems (empty: fine), and facts."""
    if data[:2] != b"MZ":
        return [], {}
    pe = struct.unpack_from("<I", data, 0x3C)[0]
    if data[pe:pe + 4] != b"PE\x00\x00":
        return ["not a PE file"], {}
    coff = pe + 4
    nsec = struct.unpack_from("<H", data, coff + 2)[0]
    opt_size = struct.unpack_from("<H", data, coff + 16)[0]
    opt = coff + 20
    magic = struct.unpack_from("<H", data, opt)[0]
    if magic != 0x20B:
        return ["not a 64-bit program"], {}
    dllchar = struct.unpack_from("<H", data, opt + 70)[0]
    ndirs = struct.unpack_from("<I", data, opt + 108)[0]
    dirs = [struct.unpack_from("<II", data, opt + 112 + 8 * i) for i in range(min(ndirs, 16))]
    secs = []
    so = opt + opt_size
    for i in range(nsec):
        vsize, va, rsize, raw = struct.unpack_from("<IIII", data, so + 40 * i + 8)
        secs.append((va, max(vsize, rsize), raw))

    def off(rva):
        for va, size, raw in secs:
            if va <= rva < va + size:
                return raw + rva - va
        return None

    facts = {"aslr+nx": bool(dllchar & 0x40) and bool(dllchar & 0x100)}
    problems = []
    if not facts["aslr+nx"]:
        problems.append("ASLR or NX is off")
    # imports
    dlls = []
    if len(dirs) > 1 and dirs[1][0]:
        p = off(dirs[1][0])
        while p is not None:
            name_rva = struct.unpack_from("<I", data, p + 12)[0]
            if name_rva == 0:
                break
            n = off(name_rva)
            dlls.append(data[n:data.index(b"\x00", n)].decode("ascii", "replace"))
            p += 20
    facts["imports"] = dlls
    bad = [d for d in dlls if d.lower().startswith("vcruntime") or d.lower().startswith("api-ms-win-crt")]
    if bad:
        problems.append("imports the C runtime (%s): build with the flags of .cargo/config.toml" % ", ".join(bad))
    # debug directory: REPRO (16)
    kinds = []
    if len(dirs) > 6 and dirs[6][0]:
        p = off(dirs[6][0])
        for i in range(dirs[6][1] // 28):
            kinds.append(struct.unpack_from("<I", data, p + 28 * i + 12)[0])
    facts["debug"] = kinds
    if 16 not in kinds:
        problems.append("no REPRO debug entry (/Brepro): the build is not reproducible")
    # load config: DependentLoadFlags
    dlf = None
    if len(dirs) > 10 and dirs[10][0]:
        p = off(dirs[10][0])
        size = struct.unpack_from("<I", data, p)[0]
        if size >= 80:
            dlf = struct.unpack_from("<H", data, p + 78)[0]
    facts["DependentLoadFlags"] = dlf
    if dlf != 0x800:
        problems.append("DependentLoadFlags is %s, not 0x800: DLLs could load from the exe's folder" % (hex(dlf) if dlf is not None else "missing"))
    return problems, facts


def main():
    files = [a for a in sys.argv[1:] if not a.startswith("--")]
    if not files:
        print(__doc__)
        sys.exit(2)
    failed = False
    for f in files:
        data = open(f, "rb").read()
        print("%s  %s" % (hashlib.sha256(data).hexdigest(), f))
        problems = []
        if f.lower().endswith(".pdb"):
            problems.append("a .pdb is never published (it holds the build machine's paths)")
        problems += ["path: " + h for h in path_hits(data)]
        pe_problems, facts = pe_checks(data)
        problems += pe_problems
        if facts:
            print("    imports: %s" % ", ".join(facts.get("imports", [])))
            print("    DependentLoadFlags: %s, debug entries: %s" % (facts.get("DependentLoadFlags"), facts.get("debug")))
        for p in problems:
            print("    PROBLEM " + p)
        failed = failed or bool(problems)
    print("FAIL" if failed else "ok")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
