# -*- coding: utf-8 -*-
# SPDX-License-Identifier: MIT
# Copyright (C) 2026 Uppwind
"""Uppwind Link -- le protocole MIDI, cote script du logiciel hote (FL Studio, Ableton Live).

Jumeau de l'implementation JavaScript du studio (link-protocol.js, servi par Uppwind Studio) : MEMES regles,
octet pour octet. Les deux sont testes contre fixtures/*.json. Ce fichier (shared/) est la SOURCE ; une copie
identique vit dans chaque script hote (fl-studio/, ableton/UppwindLink/) -- tests/test_protocol.py verifie
qu'elles n'ont pas diverge.

Contraintes : Python 3.7 (Ableton Live 11) a 3.12 (FL Studio 2024+), bibliotheque standard
minimale (json, math) -- le bac a sable des scripts de FL n'a ni fichiers ni reseau.

SysEx : F0 7D 55 57 <ver=01> <cmd> <req> <seq> <last> <data...> F7 ; data = JSON canonique en
ASCII 7 bits, 200 octets au plus par morceau, 128 morceaux au plus. Voir link-protocol.js pour
la table des commandes et le sens de chaque champ.
"""

import json
import math

LINK_VERSION = 1
SYSEX_ID = (0x7D, 0x55, 0x57)
MAX_CHUNK_DATA = 200
MAX_CHUNKS = 128
MAX_MESSAGE_BYTES = MAX_CHUNK_DATA * MAX_CHUNKS
SLOT_MIN = 1
SLOT_MAX = 16
MAX_ID = 65535
PING_INTERVAL_MS = 3000
LINK_TIMEOUT_MS = 15000
REASSEMBLY_TIMEOUT_MS = 3000
# Le tempo (cap 'tempo', cf. link-protocol.js) : bornes, 3 decimales, au plus un message toutes les
# 50 ms, deux tempos a moins de TEMPO_EPS sont le meme, le script ignore son tempo TEMPO_HOLD_MS apres
# un TEMPO du studio (il ne le renvoie pas)
TEMPO_MIN = 20
TEMPO_MAX = 999
TEMPO_DECIMALS = 3
TEMPO_MIN_INTERVAL_MS = 50
TEMPO_EPS = 0.001
TEMPO_ECHO_MS = 1500
TEMPO_HOLD_MS = 300
PASS_CCS = (1, 7, 10, 11, 64)
ALL_OFF_CCS = (120, 123)
PORT_NAMES = {"toDaw": "Uppwind Link", "fromDaw": "Uppwind Link Back"}

HELLO = 0x01
HELLO_ACK = 0x02
LIST = 0x10
LIST_REPLY = 0x11
CHANGED = 0x12
BIND = 0x20
BIND_ACK = 0x21
UNBIND = 0x22
SET = 0x30
PLUGINS = 0x40
PLUGINS_REPLY = 0x41
ADD = 0x42
ADDED = 0x43
REMOVE = 0x44
TEMPO = 0x50
TEMPO_CHANGED = 0x51
PING = 0x70
PONG = 0x71
ERROR = 0x7F

CAPS = ("list", "bind", "set", "plugins", "add", "remove", "audition", "tempo", "notes")
# (25 oct.) BIND.via : 'port' (le port loopMIDI « Uppwind Link », canal n) ou 'plugin' (cap 'notes' : la piste prend
# ses notes du plugin Uppwind Link Notes, sans port MIDI) ; BIND_ACK.via le redit, BIND_ACK.notes = le numero du Notes
VIAS = ("port", "plugin")
NOTES_TAG_MAX = 255

ERR_NOT_7BIT = "not_7bit"
ERR_BAD_FRAME = "bad_frame"
ERR_VERSION = "version"
ERR_TOO_LONG = "too_long"
ERR_SEQ = "seq"
ERR_TOO_LARGE = "too_large"
ERR_BAD_JSON = "bad_json"
ERR_UNKNOWN_CMD = "unknown_cmd"
ERR_WRONG_DIRECTION = "wrong_direction"
ERR_INVALID = "invalid"
ERR_UNSUPPORTED = "unsupported"
ERR_NOT_FOUND = "not_found"
ERR_INTERNAL = "internal"


# -- Les schemas (memes regles que link-protocol.js) ---------------------------------------
def _int(lo, hi, req=False):
    return {"t": "int", "lo": lo, "hi": hi, "req": req}


def _num(lo, hi, req=False, dp=None):
    return {"t": "num", "lo": lo, "hi": hi, "req": req, "dp": dp}


def _str(mx, mn=0, req=False):
    return {"t": "str", "max": mx, "min": mn, "req": req}


def _list(item, mx, req=False):
    return {"t": "list", "item": item, "max": mx, "req": req}


def _obj(fields, req=False):
    # fields : liste de (cle, type) -- l'ordre est celui de la sortie
    return {"t": "obj", "fields": fields, "req": req}


def _req(t):
    r = dict(t)
    r["req"] = True
    return r


_COLOR = {"t": "color", "req": False}
_CAPS_T = {"t": "caps", "req": False}
_BOOL = {"t": "bool", "req": False}
_NEXT = {"t": "nint", "lo": 0, "hi": 1000000, "req": False}
_SLOT = _int(SLOT_MIN, SLOT_MAX)
_ID = _int(0, MAX_ID)
_BPM = _num(TEMPO_MIN, TEMPO_MAX, dp=TEMPO_DECIMALS)
_VIA = {"t": "enum", "of": VIAS, "req": False}

ITEM_SCHEMA = _obj([
    ("id", _req(_ID)), ("name", _req(_str(64))), ("plugin", _str(64)), ("kind", _str(24)),
    ("color", _COLOR), ("slots", _list(_SLOT, 16)),
])
PLUGIN_SCHEMA = _obj([
    ("id", _req(_str(200, 1))), ("name", _req(_str(80))), ("vendor", _str(60)), ("kind", _str(24)),
])

# code -> (nom, de qui, reponse, capacite, schema)
COMMANDS = {
    HELLO: ("HELLO", "studio", HELLO_ACK, None,
            _obj([("v", _req(_int(0, 127))), ("app", _str(40)), ("caps", _CAPS_T)])),
    HELLO_ACK: ("HELLO_ACK", "daw", None, None,
                _obj([("v", _req(_int(0, 127))), ("daw", _req(_str(40))), ("dawVersion", _str(60)),
                      ("project", _str(120)), ("caps", _req(_CAPS_T)), ("bpm", _BPM)])),
    LIST: ("LIST", "studio", LIST_REPLY, "list", _obj([("start", _int(0, 1000000))])),
    LIST_REPLY: ("LIST_REPLY", "daw", None, None,
                 _obj([("items", _req(_list(ITEM_SCHEMA, 1000))), ("next", _NEXT)])),
    CHANGED: ("CHANGED", "daw", None, None, _obj([])),
    BIND: ("BIND", "studio", BIND_ACK, "bind", _obj([("slot", _req(_SLOT)), ("id", _req(_ID)), ("via", _VIA)])),
    BIND_ACK: ("BIND_ACK", "daw", None, None, _obj([("slot", _req(_SLOT)), ("id", _req(_ID)), ("via", _VIA),
                                                    ("notes", _int(0, NOTES_TAG_MAX))])),
    UNBIND: ("UNBIND", "studio", None, "bind", _obj([("slot", _req(_SLOT))])),
    SET: ("SET", "studio", None, "set",
          _obj([("id", _req(_ID)), ("vol", _num(0, 1)), ("pan", _num(-1, 1)), ("mute", _BOOL),
                ("solo", _BOOL)])),
    PLUGINS: ("PLUGINS", "studio", PLUGINS_REPLY, "plugins", _obj([("start", _int(0, 1000000))])),
    PLUGINS_REPLY: ("PLUGINS_REPLY", "daw", None, None,
                    _obj([("items", _req(_list(PLUGIN_SCHEMA, 2000))), ("next", _NEXT)])),
    ADD: ("ADD", "studio", ADDED, "add", _obj([("plugin", _req(_str(200, 1))), ("name", _str(64))])),
    ADDED: ("ADDED", "daw", None, None, _obj([("item", _req(ITEM_SCHEMA))])),
    REMOVE: ("REMOVE", "studio", None, "remove", _obj([("id", _req(_ID))])),
    TEMPO: ("TEMPO", "studio", None, "tempo", _obj([("bpm", _req(_BPM))])),
    TEMPO_CHANGED: ("TEMPO_CHANGED", "daw", None, None, _obj([("bpm", _req(_BPM))])),
    PING: ("PING", "studio", PONG, None, _obj([("t", _req(_num(-1e14, 1e14)))])),
    PONG: ("PONG", "daw", None, None, _obj([("t", _req(_num(-1e14, 1e14)))])),
    ERROR: ("ERROR", "any", None, None, _obj([("code", _req(_str(32, 1))), ("msg", _str(200))])),
}


def command_name(cmd):
    c = COMMANDS.get(cmd)
    return c[0] if c else "0x%02X" % (int(cmd) & 0xFF)


# -- Le JSON canonique -------------------------------------------------------------------
def _is_number(v):
    return isinstance(v, (int, float)) and not isinstance(v, bool)


def _canon_number(x):
    if isinstance(x, int):
        if abs(x) >= 10 ** 15:
            raise ValueError("link: number out of range")
        return x
    x = float(x)
    if math.isnan(x) or math.isinf(x):
        raise ValueError("link: numbers must be finite")
    if abs(x) >= 1e15:
        raise ValueError("link: number out of range")
    if x.is_integer():
        return int(x)
    # Math.round de JS : arrondi au plus proche, demi vers +infini
    y = x * 10000.0
    f = math.floor(y)
    n = f + 1 if (y - f) >= 0.5 else f
    if n == 0:
        return 0
    # String(n / 1e4) de JS : un quotient entier s'ecrit sans « .0 » -- meme quand n n'est pas un
    # multiple de 10000 (au-dela de ~1e11, la division arrondit a un entier : 489653094195265.06)
    r = n / 10000.0
    return int(r) if r.is_integer() else r


def _round_dp(x, dp):
    """roundDp de JS : un entier reste tel quel ; sinon x * 10^dp, l'entier le plus proche (demi vers
    +infini, comme Math.round), / 10^dp -- les memes operations IEEE, donc le meme nombre."""
    if isinstance(x, int):
        return x
    k = 10.0 ** dp
    y = x * k
    if math.isinf(y):              # Math.round(Infinity) / k = Infinity (math.floor leverait)
        return y / k
    f = math.floor(y)
    n = f + 1 if (y - f) >= 0.5 else f
    return n / k


def round_bpm(bpm):
    """Le bpm tel qu'il part (roundBpm) : ramene dans TEMPO_MIN..TEMPO_MAX, 3 decimales ; None sinon."""
    if not _is_number(bpm) or (isinstance(bpm, float) and (math.isnan(bpm) or math.isinf(bpm))):
        return None
    return min(TEMPO_MAX, max(TEMPO_MIN, _round_dp(bpm, TEMPO_DECIMALS)))


def _normalize(v):
    if v is None or isinstance(v, bool) or isinstance(v, str):
        return v
    if _is_number(v):
        return _canon_number(v)
    if isinstance(v, (list, tuple)):
        return [_normalize(x) for x in v]
    if isinstance(v, dict):
        out = {}
        for k, x in v.items():
            if not isinstance(k, str):
                raise TypeError("link: object keys must be text")
            out[k] = _normalize(x)
        return out
    raise TypeError("link: cannot encode %s" % type(v).__name__)


def canonical_json(value):
    return json.dumps(_normalize(value), ensure_ascii=True, separators=(",", ":"), allow_nan=False)


# -- Encoder -----------------------------------------------------------------------------
def encode_message(cmd, req, data=None):
    """Un message -> la liste de ses morceaux SysEx (bytes, chacun F0 ... F7)."""
    if cmd not in COMMANDS:
        raise ValueError("link: unknown command %r" % (cmd,))
    if not isinstance(req, int) or isinstance(req, bool) or req < 0 or req > 127:
        raise ValueError("link: req must be 0..127")
    if data is None:
        data = {}
    if not isinstance(data, dict):
        raise TypeError("link: data must be an object")
    text = canonical_json(data).encode("ascii")
    if len(text) > MAX_MESSAGE_BYTES:
        raise ValueError("link: message too large")
    count = max(1, (len(text) + MAX_CHUNK_DATA - 1) // MAX_CHUNK_DATA)
    chunks = []
    for seq in range(count):
        part = text[seq * MAX_CHUNK_DATA:(seq + 1) * MAX_CHUNK_DATA]
        head = bytes([0xF0, SYSEX_ID[0], SYSEX_ID[1], SYSEX_ID[2], LINK_VERSION, cmd, req, seq,
                      1 if seq == count - 1 else 0])
        chunks.append(head + part + bytes([0xF7]))
    return chunks


def encode_error(req, code, msg=None):
    data = {"code": str(code)}
    if msg is not None and msg != "":
        data["msg"] = str(msg)[:200]       # 200 points de code, comme encodeError
    return encode_message(ERROR, req, data)


# -- Decoder un morceau ------------------------------------------------------------------
def parse_chunk(raw):
    """-> None (pas a nous), {'error', 'cmd'?, 'req'?}, ou {'ver','cmd','req','seq','last','data'}."""
    try:
        b = list(raw)
    except TypeError:
        return None
    lo, hi = 0, len(b)
    if hi > 0 and b[0] == 0xF0:
        lo = 1
    if hi > lo and b[hi - 1] == 0xF7:
        hi -= 1
    if hi - lo < 3 or b[lo] != SYSEX_ID[0] or b[lo + 1] != SYSEX_ID[1] or b[lo + 2] != SYSEX_ID[2]:
        return None
    for i in range(lo, hi):
        v = b[i]
        if not isinstance(v, int) or isinstance(v, bool) or v < 0 or v > 0x7F:
            return {"error": ERR_NOT_7BIT}
    if hi - lo < 8:
        return {"error": ERR_BAD_FRAME}
    ver, cmd, req, seq, last = b[lo + 3], b[lo + 4], b[lo + 5], b[lo + 6], b[lo + 7]
    if ver != LINK_VERSION:
        return {"error": ERR_VERSION, "cmd": cmd, "req": req}
    if last not in (0, 1):
        return {"error": ERR_BAD_FRAME, "cmd": cmd, "req": req}
    if hi - lo - 8 > MAX_CHUNK_DATA:
        return {"error": ERR_TOO_LONG, "cmd": cmd, "req": req}
    if cmd not in COMMANDS:
        return {"error": ERR_UNKNOWN_CMD, "cmd": cmd, "req": req}
    return {"ver": ver, "cmd": cmd, "req": req, "seq": seq, "last": last,
            "data": bytes(b[lo + 8:hi])}


def is_link_sysex(raw):
    try:
        b = list(raw[:4])
    except TypeError:
        return False
    if len(b) < 3:
        return False
    a = 1 if b[0] == 0xF0 else 0
    return len(b) >= a + 3 and b[a] == SYSEX_ID[0] and b[a + 1] == SYSEX_ID[1] and b[a + 2] == SYSEX_ID[2]


def _reject_constant(name):
    raise ValueError("link: %s is not JSON" % name)


# -- Reassembler -------------------------------------------------------------------------
class Reassembler(object):
    """push(raw, now_ms) -> {'type': 'ignored'|'partial'|'error'|'message', ...} (cf. JS)."""

    def __init__(self, timeout_ms=REASSEMBLY_TIMEOUT_MS, max_pending=16):
        self.timeout_ms = timeout_ms
        self.max_pending = max_pending
        self.pending = []   # [(cle, entree)] dans l'ordre d'arrivee

    def reset(self):
        self.pending = []

    def push(self, raw, now=0):
        t = now if _is_number(now) else 0
        self.pending = [(k, p) for (k, p) in self.pending if t - p["t0"] <= self.timeout_ms]
        c = parse_chunk(raw)
        if c is None:
            return {"type": "ignored"}
        if "error" in c:
            return {"type": "error", "code": c["error"], "cmd": c.get("cmd"), "req": c.get("req")}
        key = (c["cmd"], c["req"])
        at = -1
        for i, (k, _p) in enumerate(self.pending):
            if k == key:
                at = i
                break
        if c["seq"] == 0:
            entry = {"next": 0, "parts": [], "size": 0, "t0": t}
        else:
            if at < 0 or self.pending[at][1]["next"] != c["seq"]:
                if at >= 0:
                    del self.pending[at]
                return {"type": "error", "code": ERR_SEQ, "cmd": c["cmd"], "req": c["req"]}
            entry = self.pending[at][1]
        if at >= 0:
            del self.pending[at]
        size = entry["size"] + len(c["data"])
        if size > MAX_MESSAGE_BYTES:
            return {"type": "error", "code": ERR_TOO_LARGE, "cmd": c["cmd"], "req": c["req"]}
        entry = {"next": c["seq"] + 1, "parts": entry["parts"] + [c["data"]], "size": size,
                 "t0": entry["t0"]}
        if not c["last"]:
            self.pending.append((key, entry))
            if len(self.pending) > self.max_pending:
                self.pending.sort(key=lambda kp: kp[1]["t0"])
                self.pending = self.pending[len(self.pending) - self.max_pending:]
            return {"type": "partial"}
        text = b"".join(entry["parts"]).decode("ascii")
        try:
            data = json.loads(text, parse_constant=_reject_constant) if text else {}
        except Exception:   # ValueError, et RecursionError sur un JSON imbrique a l'exces
            return {"type": "error", "code": ERR_BAD_JSON, "cmd": c["cmd"], "req": c["req"]}
        if not isinstance(data, dict):
            return {"type": "error", "code": ERR_BAD_JSON, "cmd": c["cmd"], "req": c["req"]}
        return {"type": "message", "cmd": c["cmd"], "req": c["req"], "name": command_name(c["cmd"]),
                "data": data}


def decode_chunks(chunks, now=0):
    r = Reassembler()
    for ch in chunks:
        out = r.push(ch, now)
        if out["type"] in ("message", "error"):
            return out
    return {"type": "partial"}


# -- Valider (et nettoyer) ---------------------------------------------------------------
class _Invalid(Exception):
    pass


def _clean_string(v, mx):
    s = "".join(ch for ch in v if not (ord(ch) < 0x20 or 0x7F <= ord(ch) <= 0x9F))
    return s[:mx]


def _is_color(v):
    if not isinstance(v, str) or len(v) != 7 or v[0] != "#":
        return False
    return all(ch in "0123456789abcdefABCDEF" for ch in v[1:])


def _check(t, v, path):
    k = t["t"]
    if k == "int" or (k == "nint" and v is not None):
        ok = _is_number(v) and not (isinstance(v, float) and (math.isnan(v) or math.isinf(v)))
        if ok and isinstance(v, float):
            ok = v.is_integer()
        if not ok or v < t["lo"] or v > t["hi"]:
            raise _Invalid("%s: expected an integer %d..%d" % (path, t["lo"], t["hi"]))
        return int(v)
    if k == "nint":
        return None
    if k == "num":
        if not _is_number(v) or (isinstance(v, float) and (math.isnan(v) or math.isinf(v))):
            raise _Invalid("%s: expected a number" % path)
        dp = t.get("dp")
        return min(t["hi"], max(t["lo"], v if dp is None else _round_dp(v, dp)))
    if k == "bool":
        if not isinstance(v, bool):
            raise _Invalid("%s: expected true or false" % path)
        return v
    if k == "str":
        if not isinstance(v, str):
            raise _Invalid("%s: expected text" % path)
        s = _clean_string(v, t["max"])
        if len(s) < t["min"]:
            raise _Invalid("%s: must not be empty" % path)
        return s
    if k == "color":
        return v.lower() if _is_color(v) else None
    if k == "enum":                    # (25 oct.) un mot connu, sinon le champ tombe (comme une couleur)
        return v if isinstance(v, str) and v in t["of"] else None
    if k == "caps":
        if not isinstance(v, list):
            raise _Invalid("%s: expected a list" % path)
        out = []
        for c in v[:32]:
            if isinstance(c, str) and c in CAPS and c not in out:
                out.append(c)
        return out
    if k == "list":
        if not isinstance(v, list):
            raise _Invalid("%s: expected a list" % path)
        out = []
        for x in v:
            if len(out) >= t["max"]:
                break
            try:
                out.append(_check(t["item"], x, path + "[]"))
            except _Invalid:
                pass
        return out
    if k == "obj":
        if not isinstance(v, dict):
            raise _Invalid("%s: expected an object" % (path or "data"))
        out = {}
        for key, ft in t["fields"]:
            p = (path + "." if path else "") + key
            if key not in v:
                if ft["req"]:
                    raise _Invalid("%s: missing" % p)
                continue
            r = _check(ft, v[key], p)
            if r is not None or ft["t"] == "nint":
                out[key] = r
        return out
    raise ValueError("link: unknown schema type %s" % k)


def validate_message(cmd, data, frm=None):
    """-> {'ok': True, 'data': copie nettoyee} | {'ok': False, 'error': {'code', 'msg'}}."""
    c = COMMANDS.get(cmd)
    if c is None:
        return {"ok": False, "error": {"code": ERR_UNKNOWN_CMD, "msg": "unknown command %s" % command_name(cmd)}}
    name, sender, _reply, _cap, schema = c
    if frm and sender != "any" and sender != frm:
        return {"ok": False, "error": {"code": ERR_WRONG_DIRECTION, "msg": "%s is not sent by the %s" % (name, frm)}}
    try:
        return {"ok": True, "data": _check(schema, data, "")}
    except _Invalid as e:
        return {"ok": False, "error": {"code": ERR_INVALID, "msg": "%s %s" % (name, e)}}


def next_req(req):
    r = req if isinstance(req, int) and not isinstance(req, bool) else 0
    return (r % 127) + 1


def slot_of_status(status):
    return (status & 0x0F) + 1 if isinstance(status, int) and 0x80 <= status < 0xF0 else 0
