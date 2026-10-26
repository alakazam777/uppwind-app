# -*- coding: utf-8 -*-
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Uppwind
"""Uppwind Link Dev -- l'outil en ligne de commande qui pilote le canal local du script de Live : le vrai script
UppwindLink depuis le 24 oct. (v1.1.0), ou le script de dev de l'etape 0 (ableton/UppwindLinkDev) — celui qui sert.

    python dev/uwlink_dev.py hello
    python dev/uwlink_dev.py list [--track 2]
    python dev/uwlink_dev.py probe [2.0]
    python dev/uwlink_dev.py open 2.0 [--select]
    python dev/uwlink_dev.py close 2.0
    python dev/uwlink_dev.py state 2.0
    python dev/uwlink_dev.py preset 2.0 5
    python dev/uwlink_dev.py events
    python dev/uwlink_dev.py raw '{"cmd": "HELLO"}'

Un chemin « 2.0 » = la piste 2, son device 0 (les deux comptent depuis 0, comme le LOM) ; dans un Rack :
« 2.0.1.0 » = la chaine 1 du device 2.0, son device 0. `list` les donne tous.

Le script, au demarrage de Live, ecrit son port et un jeton dans ~/.uppwind-link-dev.json (UWLINK_DEV_FILE ou --file
pour un autre chemin) ; l'outil les lit, envoie une requete UDP a 127.0.0.1 et attend la reponse (--timeout, 3 s).
Live repond depuis update_display (toutes les ~100 ms) : une reponse prend 0,1 a 0,3 s.
Python 3.7+ ; rien a installer. --json : la reponse brute.
"""

import argparse
import json
import os
import socket
import sys
import time

STATE_NAME = ".uppwind-link-dev.json"
ENV_FILE = "UWLINK_DEV_FILE"
HOST = "127.0.0.1"


def state_file(override=None):
    if override:
        return override
    return os.environ.get(ENV_FILE) or os.path.join(os.path.expanduser("~"), STATE_NAME)


def read_state(path):
    try:
        with open(path) as f:
            st = json.load(f)
    except (IOError, OSError):
        raise SystemExit("No %s: is Live running with UppwindLink (1.1.0 or later) chosen as a Control Surface? "
                         "(see ableton/README.md, Plugin windows)" % path)
    except ValueError:
        raise SystemExit("%s is not readable JSON: restart Live." % path)
    if not isinstance(st, dict) or not st.get("token") or not st.get("port"):
        raise SystemExit("%s has no port or token: restart Live." % path)
    return st


def request(msg, path=None, timeout=3.0):
    """Une requete -> la reponse (dict). Leve TimeoutError si Live ne repond pas."""
    st = read_state(state_file(path))
    body = dict(msg)
    body["token"] = st["token"]
    body["id"] = int(time.time() * 1000) % 1000000000
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        s.bind((HOST, 0))
        s.settimeout(0.2)
        s.sendto(json.dumps(body).encode("utf-8"), (HOST, int(st["port"])))
        t0 = time.time()
        while time.time() - t0 < timeout:
            try:
                raw, addr = s.recvfrom(65535)
            except socket.timeout:
                continue
            except OSError:
                continue            # Windows : 10054 si le port ne repond pas encore
            if addr[0] != HOST:
                continue
            try:
                rep = json.loads(raw.decode("utf-8"))
            except ValueError:
                continue
            if isinstance(rep, dict) and rep.get("id") == body["id"]:
                return rep
        raise TimeoutError("Live did not answer on udp://%s:%s within %.1f s (Live closed? another Live? restart it)"
                           % (HOST, st["port"], timeout))
    finally:
        s.close()


# -- L'affichage ----------------------------------------------------------------------------------
def show_list(rep):
    for t in rep.get("tracks", []):
        print("track %d  %s" % (t["index"], t["name"]))
        for d in t.get("devices", []):
            mark = "PLUGIN" if d.get("plugin") else "      "
            ed = " editor-api" if d.get("editor") else ""
            print("   %-10s %s %s  (%s)%s" % (d["path"], mark, d["name"], d["class_name"], ed))


def show_probe(rep):
    devs = rep.get("devices", [])
    if not devs:
        print("no plugin device in this set")
    for d in devs:
        print("%s  %s  class=%s (%s)" % (d["path"], d["name"], d["class_name"], d.get("class_display_name")))
        if d.get("preset_count") is None:
            print("   presets: <%s>" % d.get("presets_error"))
        else:
            shown = d.get("presets", [])
            print("   presets: %d%s" % (d["preset_count"], (" -> " + " | ".join(shown[:12]) + (" ..." if len(shown) > 12 else "")) if shown else ""))
        sel = d.get("selected_preset_index")
        print("   selected_preset_index: %s" % (sel if "selected_preset_index_error" not in d else "<%s>" % d["selected_preset_index_error"]))
        ed = d.get("is_editor_open")
        print("   is_editor_open: %s" % (ed if "is_editor_open_error" not in d else "<%s>" % d["is_editor_open_error"]))
        print("   parameters: %s" % d.get("parameters"))


def show(cmd, rep):
    if not rep.get("ok"):
        print("ERROR: %s" % rep.get("error"))
        return
    if cmd == "LIST":
        show_list(rep)
    elif cmd == "PROBE":
        show_probe(rep)
    elif cmd == "EVENTS":
        evs = rep.get("events", [])
        if not evs:
            print("nothing changed by itself")
        for e in evs:
            print("%s  %s" % (time.strftime("%H:%M:%S", time.localtime(e["t"])), e["text"]))
    else:
        print(json.dumps(dict((k, v) for k, v in rep.items() if k != "id"), indent=1, ensure_ascii=False))


def build(argv):
    p = argparse.ArgumentParser(prog="uwlink_dev", description="Drive the plugin-window channel of the UppwindLink script (or the older UppwindLinkDev) inside Ableton Live.")
    p.add_argument("--file", help="the state file (default ~/%s)" % STATE_NAME)
    p.add_argument("--timeout", type=float, default=3.0)
    p.add_argument("--json", action="store_true", help="print the raw reply")
    sub = p.add_subparsers(dest="what")
    sub.required = True
    sub.add_parser("hello")
    s = sub.add_parser("list")
    s.add_argument("--track", type=int)
    s = sub.add_parser("probe")
    s.add_argument("path", nargs="?")
    s = sub.add_parser("open")
    s.add_argument("path")
    s.add_argument("--select", action="store_true", help="select the track and the device first")
    for name in ("close", "state"):
        s = sub.add_parser(name)
        s.add_argument("path")
    s = sub.add_parser("preset")
    s.add_argument("path")
    s.add_argument("index", type=int)
    sub.add_parser("events")
    s = sub.add_parser("raw")
    s.add_argument("message", help="a JSON object, e.g. '{\"cmd\": \"HELLO\"}'")
    return p.parse_args(argv)


def message_of(a):
    w = a.what
    if w == "hello":
        return {"cmd": "HELLO"}
    if w == "list":
        return {"cmd": "LIST", "track": a.track} if a.track is not None else {"cmd": "LIST"}
    if w == "probe":
        return {"cmd": "PROBE", "path": a.path} if a.path else {"cmd": "PROBE"}
    if w == "open":
        return {"cmd": "EDITOR_OPEN", "path": a.path, "select": bool(a.select)}
    if w == "close":
        return {"cmd": "EDITOR_CLOSE", "path": a.path}
    if w == "state":
        return {"cmd": "EDITOR_STATE", "path": a.path}
    if w == "preset":
        return {"cmd": "PRESET_SELECT", "path": a.path, "index": a.index}
    if w == "events":
        return {"cmd": "EVENTS"}
    m = json.loads(a.message)
    if not isinstance(m, dict):
        raise SystemExit("raw: give a JSON object")
    return m


def main(argv=None):
    try:
        sys.stdout.reconfigure(errors="replace")      # une console Windows (cp1252) et un preset au nom accentue
    except Exception:
        pass
    a = build(sys.argv[1:] if argv is None else argv)
    msg = message_of(a)
    t0 = time.time()
    try:
        rep = request(msg, a.file, a.timeout)
    except TimeoutError as e:
        print("ERROR: %s" % e)
        return 2
    if a.json:
        print(json.dumps(rep, indent=1, ensure_ascii=False))
    else:
        show(str(msg.get("cmd", "")).upper(), rep)
        print("(%.0f ms)" % ((time.time() - t0) * 1000))
    return 0 if rep.get("ok") else 1


if __name__ == "__main__":
    sys.exit(main())
