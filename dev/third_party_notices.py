# -*- coding: utf-8 -*-
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Uppwind
"""Uppwind Link -- rewrite the third-party notices from the crates each program really links (offline).

    python -I dev/third_party_notices.py [--check]

Two files: THIRD-PARTY-NOTICES.txt (the companion, uppwind-companion.exe) and, when plugin/plugins/ is there,
plugin/THIRD-PARTY-NOTICES.txt (the Uppwind Link plugins, the .vst3 bundle). Asks cargo which crates end up in each Windows binary (`cargo tree --target
x86_64-pc-windows-msvc -e normal`, proc macros and their build-only dependencies left out, our own path crates
left out), and for the companion also in the macOS one (aarch64-apple-darwin and x86_64-apple-darwin: the crates
only the Mac program links get a list of their own), then reads each crate's licence files from the local cargo
registry (CARGO_HOME, %USERPROFILE%\\.cargo by default). Run it after any dependency change; --check only says
whether the files are up to date (for CI). Nothing is downloaded: `cargo fetch --locked --manifest-path
companion/Cargo.toml` once first (it fetches the crates of every target, the macOS ones included).
"""
import os
import re
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
LINK = os.path.dirname(HERE)
OUT = os.path.join(LINK, "THIRD-PARTY-NOTICES.txt")
PLUGIN_OUT = os.path.join(LINK, "plugin", "THIRD-PARTY-NOTICES.txt")
TARGET = "x86_64-pc-windows-msvc"
MAC_TARGETS = ("aarch64-apple-darwin", "x86_64-apple-darwin")


def linked_and_build_only(manifest, package=None, target=TARGET):
    """(linked, build_only) : [(name, version)] from cargo tree. Path crates (ours: uwl-proto…) are left out."""
    cmd = ["cargo", "tree", "--offline", "--locked", "--manifest-path", manifest]
    if package:
        cmd += ["-p", package]
    cmd += ["--target", target, "-e", "normal", "--prefix", "depth", "--format", "{p}"]
    out = subprocess.run(cmd, check=True, capture_output=True, text=True).stdout
    linked, build, macro_depth = {}, set(), None
    for line in out.splitlines():
        m = re.match(r"^(\d+)(\S+) v(\S+)(.*)$", line.strip())
        if not m:
            continue
        depth, name, ver, rest = int(m.group(1)), m.group(2), m.group(3), m.group(4)
        if depth == 0:
            continue
        if macro_depth is not None and depth > macro_depth:
            build.add(name)
            continue
        macro_depth = None
        if "(proc-macro)" in rest:
            build.add(name)
            macro_depth = depth
            continue
        if re.search(r"\((?![^)]*proc-macro)[^)]*[\\/][^)]*\)", rest):
            continue  # a local path crate of this repository (GPL-3.0-or-later, like the rest)
        linked[name] = ver
    return sorted(linked.items()), sorted(build - set(linked))


def registry_dirs():
    home = os.environ.get("CARGO_HOME") or os.path.join(os.path.expanduser("~"), ".cargo")
    base = os.path.join(home, "registry", "src")
    return [os.path.join(base, d) for d in sorted(os.listdir(base))] if os.path.isdir(base) else []


def crate_dir(name, ver):
    for r in registry_dirs():
        d = os.path.join(r, "%s-%s" % (name, ver))
        if os.path.isdir(d):
            return d
    raise SystemExit("%s %s is not in the local cargo registry: build the companion and the plugins once" % (name, ver))


def toml_field(text, key):
    m = re.search(r'^%s\s*=\s*(\[[^\]]*\]|.+)$' % re.escape(key), text, re.M | re.S)
    if not m:
        return None
    v = m.group(1).strip()
    if v.startswith("["):
        items = [x.strip().strip('"') for x in v.strip("[]").replace("\n", ",").split(",")]
        return ", ".join(x for x in items if x)
    v = v.splitlines()[0].strip()
    return v.strip('"')


def read_text(d, names, marker):
    for name in names:
        p = os.path.join(d, name)
        if os.path.isfile(p):
            t = open(p, encoding="utf-8").read().replace("\r\n", "\n").strip("\n")
            if marker in t:
                return t
    return None


def licence_files(d):
    return [f for f in sorted(os.listdir(d)) if f.upper().startswith(("LICEN", "COPYING", "UNLICENSE"))]


def mit_text(d):
    t = read_text(d, ("LICENSE-MIT", "LICENSE-MIT.txt", "LICENSE-MIT.md", "LICENSE.MIT", "LICENSE-LIBM-MIT", "LICENSE"),
                  "Permission is hereby granted, free of charge")
    # the Unicode licence uses the same words: it is not the MIT licence
    return None if t is None or "UNICODE LICENSE" in t.upper() else t


def copyright_lines(d):
    out = []
    for f in licence_files(d):
        t = open(os.path.join(d, f), encoding="utf-8").read()
        out += [l.strip() for l in t.splitlines() if l.strip().lower().startswith("copyright ") and "[yyyy]" not in l
                and not l.strip().lower().startswith("copyright holders")]
    return out


# The MIT and ISC licences as published by the OSI, for a crate whose Cargo.toml names the licence but which ships no
# text of it (webview2-com, nice-plug…): the copyright belongs to the crate's authors (see the list and its repository).
STANDARD_MIT = """Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the Software without restriction, including without limitation
the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to
permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of
the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE
WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR
COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR
OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE."""

STANDARD_ISC = """Permission to use, copy, modify, and/or distribute this software for any purpose with or without fee is hereby
granted, provided that the above copyright notice and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH REGARD TO THIS SOFTWARE INCLUDING ALL
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN
AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR
PERFORMANCE OF THIS SOFTWARE."""

NO_FILE = " (standard text: the crate ships none)"


def has_option(lic, spdx):
    """Is `spdx` one of the choices of an OR expression (or the whole licence)?"""
    return re.search(r"(^|[\s(])%s($|[\s)])" % re.escape(spdx), lic) is not None


def chosen(name, ver, d, lic):
    """The licence(s) a crate is used under → [(kind, text)]. Any expression not handled here stops the script."""
    lic = lic.replace("/", " OR ")  # the old « MIT/Apache-2.0 » spelling
    if lic == "Apache-2.0 AND MIT":  # dpi: Apache-2.0, and a libm part under MIT; both apply
        a, m = read_text(d, ("LICENSE-APACHE", "LICENSE"), "Apache License"), mit_text(d)
        if not a or not m:
            raise SystemExit("%s %s: missing the Apache-2.0 or the MIT text" % (name, ver))
        return [("Apache-2.0", a), ("MIT", m)]
    if has_option(lic, "MIT") and " AND " not in lic.replace("AND IJG", ""):
        t = mit_text(d)
        if t is not None:
            return [("MIT", t)]
        # no MIT text in the crate (none at all, or only a COPYING that names the choices): the standard text, after
        # the copyright lines the crate does carry
        held = copyright_lines(d)
        return [("MIT" + NO_FILE, "\n".join(held + ([""] if held else []) + [STANDARD_MIT]))]
    if lic == "ISC":
        t = read_text(d, ("LICENSE", "LICENSE-ISC", "LICENSE.md", "COPYING"), "Permission to use, copy, modify, and/or distribute")
        if t is not None:
            return [("ISC", t)]
        if licence_files(d):
            raise SystemExit("no ISC text for %s %s" % (name, ver))
        return [("ISC" + NO_FILE, STANDARD_ISC)]
    if lic == "Unicode-3.0":
        t = read_text(d, ("LICENSE",), "UNICODE LICENSE V3")
        if not t:
            raise SystemExit("no Unicode-3.0 text for %s %s" % (name, ver))
        return [("Unicode-3.0", t)]
    if has_option(lic, "CC0-1.0"):
        t = read_text(d, ("LICENSE", "LICENSE-CC0"), "CC0 1.0 Universal")
        if not t:
            raise SystemExit("no CC0 text for %s %s" % (name, ver))
        return [("CC0-1.0", t)]
    if lic == "MPL-2.0":  # option-ext, macOS only: file-level copyleft, compatible with the GPL (MPL-2.0 section 3.3)
        t = read_text(d, ("LICENSE", "LICENSE.txt", "LICENSE.md", "LICENSE-MPL"), "Mozilla Public License Version 2.0")
        if not t:
            raise SystemExit("no MPL-2.0 text for %s %s" % (name, ver))
        return [("MPL-2.0", t)]
    if lic == "Apache-2.0":
        t = read_text(d, ("LICENSE-APACHE", "LICENSE"), "Apache License")
        if not t:
            raise SystemExit("no Apache-2.0 text for %s %s" % (name, ver))
        out = [("Apache-2.0", t)]
        n = read_text(d, ("NOTICE", "NOTICE.txt", "NOTICE.md"), "")
        if n:
            out.append(("Apache-2.0 NOTICE of %s" % name, n))
        return out
    raise SystemExit("%s %s: licence %r is not handled: check it is GPL-3.0 compatible and add it here" % (name, ver, lic))


# Not a crate: a Microsoft binary that webview2-com-sys ships and that is linked statically into the companion.
WEBVIEW2_LOADER = [
    "Microsoft WebView2 loader: WebView2LoaderStatic.lib (x64), from Microsoft's WebView2 SDK (NuGet package",
    "Microsoft.Web.WebView2), shipped inside the webview2-com-sys crate and linked statically into",
    "uppwind-companion.exe. Copyright (C) Microsoft Corporation. Used under the WebView2 SDK's licence:",
    "https://www.nuget.org/packages/Microsoft.Web.WebView2 (see its LICENSE.txt). It finds and starts the",
    "WebView2 Runtime that Windows provides; no part of the runtime is in the program. The companion's licence",
    "allows this link: see the additional permission under GNU GPL version 3 section 7 in LICENSE-EXCEPTION.txt.",
]

COMPANION_HEAD = [
    "Uppwind Link companion -- third-party notices", "=" * 46, "",
    "The Uppwind Link companion (uppwind-companion.exe on Windows, uppwind-companion on macOS) is free software under",
    "the GNU General Public License, version 3 or (at your option) any later version: see LICENSE. It is built with",
    "the Rust crates listed below, downloaded from crates.io when you build it (none of their source is in this",
    "repository): a first list for Windows, then the crates that only the macOS program links.", "",
    "Crates offered under \"MIT OR Apache-2.0\", \"Apache-2.0 OR MIT\", \"MIT OR Apache-2.0 OR Zlib\" or",
    "\"Unlicense OR MIT\" are used under the MIT option; the others under the licence given in the list (Apache-2.0,",
    "Unicode-3.0, CC0-1.0, MPL-2.0), all compatible with the GPL version 3. jpeg-encoder also contains code derived",
    "from the Independent JPEG Group's software (its fdct.rs and quantization tables), under the IJG licence below:", "",
    "    This software is based in part on the work of the Independent JPEG Group.", "",
    "zmij is a Rust port of Victor Zverovich's Zmij (C++, MIT licence).", "",
    "option-ext (macOS program only) is under the Mozilla Public License 2.0 (text below), used unmodified: its source",
    "code is available from crates.io (https://crates.io/crates/option-ext) and from its repository given in the list.",
    "The MPL-2.0 allows its combination with the GPL (its section 3.3).", ""] + WEBVIEW2_LOADER + ["",
    "The scripts for Ableton Live and FL Studio (ableton/UppwindLink/, fl-studio/, shared/) are under the MIT",
    "licence (LICENSE-MIT), also the copies that the companion embeds and installs (--setup). They, the dev tool",
    "and the tests use only the Python standard library.", ""]

PLUGIN_HEAD = [
    "Uppwind Link plugins -- third-party notices", "=" * 43, "",
    "The Uppwind Link plugins (Uppwind Link Return and Uppwind Link Notes, the \"Uppwind Link.vst3\" bundle) are free",
    "software under the GNU General Public License, version 3 or (at your option) any later version: see ../LICENSE.",
    "They are built with the Rust crates listed below, downloaded from crates.io when you build them (none of their",
    "source is in this repository).", "",
    "Crates offered under several licences (\"MIT OR Apache-2.0\", \"BlueOak-1.0.0 OR MIT OR Apache-2.0\",",
    "\"Apache-2.0 OR MIT OR Unlicense\"…) are used under the MIT option; nice-plug, nice-plug-core and nice-log under",
    "the ISC licence; all compatible with the GPL version 3. The VST3 interfaces come from the vst3 crate (bindings",
    "generated from Steinberg's VST 3 SDK headers, MIT OR Apache-2.0). VST is a registered trademark of Steinberg",
    "Media Technologies GmbH.", "",
    "The test host and the bundler (testhost/: uwl-host, uwl-bundle) are development tools: they are not part of the",
    "bundle.", ""]


def render(head, title, manifest, package=None, mac_title=None):
    """The notices of one program: its Windows crates, then (mac_title given) the crates only its macOS build links."""
    linked, build_only = linked_and_build_only(manifest, package)
    lists = [(title, linked)]
    if mac_title:
        mac, mac_build = set(), set(build_only)
        for t in MAC_TARGETS:
            l, b = linked_and_build_only(manifest, package, t)
            mac.update(l)
            mac_build.update(b)
        lists.append((mac_title, sorted(mac - set(linked))))
        everything = {n for _, crates in lists for n, _ in crates}
        build_only = sorted(mac_build - everything)
    groups, ijg, sections = {}, None, []
    for list_title, crates in lists:
        rows = []
        for name, ver in crates:
            d = crate_dir(name, ver)
            cargo = open(os.path.join(d, "Cargo.toml"), encoding="utf-8").read()
            lic = toml_field(cargo, "license") or "?"
            texts = chosen(name, ver, d, lic)
            holders = []
            for kind, t in texts:
                # (a wrapped MIT text has a line starting « COPYRIGHT HOLDERS BE LIABLE »: not a holder)
                holders += [l.strip() for l in t.splitlines() if l.strip().lower().startswith("copyright") and "notice" not in l.lower()
                            and "[yyyy]" not in l and not l.strip().lower().startswith("copyright holders")]
                groups.setdefault((kind, t), []).append("%s %s" % (name, ver))
            if texts[0][0] in ("Unicode-3.0", "CC0-1.0", "MPL-2.0"):
                holders = [h for h in holders if h.lower().startswith("copyright ©") or h.lower().startswith("copyright (c)")][:1]
            rows.append((name, ver, lic, holders, toml_field(cargo, "authors") or "", toml_field(cargo, "repository") or ""))
            if "IJG" in lic:
                ijg = open(os.path.join(d, "LICENSE-IJG"), encoding="utf-8").read().replace("\r\n", "\n").strip("\n")
        sections.append((list_title, rows))
    no_mail = lambda s: re.sub(r"\s*<[^>]*@[^>]*>", "", s)  # noqa: E731 (the licence texts below keep them verbatim)
    w = list(head)
    for i, (list_title, rows) in enumerate(sections):
        w += ([""] if i else []) + [list_title, "-" * 74]
        for name, ver, lic, holders, authors, repo in rows:
            who = no_mail("; ".join(dict.fromkeys(holders)) if holders else ("authors: " + authors if authors else ""))
            w.append("  %-20s %-9s %s" % (name, ver, lic))
            if who:
                w.append("      %s" % who)
            if repo:
                w.append("      %s" % repo)
    w += ["", "Used only while building (not part of the program): " + ", ".join(build_only) + ".", "",
          "Licence texts", "-" * 13]
    for (kind, text), crates in sorted(groups.items(), key=lambda kv: (kv[1][0], kv[0][0])):
        w += ["", "--- %s licence, as shipped by: %s" % (kind, ", ".join(crates)), "", text]
    if ijg:
        w += ["", "--- The Independent JPEG Group's licence (jpeg-encoder, LICENSE-IJG)", "", ijg]
    w.append("")
    return "\n".join(w)


def main():
    files = [
        (OUT, render(COMPANION_HEAD, "Crates linked into uppwind-companion.exe (Windows, %s)" % TARGET,
                     os.path.join(LINK, "companion", "Cargo.toml"),
                     mac_title="Crates linked only into the macOS uppwind-companion (%s)" % ", ".join(MAC_TARGETS))),
    ]
    # the plugins are not in every copy of this folder (not published yet: only plugin/proto is, for the companion)
    if os.path.isfile(os.path.join(LINK, "plugin", "plugins", "Cargo.toml")):
        files.append((PLUGIN_OUT, render(PLUGIN_HEAD, "Crates linked into the plugins' DLL (Windows, %s)" % TARGET,
                                         os.path.join(LINK, "plugin", "Cargo.toml"), "uppwind-link")))
    stale = 0
    for path, text in files:
        old = open(path, encoding="utf-8").read() if os.path.isfile(path) else ""
        name = os.path.relpath(path, LINK).replace("\\", "/")
        if "--check" in sys.argv:
            print("%s is %s" % (name, "up to date" if old == text else "STALE: run dev/third_party_notices.py"))
            stale += old != text
            continue
        with open(path, "w", encoding="utf-8", newline="\n") as f:
            f.write(text)
        print("wrote %s" % name)
    sys.exit(1 if stale else 0)


if __name__ == "__main__":
    main()
