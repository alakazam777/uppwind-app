# -*- coding: utf-8 -*-
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Uppwind
"""Uppwind Link -- le jumeau Python du protocole contre les fixtures dorees.

    python -I tests/test_protocol.py

Les fixtures (fixtures/*.json) sont ecrites par l'implementation JavaScript du protocole (celle du
studio) : ici, uwlink_protocol.py doit retrouver les
memes octets, les memes erreurs, les memes donnees nettoyees. Aucune dependance (pas de pytest) :
tourne aussi sous le Python embarque de FL Studio.
"""

import json
import os
import sys

sys.dont_write_bytecode = True   # pas de __pycache__ dans les dossiers des scripts

HERE = os.path.dirname(os.path.abspath(__file__))
LINK = os.path.dirname(HERE)
sys.path.insert(0, os.path.join(LINK, "shared"))

import uwlink_protocol as P  # noqa: E402

FIX = os.path.join(LINK, "fixtures")
TESTS = []


def test(fn):
    TESTS.append(fn)
    return fn


@test
def the_three_copies_of_the_protocol_are_identical():
    # shared/ est la source ; fl-studio/ et ableton/UppwindLink/ en gardent une copie identique (les scripts installes)
    src = open(os.path.join(LINK, "shared", "uwlink_protocol.py"), "rb").read()
    for copy in (("fl-studio",), ("ableton", "UppwindLink")):
        p = os.path.join(LINK, *(copy + ("uwlink_protocol.py",)))
        assert open(p, "rb").read() == src, "%s differs from shared/uwlink_protocol.py: copy it again" % "/".join(copy)


def hexs(b):
    return " ".join("%02X" % x for x in bytearray(b))


def unhex(s):
    return bytes(int(h, 16) for h in s.split())


def read_fix(name):
    with open(os.path.join(FIX, name), "r", encoding="utf-8") as f:
        return json.load(f)["cases"]


def strip_out(o):
    r = {"type": o["type"]}
    for k in ("code", "cmd", "req"):
        if o.get(k) is not None:
            r[k] = o[k]
    if o["type"] == "message":
        r["data"] = o["data"]
    return r


@test
def hello_bytes_by_hand():
    chunks = P.encode_message(P.HELLO, 5, {"v": 1, "app": "Uppwind Studio"})
    assert len(chunks) == 1
    want = bytes([0xF0, 0x7D, 0x55, 0x57, 1, 1, 5, 0, 1]) + b'{"v":1,"app":"Uppwind Studio"}' + bytes([0xF7])
    assert chunks[0] == want, hexs(chunks[0])


@test
def messages_fixture():
    cases = read_fix("messages.json")
    assert len(cases) >= 20
    for c in cases:
        got = [hexs(x) for x in P.encode_message(c["cmd"], c["req"], c["data"])]
        assert got == c["chunks"], (c["name"], got, c["chunks"])
        out = P.decode_chunks([unhex(h) for h in c["chunks"]])
        assert out["type"] == "message", (c["name"], out)
        assert out["cmd"] == c["cmd"] and out["req"] == c["req"]
        assert out["data"] == c["data"], (c["name"], out["data"])
        sender = P.COMMANDS[c["cmd"]][1]
        v = P.validate_message(c["cmd"], c["data"], None if sender == "any" else sender)
        assert v["ok"], (c["name"], v)
        if not c["name"].endswith("-raw-escapes"):
            assert v["data"] == c["data"], (c["name"], v["data"])


@test
def errors_fixture():
    for c in read_fix("errors.json"):
        r = P.Reassembler()
        nows = c.get("nows")
        outs = [strip_out(r.push(unhex(h), nows[i] if nows else 0)) for i, h in enumerate(c["chunks"])]
        assert outs == c["outs"], (c["name"], outs, c["outs"])


@test
def validation_fixture():
    for c in read_fix("validation.json"):
        v = P.validate_message(c["cmd"], c["data"], c["from"])
        assert v["ok"] == c["ok"], (c["name"], v)
        if c["ok"]:
            assert v["data"] == c["out"], (c["name"], v["data"], c["out"])
        else:
            assert v["error"]["code"] == c["code"], (c["name"], v)


@test
def numbers_fixture():
    cases = read_fix("numbers.json")
    assert len(cases) > 200
    for c in cases:
        got = P.canonical_json(c["x"])
        assert got == c["json"], (c["x"], got, c["json"])


@test
def python_only_number_shapes():
    # Ce que JSON.stringify ne sait pas ecrire mais qu'un script Python peut produire
    assert P.canonical_json({"a": 1.0, "b": -0.0, "c": True, "d": None}) == '{"a":1,"b":0,"c":true,"d":null}'
    assert P.canonical_json((1, 2)) == "[1,2]"
    # un grand nombre dont l'arrondi a 4 decimales retombe sur un entier : sans « .0 », comme JS
    assert P.canonical_json(489653094195265.06) == "489653094195265"
    assert P.canonical_json({"t": -68913164595142.01}) == '{"t":-68913164595142}'
    for bad in (float("nan"), float("inf"), 1e15, 10 ** 15):
        try:
            P.canonical_json(bad)
        except ValueError:
            pass
        else:
            raise AssertionError("accepted %r" % (bad,))
    try:
        P.canonical_json({1: 2})
    except TypeError:
        pass
    else:
        raise AssertionError("accepted a numeric key")
    # "3.0" dans le JSON = un entier pour la validation (comme en JS, ou 3.0 === 3)
    raw = P.encode_message(P.BIND, 1, {"slot": 1, "id": 1})[0]
    raw = raw.replace(b'"slot":1', b'"slot":3.0')
    out = P.decode_chunks([raw])
    v = P.validate_message(out["cmd"], out["data"], "studio")
    assert v["ok"] and v["data"] == {"slot": 3, "id": 1} and isinstance(v["data"]["slot"], int), v


@test
def tempo_round_bpm_matches_the_schema():
    # 24 oct. : round_bpm (ce que les scripts envoient) = ce que la validation garde, comme roundBpm en JS
    assert P.TEMPO == 0x50 and P.TEMPO_CHANGED == 0x51 and "tempo" in P.CAPS
    assert P.COMMANDS[P.TEMPO][3] == "tempo" and P.COMMANDS[P.TEMPO_CHANGED][1] == "daw"
    for c in read_fix("validation.json"):
        if c["name"].startswith("tempo-rounding-"):
            assert P.round_bpm(c["data"]["bpm"]) == c["out"]["bpm"], c
    assert P.round_bpm(120) == 120 and isinstance(P.round_bpm(120), int)
    assert P.round_bpm(120.0) == 120
    assert P.round_bpm(97.33333333) == 97.333
    assert P.canonical_json({"bpm": P.round_bpm(97.33333333)}) == '{"bpm":97.333}'
    assert P.canonical_json({"bpm": P.round_bpm(140.0)}) == '{"bpm":140}'
    for bad in (True, None, "120", float("nan"), float("inf"), [120]):
        assert P.round_bpm(bad) is None, bad
    # un nombre enorme : x * 1000 deborde (Infinity, comme en JS), ramene a 999 sans lever
    assert P.validate_message(P.TEMPO_CHANGED, {"bpm": 1.7e308}, "daw")["data"] == {"bpm": 999}
    assert P.validate_message(P.TEMPO_CHANGED, {"bpm": 120.0}, "daw")["data"] == {"bpm": 120}
    # « 120.0 » dans le JSON : le meme octet que 120 a l'envoi
    assert P.encode_message(P.TEMPO, 0, {"bpm": 120.0}) == P.encode_message(P.TEMPO, 0, {"bpm": 120})


@test
def hostile_json_never_raises():
    # NaN / Infinity : json.loads les accepte par defaut, pas nous
    for body in (b'{"t":NaN}', b'{"t":Infinity}', b'{"t":-Infinity}'):
        raw = bytes([0xF0, 0x7D, 0x55, 0x57, 1, P.PING, 3, 0, 1]) + body + bytes([0xF7])
        assert P.decode_chunks([raw])["code"] == "bad_json", body
    # imbrication extreme (JSON valide, 12 000 niveaux) : RecursionError devient bad_json
    text = b'{"a":' + b"[" * 12000 + b"]" * 12000 + b"}"
    assert len(text) <= P.MAX_MESSAGE_BYTES
    n = (len(text) + 199) // 200
    chunks = []
    for seq in range(n):
        part = text[seq * 200:(seq + 1) * 200]
        chunks.append(bytes([0xF0, 0x7D, 0x55, 0x57, 1, P.LIST_REPLY, 4, seq, 1 if seq == n - 1 else 0]) + part + bytes([0xF7]))
    out = P.decode_chunks(chunks)
    assert out["type"] == "error" and out["code"] == "bad_json", out


@test
def framing_is_optional_and_lists_work():
    msg = P.encode_message(P.PING, 9, {"t": 1})[0]
    assert P.parse_chunk(msg[1:-1])["cmd"] == P.PING
    assert P.parse_chunk(list(msg))["req"] == 9
    assert P.parse_chunk(bytearray(msg))["seq"] == 0
    assert P.parse_chunk(None) is None
    assert P.parse_chunk([0xF0, 0x7E, 0x7F, 0x06, 0x01, 0xF7]) is None
    assert P.is_link_sysex(msg) and P.is_link_sysex(msg[1:]) and not P.is_link_sysex(b"\x90\x3c\x64")


@test
def reassembly_timeout_and_cap():
    big = {"items": [{"id": 1, "name": "n" * 300}]}
    r = P.Reassembler(max_pending=16)
    for req in range(1, 21):
        r.push(P.encode_message(P.LIST_REPLY, req, big)[0], req)
    assert len(r.pending) == 16
    assert r.push(P.encode_message(P.LIST_REPLY, 2, big)[1], 21)["code"] == "seq"
    assert r.push(P.encode_message(P.LIST_REPLY, 7, big)[1], 21)["type"] == "message"
    r = P.Reassembler()
    m = P.encode_message(P.LIST_REPLY, 7, big)
    r.push(m[0], 1000)
    assert r.push(m[1], 1000 + P.REASSEMBLY_TIMEOUT_MS + 1)["code"] == "seq"


@test
def encode_error_cuts_at_200_code_points_like_js():
    # encodeError (JS) coupe a 200 points de code : un emoji a la frontiere reste entier, des deux cotes
    msg = "a" * 199 + u"\U0001F3B9\U0001F3B9"
    got = P.encode_error(1, "x", msg)
    assert got == P.encode_message(P.ERROR, 1, {"code": "x", "msg": "a" * 199 + u"\U0001F3B9"}), got
    assert P.decode_chunks(got)["data"]["msg"] == "a" * 199 + u"\U0001F3B9"
    assert "msg" not in P.decode_chunks(P.encode_error(1, "x", ""))["data"]
    assert P.decode_chunks(P.encode_error(1, "x", 0))["data"]["msg"] == "0"     # comme String(0) en JS


@test
def request_ids_and_slots():
    assert P.next_req(0) == 1 and P.next_req(127) == 1 and P.next_req(126) == 127
    assert P.slot_of_status(0x90) == 1 and P.slot_of_status(0x8F) == 16 and P.slot_of_status(0xF0) == 0
    assert P.command_name(P.BIND) == "BIND" and P.command_name(0x55) == "0x55"


@test
def encode_refuses_bad_input():
    for args in ((0x55, 1, {}), (P.PING, 128, {"t": 1}), (P.PING, True, {"t": 1}), (P.PING, 1, [1])):
        try:
            P.encode_message(*args)
        except (ValueError, TypeError):
            pass
        else:
            raise AssertionError("accepted %r" % (args,))
    try:
        P.encode_message(P.ERROR, 1, {"code": "c", "msg": "x" * P.MAX_MESSAGE_BYTES})
    except ValueError:
        pass
    else:
        raise AssertionError("accepted an oversized message")


def main():
    failed = 0
    for fn in TESTS:
        try:
            fn()
            print("ok   " + fn.__name__)
        except Exception as e:  # noqa: BLE001
            failed += 1
            import traceback
            print("FAIL " + fn.__name__)
            traceback.print_exc()
    print("python %s: %d/%d passed" % (sys.version.split()[0], len(TESTS) - failed, len(TESTS)))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
