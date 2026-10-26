# -*- coding: utf-8 -*-
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Uppwind
"""Uppwind Link -- le script FL Studio (fl-studio/device_UppwindLink.py) contre un faux FL.

    python -I tests/test_fl_script.py

Faux modules channels / device / general / mixer / plugins / ui ; le VRAI midi.py de FL quand FL est
installe (C:/Program Files (x86)/Image-Line/FL Studio 2026/Shared/Python/Lib), pour verifier que
les constantes utilisees existent. Le faux FL imite ce que l'on sait du vrai : index globaux
(useGlobalIndex, API 33+), mute par BASCULE regroupee par tick, VST a 4240 parametres (dont 128
« MIDI CC » a partir de 4096), loi de volume des canaux (utils.VolTodB), sortie SysEx seulement si
un port de sortie est lie. Horloge simulee : aucun sommeil.
"""

import importlib
import importlib.util
import math
import os
import sys

sys.dont_write_bytecode = True   # pas de __pycache__ dans les dossiers des scripts
import types

HERE = os.path.dirname(os.path.abspath(__file__))
LINK = os.path.dirname(HERE)
FL_DIR = os.path.join(LINK, "fl-studio")
FL_LIB = "C:/Program Files (x86)/Image-Line/FL Studio 2026/Shared/Python/Lib"
sys.path.insert(0, FL_DIR)

import uwlink_protocol as P  # noqa: E402  (la copie du dossier FL)

TESTS = []


def test(fn):
    TESTS.append(fn)
    return fn


def load_midi():
    path = os.path.join(FL_LIB, "midi.py")
    if os.path.exists(path):
        spec = importlib.util.spec_from_file_location("midi", path)
        mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(mod)
        mod.__real__ = True
        return mod
    mod = types.ModuleType("midi")
    for k, v in dict(CT_Sampler=0, CT_Hybrid=1, CT_GenPlug=2, CT_Layer=3, CT_AudioClip=4, CT_AutoClip=5,
                     PIM_None=0, HW_Dirty_Names=16384, HW_Dirty_Colors=8192, HW_Dirty_ChannelRack=262144,
                     HW_Dirty_ChannelRackGroup=32768, HW_Dirty_LEDs=256, CE_New=0, CE_Delete=1, CE_Replace=2,
                     CE_Rename=3, CE_Select=4, PL_Start=0, PL_LoadOk=100, PL_LoadError=101).items():
        setattr(mod, k, v)
    mod.__real__ = False
    return mod


MIDI = load_midi()


# -- Le faux FL ---------------------------------------------------------------------------
class World(object):
    def __init__(self, api=40):
        self.api = api
        self.t = 1000.0
        self.chans = []
        self.notes = []          # (index, note, velocite)
        self.params = []         # (index, param, valeur)
        self.pitch = []          # (index, valeur)
        self.sent = []           # SysEx envoyes
        self.toggles = {}        # bascules de mute en attente (appliquees au tick)
        self.assigned = True
        self.title = "Night drive.flp - FL Studio 2026"
        self.non_global = []     # appels sans useGlobalIndex (API >= 33 : interdit)
        # 24 oct. : le tempo, en milliemes de BPM comme FL ; un REC_Tempo s'applique au tick suivant
        # (le pire cas : le script ne doit pas prendre l'ancien tempo pour un changement) ; docs : le sens de
        # getCurrentTempo(asInt) selon la doc (1 -> entier x 1000) plutot que selon les scripts (1 -> BPM)
        self.tempo = 140000
        self.tempo_next = None
        self.rec = []            # (id, valeur, drapeaux) de general.processRECEvent
        self.docs = False

    def add(self, name, plugin=None, ctype=2, color=0x5F7581, vst=False, vol=0.78125, pan=0.0, muted=False, port=-2,
            pnames=None):
        # (26 oct.) port : le port d'entree MIDI du wrapper (-2 : aucun, comme FL) ; pnames : les noms des parametres
        self.chans.append({"name": name, "plugin": plugin, "type": ctype, "color": color, "vst": vst,
                           "vol": vol, "pan": pan, "muted": muted, "port": port, "pnames": pnames or {}, "pv": {}})

    def add_notes(self, name="Uppwind Link Notes", port=10, tagged=True):
        """(26 oct.) Un canal Uppwind Link Notes : ses ports d'entree et de sortie sur `port` (FL ne montre au script
        que l'entree) ; tagged=False : un plugin d'avant le 25 oct., sans « Link tag »."""
        self.add(name, "Uppwind Link Notes", vst=True, port=port,
                 pnames={0: "Link tag", 1: "Bypass"} if tagged else {0: "Bypass"})

    def tick(self):
        # FL applique les bascules de mute du tick (plusieurs bascules d'un meme tick = une seule)
        for i in list(self.toggles):
            if i < len(self.chans):
                self.chans[i]["muted"] = not self.chans[i]["muted"]
        self.toggles = {}
        if self.tempo_next is not None:
            self.tempo, self.tempo_next = self.tempo_next, None

    def g(self, flag, fn):
        if self.api >= 33 and flag is not True:
            self.non_global.append(fn)


def make_modules(w):
    ch = types.ModuleType("channels")

    def channelCount(globalCount=False):
        return len(w.chans)

    def getChannelName(i, g=False):
        w.g(g, "getChannelName")
        return w.chans[i]["name"]

    def getChannelColor(i, g=False):
        w.g(g, "getChannelColor")
        return w.chans[i]["color"] | (0xFF << 24 if w.chans[i]["color"] & 0x800000 else 0)

    def getChannelType(i, g=False):
        w.g(g, "getChannelType")
        return w.chans[i]["type"]

    def getChannelVolume(i, mode=False, g=False):
        w.g(g, "getChannelVolume")
        v = w.chans[i]["vol"]
        if mode:
            gain = (math.exp(v * math.log(11)) - 1) * 0.1      # utils.VolTodB
            return -math.inf if gain <= 0 else 20 * math.log10(gain)
        return v

    def setChannelVolume(i, v, pick=0, g=False):
        w.g(g, "setChannelVolume")
        w.chans[i]["vol"] = max(0.0, min(1.0, v))

    def getChannelPan(i, g=False):
        w.g(g, "getChannelPan")
        return w.chans[i]["pan"]

    def setChannelPan(i, p, pick=0, g=False):
        w.g(g, "setChannelPan")
        w.chans[i]["pan"] = p

    def isChannelMuted(i, g=False):
        w.g(g, "isChannelMuted")
        return w.chans[i]["muted"]

    def muteChannel(i, value=-1, g=False):
        w.g(g, "muteChannel")
        assert value == -1, "the script must toggle"
        w.toggles[i] = True

    def setChannelPitch(i, v, mode=0, pick=0, g=False):
        w.g(g, "setChannelPitch")
        w.pitch.append((i, v))

    def midiNoteOn(i, note, vel, chan=-1):
        assert 0 <= i < len(w.chans), "note to a channel that does not exist"
        w.notes.append((i, note, vel))

    def getChannelIndex(i):
        return i

    def getChannelMidiInPort(i, g=False):
        w.g(g, "getChannelMidiInPort")
        return w.chans[i]["port"]

    for f in (channelCount, getChannelName, getChannelColor, getChannelType, getChannelVolume, setChannelVolume,
              getChannelPan, setChannelPan, isChannelMuted, muteChannel, setChannelPitch, midiNoteOn, getChannelIndex,
              getChannelMidiInPort):
        setattr(ch, f.__name__, f)

    pl = types.ModuleType("plugins")

    def isValid(i, slot=-1, g=False):
        w.g(g, "isValid") if w.api >= 26 else None
        return 0 <= i < len(w.chans) and w.chans[i]["type"] == 2 and w.chans[i]["plugin"] is not None

    def getPluginName(i, slot=-1, user=False, g=False):
        w.g(g, "getPluginName") if w.api >= 26 else None
        return w.chans[i]["plugin"]

    def getParamCount(i, slot=-1, g=False):
        return 4240 if w.chans[i]["vst"] else 40

    def getParamName(p, i, slot=-1, g=False):
        w.g(g, "getParamName") if w.api >= 26 else None
        return w.chans[i]["pnames"].get(p, "")

    def getParamValue(p, i, slot=-1, g=False):
        w.g(g, "getParamValue") if w.api >= 26 else None
        return w.chans[i]["pv"].get(p, 0.0)

    def setParamValue(v, p, i, slot=-1, pick=0, g=False):
        assert 0.0 <= v <= 1.0, "a VST parameter is 0..1"
        w.params.append((i, p, v))
        w.chans[i]["pv"][p] = v

    for f in (isValid, getPluginName, getParamCount, getParamName, getParamValue, setParamValue):
        setattr(pl, f.__name__, f)

    dev = types.ModuleType("device")
    dev.midiOutSysex = lambda b: w.sent.append(bytes(b))
    dev.isAssigned = lambda: w.assigned
    dev.getPortNumber = lambda: 77

    gen = types.ModuleType("general")
    gen.getVersion = lambda: w.api
    rec_tempo = getattr(MIDI, "REC_Tempo", 0x4000 * 0x10000 + 5)

    def processRECEvent(eventId, value, flags):
        assert isinstance(value, int), "REC values are integers"
        w.rec.append((eventId, value, flags))
        if eventId == rec_tempo:
            assert 10000 <= value <= 522000, "FL's tempo goes from 10 to 522 BPM"
            w.tempo_next = value
        return value

    gen.processRECEvent = processRECEvent

    mx = types.ModuleType("mixer")

    def getCurrentTempo(asInt=0):
        # ce que les scripts d'Image-Line et de NI constatent : 0 -> entier x 1000, 1 -> BPM ; w.docs : l'inverse
        wants_float = bool(asInt) != w.docs
        return w.tempo / 1000.0 if wants_float else w.tempo

    mx.getCurrentTempo = getCurrentTempo

    ui = types.ModuleType("ui")
    ui.getProgTitle = lambda: w.title
    ui.getVersion = lambda mode=4: (2026, 1, 0, 5123)[mode] if mode < 4 else "Producer Edition v2026.1.0 [build 5123]"
    return {"channels": ch, "plugins": pl, "device": dev, "general": gen, "mixer": mx, "ui": ui, "midi": MIDI}


class Ev(object):
    def __init__(self, status, d1=0, d2=0, sysex=None):
        self.status = status
        self.data1 = d1
        self.data2 = d2
        self.sysex = sysex
        self.handled = False
        self.midiId = status & 0xF0 if status < 0xF0 else status
        self.midiChan = status & 0x0F if status < 0xF0 else 0


class Rig(object):
    """Le script charge contre un faux FL, et un faux studio qui lui parle."""

    def __init__(self, api=40, setup=None):
        self.w = World(api)
        if setup:
            setup(self.w)
        else:
            self.w.add("Kick", None, ctype=0, color=0xA03020)
            self.w.add("Serum Bass", "Serum", vst=True, color=0x10FF80)
            self.w.add("FLEX Keys", "FLEX", color=0x2040A0)
            self.w.add("Automation Clip", None, ctype=5)
            self.w.add("Café ☕ Pad", "Pigments", vst=True, color=0xFF00AA)
        for k, v in make_modules(self.w).items():
            sys.modules[k] = v
        sys.modules.pop("device_UppwindLink", None)
        self.s = importlib.import_module("device_UppwindLink")
        self.s._now = lambda: self.w.t
        self.rx = P.Reassembler()
        self.req = 0
        self.s.OnInit()

    def advance(self, seconds, idle=True):
        steps = max(1, int(round(seconds / 0.02)))
        for _ in range(steps):
            self.w.t += seconds / steps
            if idle:
                self.s.OnIdle()
            self.w.tick()

    def send(self, cmd, data=None, req=None):
        if req is None:
            self.req = P.next_req(self.req)
            req = self.req
        for chunk in P.encode_message(cmd, req, data or {}):
            ev = Ev(0xF0, sysex=chunk)
            self.s.OnMidiMsg(ev)
            assert ev.handled, "our SysEx must be handled"
        return req

    def replies(self):
        for _ in range(40):
            self.s.OnIdle()
        out = []
        for b in self.w.sent:
            r = self.rx.push(b, self.w.t * 1000)
            assert r["type"] in ("partial", "message"), r
            if r["type"] == "message":
                v = P.validate_message(r["cmd"], r["data"], "daw")
                assert v["ok"], (r, v)
                out.append((r["cmd"], r["req"], v["data"]))
        self.w.sent = []
        return out

    def ask(self, cmd, data=None):
        req = self.send(cmd, data)
        rep = [r for r in self.replies() if r[1] == req]
        assert len(rep) == 1, rep
        return rep[0]

    def midi(self, status, d1, d2):
        ev = Ev(status, d1, d2)
        self.s.OnMidiMsg(ev)
        return ev

    def hello(self):
        cmd, req, data = self.ask(P.HELLO, {"v": 1, "app": "Uppwind Studio"})
        assert cmd == P.HELLO_ACK
        return data

    def bind(self, slot, i):
        return self.ask(P.BIND, {"slot": slot, "id": i})


# -- Les tests ------------------------------------------------------------------------------
@test
def hello_ack_describes_fl_honestly():
    r = Rig()
    d = r.hello()
    assert d["daw"] == "FL Studio"
    assert d["project"] == "Night drive", d
    assert d["caps"] == ["list", "bind", "set", "audition", "tempo", "notes"], d["caps"]
    assert d["bpm"] == 140, d
    for cap in ("plugins", "add", "remove"):
        assert cap not in d["caps"]
    assert "2026.1.0" in d["dawVersion"] and "API 40" in d["dawVersion"], d
    assert not r.w.non_global, r.w.non_global


@test
def list_shows_playable_channels_with_colors():
    r = Rig()
    r.hello()
    cmd, _req, d = r.ask(P.LIST)
    assert cmd == P.LIST_REPLY
    items = d["items"]
    assert [i["id"] for i in items] == [0, 1, 2, 4], items          # pas le clip d'automation
    assert items[0] == {"id": 0, "name": "Kick", "plugin": "Sampler", "kind": "sampler", "color": "#a03020"}
    assert items[1]["plugin"] == "Serum" and items[1]["kind"] == "plugin" and items[1]["color"] == "#10ff80"
    assert items[3]["name"] == "Café ☕ Pad"
    assert "next" not in d
    assert not r.w.non_global, r.w.non_global


@test
def list_paginates_a_huge_rack():
    def setup(w):
        for i in range(400):
            w.add("Instrument number %03d with a long name" % i, "Some Very Long Plugin Name %d" % i, vst=True)
    r = Rig(setup=setup)
    r.hello()
    seen = []
    start = None
    for _ in range(10):
        _c, _r, d = r.ask(P.LIST, {} if start is None else {"start": start})
        seen += [i["id"] for i in d["items"]]
        start = d.get("next")
        if start is None:
            break
    assert seen == list(range(400)), (len(seen), seen[:3], seen[-3:])


@test
def bind_routes_notes_to_the_channel_and_nothing_leaks():
    r = Rig()
    r.hello()
    assert r.bind(1, 2) == (P.BIND_ACK, r.req, {"slot": 1, "id": 2})
    ev = r.midi(0x90, 60, 100)
    assert ev.handled
    ev = r.midi(0x80, 60, 40)
    assert ev.handled
    ev = r.midi(0x90, 62, 90)
    r.midi(0x90, 62, 0)                                             # note on a 0 = note off
    assert r.w.notes == [(2, 60, 100), (2, 60, 0), (2, 62, 90), (2, 62, 0)], r.w.notes
    # un slot non relie : consomme, rien ne joue (et surtout pas le canal selectionne de FL)
    r.w.notes = []
    ev = r.midi(0x93, 60, 100)
    assert ev.handled and r.w.notes == []
    # LIST montre le lien
    _c, _r, d = r.ask(P.LIST)
    assert [i.get("slots") for i in d["items"]] == [None, None, [1], None]


@test
def bind_errors_are_answered_with_the_request_id():
    r = Rig()
    r.hello()
    cmd, req, d = r.bind(1, 3)                                        # clip d'automation
    assert cmd == P.ERROR and req == r.req and d["code"] == "not_found", d
    cmd, req, d = r.bind(1, 99)
    assert cmd == P.ERROR and d["code"] == "not_found"
    cmd, req, d = r.ask(P.BIND, {"slot": 0, "id": 1})
    assert cmd == P.ERROR and d["code"] == "invalid", d
    for c, data in ((P.PLUGINS, {}), (P.ADD, {"plugin": "Serum"}), (P.REMOVE, {"id": 1})):
        cmd, req, d = r.ask(c, data)
        assert cmd == P.ERROR and d["code"] == "unsupported" and "channel rack" in d["msg"], d


@test
def rebinding_releases_held_notes_first():
    r = Rig()
    r.hello()
    r.bind(1, 1)
    r.midi(0x90, 60, 100)
    r.bind(1, 2)
    r.midi(0x80, 60, 0)                                              # le note off du note on d'avant
    assert r.w.notes == [(1, 60, 100), (1, 60, 0)], r.w.notes
    r.midi(0x90, 64, 100)
    assert r.w.notes[-1] == (2, 64, 100)


@test
def set_drives_volume_pan_and_mute_by_toggle():
    r = Rig()
    r.hello()
    r.bind(1, 1)
    r.send(P.SET, {"id": 1, "vol": 1.0, "pan": -0.5})
    assert abs(r.w.chans[1]["vol"] - 1.0) < 1e-9
    assert r.w.chans[1]["pan"] == -0.5
    r.send(P.SET, {"id": 1, "vol": 0.5})                              # -6 dB
    v = r.w.chans[1]["vol"]
    gain = (11 ** v - 1) / 10
    assert abs(20 * math.log10(gain) - 20 * math.log10(0.5)) < 1e-6, v
    r.send(P.SET, {"id": 1, "vol": 0.0})
    assert r.w.chans[1]["vol"] == 0.0
    # mute : deux SET dans le meme tick ne doivent pas se defaire
    r.send(P.SET, {"id": 1, "mute": True})
    r.send(P.SET, {"id": 1, "mute": True})
    r.advance(0.5)
    assert r.w.chans[1]["muted"] is True
    r.send(P.SET, {"id": 1, "mute": False})
    r.advance(0.5)
    assert r.w.chans[1]["muted"] is False
    # le studio est le maitre tant que c'est relie : un mute a la main dans FL est defait
    r.w.chans[1]["muted"] = True
    r.advance(0.5)
    assert r.w.chans[1]["muted"] is False
    assert not r.w.non_global, r.w.non_global


@test
def solo_silences_only_the_other_linked_instruments():
    r = Rig()
    r.hello()
    r.bind(1, 1)
    r.bind(2, 2)
    r.send(P.SET, {"id": 1, "solo": True})
    r.advance(0.5)
    assert r.w.chans[1]["muted"] is False
    assert r.w.chans[2]["muted"] is True
    assert r.w.chans[0]["muted"] is False                            # le reste du projet : intouche
    r.send(P.SET, {"id": 1, "solo": False})
    r.advance(0.5)
    assert r.w.chans[2]["muted"] is False


@test
def unbind_and_hello_restore_what_fl_had():
    r = Rig()
    r.w.chans[2]["vol"] = 0.6
    r.w.chans[2]["pan"] = 0.3
    r.hello()
    r.bind(4, 2)
    r.send(P.SET, {"id": 2, "vol": 0.9, "pan": -1.0, "mute": True})
    r.advance(0.5)
    assert r.w.chans[2]["muted"] is True
    r.midi(0x93, 50, 100)
    r.send(P.UNBIND, {"slot": 4})
    r.advance(0.5)
    assert r.w.notes[-1] == (2, 50, 0), r.w.notes                   # note relachee
    assert abs(r.w.chans[2]["vol"] - 0.6) < 1e-9 and r.w.chans[2]["pan"] == 0.3
    assert r.w.chans[2]["muted"] is False
    # un nouveau HELLO (le studio rechargé) remet tout comme avant aussi
    r.bind(4, 2)
    r.send(P.SET, {"id": 2, "vol": 0.2, "mute": True})
    r.advance(0.5)
    r.hello()
    r.advance(0.5)
    assert abs(r.w.chans[2]["vol"] - 0.6) < 1e-9 and r.w.chans[2]["muted"] is False
    r.midi(0x93, 50, 100)
    assert r.w.notes[-1] == (2, 50, 0)                               # plus de lien apres HELLO


@test
def sustain_pedal_is_emulated():
    r = Rig()
    r.hello()
    r.bind(1, 2)
    r.midi(0xB0, 64, 127)
    r.midi(0x90, 60, 100)
    r.midi(0x80, 60, 0)
    assert r.w.notes == [(2, 60, 100)], r.w.notes                    # retenue
    r.midi(0x90, 60, 80)                                             # rejouee pendant la pedale
    assert r.w.notes[-2:] == [(2, 60, 0), (2, 60, 80)], r.w.notes
    r.midi(0x80, 60, 0)
    r.midi(0xB0, 64, 0)
    assert r.w.notes[-1] == (2, 60, 0)
    assert r.w.params == []                                           # CC64 n'est pas envoye au plugin


@test
def mod_wheel_reaches_vsts_through_their_midi_cc_parameters():
    r = Rig()
    r.hello()
    r.bind(1, 1)                                                      # Serum (VST)
    r.bind(2, 2)                                                      # FLEX (natif)
    r.midi(0xB0, 1, 127)
    r.midi(0xB0, 11, 64)
    r.midi(0xB1, 1, 100)
    r.midi(0xB0, 74, 10)                                              # pas dans PASS_CCS
    assert r.w.params == [(1, 4097, 1.0), (1, 4107, 64 / 127.0)], r.w.params


@test
def pitch_bend_moves_the_channel_pitch_and_resets_on_release():
    r = Rig()
    r.hello()
    r.bind(1, 2)
    r.midi(0xE0, 0, 0x60)                                             # vers le haut
    r.midi(0xE0, 0x7F, 0x7F)
    r.midi(0xE0, 0, 0)
    assert [round(v, 4) for _i, v in r.w.pitch] == [0.5, round(8191 / 8192.0, 4), -1.0], r.w.pitch
    r.midi(0xB0, 123, 0)                                              # tout couper : la hauteur revient
    assert r.w.pitch[-1] == (2, 0.0)


@test
def changes_are_pushed_once_things_settle():
    r = Rig()
    r.hello()
    r.advance(1.0)
    assert r.replies() == []
    r.w.chans[2]["name"] = "Rhodes"
    r.s.OnDirtyChannel(2, MIDI.CE_Rename)
    r.advance(0.1)
    assert r.replies() == []                                          # pas avant 250 ms
    r.advance(0.3)
    assert [c for c, _q, _d in r.replies()] == [P.CHANGED]
    r.s.OnDirtyChannel(2, MIDI.CE_Select)                             # une selection ne change rien
    r.s.OnRefresh(MIDI.HW_Dirty_LEDs)
    r.advance(1.0)
    assert r.replies() == []
    r.w.chans[0]["color"] = 0x112233
    r.s.OnRefresh(MIDI.HW_Dirty_Colors)
    r.advance(0.5)
    assert [c for c, _q, _d in r.replies()] == [P.CHANGED]


@test
def a_link_follows_its_instrument_when_channels_move():
    r = Rig()
    r.hello()
    r.bind(1, 2)                                                      # FLEX Keys
    r.send(P.SET, {"id": 2, "vol": 0.5})
    r.w.chans.insert(0, {"name": "New 808", "plugin": None, "type": 0, "color": 0, "vst": False,
                         "vol": 0.78125, "pan": 0.0, "muted": False})
    r.s.OnDirtyChannel(0, MIDI.CE_New)
    r.advance(0.5)
    assert [c for c, _q, _d in r.replies()] == [P.CHANGED]
    _c, _r, d = r.ask(P.LIST)
    assert [i["id"] for i in d["items"] if i.get("slots") == [1]] == [3], d
    r.midi(0x90, 60, 100)
    assert r.w.notes[-1] == (3, 60, 100)
    # deplace (meme nombre de canaux) : suivi aussi
    r.w.chans[2], r.w.chans[3] = r.w.chans[3], r.w.chans[2]
    r.s.OnRefresh(MIDI.HW_Dirty_ChannelRack)
    r.advance(0.5)
    r.midi(0x80, 60, 0)
    assert r.w.notes[-1] == (2, 60, 0), r.w.notes                     # le note off suit aussi
    # renomme sur place : le lien reste
    r.w.chans[2]["name"] = "Keys (renamed)"
    r.s.OnDirtyChannel(2, MIDI.CE_Rename)
    r.advance(0.5)
    r.midi(0x90, 61, 100)
    assert r.w.notes[-1] == (2, 61, 100)
    # supprime : le lien tombe, rien ne joue a sa place
    r.midi(0x80, 61, 0)
    del r.w.chans[2]
    r.s.OnDirtyChannel(2, MIDI.CE_Delete)
    r.advance(0.5)
    n = len(r.w.notes)
    r.midi(0x90, 62, 100)
    assert len(r.w.notes) == n
    _c, _r, d = r.ask(P.LIST)
    assert all("slots" not in i for i in d["items"]), d


@test
def a_rename_keeps_the_link_even_when_a_twin_has_the_old_name():
    def setup(w):
        w.add("Kick", None, ctype=0)
        w.add("Serum", "Serum", vst=True)
        w.add("Pad", "Pigments", vst=True)
        w.add("Serum", "Serum", vst=True)                            # un jumeau exact (renomme a la main)
    r = Rig(setup=setup)
    r.hello()
    r.bind(1, 1)
    r.w.chans[1]["name"] = "Bass"                                     # on renomme celui qui est relie
    r.s.OnDirtyChannel(1, MIDI.CE_Rename)
    r.advance(0.5)
    r.midi(0x90, 60, 100)
    assert r.w.notes[-1] == (1, 60, 100), r.w.notes                  # pas vers le jumeau (3)
    # et un vrai deplacement, avec le jumeau toujours la, est suivi
    r.midi(0x80, 60, 0)
    r.w.chans[1], r.w.chans[2] = r.w.chans[2], r.w.chans[1]
    r.s.OnRefresh(MIDI.HW_Dirty_ChannelRack)
    r.advance(0.5)
    r.midi(0x90, 62, 100)
    assert r.w.notes[-1] == (2, 62, 100), r.w.notes


@test
def a_new_project_drops_the_links():
    r = Rig()
    r.hello()
    r.bind(1, 1)
    r.midi(0x90, 60, 100)
    r.s.OnProjectLoad(MIDI.PL_Start)
    assert r.w.notes[-1] == (1, 60, 0)
    r.s.OnProjectLoad(MIDI.PL_LoadOk)
    assert [c for c, _q, _d in r.replies()] == [P.CHANGED]
    n = len(r.w.notes)
    r.midi(0x90, 60, 100)
    assert len(r.w.notes) == n


@test
def silence_from_the_studio_releases_held_notes():
    r = Rig()
    r.hello()
    r.bind(1, 1)
    r.midi(0x90, 60, 100)
    r.advance(10.0)
    assert r.w.notes[-1] == (1, 60, 100)
    r.advance(6.0)
    assert r.w.notes[-1] == (1, 60, 0)


@test
def ping_pong_and_foreign_or_looped_sysex():
    r = Rig()
    r.hello()
    assert r.ask(P.PING, {"t": 4242.5}) == (P.PONG, r.req, {"t": 4242.5})
    ev = Ev(0xF0, sysex=bytes([0xF0, 0x7E, 0x7F, 0x06, 0x01, 0xF7]))   # pas a nous
    r.s.OnMidiMsg(ev)
    assert ev.handled is False
    # notre propre reponse revenue sur l'entree (port de sortie mal choisi) : ignoree, sans reponse
    for chunk in P.encode_message(P.LIST_REPLY, 5, {"items": []}):
        ev = Ev(0xF0, sysex=chunk)
        r.s.OnMidiMsg(ev)
        assert ev.handled
    assert r.replies() == []
    # un studio plus recent : on le dit
    ev = Ev(0xF0, sysex=bytes([0xF0, 0x7D, 0x55, 0x57, 0x02, 0x10, 0x09, 0x00, 0x01, 0x7B, 0x7D, 0xF7]))
    r.s.OnMidiMsg(ev)
    assert ev.handled
    rep = r.replies()
    assert rep[0][0] == P.ERROR and rep[0][1] == 9 and rep[0][2]["code"] == "version", rep
    # OnSysEx apres OnMidiMsg pour le meme message : traite une fois
    chunk = P.encode_message(P.PING, 20, {"t": 1})[0]
    r.s.OnMidiMsg(Ev(0xF0, sysex=chunk))
    r.s.OnSysEx(Ev(0xF0, sysex=chunk))
    assert len(r.replies()) == 1
    # une version de FL qui ne passe le SysEx qu'a OnSysEx
    chunk = P.encode_message(P.PING, 21, {"t": 2})[0]
    ev = Ev(0xF0, sysex=chunk)
    r.s.OnSysEx(ev)
    assert ev.handled and r.replies()[0][:2] == (P.PONG, 21)


@test
def no_output_port_means_a_clear_hint_not_a_crash():
    r = Rig()
    r.w.assigned = False
    printed = []
    r.s._log = lambda m: printed.append(m)
    r.send(P.HELLO, {"v": 1})
    r.advance(0.1)
    assert r.w.sent == []
    assert any("same Port number" in m for m in printed), printed


@test
def older_api_without_global_indexes_still_works():
    r = Rig(api=30)
    d = r.hello()
    assert d["caps"] == ["list", "bind", "set", "audition", "tempo", "notes"]
    r.bind(1, 1)
    r.midi(0x90, 60, 100)
    assert r.w.notes == [(1, 60, 100)]
    r.send(P.SET, {"id": 1, "vol": 1.0, "mute": True})
    r.advance(0.5)
    assert r.w.chans[1]["muted"] is True


# -- 24 oct. : le tempo partage (cap 'tempo') ---------------------------------------------------
def tempo_rig(caps=("tempo",), docs=False):
    r = Rig()
    r.w.docs = docs
    cmd, _req, d = r.ask(P.HELLO, {"v": 1, "app": "Uppwind Studio", "caps": list(caps)})
    assert cmd == P.HELLO_ACK and d["bpm"] == 140, d
    return r


def pushed(r):
    return [(c, d) for c, q, d in r.replies() if q == 0]


@test
def tempo_from_the_studio_goes_through_rec_tempo_and_is_never_sent_back():
    r = tempo_rig()
    r.send(P.TEMPO, {"bpm": 128.5})
    assert r.w.rec == [(MIDI.REC_Tempo if MIDI.__real__ else r.s.REC_TEMPO, 128500, r.s.REC_TEMPO_FLAGS)], r.w.rec
    if MIDI.__real__:
        assert r.s.REC_TEMPO_FLAGS == MIDI.REC_Control | MIDI.REC_UpdateControl
    # FL l'applique au tick suivant : pendant ce tick, le tempo lu est encore l'ancien — pas un changement
    r.advance(1.0)
    assert r.w.tempo == 128500
    assert pushed(r) == [], "the studio's own tempo: no TEMPO_CHANGED"
    # un BPM tire dans le studio : chaque TEMPO pose, aucun ne revient
    for i in range(10):
        r.send(P.TEMPO, {"bpm": 100 + i * 0.5})
        r.advance(0.03)
    r.advance(1.0)
    assert r.w.tempo == 104500 and pushed(r) == []
    # FL va de 10 a 522 BPM ; 3 decimales = des milliemes entiers
    r.send(P.TEMPO, {"bpm": 999})
    r.send(P.TEMPO, {"bpm": 97.333})
    assert [v for _i, v, _f in r.w.rec[-2:]] == [522000, 97333]


@test
def a_tempo_change_in_fl_is_pushed_at_most_every_50_ms_last_value_wins():
    r = tempo_rig()
    r.w.tempo = 133000                                                 # le tempo change dans FL
    r.advance(0.1)
    assert pushed(r) == [(P.TEMPO_CHANGED, {"bpm": 133})]
    # le tempo de FL tire a la souris : une valeur toutes les 10 ms pendant 0,3 s
    t0 = r.w.t
    for i in range(30):
        r.w.tempo = 133000 + (i + 1) * 250
        r.advance(0.01)
    r.advance(0.2)
    got = pushed(r)
    assert 2 <= len(got) <= int((r.w.t - t0) / 0.05) + 1, got
    assert got[-1] == (P.TEMPO_CHANGED, {"bpm": 140.5}), got
    r.advance(1.0)
    assert pushed(r) == []
    # le studio le reprend (TEMPO egal) : rien ne repart
    r.send(P.TEMPO, {"bpm": 140.5})
    r.advance(1.0)
    assert pushed(r) == []


@test
def a_fl_change_right_after_a_studio_tempo_is_reported_once_the_hold_ends():
    r = tempo_rig()
    r.send(P.TEMPO, {"bpm": 128})
    r.advance(0.04)
    r.w.tempo = 133000
    r.advance(0.1)
    assert pushed(r) == []
    r.advance(P.TEMPO_HOLD_MS / 1000.0)
    assert pushed(r) == [(P.TEMPO_CHANGED, {"bpm": 133})], "not lost"


@test
def no_tempo_pushed_to_a_studio_that_did_not_ask():
    r = Rig()
    d = r.hello()                                                      # un ancien studio : pas de caps
    assert d["bpm"] == 140
    r.w.tempo = 90000
    r.advance(0.5)
    assert pushed(r) == []


@test
def get_current_tempo_read_either_way_and_a_new_project_is_a_tempo_change():
    # asInt dans le sens de la doc (1 -> entier x 1000) ou dans celui que les scripts constatent : le meme tempo
    for docs in (False, True):
        r = tempo_rig(docs=docs)
        r.w.tempo = 87125
        r.advance(0.1)
        assert pushed(r) == [(P.TEMPO_CHANGED, {"bpm": 87.125})], docs
    # un autre projet ouvert dans FL, a un autre tempo : le studio le sait
    r.w.tempo = 95000
    r.s.OnProjectLoad(MIDI.PL_LoadOk)
    r.advance(0.1)
    assert (P.TEMPO_CHANGED, {"bpm": 95}) in pushed(r)


# -- 26 oct. : les notes par Uppwind Link Notes (cap 'notes', BIND.via 'plugin') ---------------------------
def notes_rig(**kw):
    def setup(w):
        w.add("Kick", None, ctype=0)
        w.add("Serum Bass", "Serum", vst=True, port=10)               # le port d'entree 10
        w.add("Pad", "Pigments", vst=True)                             # sans port
        w.add_notes("Uppwind Link Notes", port=10, **kw)               # entree et sortie sur 10
        w.add("Keys", "Diva", vst=True, port=12)                       # un port, mais pas de Notes dessus
    r = Rig(setup=setup)
    printed = []
    r.s._log = lambda m: printed.append(m)
    r.printed = printed
    return r


def tag_of(r, i):
    return int(round(r.w.chans[i]["pv"].get(0, 0.0) * 255))


@test
def a_notes_channel_is_hidden_and_takes_its_midi_port_as_link_tag():
    r = notes_rig()
    r.hello()
    assert tag_of(r, 3) == 10, r.w.params                              # HELLO : le numero = le port
    _c, _r, d = r.ask(P.LIST)
    assert [i["name"] for i in d["items"]] == ["Kick", "Serum Bass", "Pad", "Keys"], d   # pas le Notes
    cmd, _q, e = r.bind(1, 3)                                           # le Notes n'est pas un instrument
    assert cmd == P.ERROR and e["code"] == "not_found", e
    assert not r.w.non_global, r.w.non_global
    # un port change a la main : le numero suit (le filet de securite, toutes les 2 s)
    r.w.chans[3]["port"] = 20
    r.advance(2.5)
    assert tag_of(r, 3) == 20
    assert any("Link tag 20" in m for m in r.printed), r.printed


@test
def bind_via_plugin_names_the_notes_on_the_instruments_input_port():
    r = notes_rig()
    r.hello()
    cmd, _q, d = r.ask(P.BIND, {"slot": 1, "id": 1, "via": "plugin"})
    assert (cmd, d) == (P.BIND_ACK, {"slot": 1, "id": 1, "via": "plugin", "notes": 10}), d
    # le mix passe toujours par le script (volume, pan, mute du canal)
    r.send(P.SET, {"id": 1, "vol": 1.0, "pan": 0.25})
    assert abs(r.w.chans[1]["vol"] - 1.0) < 1e-9 and r.w.chans[1]["pan"] == 0.25
    _c, _r, d = r.ask(P.LIST)
    assert [i.get("slots") for i in d["items"]] == [None, [1], None, None]
    assert any("plays from the Uppwind Link Notes on MIDI port 10" in m for m in r.printed), r.printed
    # un BIND sans via (un studio d'avant) : comme avant, sans via ni notes
    assert r.bind(2, 1)[2] == {"slot": 2, "id": 1}


@test
def bind_via_plugin_falls_back_to_loopmidi_and_says_why():
    r = notes_rig()
    r.hello()
    _c, _q, d = r.ask(P.BIND, {"slot": 1, "id": 2, "via": "plugin"})   # Pad : sans port
    assert d == {"slot": 1, "id": 2, "via": "port"}, d
    assert any("has no MIDI input port" in m for m in r.printed), r.printed
    _c, _q, d = r.ask(P.BIND, {"slot": 2, "id": 4, "via": "plugin"})   # Keys : port 12, aucun Notes dessus
    assert d == {"slot": 2, "id": 4, "via": "port"}, d
    assert any("no Uppwind Link Notes on MIDI port 12" in m for m in r.printed), r.printed
    # par ici : le script joue les notes recues, comme avant
    r.midi(0x91, 60, 100)
    assert r.w.notes == [(4, 60, 100)], r.w.notes
    # un Notes d'avant le 25 oct. (sans « Link tag ») : jamais choisi
    o = notes_rig(tagged=False)
    o.hello()
    _c, _q, d = o.ask(P.BIND, {"slot": 1, "id": 1, "via": "plugin"})
    assert d["via"] == "port", d
    assert any("older Uppwind Link Notes" in m for m in o.printed), o.printed


@test
def a_route_that_changes_drops_the_link_and_pushes_changed():
    r = notes_rig()
    r.hello()
    r.ask(P.BIND, {"slot": 1, "id": 1, "via": "plugin"})
    r.replies()
    r.w.chans[1]["port"] = 30                                          # l'instrument change de port
    r.advance(2.5)
    assert [c for c, _q, _d in r.replies()] == [P.CHANGED]
    _c, _r, d = r.ask(P.LIST)
    assert all("slots" not in i for i in d["items"]), d
    # le studio relie a nouveau : plus de Notes sur 30, par ici
    _c, _q, d = r.ask(P.BIND, {"slot": 1, "id": 1, "via": "plugin"})
    assert d["via"] == "port"
    # le Notes passe sur 30 aussi : la route revient au prochain BIND
    r.w.chans[3]["port"] = 30
    r.advance(2.5)
    assert tag_of(r, 3) == 30
    _c, _q, d = r.ask(P.BIND, {"slot": 1, "id": 1, "via": "plugin"})
    assert d == {"slot": 1, "id": 1, "via": "plugin", "notes": 30}, d


@test
def two_notes_on_one_port_are_reported_once():
    def setup(w):
        w.add("Serum", "Serum", vst=True, port=10)
        w.add_notes("Notes A", port=10)
        w.add_notes("Notes B", port=10)
    r = Rig(setup=setup)
    printed = []
    r.s._log = lambda m: printed.append(m)
    r.hello()
    r.advance(5.0)
    assert len([m for m in printed if "share MIDI port 10" in m]) == 1, printed
    _c, _q, d = r.ask(P.BIND, {"slot": 1, "id": 0, "via": "plugin"})
    assert d["notes"] == 10


@test
def uses_only_midi_constants_that_fl_really_has():
    if not MIDI.__real__:
        print("    (FL Studio not installed: real midi.py not checked)")
        return
    src = open(os.path.join(FL_DIR, "device_UppwindLink.py"), encoding="utf-8").read()
    import re
    names = set(re.findall(r'getattr\(midi, "([A-Za-z_]+)"', src)) | set(re.findall(r"\bmidi\.([A-Z][A-Za-z0-9_]*)", src))
    missing = sorted(n for n in names if not hasattr(MIDI, n))
    assert not missing, missing


@test
def the_script_header_is_what_fl_reads():
    src = open(os.path.join(FL_DIR, "device_UppwindLink.py"), encoding="utf-8").read().splitlines()
    assert src[0] == "# name=Uppwind Link"
    assert src[1].startswith("# supportedDevices=") and "Uppwind Link" in src[1]
    # FL n'importe que des modules de la bibliotheque standard presents dans son Python embarque
    text = "\n".join(src)
    for mod in ("socket", "os", "subprocess", "threading", "open("):
        assert ("import " + mod) not in text and (mod != "open(" or " open(" not in text), mod


def main():
    failed = 0
    for fn in TESTS:
        try:
            fn()
            print("ok   " + fn.__name__)
        except Exception:  # noqa: BLE001
            failed += 1
            import traceback
            print("FAIL " + fn.__name__)
            traceback.print_exc()
    print("fl-studio script, python %s (%s midi.py): %d/%d passed"
          % (sys.version.split()[0], "real" if MIDI.__real__ else "fake", len(TESTS) - failed, len(TESTS)))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
