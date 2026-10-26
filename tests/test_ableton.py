# -*- coding: utf-8 -*-
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Uppwind
"""Uppwind Link -- le Remote Script Ableton (ableton/UppwindLink) contre un faux Live.

    python -I tests/test_ableton.py

Ableton Live n'est pas installe ici : ce test fait tourner la logique (link_core.LinkCore) et la
coquille (UppwindLink.py, sans Live ni _Framework) sur un faux Live Object Model ecrit d'apres la
documentation du LOM. Il verifie aussi que tout le paquet se lit en Python 3.7 (Live 11).
"""

import ast
import json
import math
import os
import shutil
import sys
import tempfile

sys.dont_write_bytecode = True   # pas de __pycache__ dans les dossiers des scripts

# 24 oct. : le vrai script ouvre maintenant le canal local du compagnon (editor_channel.py). Aucun test n'ecrit le
# vrai ~/.uppwind-link-dev.json (celui du Live de la machine) ni n'occupe 47391… : un fichier d'etat a part, un port
# au hasard (sauf dev_shell_and_cli_over_udp, qui verifie la plage de ports et retablit ce choix).
TMP = tempfile.mkdtemp(prefix="uwlink-abl-")
os.environ["UWLINK_DEV_FILE"] = os.path.join(TMP, "state.json")
os.environ["UWLINK_CHANNEL_PORT"] = "0"

HERE = os.path.dirname(os.path.abspath(__file__))
LINK = os.path.dirname(HERE)
ABL = os.path.join(LINK, "ableton")
sys.path.insert(0, ABL)

import UppwindLink  # noqa: E402
from UppwindLink import link_core as C  # noqa: E402
from UppwindLink import uwlink_protocol as P  # noqa: E402

TESTS = []


def test(fn):
    TESTS.append(fn)
    return fn


# -- Le faux Live -----------------------------------------------------------------------------
def live_db(v):
    # une courbe monotone, de la forme de celle de Live : 0.85 = 0 dB, 1.0 = +6 dB
    if v <= 0.0:
        return -math.inf
    if v >= 0.4:
        return 40.0 * (v - 0.85)
    return 40.0 * (0.4 - 0.85) + 60.0 * math.log10(v / 0.4)


class Param(object):
    def __init__(self, value, lo, hi, db=False):
        self.value, self.min, self.max, self._db = value, lo, hi, db

    def str_for_value(self, v):
        if not self._db:
            return "%d" % round(v * 50)
        d = live_db(v)
        return "-inf dB" if d == -math.inf else "%.1f dB" % d


class Mixer(object):
    def __init__(self):
        self.volume = Param(0.85, 0.0, 1.0, db=True)
        self.panning = Param(0.0, -1.0, 1.0)


class Device(object):
    def __init__(self, name, class_name="PluginDevice", type=1, can_have_chains=False):
        self.name, self.class_name, self.type, self.can_have_chains = name, class_name, type, can_have_chains


class Routing(object):
    def __init__(self, name):
        self.display_name = name

    def __repr__(self):
        return "<%s>" % self.display_name


ALL_INS = Routing("All Ins")
LINK_IN = Routing("Uppwind Link")
LINK_BACK = Routing("Uppwind Link Back")
KEYS = Routing("Computer Keyboard")
ALL_CH = Routing("All Channels")
CHANNELS = [Routing("Ch. %d" % n) for n in range(1, 17)]


class Track(object):
    def __init__(self, name, devices=(), midi=True, color=0x10AA55, link_port=True):
        self.name, self.color, self.has_midi_input = name, color, midi
        self.devices = list(devices)
        self.mute = False
        self.mixer_device = Mixer()
        self._types = [ALL_INS, KEYS] + ([LINK_IN, LINK_BACK] if link_port else [])
        self.input_routing_type = ALL_INS
        self.input_routing_channel = ALL_CH
        self.current_monitoring_state = 1

    @property
    def available_input_routing_types(self):
        return list(self._types)

    @property
    def available_input_routing_channels(self):
        return [ALL_CH] + CHANNELS


class View(object):
    selected_track = None
    selected_device = None

    def select_device(self, device):
        self.selected_device = device


class Song(object):
    def __init__(self, tracks):
        self.tracks = list(tracks)
        self.view = View()
        # 24 oct. : Song.tempo (20..999) et son rappel, comme le LOM (add / remove / has_listener ; un
        # rappel ajoute deux fois, ou retire sans etre la, leve — comme Live)
        self._tempo = 120.0
        self._tempo_ls = []
        self.tempo_sets = []

    @property
    def tempo(self):
        return self._tempo

    @tempo.setter
    def tempo(self, v):
        v = float(v)
        if not 20.0 <= v <= 999.0:
            raise ValueError("Invalid tempo")
        self.tempo_sets.append(v)
        moved = v != self._tempo
        self._tempo = v
        if moved:
            for f in list(self._tempo_ls):
                f()

    def add_tempo_listener(self, f):
        if f in self._tempo_ls:
            raise RuntimeError("Listener already connected")
        self._tempo_ls.append(f)

    def remove_tempo_listener(self, f):
        if f not in self._tempo_ls:
            raise RuntimeError("Listener not connected")
        self._tempo_ls.remove(f)

    def tempo_has_listener(self, f):
        return f in self._tempo_ls

    def create_midi_track(self, index):
        self.tracks.insert(index, Track("%d-MIDI" % (index + 1), color=0x777777))

    def delete_track(self, index):
        del self.tracks[index]


class Item(object):
    def __init__(self, name, children=(), device=False, uri=None):
        self.name, self.children = name, list(children)
        self.is_device = device
        self.is_loadable = device
        self.is_folder = not device
        self.uri = uri


class Browser(object):
    def __init__(self, song):
        self.song = song
        self.instruments = Item("Instruments", [Item("Wavetable", [Item("Preset A", device=False)], device=True, uri="query:Synths#Wavetable"),
                                                Item("Drum Rack", device=True, uri="query:Drums#Drum%20Rack"),
                                                Item("Presets folder")])
        self.plugins = Item("Plug-Ins", [
            Item("VST3", [Item("Xfer Records", [Item("Serum", device=True, uri="query:Plugins#VST3:Xfer%20Records:Serum")]),
                          Item("Arturia", [Item("Pigments", device=True)])]),
            Item("VST", [Item("Vital", device=True, uri="query:Plugins#VST:Vital")]),
        ])
        self.loaded = []

    def load_item(self, item):
        t = self.song.view.selected_track
        t.devices.append(Device(item.name, "PluginDevice", 1))
        self.loaded.append(item.name)


class Rig(object):
    def __init__(self, link_port=True):
        self.song = Song([
            Track("Drums", [Device("Drum Rack", "DrumGroupDevice", 1, True)], color=0xFF0000),
            Track("Vocals", [], midi=False),
            Track("Lead", [Device("Arpeggiator", "MxDeviceMidiEffect", 4), Device("Serum")], color=0x00FF88, link_port=link_port),
            Track("Empty MIDI", []),
            Track("Keys", [Device("Electric", "LoungeLizard", 1)], color=0x2244CC),
        ])
        self.browser = Browser(self.song)
        self.t = 100.0
        self.sent = []
        self.logs = []
        self.core = C.LinkCore(song=lambda: self.song, send=self.sent.append, log=self.logs.append,
                               now=lambda: self.t, browser=lambda: self.browser, version=lambda: "12.1.5")
        self.rx = P.Reassembler()
        self.req = 0

    def send(self, cmd, data=None):
        self.req = P.next_req(self.req)
        for chunk in P.encode_message(cmd, self.req, data or {}):
            assert self.core.receive(tuple(bytearray(chunk)))
        return self.req

    def replies(self):
        for _ in range(60):
            self.core.flush()
        out = []
        for b in self.sent:
            assert isinstance(b, tuple) and all(isinstance(x, int) for x in b)
            r = self.rx.push(b, self.t * 1000)
            if r["type"] == "message":
                v = P.validate_message(r["cmd"], r["data"], "daw")
                assert v["ok"], v
                out.append((r["cmd"], r["req"], v["data"]))
        self.sent[:] = []
        return out

    def ask(self, cmd, data=None):
        req = self.send(cmd, data)
        rep = [r for r in self.replies() if r[1] == req]
        assert len(rep) == 1, rep
        return rep[0]

    def advance(self, s):
        steps = max(1, int(s / 0.1))
        for _ in range(steps):
            self.t += s / steps
            self.core.tick()


# -- Les tests --------------------------------------------------------------------------------
@test
def hello_and_caps():
    r = Rig()
    cmd, _req, d = r.ask(P.HELLO, {"v": 1})
    assert cmd == P.HELLO_ACK
    assert d["daw"] == "Ableton Live" and d["dawVersion"] == "12.1.5"
    assert d["caps"] == ["list", "bind", "set", "plugins", "add", "remove", "audition", "tempo", "notes"]
    assert d["bpm"] == 120, d


@test
def list_shows_midi_tracks_with_an_instrument():
    r = Rig()
    r.ask(P.HELLO, {"v": 1})
    _c, _r, d = r.ask(P.LIST)
    assert [(i["id"], i["name"], i["plugin"], i["kind"]) for i in d["items"]] == [
        (0, "Drums", "Drum Rack", "rack"), (2, "Lead", "Serum", "plugin"), (4, "Keys", "Electric", "instrument")], d
    assert d["items"][1]["color"] == "#00ff88"


@test
def bind_routes_the_track_input_and_unbind_restores_it():
    r = Rig()
    r.ask(P.HELLO, {"v": 1})
    lead = r.song.tracks[2]
    assert r.ask(P.BIND, {"slot": 3, "id": 2}) == (P.BIND_ACK, r.req, {"slot": 3, "id": 2, "via": "port"})
    assert lead.input_routing_type is LINK_IN
    assert lead.input_routing_channel.display_name == "Ch. 3"
    assert lead.current_monitoring_state == 0
    _c, _r, d = r.ask(P.LIST)
    assert [i.get("slots") for i in d["items"]] == [None, [3], None]
    r.send(P.UNBIND, {"slot": 3})
    assert lead.input_routing_type is ALL_INS and lead.input_routing_channel is ALL_CH
    assert lead.current_monitoring_state == 1
    # les notes ne passent pas par le script (Live les joue lui-meme)
    assert r.core.receive((0x92, 60, 100)) is False


@test
def bind_explains_a_missing_track_switch():
    r = Rig(link_port=False)
    r.ask(P.HELLO, {"v": 1})
    cmd, _req, d = r.ask(P.BIND, {"slot": 1, "id": 2})
    assert cmd == P.ERROR and d["code"] == "not_found" and "Track" in d["msg"], d
    cmd, _req, d = r.ask(P.BIND, {"slot": 1, "id": 1})                 # piste audio
    assert cmd == P.ERROR and d["code"] == "not_found"
    cmd, _req, d = r.ask(P.BIND, {"slot": 1, "id": 3})                 # MIDI sans instrument
    assert cmd == P.ERROR


@test
def bind_refuses_when_live_offers_no_channel_n():
    # Sans « Ch. n », la piste ecouterait All Channels, donc les 16 slots : on refuse et on ne touche a rien
    class NoChannels(Track):
        @property
        def available_input_routing_channels(self):
            return [ALL_CH]

    r = Rig()
    odd = NoChannels("Odd", [Device("Vital")])
    r.song.tracks.append(odd)
    r.ask(P.HELLO, {"v": 1})
    r.ask(P.BIND, {"slot": 2, "id": 4})                                # Keys, relie normalement
    cmd, _req, d = r.ask(P.BIND, {"slot": 2, "id": 5})
    assert cmd == P.ERROR and d["code"] == "not_found" and "Ch. 2" in d["msg"], d
    assert odd.input_routing_type is ALL_INS and odd.input_routing_channel is ALL_CH
    assert odd.current_monitoring_state == 1
    keys = r.song.tracks[4]
    assert keys.input_routing_type is LINK_IN and keys.input_routing_channel.display_name == "Ch. 2"
    _c, _r, d = r.ask(P.LIST)
    assert [i.get("slots") for i in d["items"]] == [None, None, [2], None], d


@test
def one_track_one_slot():
    r = Rig()
    r.ask(P.HELLO, {"v": 1})
    r.ask(P.BIND, {"slot": 1, "id": 4})
    r.ask(P.BIND, {"slot": 2, "id": 4})
    keys = r.song.tracks[4]
    assert keys.input_routing_channel.display_name == "Ch. 2"
    _c, _r, d = r.ask(P.LIST)
    assert d["items"][2]["slots"] == [2]                               # le slot 1 attend, invisible
    r.ask(P.BIND, {"slot": 2, "id": 0})                                # le slot change de piste
    # 21 oct. : la piste n'est pas rendue a son entree d'avant — le slot 1 la veut encore, il la reprend
    assert keys.input_routing_type is LINK_IN and keys.input_routing_channel.display_name == "Ch. 1"
    _c, _r, d = r.ask(P.LIST)
    assert d["items"][0]["slots"] == [2] and d["items"][2]["slots"] == [1], d
    r.send(P.UNBIND, {"slot": 1})
    assert keys.input_routing_type is ALL_INS and keys.current_monitoring_state == 1


@test
def unbind_of_the_displacing_slot_does_not_strand_the_track():
    # 21 oct. (Kontakt muet) : deux pistes du studio sur la meme piste de Live. Le second BIND
    # prenait la piste, son UNBIND rendait l'entree d'avant : le premier slot jouait dans le vide
    r = Rig()
    r.ask(P.HELLO, {"v": 1})
    r.ask(P.BIND, {"slot": 1, "id": 4})
    r.send(P.SET, {"id": 4, "vol": 0.5})
    r.ask(P.BIND, {"slot": 3, "id": 4})
    keys = r.song.tracks[4]
    assert keys.input_routing_channel.display_name == "Ch. 3"
    r.send(P.UNBIND, {"slot": 3})
    assert keys.input_routing_type is LINK_IN and keys.input_routing_channel.display_name == "Ch. 1"
    assert keys.current_monitoring_state == C.MONITOR_IN
    assert abs(live_db(keys.mixer_device.volume.value) - 20 * math.log10(0.5)) < 0.15
    _c, _r, d = r.ask(P.LIST)
    assert d["items"][2]["slots"] == [1], d
    # le slot deplace qui s'en va de lui-meme : rien ne bouge pour celui qui joue
    r.ask(P.BIND, {"slot": 3, "id": 4})
    r.send(P.UNBIND, {"slot": 1})
    assert keys.input_routing_channel.display_name == "Ch. 3"
    r.send(P.UNBIND, {"slot": 3})
    assert keys.input_routing_type is ALL_INS and keys.current_monitoring_state == 1
    assert keys.mixer_device.volume.value == 0.85
    assert not r.core.binds and not r.core.waiting


@test
def set_uses_lives_own_volume_curve():
    r = Rig()
    r.ask(P.HELLO, {"v": 1})
    r.ask(P.BIND, {"slot": 1, "id": 2})
    lead = r.song.tracks[2]
    for gain in (1.0, 0.5, 0.25, 0.1, 0.01):
        r.send(P.SET, {"id": 2, "vol": gain})
        want = 20 * math.log10(gain)
        assert abs(live_db(lead.mixer_device.volume.value) - want) < 0.15, (gain, lead.mixer_device.volume.value)
    r.send(P.SET, {"id": 2, "vol": 0})
    assert lead.mixer_device.volume.value == 0.0
    r.send(P.SET, {"id": 2, "pan": -0.4, "mute": True})
    assert lead.mixer_device.panning.value == -0.4 and lead.mute is True
    r.send(P.UNBIND, {"slot": 1})
    assert lead.mixer_device.volume.value == 0.85 and lead.mixer_device.panning.value == 0.0 and lead.mute is False
    # sans str_for_value lisible : l'approximation
    p = Param(0.85, 0.0, 1.0)
    p.str_for_value = lambda v: "??"
    assert abs(C.gain_to_live_volume(p, 0.5) - (0.85 - 6.0206 / 40)) < 1e-3


@test
def solo_only_touches_linked_tracks():
    r = Rig()
    r.ask(P.HELLO, {"v": 1})
    r.ask(P.BIND, {"slot": 1, "id": 2})
    r.ask(P.BIND, {"slot": 2, "id": 4})
    r.send(P.SET, {"id": 2, "solo": True})
    assert r.song.tracks[2].mute is False and r.song.tracks[4].mute is True and r.song.tracks[0].mute is False
    r.send(P.SET, {"id": 2, "solo": False})
    assert r.song.tracks[4].mute is False
    # HELLO (le studio recharge) : tout revient comme avant
    r.send(P.SET, {"id": 4, "vol": 0.1, "mute": True})
    r.ask(P.HELLO, {"v": 1})
    k = r.song.tracks[4]
    assert k.mute is False and k.mixer_device.volume.value == 0.85 and k.input_routing_type is ALL_INS


@test
def plugins_add_and_remove():
    r = Rig()
    r.ask(P.HELLO, {"v": 1})
    _c, _r, d = r.ask(P.PLUGINS)
    names = [(i["name"], i["vendor"], i["kind"]) for i in d["items"]]
    assert names == [("Wavetable", "Ableton", "instrument"), ("Drum Rack", "Ableton", "instrument"),
                     ("Serum", "Xfer Records", "plugin"), ("Pigments", "Arturia", "plugin"), ("Vital", "", "plugin")], names
    serum = [i for i in d["items"] if i["name"] == "Serum"][0]
    assert serum["id"] == "query:Plugins#VST3:Xfer%20Records:Serum"
    cmd, _req, d = r.ask(P.ADD, {"plugin": serum["id"], "name": "Bass"})
    assert cmd == P.ADDED, d
    item = d["item"]
    assert item["id"] == 5 and item["name"] == "Bass" and item["plugin"] == "Serum"
    assert r.song.tracks[5].devices[0].name == "Serum" and r.browser.loaded == ["Serum"]
    _c, _r, d = r.ask(P.LIST)
    assert d["items"][-1]["name"] == "Bass"
    cmd, _req, d = r.ask(P.ADD, {"plugin": "nope"})
    assert cmd == P.ERROR and d["code"] == "not_found"
    # REMOVE : seulement ce que le studio a ajoute
    cmd, _req, d = r.ask(P.REMOVE, {"id": 0})
    assert cmd == P.ERROR and d["code"] == "unsupported", d
    assert len(r.song.tracks) == 6
    r.ask(P.BIND, {"slot": 5, "id": 5})
    r.send(P.REMOVE, {"id": 5})
    assert len(r.song.tracks) == 5 and 5 not in r.core.binds


@test
def plugins_paginate():
    r = Rig()
    r.browser.plugins = C_item_tree(900)
    r.ask(P.HELLO, {"v": 1})
    seen, start = [], None
    for _ in range(20):
        _c, _r, d = r.ask(P.PLUGINS, {} if start is None else {"start": start})
        seen += [i["name"] for i in d["items"]]
        start = d.get("next")
        if start is None:
            break
    assert len(seen) == 2 + 900, len(seen)


@test
def plugins_loadable_items_that_are_not_marked_device():
    # 14 oct. : un plugin chargeable (is_loadable) mais sans is_device, sous un dossier a 3 niveaux, est trouve ;
    # un arbre sans aucun plugin le dit dans le Log.txt de Live (son haut, pour savoir pourquoi)
    r = Rig()
    vst = Item("Serum 2", uri="query:Plugins#VST3:Xfer%20Records:Serum%202")
    vst.is_loadable, vst.is_folder = True, False
    r.browser.plugins = Item("Plug-Ins", [Item("VST3", [Item("Xfer Records", [Item("Synths", [vst])])])])
    r.ask(P.HELLO, {"v": 1})
    _c, _r, d = r.ask(P.PLUGINS)
    got = [(i["name"], i["vendor"], i["kind"]) for i in d["items"] if i["kind"] == "plugin"]
    assert got == [("Serum 2", "Synths", "plugin")], got
    logs = []
    r.core._log = logs.append
    r.browser.plugins = Item("Plug-Ins", [Item("VST3", [])])
    r.ask(P.PLUGINS)
    assert any("no plugin found" in m and "VST3[f=1" in m for m in logs), logs


def C_item_tree(n):
    return Item("Plug-Ins", [Item("VST3", [Item("Vendor %d" % (i // 10), [
        Item("A plugin with a rather long name number %04d" % i, device=True,
             uri="query:Plugins#VST3:Vendor%%20%d:A%%20plugin%%20with%%20a%%20rather%%20long%%20name%%20number%%20%04d" % (i // 10, i))])
        for i in range(n)])])


@test
def changes_and_deleted_tracks():
    r = Rig()
    r.ask(P.HELLO, {"v": 1})
    r.ask(P.BIND, {"slot": 1, "id": 4})
    r.advance(1.5)
    assert r.replies() == []
    r.song.tracks[2].name = "Lead 2"
    r.advance(1.5)
    assert [c for c, _q, _d in r.replies()] == [P.CHANGED]
    r.song.tracks.insert(0, Track("New", [Device("Operator", "Operator", 1)]))
    r.advance(1.5)
    assert [c for c, _q, _d in r.replies()] == [P.CHANGED]
    _c, _r, d = r.ask(P.LIST)
    assert [i["id"] for i in d["items"] if i.get("slots")] == [5]       # le lien suit la piste
    del r.song.tracks[5]
    r.advance(1.5)
    assert [c for c, _q, _d in r.replies()] == [P.CHANGED]
    assert r.core.binds == {}


# -- 24 oct. : le tempo partage (cap 'tempo') ----------------------------------------------------
def tempo_rig(caps=("tempo",), bpm=120.0):
    r = Rig()
    r.song._tempo = bpm
    cmd, _req, d = r.ask(P.HELLO, {"v": 1, "app": "Uppwind Studio", "caps": list(caps)})
    assert cmd == P.HELLO_ACK and d["bpm"] == P.round_bpm(bpm), d
    return r


def pushed(r):
    return [(c, d) for c, q, d in r.replies() if q == 0]


@test
def tempo_from_the_studio_sets_song_tempo_and_is_never_sent_back():
    r = tempo_rig()
    assert r.song._tempo_ls == [r.core._tempo_cb], "the listener is on Song.tempo"
    r.send(P.TEMPO, {"bpm": 128.5})
    assert r.song.tempo == 128.5 and r.song.tempo_sets == [128.5]
    assert pushed(r) == [], "the studio's own tempo: no TEMPO_CHANGED (the listener fired, nothing went out)"
    r.advance(1.0)
    assert pushed(r) == []
    # un BPM tire dans le studio : chaque TEMPO pose, aucun ne revient
    for i in range(10):
        r.send(P.TEMPO, {"bpm": 100 + i})
        r.advance(0.05)
    r.advance(1.0)
    assert r.song.tempo == 109 and pushed(r) == []
    # au-dela de Live : borne (le schema garde deja 20..999)
    r.send(P.TEMPO, {"bpm": 999})
    assert r.song.tempo == 999.0


@test
def a_tempo_change_in_live_is_pushed_at_most_every_50_ms_last_value_wins():
    r = tempo_rig()
    r.song.tempo = 130.0                                               # le tempo change dans Live
    assert pushed(r) == [(P.TEMPO_CHANGED, {"bpm": 130})], "the listener pushes it right away"
    # le bouton de tempo de Live, tire : des rappels a la suite dans le meme instant
    for v in (131.0, 132.5, 134.25, 136.0, 140.123456):
        r.song.tempo = v
    assert pushed(r) == [], "not within 50 ms of the last one"
    r.advance(0.1)                                                     # tick : le dernier, une fois
    assert pushed(r) == [(P.TEMPO_CHANGED, {"bpm": 140.123})]
    r.advance(1.0)
    assert pushed(r) == []
    # puis le studio le reprend (TEMPO egal) : rien ne repart
    r.send(P.TEMPO, {"bpm": 140.123})
    r.advance(0.5)
    assert pushed(r) == []


@test
def a_live_change_right_after_a_studio_tempo_is_reported_once_the_hold_ends():
    r = tempo_rig()
    r.send(P.TEMPO, {"bpm": 128})
    r.song.tempo = 133.0                                               # dans Live, aussitot
    assert pushed(r) == []
    r.advance(P.TEMPO_HOLD_MS / 1000.0 + 0.15)
    assert pushed(r) == [(P.TEMPO_CHANGED, {"bpm": 133})], "not lost"


@test
def no_tempo_pushed_to_a_studio_that_did_not_ask_and_no_listener_api_still_polls():
    r = Rig()
    r.ask(P.HELLO, {"v": 1})                                           # un ancien studio : pas de caps
    r.song.tempo = 90.0
    r.advance(0.5)
    assert pushed(r) == []
    # un Live sans add_tempo_listener (ou un rappel perdu) : tick() relit le tempo
    r = Rig()
    r.song.add_tempo_listener = None
    r.ask(P.HELLO, {"v": 1, "caps": ["tempo"]})
    assert r.song._tempo_ls == []
    r.song.tempo = 97.5
    assert pushed(r) == []
    r.advance(0.1)
    assert pushed(r) == [(P.TEMPO_CHANGED, {"bpm": 97.5})]


@test
def hello_again_and_shutdown_keep_one_listener_and_leave_the_tempo():
    r = tempo_rig()
    r.ask(P.HELLO, {"v": 1, "caps": ["tempo"]})
    r.ask(P.HELLO, {"v": 1, "caps": ["tempo"]})
    assert len(r.song._tempo_ls) == 1
    r.send(P.TEMPO, {"bpm": 150})
    r.core.shutdown()
    assert r.song._tempo_ls == [], "the listener is removed"
    assert r.song.tempo == 150.0, "the tempo is shared, not borrowed: it stays"
    # HELLO_ACK sans tempo lisible : pas de bpm
    r = Rig()
    r.core._song = lambda: None
    _c, _q, d = r.ask(P.HELLO, {"v": 1, "caps": ["tempo"]})
    assert "bpm" not in d, d


@test
def shell_runs_without_live_or_framework():
    import importlib
    shell = importlib.import_module("UppwindLink.UppwindLink")
    assert shell._FRAMEWORK is False

    class CInstance(object):
        def __init__(self):
            self.sent, self.logs = [], []
            self._song = Rig().song

        def song(self):
            return self._song

        def send_midi(self, b):
            self.sent.append(b)

        def log_message(self, m):
            self.logs.append(m)

    c = CInstance()
    s = UppwindLink.create_instance(c)
    assert s.suggest_input_port() == "Uppwind Link" and s.suggest_output_port() == "Uppwind Link Back"
    for chunk in P.encode_message(P.HELLO, 3, {"v": 1}):
        s.receive_midi(tuple(bytearray(chunk)))
    s.update_display()
    out = P.decode_chunks(c.sent)
    assert out["cmd"] == P.HELLO_ACK and out["data"]["daw"] == "Ableton Live", out
    s.receive_midi((0x90, 60, 100))
    s.disconnect()
    assert any("ready" in m for m in c.logs)


@test
def every_file_reads_as_python_37():
    folder = os.path.join(ABL, "UppwindLink")
    for f in sorted(os.listdir(folder)):
        if f.endswith(".py"):
            src = open(os.path.join(folder, f), encoding="utf-8").read()
            ast.parse(src, filename=f, feature_version=(3, 7))
    src = open(os.path.join(LINK, "shared", "uwlink_protocol.py"), encoding="utf-8").read()
    ast.parse(src, feature_version=(3, 7))


# -- Les fenetres des plugins (editor_core.py, editor_channel.py) : le canal local du compagnon -----------------
# Le faux Live d'apres la doc du LOM : un VST2 expose ses programmes (presets, selected_preset_index), un VST3
# (Serum 2, Kontakt 8) une liste vide et -1 ; is_editor_open n'existe qu'a partir de Live 12.4.3.
sys.path.insert(0, os.path.join(LINK, "dev"))
from UppwindLink import editor_core as EC  # noqa: E402
from UppwindLink import editor_channel as ECH  # noqa: E402
import uwlink_dev  # noqa: E402


class PluginDev(Device):
    def __init__(self, name, presets=(), selected=-1, editor=True, cls="PluginDevice"):
        Device.__init__(self, name, cls, 1)
        self.presets = list(presets)
        self.selected_preset_index = selected
        self.parameters = [object()] * 9
        if editor:
            self.is_editor_open = False


class OldPluginDev(Device):
    """Live < 12.4.3 : pas de is_editor_open (lire leve AttributeError)."""
    def __init__(self, name):
        Device.__init__(self, name, "PluginDevice", 1)
        self.presets = []
        self.selected_preset_index = -1


class Chain(object):
    def __init__(self, devices):
        self.devices = list(devices)


class Rack(Device):
    def __init__(self, name, chains):
        Device.__init__(self, name, "InstrumentGroupDevice", 1, can_have_chains=True)
        self.chains = list(chains)


class EditorRig(object):
    def __init__(self, editor=True, version="12.4.6"):
        mk = (lambda n, **k: PluginDev(n, editor=editor, **k))
        self.kontakt = mk("Kontakt 8")
        self.serum = mk("Serum 2")
        self.vst2 = mk("Diva", presets=["Init", "Bass Wobble", "Pad Été"], selected=1)
        self.inner = mk("Vital", presets=["A", "B"], selected=0)
        self.song = Song([
            Track("Keys", [self.kontakt]),
            Track("Bass", [Device("Arpeggiator", "MxDeviceMidiEffect", 4), self.serum]),
            Track("Vocals", [Device("EQ Eight", "Eq8", 2)], midi=False),
            Track("Pads", [self.vst2]),
            Track("Rack", [Rack("Instrument Rack", [Chain([Device("Operator", "Operator", 1)]), Chain([self.inner])])]),
        ])
        self.t = 50.0
        self.logs = []
        self.core = EC.EditorCore(song=lambda: self.song, log=self.logs.append, now=lambda: self.t, version=lambda: version,
                                  tag="[Uppwind Link]")

    def ask(self, cmd, **kw):
        req = dict(kw)
        req["cmd"] = cmd
        return self.core.handle(req)


@test
def dev_paths_and_list():
    assert EC.parse_path("2.0") == [2, 0] and EC.parse_path("4.0.1.0") == [4, 0, 1, 0] and EC.parse_path([1, 1]) == [1, 1]
    for bad in ("2", "a.b", "-1.0", "1.0.1", "", None, True, "1.0.0.0.0.0.0.0"):
        assert EC.parse_path(bad) is None, bad
    r = EditorRig()
    rep = r.ask("LIST")
    assert rep["ok"]
    paths = [(d["path"], d["name"], d["plugin"]) for t in rep["tracks"] for d in t["devices"]]
    assert paths == [("0.0", "Kontakt 8", True), ("1.0", "Arpeggiator", False), ("1.1", "Serum 2", True),
                     ("2.0", "EQ Eight", False), ("3.0", "Diva", True), ("4.0", "Instrument Rack", False),
                     ("4.0.0.0", "Operator", False), ("4.0.1.0", "Vital", True)], paths
    one = r.ask("LIST", track=3)
    assert [t["index"] for t in one["tracks"]] == [3]
    assert r.ask("NOPE")["ok"] is False and r.ask("../x")["ok"] is False
    hello = r.ask("HELLO")
    assert hello["live"] == "12.4.6" and hello["plugins"] == 4 and hello["editor_api"] is True


@test
def dev_editor_open_and_close():
    r = EditorRig()
    rep = r.ask("EDITOR_OPEN", path="1.1")
    assert rep["ok"] and rep["before"] is False and rep["after"] is True and rep["selected"] is False, rep
    assert r.serum.is_editor_open is True and r.song.view.selected_track is None
    rep = r.ask("EDITOR_OPEN", path="0.0", select=True)
    assert rep["ok"] and rep["selected"] is True
    assert r.song.view.selected_track is r.song.tracks[0] and r.song.view.selected_device is r.kontakt
    assert r.ask("EDITOR_STATE", path="0.0")["is_editor_open"] is True
    rep = r.ask("EDITOR_CLOSE", path="0.0")
    assert rep["ok"] and rep["after"] is False and r.kontakt.is_editor_open is False
    # dans un Rack
    assert r.ask("EDITOR_OPEN", path="4.0.1.0")["ok"] and r.inner.is_editor_open is True
    # les chemins faux le disent
    for p, word in (("9.0", "no track 9"), ("0.5", "no device 5"), ("4.0.7.0", "no chain 7"), ("x", "device path")):
        rep = r.ask("EDITOR_OPEN", path=p)
        assert rep["ok"] is False and word in rep["error"], (p, rep)
    assert any("EDITOR_OPEN 1.1 'Serum 2' select=False before=False after=True" in m for m in r.logs), r.logs
    assert all(m.startswith("[Uppwind Link] ") for m in r.logs)


@test
def dev_editor_needs_live_12_4_3():
    r = EditorRig(editor=False, version="12.4.2")
    r.song.tracks.append(Track("Old", [OldPluginDev("Massive")]))
    rep = r.ask("EDITOR_OPEN", path="5.0")
    assert rep["ok"] is False and "12.4.3" in rep["error"] and "12.4.2" in rep["error"], rep
    assert r.ask("HELLO")["editor_api"] is False

    class Refuses(PluginDev):
        locked = False

        def __setattr__(self, k, v):
            if k == "is_editor_open" and self.locked:
                raise RuntimeError("Changes cannot be triggered by notifications")
            PluginDev.__setattr__(self, k, v)

    r = EditorRig()
    k = Refuses("Kontakt 8")
    k.locked = True
    r.song.tracks[0].devices[0] = k
    rep = r.ask("EDITOR_OPEN", path="0.0")
    assert rep["ok"] is False and "RuntimeError" in rep["error"], rep


@test
def dev_probe_logs_presets():
    r = EditorRig()
    rep = r.ask("PROBE")
    got = dict((d["name"], (d["preset_count"], d["selected_preset_index"], d["is_editor_open"])) for d in rep["devices"])
    assert got == {"Kontakt 8": (0, -1, False), "Serum 2": (0, -1, False), "Diva": (3, 1, False), "Vital": (2, 0, False)}, got
    diva = [d for d in rep["devices"] if d["name"] == "Diva"][0]
    assert diva["presets"] == ["Init", "Bass Wobble", "Pad Été"] and diva["parameters"] == 9
    line = [m for m in r.logs if "PROBE 3.0 'Diva'" in m]
    assert line and "presets=3 [Init | Bass Wobble | Pad Été]" in line[0] and "selected_preset_index=1" in line[0], r.logs
    assert any("PROBE 0.0 'Kontakt 8'" in m and "presets=0 []" in m and "selected_preset_index=-1" in m for m in r.logs)
    # un seul device, meme s'il n'est pas un plugin ; une propriete absente est notee, pas fatale
    rep = r.ask("PROBE", path="2.0")
    assert rep["ok"] and rep["devices"][0]["preset_count"] is None and "AttributeError" in rep["devices"][0]["presets_error"]
    assert rep["devices"][0]["is_editor_open"] is None
    # beaucoup de presets : 200 dans la reponse, toutes (coupees) dans Log.txt
    r.vst2.presets = ["P%03d" % i for i in range(600)]
    rep = r.ask("PROBE", path="3.0")
    assert rep["devices"][0]["preset_count"] == 600 and len(rep["devices"][0]["presets"]) == 200
    assert "...(cut)" in r.logs[-1]


@test
def dev_preset_select_and_events():
    r = EditorRig()
    rep = r.ask("PRESET_SELECT", path="3.0", index=2)
    assert rep["ok"] and rep["before"] == 1 and rep["after"] == 2 and r.vst2.selected_preset_index == 2
    assert r.ask("PRESET_SELECT", path="3.0", index="x")["ok"] is False
    assert r.ask("PRESET_SELECT", path="2.0", index=0)["ok"] is False          # pas un plugin
    # une fenetre ouverte par le script, puis fermee a la main dans Live : le script le voit
    r.ask("EDITOR_OPEN", path="1.1")
    r.core.tick()
    r.serum.is_editor_open = False
    r.t += 0.6
    r.core.tick()
    evs = r.ask("EVENTS")["events"]
    assert len(evs) == 1 and "changed by itself: True -> False" in evs[0]["text"], evs
    # le device supprime : la veille s'arrete et le dit
    del r.song.tracks[1].devices[1]
    r.serum.__dict__.pop("is_editor_open")
    r.t += 0.6
    r.core.tick()
    assert "is gone" in r.ask("EVENTS")["events"][-1]["text"] and r.core.watch == []


@test
def dev_channel_refuses_strangers():
    seen = []
    ch = ECH.LocalChannel(lambda q: (seen.append(q), {"ok": True})[1], port=0, path=os.devnull)
    ch.token = "a" * 64
    ok = '{"token": "%s", "id": 3, "cmd": "HELLO"}' % ch.token
    assert ch.accept(ok.encode(), ("127.0.0.1", 5000)) == {"ok": True, "id": 3}
    assert ch.accept(ok.encode(), ("192.0.2.1", 5000)) is None
    assert ch.accept(ok.replace("aaaa", "aaab", 1).encode(), ("127.0.0.1", 5000)) is None
    assert ch.accept(b'{"id": 1, "cmd": "HELLO"}', ("127.0.0.1", 5000)) is None
    assert ch.accept(b"not json", ("127.0.0.1", 5000)) is None
    assert ch.accept(b"[1, 2]", ("127.0.0.1", 5000)) is None
    assert ch.accept(b" " * (ECH.MAX_IN + 1), ("127.0.0.1", 5000)) is None
    assert ch.rejected == 6 and len(seen) == 1
    boom = ECH.LocalChannel(lambda q: 1 / 0, port=0, path=os.devnull)
    rep = boom.accept(('{"token": "%s", "id": 9}' % boom.token).encode(), ("127.0.0.1", 1))
    assert rep["ok"] is False and "ZeroDivisionError" in rep["error"] and rep["id"] == 9


@test
def channel_and_cli_over_udp_on_the_real_ports():
    # la coquille (sans Live ni _Framework) ouvre le canal sur la vraie plage de ports et ecrit son fichier d'etat ;
    # l'outil en ligne de commande lui parle par un vrai UDP sur 127.0.0.1, pendant que « Live » appelle update_display
    import importlib
    import tempfile
    import threading
    shell = importlib.import_module("UppwindLink.UppwindLink")
    assert shell._FRAMEWORK is False
    tmp = tempfile.mkdtemp(prefix="uwlinkch-")
    state = os.path.join(tmp, "state.json")
    old = os.environ.get(ECH.ENV_FILE)
    old_port = os.environ.pop(ECH.ENV_PORT, None)       # la vraie plage 47391… ici
    os.environ[ECH.ENV_FILE] = state
    fresh_registry()
    try:
        rig = EditorRig()
        c = LiveCInstance(rig.song)
        s = UppwindLink.create_instance(c)
        assert s.suggest_input_port() == "Uppwind Link" and s.suggest_output_port() == "Uppwind Link Back"
        st = uwlink_dev.read_state(state)
        assert st["port"] == s._uw_chan.port and len(st["token"]) == 64 and st["script"] == "UppwindLink"
        assert 47391 <= st["port"] < 47391 + ECH.PORT_TRIES

        def ask(msg):
            box = {}
            th = threading.Thread(target=lambda: box.setdefault("rep", uwlink_dev.request(msg, state, 3.0)))
            th.start()
            for _ in range(300):
                s.update_display()
                th.join(0.01)
                if not th.is_alive():
                    break
            assert "rep" in box, msg
            return box["rep"]

        assert ask({"cmd": "HELLO"})["live"] == ""            # pas de Live : pas de version
        rep = ask({"cmd": "EDITOR_OPEN", "path": "1.1", "select": True})
        assert rep["ok"] and rep["after"] is True and rig.serum.is_editor_open is True, rep
        rep = ask(uwlink_dev.message_of(uwlink_dev.build(["probe", "3.0"])))
        assert rep["devices"][0]["presets"][2] == "Pad Été"
        # un jeton faux : pas de reponse (l'outil attend, puis le dit)
        bad = dict(st)
        bad["token"] = "0" * 64
        with open(state + ".bad", "w") as f:
            json.dump(bad, f)
        try:
            uwlink_dev.request({"cmd": "HELLO"}, state + ".bad", 0.4)
            assert False, "a wrong token got an answer"
        except TimeoutError:
            pass
        s.update_display()
        assert s._uw_chan.rejected >= 1
        s.disconnect()
        assert not os.path.exists(state)
        assert any("ready" in m for m in c.logs) and any("listening on udp://127.0.0.1:" in m for m in c.logs)
        # les sous-commandes de l'outil -> les messages du script
        mo = lambda *a: uwlink_dev.message_of(uwlink_dev.build(list(a)))   # noqa: E731
        assert mo("open", "2.0", "--select") == {"cmd": "EDITOR_OPEN", "path": "2.0", "select": True}
        assert mo("close", "2.0") == {"cmd": "EDITOR_CLOSE", "path": "2.0"}
        assert mo("preset", "3.0", "4") == {"cmd": "PRESET_SELECT", "path": "3.0", "index": 4}
        assert mo("list", "--track", "1") == {"cmd": "LIST", "track": 1} and mo("probe") == {"cmd": "PROBE"}
    finally:
        if old is None:
            os.environ.pop(ECH.ENV_FILE, None)
        else:
            os.environ[ECH.ENV_FILE] = old
        if old_port is not None:
            os.environ[ECH.ENV_PORT] = old_port
        shutil.rmtree(tmp, ignore_errors=True)


@test
def dev_tool_reads_as_python_37_and_never_sends_midi():
    folder = os.path.join(LINK, "dev")
    for f in sorted(os.listdir(folder)):
        if f.endswith(".py"):
            src = open(os.path.join(folder, f), encoding="utf-8").read()
            ast.parse(src, filename=f, feature_version=(3, 7))
            assert "send_midi" not in src, f                    # l'outil ne parle que le canal local


# -- 24 oct. : UN SEUL Control Surface — le vrai script porte aussi les fenetres des plugins ---------------------


def fresh_registry():
    """Le registre commun des canaux (un attribut de sys, comme dans le Python de Live) remis a zero."""
    setattr(sys, ECH.REGISTRY, {"owner": None})


class LiveCInstance(object):
    """Ce que Live passe a create_instance : le Song, l'envoi MIDI, Log.txt."""

    def __init__(self, song):
        self._song = song
        self.sent, self.logs = [], []

    def song(self):
        return self._song

    def send_midi(self, b):
        self.sent.append(b)

    def log_message(self, m):
        self.logs.append(m)


def over_udp(shells, msg, state, timeout=3.0):
    """uwlink_dev.request dans un fil, pendant que « Live » appelle update_display de chaque script."""
    import threading
    box = {}
    th = threading.Thread(target=lambda: box.setdefault("rep", uwlink_dev.request(msg, state, timeout)))
    th.start()
    for _ in range(400):
        for s in shells:
            s.update_display()
        th.join(0.01)
        if not th.is_alive():
            break
    th.join(timeout + 1)
    assert "rep" in box, msg
    return box["rep"]


@test
def the_state_file_keeps_its_name():
    assert ECH.STATE_NAME == ".uppwind-link-dev.json", "the companion reads this name (old companions too)"


@test
def merged_script_opens_plugin_windows_and_keeps_the_midi_link():
    fresh_registry()
    state = os.environ["UWLINK_DEV_FILE"]
    rig = EditorRig()
    c = LiveCInstance(rig.song)
    s = UppwindLink.create_instance(c)
    try:
        st = uwlink_dev.read_state(state)
        assert st["script"] == "UppwindLink" and st["version"] == "1.3.1" and st["editor"] == EC.EDITOR_VERSION, st
        assert st["link"] == 2, "the companion learns that LINK is served here, and that an empty read may wait"
        assert st["port"] == s._uw_chan.port and len(st["token"]) == 64
        # le compagnon : LIST, EDITOR_OPEN / STATE / CLOSE, EVENTS — par le vrai UDP
        rep = over_udp([s], {"cmd": "LIST"}, state)
        assert [d["path"] for t in rep["tracks"] for d in t["devices"] if d["plugin"]] == ["0.0", "1.1", "3.0", "4.0.1.0"], rep
        rep = over_udp([s], {"cmd": "EDITOR_OPEN", "path": "3.0"}, state)
        assert rep["ok"] and rep["before"] is False and rep["after"] is True and rig.vst2.is_editor_open is True, rep
        assert over_udp([s], {"cmd": "EDITOR_STATE", "path": "3.0"}, state)["is_editor_open"] is True
        assert over_udp([s], {"cmd": "EDITOR_CLOSE", "path": "3.0"}, state)["after"] is False
        assert over_udp([s], {"cmd": "EVENTS"}, state)["ok"] is True
        # le lien MIDI, dans le meme script, inchange
        for chunk in P.encode_message(P.HELLO, 3, {"v": 1}):
            s.receive_midi(tuple(bytearray(chunk)))
        s.update_display()
        out = P.decode_chunks(c.sent)
        assert out["cmd"] == P.HELLO_ACK and out["data"]["daw"] == "Ableton Live", out
        assert s.suggest_input_port() == "Uppwind Link" and s.suggest_output_port() == "Uppwind Link Back"
        assert any(m.startswith("[Uppwind Link] EDITOR_OPEN 3.0 'Diva'") for m in c.logs), c.logs
        assert any("[Uppwind Link] listening on udp://127.0.0.1:" in m for m in c.logs)
    finally:
        s.disconnect()
    assert not os.path.exists(state), "the state file goes with the script"


@test
def merged_script_on_live_before_12_4_3_says_so():
    fresh_registry()
    state = os.environ["UWLINK_DEV_FILE"]
    rig = EditorRig(editor=False, version="12.4.2")
    rig.song.tracks.append(Track("Old", [OldPluginDev("Massive")]))
    s = UppwindLink.create_instance(LiveCInstance(rig.song))
    try:
        rep = over_udp([s], {"cmd": "EDITOR_OPEN", "path": "5.0"}, state)
        assert rep["ok"] is False and "12.4.3" in rep["error"], rep
        rep = over_udp([s], {"cmd": "LIST", "track": 5}, state)
        assert rep["tracks"][0]["devices"][0]["editor"] is False, rep
    finally:
        s.disconnect()


@test
def merged_script_survives_a_channel_that_cannot_open():
    # le canal ne s'ouvre pas (un registre illisible, un port refuse…) : le lien MIDI marche quand meme
    fresh_registry()
    orig = ECH.LocalChannel.open
    ECH.LocalChannel.open = lambda self: (_ for _ in ()).throw(OSError("no socket for you"))
    try:
        c = LiveCInstance(Rig().song)
        s = UppwindLink.create_instance(c)
        assert s._uw_chan is None and any("plugin windows (companion channel) off" in m for m in c.logs), c.logs
        for chunk in P.encode_message(P.HELLO, 3, {"v": 1}):
            s.receive_midi(tuple(bytearray(chunk)))
        s.update_display()
        assert P.decode_chunks(c.sent)["cmd"] == P.HELLO_ACK
        s.disconnect()
    finally:
        ECH.LocalChannel.open = orig


@test
def a_higher_priority_channel_takes_over_and_gives_back():
    # deux copies du canal dans le meme Python (deux scripts choisis dans Live) : la plus prioritaire sert, l'autre
    # attend sans socket (« stands by ») et reprend le canal toute seule quand la premiere s'arrete
    reg = {"owner": None}
    path = os.environ["UWLINK_DEV_FILE"]
    low = ECH.LocalChannel(lambda q: {"ok": True}, port=0, path=path, info={"script": "Low"}, priority=1, registry=reg)
    assert low.open() and low.serving
    high = ECH.LocalChannel(lambda q: {"ok": True}, port=0, path=path, info={"script": "High"}, priority=2, registry=reg)
    assert high.open() and high.serving
    assert not low.serving and low.standby and reg["owner"] is high
    assert uwlink_dev.read_state(path)["script"] == "High"
    high.close()
    for _ in range(ECH.RETAKE_POLLS + 1):
        low.poll()
    assert low.serving and reg["owner"] is low and uwlink_dev.read_state(path)["script"] == "Low"
    low.close()
    assert not os.path.exists(path)


@test
def a_disconnected_channel_never_comes_back():
    reg = {"owner": None}
    a = ECH.LocalChannel(lambda q: {"ok": True}, port=0, path=os.environ["UWLINK_DEV_FILE"], info={"script": "A"}, priority=2, registry=reg)
    b = ECH.LocalChannel(lambda q: {"ok": True}, port=0, path=os.environ["UWLINK_DEV_FILE"], info={"script": "B"}, priority=1, registry=reg)
    assert a.open() and not b.open() and b.standby
    b.close()                                   # le script de dev retire pendant qu'il attendait
    a.close()
    for _ in range(ECH.RETAKE_POLLS * 2):
        b.poll()
    assert not b.serving and reg["owner"] is None
    # deux scripts de meme rang : le premier garde le canal
    c = ECH.LocalChannel(lambda q: {"ok": True}, port=0, path=os.devnull, info={"script": "C"}, priority=1, registry=reg)
    d = ECH.LocalChannel(lambda q: {"ok": True}, port=0, path=os.devnull, info={"script": "D"}, priority=1, registry=reg)
    assert c.open() and not d.open() and reg["owner"] is c
    c.close()
    d.close()


# -- 25 oct. : SANS loopMIDI — les notes par le plugin Uppwind Link Notes, les commandes par le canal du compagnon ---
# Le faux Live d'apres le LOM : le menu « MIDI From » d'une piste MIDI liste les autres pistes MIDI par leur nom ; une
# fois une piste choisie, ses canaux sont « Pre FX », « Post FX », « Post Mixer » et la sortie MIDI de chaque plugin qui
# en a une (« Uppwind Link Notes »). Un plugin VST3 expose ses parametres en valeurs normalisees (0..1), avec le texte
# que le plugin leur donne (str_for_value). Une piste neuve peut n'apparaitre dans les menus des autres qu'un peu plus
# tard (lag) : le script attend sans repondre, puis repond.
from UppwindLink import notes_route as NR  # noqa: E402

PRE_FX, POST_FX, POST_MIX = Routing("Pre FX"), Routing("Post FX"), Routing("Post Mixer")
NO_INPUT = Routing("No Input")


class TagParam(object):
    """« Link tag » : normalise 0..1 (255 pas), le texte est l'entier (comme nice-plug le formate)."""

    def __init__(self, n=0, text=True):
        self.name, self.min, self.max, self.value, self._text = "Link tag", 0.0, 1.0, n / 255.0, text

    def str_for_value(self, v):
        return "%d" % round(v * 255) if self._text else "?"


class NotesDev(Device):
    def __init__(self, tag=0, with_tag=True, text=True, name="Uppwind Link Notes"):
        Device.__init__(self, name, "PluginDevice", 1)
        self.parameters = [Param(1.0, 0.0, 1.0)] + ([TagParam(tag, text)] if with_tag else [])
        self.parameters[0].name = "Device On"


class PSong(Song):
    """Le Song du chemin sans loopMIDI : ses pistes connaissent le set (leurs menus d'entree), les etapes d'annulation."""

    def __init__(self, tracks, rig):
        Song.__init__(self, tracks)
        self.rig = rig
        self.undo = []
        self.deleted = []
        for t in self.tracks:
            t._song = self

    def create_midi_track(self, index):
        t = PTrack("%d-MIDI" % (index + 1))
        t._song = self
        t.visible_at = self.rig.t + self.rig.lag
        self.tracks.insert(index, t)

    def delete_track(self, index):
        self.deleted.append(self.tracks[index].name)
        del self.tracks[index]

    def begin_undo_step(self):
        self.undo.append("begin")

    def end_undo_step(self):
        self.undo.append("end")


class PTrack(Track):
    visible_at = 0.0

    @property
    def available_input_routing_types(self):
        song = getattr(self, "_song", None)
        others = []
        if song is not None:
            others = [Routing(t.name) for t in song.tracks
                      if t is not self and t.has_midi_input and song.rig.t >= getattr(t, "visible_at", 0.0)]
        return list(self._types) + [NO_INPUT] + others

    @property
    def available_input_routing_channels(self):
        rt = self.input_routing_type
        song = getattr(self, "_song", None)
        src = None
        if song is not None:
            for t in song.tracks:
                if t is not self and t.name == getattr(rt, "display_name", None):
                    src = t
        if src is None:
            return [ALL_CH] + CHANNELS
        return [PRE_FX, POST_FX, POST_MIX] + [Routing(d.name) for d in src.devices if isinstance(d, NotesDev)]


class PBrowser(Browser):
    def __init__(self, song, notes=True, with_tag=True):
        Browser.__init__(self, song)
        if notes:
            vst3 = self.plugins.children[0]
            vst3.children.append(Item("Uppwind", [Item("Uppwind Link Return", device=True), Item("Uppwind Link Notes", device=True)]))
        self.with_tag = with_tag

    def load_item(self, item):
        t = self.song.view.selected_track
        if item.name == "Uppwind Link Notes":
            t.devices.append(NotesDev(with_tag=self.with_tag))
            self.loaded.append(item.name)
            return
        Browser.load_item(self, item)


class PRig(Rig):
    def __init__(self, notes=True, with_tag=True, lag=0.0, extra=()):
        self.t = 100.0
        self.lag = lag
        tracks = [PTrack("Drums", [Device("Drum Rack", "DrumGroupDevice", 1, True)]),
                  PTrack("Lead", [Device("Serum")], color=0x00FF88),
                  PTrack("Keys", [Device("Electric", "LoungeLizard", 1)])] + list(extra)
        self.song = PSong(tracks, self)
        self.browser = PBrowser(self.song, notes=notes, with_tag=with_tag)
        self.sent = []
        self.logs = []
        self.core = C.LinkCore(song=lambda: self.song, send=self.sent.append, log=self.logs.append,
                               now=lambda: self.t, browser=lambda: self.browser, version=lambda: "12.4.3")
        self.rx = P.Reassembler()
        self.req = 0

    def track(self, name):
        return [t for t in self.song.tracks if t.name == name][0]


def plugin_bind(r, slot, i):
    return r.ask(P.BIND, {"slot": slot, "id": i, "via": "plugin"})


@test
def plugin_bind_makes_a_helper_track_and_never_touches_the_link_port():
    r = PRig()
    r.ask(P.HELLO, {"v": 1})
    lead = r.track("Lead")
    r.song.view.selected_track = r.track("Drums")
    cmd, _req, d = plugin_bind(r, 1, 1)
    assert cmd == P.BIND_ACK and d == {"slot": 1, "id": 1, "via": "plugin", "notes": 1}, (cmd, d)
    helper = r.song.tracks[-1]
    assert helper.name == u"Uppwind Link · Lead" and isinstance(helper.devices[0], NotesDev), helper.name
    assert NR.read_tag(helper.devices[0]) == 1
    # la piste visee : MIDI From = la piste d'aide / Uppwind Link Notes, Monitor In — jamais « Uppwind Link / Ch. 1 »
    assert lead.input_routing_type.display_name == helper.name
    assert lead.input_routing_channel.display_name == "Uppwind Link Notes"
    assert lead.current_monitoring_state == C.MONITOR_IN
    assert r.browser.loaded == ["Uppwind Link Notes"]
    assert r.song.undo == ["begin", "end"], "one undo step for the helper track"
    assert r.song.view.selected_track is r.track("Drums"), "Live's selection is given back"
    # le studio ne voit pas la piste d'aide ; ITEM.slots suit la piste visee
    _c, _q, lst = r.ask(P.LIST, {})
    names = [it["name"] for it in lst["items"]]
    assert helper.name not in names and names == ["Drums", "Lead", "Keys"], names
    assert [it for it in lst["items"] if it["name"] == "Lead"][0]["slots"] == [1]
    # pas un instrument jouable non plus
    err = r.ask(P.BIND, {"slot": 2, "id": 3, "via": "plugin"})
    assert err[0] == P.ERROR and err[2]["code"] == P.ERR_NOT_FOUND, err
    # une deuxieme piste : sa piste d'aide, un autre numero
    cmd, _q, d = plugin_bind(r, 2, 2)
    assert d["notes"] == 2 and r.song.tracks[-1].name == u"Uppwind Link · Keys", d
    assert r.track("Keys").input_routing_type.display_name == u"Uppwind Link · Keys"
    # un SET touche la piste visee, jamais la piste d'aide
    r.send(P.SET, {"id": 1, "mute": True})
    assert lead.mute is True and helper.mute is False


@test
def plugin_unbind_restores_the_input_and_removes_the_helper_later():
    r = PRig()
    r.ask(P.HELLO, {"v": 1})
    lead = r.track("Lead")
    plugin_bind(r, 1, 1)
    helper = r.song.tracks[-1]
    r.send(P.UNBIND, {"slot": 1})
    assert lead.input_routing_type is ALL_INS and lead.input_routing_channel is ALL_CH and lead.current_monitoring_state == 1
    assert helper in r.song.tracks, "not at once: a studio that reloads takes it back"
    r.advance(C.HELPER_GRACE_S - 1)
    assert helper in r.song.tracks
    # repris pendant le delai : la meme piste, le meme numero, rien de cree
    cmd, _q, d = plugin_bind(r, 3, 1)
    assert d["notes"] == 1 and r.browser.loaded == ["Uppwind Link Notes"] and r.song.tracks[-1] is helper
    r.advance(C.HELPER_GRACE_S + 1)
    assert helper in r.song.tracks, "bound again: it stays"
    r.send(P.UNBIND, {"slot": 3})
    r.advance(C.HELPER_GRACE_S + 1)
    assert helper not in r.song.tracks and r.song.deleted == [helper.name]
    assert r.song.undo[-2:] == ["begin", "end"], "the removal is one undo step"
    # un nouveau HELLO (le studio recharge) libere aussi ; la fermeture de Live ne retire rien
    plugin_bind(r, 1, 1)
    h2 = r.song.tracks[-1]
    r.ask(P.HELLO, {"v": 1})
    assert lead.input_routing_type is ALL_INS and h2 in r.song.tracks
    plugin_bind(r, 1, 1)
    r.core.shutdown()
    r.advance(C.HELPER_GRACE_S + 1)
    assert h2 in r.song.tracks, "a set that closes is not edited"


@test
def plugin_bind_reuses_a_helper_track_made_by_hand():
    # un montage fait a la main : une piste « Link Notes » avec le plugin, Diva en MIDI From dessus
    notes = NotesDev(tag=0)
    r = PRig(extra=[PTrack("Link Notes", [notes]), PTrack("Diva", [Device("Diva")])])
    diva = r.track("Diva")
    diva.input_routing_type = Routing("Link Notes")
    diva.input_routing_channel = Routing("Uppwind Link Notes")
    diva.current_monitoring_state = C.MONITOR_IN
    r.ask(P.HELLO, {"v": 1})
    _c, _q, lst = r.ask(P.LIST, {})
    assert "Link Notes" not in [it["name"] for it in lst["items"]], "a track that holds a Notes is a helper"
    n = len(r.song.tracks)
    cmd, _q, d = plugin_bind(r, 1, 4)
    assert cmd == P.BIND_ACK and d["notes"] == 1 and len(r.song.tracks) == n and r.browser.loaded == []
    assert NR.read_tag(notes) == 1, "a Notes without a number gets one"
    r.send(P.UNBIND, {"slot": 1})
    r.advance(C.HELPER_GRACE_S + 1)
    assert r.track("Link Notes") and r.song.deleted == [], "never removes a helper track it did not make"
    assert diva.input_routing_type.display_name == "Link Notes", "it was already so: nothing to put back"


@test
def plugin_bind_without_the_plugin_says_how_to_install_it():
    r = PRig(notes=False)
    r.ask(P.HELLO, {"v": 1})
    cmd, _q, d = plugin_bind(r, 1, 1)
    assert cmd == P.ERROR and d["code"] == P.ERR_UNSUPPORTED and "Rescan" in d["msg"], d
    assert len(r.song.tracks) == 3 and r.song.undo == [] and r.track("Lead").input_routing_type is ALL_INS
    # la route par le port, elle, marche toujours (le secours loopMIDI)
    cmd, _q, d = r.ask(P.BIND, {"slot": 1, "id": 1})
    assert cmd == P.BIND_ACK and d["via"] == "port" and r.track("Lead").input_routing_type is LINK_IN, d


@test
def plugin_bind_waits_for_live_then_answers():
    r = PRig(lag=0.35)
    r.ask(P.HELLO, {"v": 1})
    req = r.send(P.BIND, {"slot": 1, "id": 1, "via": "plugin"})
    assert [x for x in r.replies() if x[1] == req] == [], "Live does not list the new track yet: no answer"
    r.advance(0.5)
    rep = [x for x in r.replies() if x[1] == req]
    assert len(rep) == 1 and rep[0][0] == P.BIND_ACK and rep[0][2]["notes"] == 1, rep
    assert r.track("Lead").input_routing_channel.display_name == "Uppwind Link Notes"
    # jamais pret : l'erreur, apres le delai
    r = PRig(lag=999)
    r.ask(P.HELLO, {"v": 1})
    req = r.send(P.BIND, {"slot": 1, "id": 1, "via": "plugin"})
    r.advance(C.PENDING_S + 0.5)
    rep = [x for x in r.replies() if x[1] == req]
    assert len(rep) == 1 and rep[0][0] == P.ERROR and "in time" in rep[0][2]["msg"], rep
    assert r.track("Lead").input_routing_type is ALL_INS


@test
def plugin_bind_with_an_old_notes_plugin_says_so():
    r = PRig(with_tag=False)
    r.ask(P.HELLO, {"v": 1})
    req = r.send(P.BIND, {"slot": 1, "id": 1, "via": "plugin"})
    r.advance(C.PENDING_S + 0.5)
    rep = [x for x in r.replies() if x[1] == req]
    assert rep[0][0] == P.ERROR and "Link tag" in rep[0][2]["msg"], rep


@test
def duplicated_notes_get_a_new_number():
    a, b = NotesDev(tag=7), NotesDev(tag=7)
    r = PRig(extra=[PTrack(u"Uppwind Link · Lead", [a]), PTrack("Copy", [b])])
    r.ask(P.HELLO, {"v": 1})
    cmd, _q, d = plugin_bind(r, 1, 1)
    assert cmd == P.BIND_ACK and d["notes"] not in (0, 7), d
    assert NR.read_tag(a) == d["notes"] and NR.read_tag(b) == 7


class LateBrowser(PBrowser):
    """Live charge le Notes un peu apres load_item (un VST3 qui s'instancie) : il apparait au bout de `delay`."""

    def __init__(self, song, delay):
        PBrowser.__init__(self, song)
        self.delay = delay
        self.late = []

    def load_item(self, item):
        self.loaded.append(item.name)
        self.late.append((self.song.rig.t + self.delay, self.song.view.selected_track))

    def settle(self):
        for at, t in list(self.late):
            if self.song.rig.t >= at:
                t.devices.append(NotesDev())
                self.late.remove((at, t))


@test
def a_notes_plugin_that_loads_late_makes_one_helper_track_only():
    r = PRig()
    r.browser = LateBrowser(r.song, 0.6)
    r.ask(P.HELLO, {"v": 1})
    n = len(r.song.tracks)
    req = r.send(P.BIND, {"slot": 1, "id": 1, "via": "plugin"})
    for _ in range(10):
        r.advance(0.1)
        r.browser.settle()
    r.advance(0.2)
    rep = [x for x in r.replies() if x[1] == req]
    assert len(rep) == 1 and rep[0][0] == P.BIND_ACK, rep
    assert len(r.song.tracks) == n + 1 and r.browser.loaded == ["Uppwind Link Notes"], [t.name for t in r.song.tracks]


@test
def the_sweep_never_removes_a_helper_track_the_user_took_over():
    for change in ("rename", "device", "clip"):
        r = PRig()
        r.ask(P.HELLO, {"v": 1})
        plugin_bind(r, 1, 1)
        helper = r.song.tracks[-1]
        r.send(P.UNBIND, {"slot": 1})
        if change == "rename":
            helper.name = "My bassline"
        elif change == "device":
            helper.devices.append(Device("Diva"))
        else:
            helper.clip_slots = [type("Slot", (), {"has_clip": True})()]
        r.advance(C.HELPER_GRACE_S + 1)
        assert helper in r.song.tracks and r.song.deleted == [], change
    # telle que le script l'a faite : elle part
    r = PRig()
    r.ask(P.HELLO, {"v": 1})
    plugin_bind(r, 1, 1)
    helper = r.song.tracks[-1]
    r.send(P.UNBIND, {"slot": 1})
    r.advance(C.HELPER_GRACE_S + 1)
    assert helper not in r.song.tracks


@test
def link_tags_read_back_on_any_scale():
    d = NotesDev(tag=42)
    assert NR.read_tag(d) == 42
    d.parameters[1]._text = False                          # un texte illisible : l'echelle normalisee
    assert NR.read_tag(d) == 42
    p = d.parameters[1]
    p.min, p.max, p.value = 0.0, 255.0, 9.0                # une echelle en pas entiers
    assert NR.read_tag(d) == 9 and NR.write_tag(d, 200) and p.value == 200.0
    assert NR.read_tag(NotesDev(with_tag=False)) is None
    assert NR.free_tag([PTrack("x", [NotesDev(tag=1)]), PTrack("y", [NotesDev(tag=2)])]) == 3
    assert NR._names_match("3-Lead", "Lead") and NR._names_match("Lead", "lead") and not NR._names_match("XLead", "Lead")
    assert NR.is_notes_device(NotesDev(name="Renamed by hand")), "found by its Link tag parameter"


def hexes(cmd, req, data):
    return ["".join("%02x" % x for x in bytearray(c)) for c in P.encode_message(cmd, req, data)]


def link_replies(rep):
    rx = P.Reassembler()
    out = []
    for h in rep["out"]:
        m = rx.push(tuple(bytearray.fromhex(h)), 0)
        if m["type"] == "message":
            out.append((m["cmd"], m["req"], m["data"]))
    return out


@test
def the_companion_transport_carries_the_same_messages():
    r = PRig()
    rep = r.core.handle_link({"cmd": "LINK", "in": hexes(P.HELLO, 5, {"v": 1, "caps": ["tempo"]})})
    got = link_replies(rep)
    assert rep["ok"] and got[0][0] == P.HELLO_ACK and got[0][1] == 5 and "notes" in got[0][2]["caps"], rep
    assert r.core.transport == "udp" and r.sent == [], "nothing goes out over MIDI"
    # BIND par le plugin, LIST, en un aller-retour chacun
    got = link_replies(r.core.handle_link({"cmd": "LINK", "in": hexes(P.BIND, 6, {"slot": 1, "id": 1, "via": "plugin"})}))
    assert got == [(P.BIND_ACK, 6, {"slot": 1, "id": 1, "via": "plugin", "notes": 1})], got
    # ce que le script pousse (CHANGED) attend le prochain releve
    r.song.tracks[0].name = "Drums 2"
    r.advance(1.2)
    assert r.sent == []
    got = link_replies(r.core.handle_link({"cmd": "LINK", "in": []}))
    assert [g[0] for g in got] == [P.CHANGED], got
    # le tempo de Live, change a la main : par le compagnon aussi
    r.song.tempo = 98.5
    got = link_replies(r.core.handle_link({"cmd": "LINK"}))
    assert got == [(P.TEMPO_CHANGED, 0, {"bpm": 98.5})], got
    # un morceau illisible ou etranger est ignore, compte
    rep = r.core.handle_link({"cmd": "LINK", "in": ["zz", "90407f"]})
    assert rep["ok"] and rep["ignored"] == 2
    assert r.core.handle_link({"cmd": "LINK", "in": "f0"})["ok"] is False
    # beaucoup de reponses : en plusieurs releves (« more »)
    big = P.encode_message(P.ERROR, 1, {"code": "x", "msg": "y" * 190})[0]
    r.core.udp_out = [big] * 300
    rep = r.core.handle_link({"cmd": "LINK"})
    assert rep["more"] is True and sum(len(h) for h in rep["out"]) <= C.LINK_OUT_BUDGET + 500
    # un HELLO par MIDI reprend la session : le dernier gagne
    for chunk in P.encode_message(P.HELLO, 9, {"v": 1}):
        r.core.receive(tuple(bytearray(chunk)))
    assert r.core.transport == "midi" and r.core.udp_out == []
    assert P.decode_chunks(r.sent)["cmd"] == P.HELLO_ACK
    assert r.track("Lead").input_routing_type is ALL_INS, "the new session unbinds"


@test
def routes_say_where_each_track_takes_its_notes():
    r = PRig()
    r.ask(P.HELLO, {"v": 1})
    plugin_bind(r, 1, 1)
    rep = r.core.routes()
    lead = [t for t in rep["tracks"] if t["name"] == "Lead"][0]
    helper = [t for t in rep["tracks"] if t["name"].startswith("Uppwind Link")][0]
    assert lead["from"] == helper["name"] and lead["fromNotes"] == 1 and lead["channel"] == "Uppwind Link Notes", lead
    assert helper["notes"] == 1 and rep["notesInBrowser"] is True


@test
def the_shell_routes_link_and_routes_to_the_link_core():
    shell = __import__("UppwindLink.UppwindLink", fromlist=["channel_handler"])
    r = PRig()
    calls = []

    class Ed(object):
        def handle(self, q):
            calls.append(q["cmd"])
            return {"ok": True, "editor": True}

    h = shell.channel_handler(r.core, Ed())
    assert h({"cmd": "LINK", "in": hexes(P.PING, 4, {"t": 1})})["out"]
    assert h({"cmd": "routes"})["tracks"]
    assert h({"cmd": "EDITOR_OPEN", "path": "1.0"})["editor"] and calls == ["EDITOR_OPEN"]


# -- 26 oct. : la latence des commandes (le releve qui attend, la lecture rapide) ; les pistes restees sur le port ---
def link_req(ch, **kw):
    q = {"token": ch.token, "id": kw.pop("id", 1), "cmd": "LINK"}
    q.update(kw)
    return json.dumps(q).encode("utf-8")


@test
def an_empty_read_waits_in_the_channel_until_the_script_has_something():
    import socket
    r = PRig()
    clock = [0.0]
    ch = ECH.LocalChannel(lambda q: r.core.handle_link(q), port=0, path=os.path.join(TMP, "park.json"),
                          park=r.core.link_hold, now=lambda: clock[0], registry={"owner": None})
    assert ch.open()
    cli = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    cli.bind(("127.0.0.1", 0))
    cli.settimeout(0.3)

    def answers():
        got = []
        cli.settimeout(0.05)
        while True:
            try:
                got.append(json.loads(cli.recv(65536).decode("utf-8")))
            except socket.timeout:
                return got

    try:
        dest = ("127.0.0.1", ch.port)
        # le releve vide avec « wait » : garde, pas de reponse
        cli.sendto(link_req(ch, id=11, wait=400), dest)
        import time as _t
        _t.sleep(0.05)
        ch.poll()
        assert answers() == [], "nothing to say: the read waits"
        # une commande arrive pendant ce temps : repondue tout de suite (PONG dans sa reponse), le releve reste garde
        cli.sendto(link_req(ch, id=12, **{"in": hexes(P.HELLO, 3, {"v": 1})}), dest)
        _t.sleep(0.05)
        ch.poll()
        got = answers()
        assert [g["id"] for g in got] == [12] and link_replies(got[0])[0][0] == P.HELLO_ACK, got
        # le script a quelque chose a pousser (CHANGED) : le releve garde repart avec, a la lecture suivante
        r.song.tracks[0].name = "Drums 2"
        r.advance(1.2)
        ch.poll()
        got = answers()
        assert [g["id"] for g in got] == [11] and [m[0] for m in link_replies(got[0])] == [P.CHANGED], got
        # rien a dire : il repart vide a son heure (le compagnon sait que Live est la)
        cli.sendto(link_req(ch, id=13, wait=400), dest)
        _t.sleep(0.05)
        ch.poll()
        assert answers() == []
        clock[0] += 0.41
        ch.poll()
        got = answers()
        assert [g["id"] for g in got] == [13] and got[0]["out"] == [], got
        # sans « wait » (un compagnon d'avant) : la reponse tout de suite, comme avant ; jamais plus de LINK_WAIT_MAX_S
        cli.sendto(link_req(ch, id=14), dest)
        _t.sleep(0.05)
        ch.poll()
        assert [g["id"] for g in answers()] == [14]
        assert r.core.link_hold({"cmd": "LINK", "wait": 60000}) == C.LINK_WAIT_MAX_S
        assert r.core.link_hold({"cmd": "LINK", "in": ["f0"], "wait": 400}) == 0
        assert r.core.link_hold({"cmd": "EVENTS", "wait": 400}) == 0
    finally:
        cli.close()
        ch.close()


class FakeTimer(object):
    made = []

    def __init__(self, callback=None, interval=None, repeat=False):
        self.callback, self.interval, self.repeat, self.running = callback, interval, repeat, False
        FakeTimer.made.append(self)

    def start(self):
        self.running = True

    def stop(self):
        self.running = False


class FakeLiveModule(object):
    class Base(object):
        Timer = FakeTimer


@test
def a_live_timer_reads_the_channel_between_two_update_displays():
    import threading
    shell = __import__("UppwindLink.UppwindLink", fromlist=["Live"])
    fresh_registry()
    state = os.environ["UWLINK_DEV_FILE"]
    saved_live = shell.Live
    shell.Live = FakeLiveModule
    FakeTimer.made[:] = []
    rig = EditorRig()
    c = LiveCInstance(rig.song)
    try:
        s = UppwindLink.create_instance(c)
        t = FakeTimer.made[-1]
        assert t.running and t.interval == shell.FAST_POLL_MS and t.repeat is True, vars(t)
        # une commande par le vrai UDP : repondue par le minuteur seul, sans un seul update_display
        box = {}
        th = threading.Thread(target=lambda: box.setdefault("rep", uwlink_dev.request({"cmd": "EVENTS"}, state, 2.0)))
        th.start()
        for _ in range(200):
            t.callback()
            th.join(0.005)
            if not th.is_alive():
                break
        assert box.get("rep", {}).get("ok") is True, box
        # Live refuse un changement depuis le minuteur : retour a update_display (la commande a eu son ERROR)
        s._uw_core.last_failure = "RuntimeError: Changes cannot be triggered by notifications. You will need to defer your response."
        t.callback()
        assert not t.running and s._uw_fast is None
        assert any("fast channel reads off" in m for m in c.logs), c.logs
        assert over_udp([s], {"cmd": "EVENTS"}, state)["ok"] is True, "update_display still serves"
        s.disconnect()
        # pas de Timer dans ce Live : update_display seul, et il le dit
        class NoTimer(object):
            class Base(object):
                pass
        shell.Live = NoTimer
        fresh_registry()
        c2 = LiveCInstance(rig.song)
        s2 = UppwindLink.create_instance(c2)
        assert s2._uw_fast is None and any("no Live.Base.Timer" in m for m in c2.logs)
        s2.disconnect()
        # disconnect arrete le minuteur
        shell.Live = FakeLiveModule
        fresh_registry()
        s3 = UppwindLink.create_instance(LiveCInstance(rig.song))
        t3 = FakeTimer.made[-1]
        s3.disconnect()
        assert not t3.running
        t3.callback()                                    # un dernier appel de Live apres : rien
    finally:
        shell.Live = saved_live


def on_port(track, ch=3):
    track.input_routing_type = LINK_IN
    track.input_routing_channel = CHANNELS[ch - 1]
    track.current_monitoring_state = C.MONITOR_IN


@test
def a_companion_session_takes_tracks_left_on_the_loopmidi_port_back():
    # un set d'une session loopMIDI d'avant (ou un script recharge) : « Uppwind Link / Ch. 3 », le script n'en sait rien
    r = PRig()
    lead, keys = r.track("Lead"), r.track("Keys")
    on_port(lead, 3)
    on_port(keys, 4)
    rep = r.core.handle_link({"cmd": "LINK", "in": hexes(P.HELLO, 5, {"v": 1, "caps": ["tempo"]})})
    assert link_replies(rep)[0][0] == P.HELLO_ACK
    for t in (lead, keys):
        assert t.input_routing_type is ALL_INS and t.input_routing_channel is ALL_CH and t.current_monitoring_state == C.MONITOR_AUTO, t.name
    assert sum("moved '" in m for m in r.logs) == 2, r.logs
    # puis BIND par le plugin : la piste d'aide ; UNBIND rend « All Ins », jamais le port
    got = link_replies(r.core.handle_link({"cmd": "LINK", "in": hexes(P.BIND, 6, {"slot": 1, "id": 1, "via": "plugin"})}))
    assert got[0][0] == P.BIND_ACK and lead.input_routing_type.display_name == u"Uppwind Link · Lead", got
    r.core.handle_link({"cmd": "LINK", "in": hexes(P.UNBIND, 7, {"slot": 1})})
    assert lead.input_routing_type is ALL_INS and lead.current_monitoring_state == C.MONITOR_AUTO


@test
def switching_from_loopmidi_in_the_same_session_gives_the_real_input_back():
    r = PRig()
    lead = r.track("Lead")
    lead.input_routing_type = KEYS                    # l'entree choisie a la main avant le premier BIND
    lead.current_monitoring_state = 2
    r.ask(P.HELLO, {"v": 1})
    cmd, _q, d = r.ask(P.BIND, {"slot": 2, "id": 1})
    assert d["via"] == "port" and lead.input_routing_type is LINK_IN
    # le studio passe au compagnon : HELLO par le canal (sans UNBIND avant : un studio qui s'est recharge)
    r.core.handle_link({"cmd": "LINK", "in": hexes(P.HELLO, 5, {"v": 1})})
    assert lead.input_routing_type is KEYS and lead.current_monitoring_state == 2, "its own input, not All Ins"
    got = link_replies(r.core.handle_link({"cmd": "LINK", "in": hexes(P.BIND, 6, {"slot": 2, "id": 1, "via": "plugin"})}))
    assert got[0][0] == P.BIND_ACK
    r.core.handle_link({"cmd": "LINK", "in": hexes(P.UNBIND, 7, {"slot": 2})})
    assert lead.input_routing_type is KEYS and lead.current_monitoring_state == 2


@test
def a_plugin_bind_never_saves_the_loopmidi_port_as_the_input_to_give_back():
    # la session tourne deja (HELLO par MIDI) quand une piste est trouvee sur le port, sans que le script le sache
    r = PRig()
    r.ask(P.HELLO, {"v": 1})
    lead = r.track("Lead")
    on_port(lead, 1)
    cmd, _q, d = plugin_bind(r, 1, 1)
    assert cmd == P.BIND_ACK and lead.input_routing_type.display_name == u"Uppwind Link · Lead", d
    r.send(P.UNBIND, {"slot": 1})
    assert lead.input_routing_type is ALL_INS and lead.input_routing_channel is ALL_CH
    # le port s'appelle peut-etre autrement (« UppwindLink In ») ; une piste d'aide n'est jamais le port
    assert C.LinkCore._is_link_port("UppwindLink In") and C.LinkCore._is_link_port("Uppwind Link")
    for n in (u"Uppwind Link · Lead", "Uppwind Link Back", "Uppwind Link Notes", "Uppwind Link Return", "All Ins"):
        assert not C.LinkCore._is_link_port(n), n


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
    print("ableton script (fake Live), python %s: %d/%d passed" % (sys.version.split()[0], len(TESTS) - failed, len(TESTS)))
    shutil.rmtree(TMP, ignore_errors=True)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
