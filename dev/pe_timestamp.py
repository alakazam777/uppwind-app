# -*- coding: utf-8 -*-
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Uppwind
"""Uppwind Link -- give a Windows program built with /Brepro a real, reproducible build date.

    python -I dev/pe_timestamp.py [--epoch SECONDS] FILE [more files…]

/Brepro (.cargo/config.toml) makes the build reproducible by writing a hash of the program where the linker would put
the build time: the COFF header's TimeDateStamp, and the TimeDateStamp of every debug directory entry. Read as a
date, that hash can fall anywhere (0.4.15 claimed Sun Sep 9 2091), and scanners flag a PE header "from the future"
(Hybrid Analysis static-3). This script replaces those stamps with one fixed date, so the file stays reproducible:

- the date is --epoch, else SOURCE_DATE_EPOCH, else the commit time of HEAD (`git log -1 --format=%ct`): two builds
  of the same tag get the same date, hence the same bytes;
- the REPRO debug entry and its hash are kept (dev/release_check.py requires them); only the stamps change;
- the PE checksum, if the linker wrote one, is computed again; it is 0 otherwise and stays 0;
- running it twice gives the same bytes.

Run it on the plugins' DLL in the bundle BEFORE the companion embeds it, then on the companion (RELEASING.md step 4).
"""
import os
import struct
import subprocess
import sys
import time


def epoch_from_env():
    v = os.environ.get("SOURCE_DATE_EPOCH", "").strip()
    if v:
        return int(v)
    out = subprocess.run(["git", "log", "-1", "--format=%ct"], capture_output=True, text=True, check=True).stdout
    return int(out.strip())


def pe_checksum(data, checksum_off):
    """The PE checksum (the algorithm of ImageHlp's CheckSumMappedFile), the checksum field counted as 0."""
    total = 0
    n = len(data)
    padded = data + (b"\x00" if n % 2 else b"")
    for i in range(0, len(padded), 2):
        if checksum_off <= i < checksum_off + 4:
            continue
        total += padded[i] | (padded[i + 1] << 8)
        total = (total & 0xFFFF) + (total >> 16)
    total = (total & 0xFFFF) + (total >> 16)
    return (total + n) & 0xFFFFFFFF


def patch(data, stamp):
    """→ (new bytes, list of what changed). Raises ValueError if the file is not a PE file."""
    buf = bytearray(data)
    if buf[:2] != b"MZ":
        raise ValueError("not a PE file (no MZ header)")
    pe = struct.unpack_from("<I", buf, 0x3C)[0]
    if buf[pe:pe + 4] != b"PE\x00\x00":
        raise ValueError("not a PE file (no PE signature)")
    coff = pe + 4
    nsec = struct.unpack_from("<H", buf, coff + 2)[0]
    opt_size = struct.unpack_from("<H", buf, coff + 16)[0]
    opt = coff + 20
    magic = struct.unpack_from("<H", buf, opt)[0]
    if magic == 0x20B:
        dir_base, ndirs_off = opt + 112, opt + 108
    elif magic == 0x10B:
        dir_base, ndirs_off = opt + 96, opt + 92
    else:
        raise ValueError("unknown optional header (magic %#x)" % magic)
    checksum_off = opt + 64
    changes = []

    old = struct.unpack_from("<I", buf, coff + 4)[0]
    struct.pack_into("<I", buf, coff + 4, stamp)
    changes.append("COFF TimeDateStamp %#010x -> %#010x" % (old, stamp))

    ndirs = struct.unpack_from("<I", buf, ndirs_off)[0]
    secs = []
    so = opt + opt_size
    for i in range(nsec):
        vsize, va, rsize, raw = struct.unpack_from("<IIII", buf, so + 40 * i + 8)
        secs.append((va, max(vsize, rsize), raw))

    def off(rva):
        for va, size, raw in secs:
            if va <= rva < va + size:
                return raw + rva - va
        return None

    if ndirs > 6:
        rva, size = struct.unpack_from("<II", buf, dir_base + 8 * 6)
        p = off(rva) if rva else None
        if p is not None:
            for i in range(size // 28):
                e = p + 28 * i
                kind = struct.unpack_from("<I", buf, e + 12)[0]
                old = struct.unpack_from("<I", buf, e + 4)[0]
                struct.pack_into("<I", buf, e + 4, stamp)
                changes.append("debug entry %d (type %d) TimeDateStamp %#010x -> %#010x" % (i, kind, old, stamp))

    old_sum = struct.unpack_from("<I", buf, checksum_off)[0]
    if old_sum:
        new_sum = pe_checksum(bytes(buf), checksum_off)
        struct.pack_into("<I", buf, checksum_off, new_sum)
        changes.append("checksum %#010x -> %#010x" % (old_sum, new_sum))
    return bytes(buf), changes


def main():
    args = sys.argv[1:]
    epoch = None
    if "--epoch" in args:
        i = args.index("--epoch")
        epoch = int(args[i + 1])
        del args[i:i + 2]
    if not args or any(a.startswith("--") for a in args):
        print(__doc__)
        sys.exit(2)
    if epoch is None:
        epoch = epoch_from_env()
    if not 0 < epoch < 2 ** 32:
        print("the date %d does not fit a PE header" % epoch)
        sys.exit(2)
    print("build date: %s UTC (%d)" % (time.strftime("%Y-%m-%d %H:%M:%S", time.gmtime(epoch)), epoch))
    for f in args:
        data = open(f, "rb").read()
        try:
            new, changes = patch(data, epoch)
        except ValueError as e:
            print("%s: %s" % (f, e))
            sys.exit(1)
        if new != data:
            with open(f, "wb") as out:
                out.write(new)
        print(f)
        for c in changes:
            print("    " + c)


if __name__ == "__main__":
    main()
