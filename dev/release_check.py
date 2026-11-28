# -*- coding: utf-8 -*-
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Uppwind
"""Uppwind Link -- check a built file before it is published (read-only, no network).

    python -I dev/release_check.py companion/target/release/uppwind-companion.exe [more files…]

For every file: no absolute path of the machine that built it (C:\\Users\\…, /Users/…, a cargo registry or rustup
folder, another drive's folder), in ASCII or UTF-16; for a Windows program, also: DLLs loaded from System32 only
(DependentLoadFlags 0x800), no VCRUNTIME140 / api-ms-win-crt import (the C runtime is inside), a REPRO debug entry
(reproducible build), and a real build date in the PE header and debug entries (dev/pe_timestamp.py: /Brepro alone
writes a hash there, which reads as a date in the future). Prints the SHA-256 of each file. Exit code 1 if anything is wrong. A .pdb is always refused:
it is never published.
"""
import hashlib
import re
import struct
import sys
import time

# the oldest build date taken as real (1 Jan 2020); the newest is now plus 2 days
OLDEST_STAMP = 1577836800

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


def manifest(data, off, dirs):
    """→ the bytes of the first RT_MANIFEST resource, or None."""
    if len(dirs) <= 2 or not dirs[2][0]:
        return None
    base = off(dirs[2][0])
    if base is None:
        return None

    def entries(p):
        named, ids = struct.unpack_from("<HH", data, p + 12)
        for i in range(named + ids):
            yield struct.unpack_from("<II", data, p + 16 + 8 * i)

    for name, child in entries(base):
        if name != 24 or not child & 0x80000000:
            continue
        p = base + (child & 0x7FFFFFFF)
        while True:
            first = next(entries(p), None)
            if first is None:
                return None
            child = first[1]
            if child & 0x80000000:
                p = base + (child & 0x7FFFFFFF)
                continue
            rva, size = struct.unpack_from("<II", data, base + child)
            o = off(rva)
            return data[o:o + size] if o is not None else None
    return None


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

    facts = {"aslr+nx": bool(dllchar & 0x40) and bool(dllchar & 0x100), "cfg": bool(dllchar & 0x4000)}
    problems = []
    if not facts["aslr+nx"]:
        problems.append("ASLR or NX is off")
    # (F11) Control Flow Guard: the GUARD_CF flag here; the function table and GuardFlags are read with the load config
    if not facts["cfg"]:
        problems.append("no Control Flow Guard (-C control-flow-guard in .cargo/config.toml)")
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
    stamps = [struct.unpack_from("<I", data, coff + 4)[0]]
    if len(dirs) > 6 and dirs[6][0]:
        p = off(dirs[6][0])
        for i in range(dirs[6][1] // 28):
            kinds.append(struct.unpack_from("<I", data, p + 28 * i + 12)[0])
            stamps.append(struct.unpack_from("<I", data, p + 28 * i + 4)[0])
    facts["debug"] = kinds
    if 16 not in kinds:
        problems.append("no REPRO debug entry (/Brepro): the build is not reproducible")
    # build date: /Brepro writes a hash there; dev/pe_timestamp.py puts the commit date instead
    facts["timestamp"] = time.strftime("%Y-%m-%d %H:%M:%S UTC", time.gmtime(stamps[0]))
    newest = time.time() + 2 * 86400
    if any(not OLDEST_STAMP <= s <= newest for s in stamps) or len(set(stamps)) != 1:
        problems.append("the PE build date is not a real date (%s): run dev/pe_timestamp.py on it" % ", ".join("%#010x" % s for s in stamps))
    # load config: DependentLoadFlags
    dlf = None
    if len(dirs) > 10 and dirs[10][0]:
        p = off(dirs[10][0])
        size = struct.unpack_from("<I", data, p)[0]
        if size >= 80:
            dlf = struct.unpack_from("<H", data, p + 78)[0]
        # (F11, relecture) Control Flow Guard for real: the table of valid call targets, and GuardFlags saying the
        # code was instrumented (IMAGE_GUARD_CF_INSTRUMENTED 0x100); the DllCharacteristics flag alone is only a claim
        if size >= 0x94:
            facts["cfg_functions"] = struct.unpack_from("<Q", data, p + 0x88)[0]
            facts["cfg_flags"] = struct.unpack_from("<I", data, p + 0x90)[0]
    facts["DependentLoadFlags"] = dlf
    if not (facts.get("cfg_flags", 0) & 0x100) or not facts.get("cfg_functions"):
        problems.append("Control Flow Guard is not instrumented (no GuardFlags 0x100 or an empty function table)")
    # (F11) the manifest: an RT_MANIFEST (24) resource that says asInvoker and PerMonitorV2 (the exe only, not a DLL)
    is_dll = bool(struct.unpack_from("<H", data, coff + 18)[0] & 0x2000)
    man = manifest(data, off, dirs) if not is_dll else None
    facts["manifest"] = bool(man)
    if not is_dll:
        if not man:
            problems.append("no manifest (companion/build/winres.rs)")
        else:
            if b'level="asInvoker"' not in man:
                problems.append("the manifest does not say asInvoker")
            if b"PerMonitorV2" not in man:
                problems.append("the manifest does not say PerMonitorV2")
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
            print("    build date: %s" % facts.get("timestamp"))
            print("    Control Flow Guard: %s (GuardFlags %s, %s functions), manifest: %s" % (facts.get("cfg"), hex(facts.get("cfg_flags", 0)), facts.get("cfg_functions"), facts.get("manifest")))
        for p in problems:
            print("    PROBLEM " + p)
        failed = failed or bool(problems)
    print("FAIL" if failed else "ok")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
