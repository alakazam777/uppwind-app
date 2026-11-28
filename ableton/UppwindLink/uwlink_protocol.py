# -*- coding: utf-8 -*-
# SPDX-License-Identifier: MIT
# Copyright (C) 2026 Uppwind
"""Uppwind Link -- le protocole MIDI, cote script du logiciel hote (FL Studio, Ableton Live).

Jumeau de l'implementation JavaScript du studio (link-protocol.js, servi par Uppwind Studio) : MEMES regles,
octet pour octet. Les deux sont testes contre fixtures/*.json. Ce fichier (shared/) est la SOURCE ; une copie
identique vit dans chaque script hote (fl-studio/, ableton/UppwindLink/) -- tests/test_protocol.py verifie
qu'elles n'ont pas diverge.

Contraintes : Python 3.7 (Ableton Live 11) a 3.12 (FL Studio 2024+), bibliotheque standard
minimale (json, math) -- le bac a sable des scripts de FL n'a ni fichiers ni reseau. (24 nov.) hashlib et os.urandom
sont essayes, jamais exiges : sans eux, un SHA-256 en Python pur et un hasard tire de HMAC(secret, heure | compteur).

(24 nov., audit M3) L'AUTHENTIFICATION du chemin MIDI (cap 'auth') : le port loopMIDI est un bus partage, tout
programme peut y ecrire ; le script et le studio partagent donc un secret (le code du script, ou uwl_secret.py ecrit
par le compagnon), le studio le prouve (HELLO -> HELLO_ACK.auth {kid, n} -> AUTH {n, p} -> AUTH_ACK {p}), puis CHAQUE
message du studio porte en dernier "c" (un compteur qui monte) et "a" (HMAC de la cle de session, 16 octets en hexa)
-- voir encode_signed / verify_signed, et link-protocol.js pour le detail.

SysEx : F0 7D 55 57 <ver=01> <cmd> <req> <seq> <last> <data...> F7 ; data = JSON canonique en
ASCII 7 bits, 200 octets au plus par morceau, 128 morceaux au plus. Voir link-protocol.js pour
la table des commandes et le sens de chaque champ.
"""

import json
import math
import time

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
AUTH = 0x03
AUTH_ACK = 0x04
LIST = 0x10
LIST_REPLY = 0x11
CHANGED = 0x12
BIND = 0x20
BIND_ACK = 0x21
UNBIND = 0x22
SET = 0x30
PARAMS = 0x34
PARAMS_REPLY = 0x35
PARAM_SET = 0x36
PARAM_WATCH = 0x37
PARAM_CHANGED = 0x38
PLUGINS = 0x40
PLUGINS_REPLY = 0x41
ADD = 0x42
ADDED = 0x43
REMOVE = 0x44
TEMPO = 0x50
TEMPO_CHANGED = 0x51
EDITOR = 0x60
EDITOR_ACK = 0x61
PING = 0x70
PONG = 0x71
ERROR = 0x7F

CAPS = ("list", "bind", "set", "plugins", "add", "remove", "audition", "tempo", "notes", "editor", "params", "auth")
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
ERR_AUTH = "auth"

# (24 nov.) l'authentification : un secret de 15 octets (120 bits, un code de 24 caracteres), des nonces de 16 octets,
# des preuves HMAC-SHA256 de 32 octets, la signature d'un message coupee a 16 octets ; une offre (HELLO_ACK.auth) vaut
# AUTH_PENDING_MS
SECRET_BYTES = 15
NONCE_BYTES = 16
MAC_BYTES = 16
AUTH_PENDING_MS = 30000
# le compteur des messages signes : le JSON canonique ne va pas au-dela de 1e15
MAX_COUNTER = 999999999999999


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


def _hex(n, req=False):
    return {"t": "hex", "len": n, "req": req}


_AUTH_OFFER = _obj([("kid", _req(_hex(8))), ("n", _req(_hex(2 * NONCE_BYTES)))])

ITEM_SCHEMA = _obj([
    ("id", _req(_ID)), ("name", _req(_str(64))), ("plugin", _str(64)), ("kind", _str(24)),
    ("color", _COLOR), ("slots", _list(_SLOT, 16)),
])
# (19 nov., cap 'params') un parametre d'un plugin : son numero chez le logiciel hote, son nom, sa valeur 0..1
PARAM_MAX = 4095
MAX_PARAMS = 512
MAX_PARAM_CHANGES = 128
_PARAM_P = _int(0, PARAM_MAX)
_PARAM_V = _num(0, 1)
PARAM_SCHEMA = _obj([("p", _req(_PARAM_P)), ("name", _req(_str(64))), ("v", _req(_PARAM_V))])
PARAM_VALUE_SCHEMA = _obj([("p", _req(_PARAM_P)), ("v", _req(_PARAM_V))])
PLUGIN_SCHEMA = _obj([
    ("id", _req(_str(200, 1))), ("name", _req(_str(80))), ("vendor", _str(60)), ("kind", _str(24)),
])

# code -> (nom, de qui, reponse, capacite, schema)
COMMANDS = {
    HELLO: ("HELLO", "studio", HELLO_ACK, None,
            _obj([("v", _req(_int(0, 127))), ("app", _str(40)), ("caps", _CAPS_T)])),
    HELLO_ACK: ("HELLO_ACK", "daw", None, None,
                _obj([("v", _req(_int(0, 127))), ("daw", _req(_str(40))), ("dawVersion", _str(60)),
                      ("project", _str(120)), ("caps", _req(_CAPS_T)), ("bpm", _BPM), ("auth", _AUTH_OFFER)])),
    AUTH: ("AUTH", "studio", AUTH_ACK, "auth", _obj([("n", _req(_hex(2 * NONCE_BYTES))), ("p", _req(_hex(64)))])),
    AUTH_ACK: ("AUTH_ACK", "daw", None, None, _obj([("p", _req(_hex(64))), ("project", _str(120)), ("bpm", _BPM)])),
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
    PARAMS: ("PARAMS", "studio", PARAMS_REPLY, "params", _obj([("id", _req(_ID)), ("start", _int(0, 1000000))])),
    PARAMS_REPLY: ("PARAMS_REPLY", "daw", None, None,
                   _obj([("id", _req(_ID)), ("plugin", _str(64)), ("items", _req(_list(PARAM_SCHEMA, MAX_PARAMS))),
                         ("next", _NEXT)])),
    PARAM_SET: ("PARAM_SET", "studio", None, "params",
                _obj([("id", _req(_ID)), ("p", _req(_PARAM_P)), ("v", _req(_PARAM_V))])),
    PARAM_WATCH: ("PARAM_WATCH", "studio", None, "params", _obj([("id", _ID)])),
    PARAM_CHANGED: ("PARAM_CHANGED", "daw", None, None,
                    _obj([("id", _req(_ID)), ("items", _req(_list(PARAM_VALUE_SCHEMA, MAX_PARAM_CHANGES))),
                          ("hint", _str(80))])),
    PLUGINS: ("PLUGINS", "studio", PLUGINS_REPLY, "plugins", _obj([("start", _int(0, 1000000))])),
    PLUGINS_REPLY: ("PLUGINS_REPLY", "daw", None, None,
                    _obj([("items", _req(_list(PLUGIN_SCHEMA, 2000))), ("next", _NEXT)])),
    ADD: ("ADD", "studio", ADDED, "add", _obj([("plugin", _req(_str(200, 1))), ("name", _str(64))])),
    ADDED: ("ADDED", "daw", None, None, _obj([("item", _req(ITEM_SCHEMA))])),
    REMOVE: ("REMOVE", "studio", None, "remove", _obj([("id", _req(_ID))])),
    TEMPO: ("TEMPO", "studio", None, "tempo", _obj([("bpm", _req(_BPM))])),
    TEMPO_CHANGED: ("TEMPO_CHANGED", "daw", None, None, _obj([("bpm", _req(_BPM))])),
    EDITOR: ("EDITOR", "studio", EDITOR_ACK, "editor", _obj([("id", _req(_ID)), ("open", _req(_BOOL))])),
    EDITOR_ACK: ("EDITOR_ACK", "daw", None, None, _obj([("id", _req(_ID)), ("open", _req(_BOOL))])),
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
    return _chunks(cmd, req, canonical_json(data).encode("ascii"))


def _chunks(cmd, req, text):
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
                "data": data, "text": text}


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
    if k == "hex":                     # (24 nov.) exactement len chiffres hexadecimaux, rendus en minuscules
        if not isinstance(v, str) or len(v) != t["len"] or not all(ch in "0123456789abcdefABCDEF" for ch in v):
            raise _Invalid("%s: expected %d hex digits" % (path, t["len"]))
        return v.lower()
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



# -- (24 nov., audit M3) L'authentification du chemin MIDI ------------------------------------
# SHA-256 (FIPS 180-4) en Python pur : le secours d'un Python sans hashlib (le bac a sable de FL ?). Lent (~0,1 ms par
# bloc de 64 octets) mais il ne sert qu'a quelques messages par seconde.
_K256 = (
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2)


def py_sha256(data):
    data = bytes(data)
    h = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19]
    n = len(data)
    msg = data + b"\x80" + b"\x00" * ((55 - n) % 64) + (8 * n).to_bytes(8, "big")
    M = 0xFFFFFFFF
    for off in range(0, len(msg), 64):
        w = [int.from_bytes(msg[off + 4 * i:off + 4 * i + 4], "big") for i in range(16)]
        for i in range(16, 64):
            x, y = w[i - 15], w[i - 2]
            s0 = ((x >> 7) | (x << 25)) ^ ((x >> 18) | (x << 14)) ^ (x >> 3)
            s1 = ((y >> 17) | (y << 15)) ^ ((y >> 19) | (y << 13)) ^ (y >> 10)
            w.append((w[i - 16] + s0 + w[i - 7] + s1) & M)
        a, b, c, d, e, f, g, hh = h
        for i in range(64):
            S1 = (((e >> 6) | (e << 26)) ^ ((e >> 11) | (e << 21)) ^ ((e >> 25) | (e << 7))) & M
            t1 = (hh + S1 + ((e & f) ^ (~e & g)) + _K256[i] + w[i]) & M
            S0 = (((a >> 2) | (a << 30)) ^ ((a >> 13) | (a << 19)) ^ ((a >> 22) | (a << 10))) & M
            t2 = (S0 + ((a & b) ^ (a & c) ^ (b & c))) & M
            hh, g, f, e, d, c, b, a = g, f, e, (d + t1) & M, c, b, a, (t1 + t2) & M
        h = [(x + y) & M for x, y in zip(h, (a, b, c, d, e, f, g, hh))]
    return b"".join(x.to_bytes(4, "big") for x in h)


def _native_sha256():
    try:
        import hashlib
        f = lambda b: hashlib.sha256(bytes(b)).digest()     # noqa: E731
        if f(b"abc") == py_sha256(b"abc"):
            return f
    except Exception:   # pas de hashlib (ou un hashlib qui ne marche pas) : le Python pur
        pass
    return None


_NATIVE = _native_sha256()
sha256 = _NATIVE or py_sha256
SHA256_NATIVE = _NATIVE is not None


def hmac_sha256(key, msg, digest=None):
    """HMAC-SHA256 (RFC 2104) sur le SHA-256 choisi (digest : pour les tests, py_sha256)."""
    hf = digest or sha256
    key = bytes(key)
    if len(key) > 64:
        key = hf(key)
    key = key + b"\x00" * (64 - len(key))
    return hf(bytes(x ^ 0x5C for x in key) + hf(bytes(x ^ 0x36 for x in key) + bytes(msg)))


def same_bytes(a, b):
    """Comparaison en temps constant (hmac.compare_digest sans le module hmac)."""
    a, b = bytes(a), bytes(b)
    r = len(a) ^ len(b)
    for x, y in zip(a, b):
        r |= x ^ y
    return r == 0


_RNG = {"n": 0, "state": b"", "fallback": False}    # fallback : True des qu'un tirage s'est passe de os.urandom


def random_bytes(n, mix=b""):
    """n octets au hasard : os.urandom si le Python l'a ; sinon (le bac a sable de FL ?) HMAC(mix, heure | compteur |
    etat), mix = le secret du script : imprevisible pour qui ne le connait pas."""
    try:
        import os
        b = os.urandom(n)
        if len(b) == n:
            return b
    except Exception:
        pass
    _RNG["fallback"] = True
    out = b""
    while len(out) < n:
        _RNG["n"] += 1
        seed = ("%r|%r|%d|%d" % (time.time(), time.perf_counter() if hasattr(time, "perf_counter") else 0,
                                 _RNG["n"], id(out))).encode("ascii")
        _RNG["state"] = hmac_sha256(bytes(mix) + _RNG["state"], seed)
        out += _RNG["state"]
    return out[:n]


B32 = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567"


def random_is_weak():
    """(24 nov.) Un tirage s'est-il passe de os.urandom (le hasard du systeme) ? Le script le dit au journal."""
    return bool(_RNG["fallback"])


def code_of(secret):
    """Le secret (15 octets) -> le code que la personne colle dans le studio : 24 caracteres en base 32 (A-Z, 2-7), par
    groupes de 4 (« ABCD-EFGH-IJKL-MNOP-QRST-UVWX »)."""
    secret = bytes(secret)
    if len(secret) != SECRET_BYTES:
        raise ValueError("link: a secret is %d bytes" % SECRET_BYTES)
    v = int.from_bytes(secret, "big")
    chars = "".join(B32[(v >> (5 * (23 - i))) & 31] for i in range(24))
    return "-".join(chars[i:i + 4] for i in range(0, 24, 4))


def secret_of(code):
    """Le code (tirets, espaces et casse indifferents) -> les 15 octets, ou None."""
    if not isinstance(code, str):
        return None
    t = "".join(ch for ch in code.upper() if ch not in "- \t\r\n")
    if len(t) != 24 or any(ch not in B32 for ch in t):
        return None
    v = 0
    for ch in t:
        v = (v << 5) | B32.index(ch)
    return v.to_bytes(SECRET_BYTES, "big")


def _hexs(b):
    return "".join("%02x" % x for x in bytearray(b))


def key_id(secret):
    """kid : 8 caracteres qui disent QUEL secret (FL, Live, telle machine) sans rien en reveler."""
    return _hexs(hmac_sha256(secret, b"uwl-midi-kid"))[:8]


def new_nonce(mix=b""):
    return _hexs(random_bytes(NONCE_BYTES, mix))


def _proof(secret, label, first, second):
    return _hexs(hmac_sha256(secret, ("%s|%s|%s" % (label, first, second)).encode("ascii")))


def studio_proof(secret, daw_nonce, studio_nonce):
    """Ce que le studio prouve (AUTH.p) : HMAC(secret, "uwl-midi-studio|" n_script "|" n_studio), en hexa."""
    return _proof(secret, "uwl-midi-studio", daw_nonce, studio_nonce)


def daw_proof(secret, daw_nonce, studio_nonce):
    """Ce que le script prouve (AUTH_ACK.p) : HMAC(secret, "uwl-midi-daw|" n_studio "|" n_script)."""
    return _proof(secret, "uwl-midi-daw", studio_nonce, daw_nonce)


def proof_ok(given, want):
    return isinstance(given, str) and same_bytes(given.lower().encode("ascii", "replace"), want.encode("ascii"))


def session_key(secret, daw_nonce, studio_nonce):
    """La cle de la session : HMAC(secret, "uwl-midi-session|" n_script "|" n_studio) (32 octets)."""
    return hmac_sha256(secret, ("uwl-midi-session|%s|%s" % (daw_nonce, studio_nonce)).encode("ascii"))


def _msg_mac(key, cmd, req, text0):
    return _hexs(hmac_sha256(key, b"uwl-midi-msg|" + bytes([cmd, req]) + text0)[:MAC_BYTES])


_SIG_LEN = len(',"a":""}') + 2 * MAC_BYTES


def encode_signed(cmd, req, data, key, counter):
    """Un message signe -> ses morceaux. Le texte : le JSON canonique de data plus "c" (le compteur), puis "a" (la
    signature de ce texte, de cmd et de req) en DERNIER : le script verifie sur le texte exact recu (verify_signed)."""
    if data is None:
        data = {}
    if not isinstance(data, dict) or "a" in data or "c" in data:
        raise ValueError("link: signed data is an object without 'a' or 'c'")
    if not isinstance(counter, int) or isinstance(counter, bool) or counter < 1 or counter > MAX_COUNTER:
        raise ValueError("link: counter must be 1..MAX_COUNTER")
    if cmd not in COMMANDS:
        raise ValueError("link: unknown command %r" % (cmd,))
    d = dict(data)
    d["c"] = counter
    text0 = canonical_json(d).encode("ascii")
    text = text0[:-1] + b',"a":"' + _msg_mac(key, cmd, req, text0).encode("ascii") + b'"}'
    return _chunks(cmd, req, text)


def verify_signed(cmd, req, text, key, last_counter):
    """Le texte recu d'un message (Reassembler : out["text"]) -> {'ok': True, 'data': sans "a" ni "c", 'c': compteur}
    ou {'ok': False, 'why': ...}. Refuse : pas de signature, une signature fausse, un compteur qui ne monte pas."""
    if not isinstance(text, str) or len(text) < _SIG_LEN + 2 or not text.endswith('"}'):
        return {"ok": False, "why": "unsigned"}
    tail = text[-_SIG_LEN:]
    mac = tail[6:6 + 2 * MAC_BYTES]
    if not tail.startswith(',"a":"') or not all(ch in "0123456789abcdef" for ch in mac):
        return {"ok": False, "why": "unsigned"}
    text0 = (text[:-_SIG_LEN] + "}").encode("ascii")
    if not same_bytes(mac.encode("ascii"), _msg_mac(key, cmd, req, text0).encode("ascii")):
        return {"ok": False, "why": "bad signature"}
    try:
        data = json.loads(text0.decode("ascii"), parse_constant=_reject_constant)
    except Exception:
        return {"ok": False, "why": "bad json"}
    c = data.pop("c", None) if isinstance(data, dict) else None
    if not isinstance(c, int) or isinstance(c, bool) or c < 1 or c > MAX_COUNTER:
        return {"ok": False, "why": "no counter"}
    if c <= last_counter:
        return {"ok": False, "why": "replayed"}
    data.pop("a", None)
    return {"ok": True, "data": data, "c": c}
