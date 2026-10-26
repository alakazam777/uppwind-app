# -*- coding: utf-8 -*-
# SPDX-License-Identifier: MIT
# Copyright (C) 2026 Uppwind
"""Uppwind Link -- la logique du script Ableton Live, sans Live.

LinkCore ne connait Live qu'a travers les objets qu'on lui passe (song, browser) : le Remote Script
(UppwindLink.py) lui donne les vrais, le test (tests/test_ableton.py) des faux. Live Object Model
utilise : Song.tracks / create_midi_track / delete_track / view.selected_track ; Track.name, color,
has_midi_input, devices, mute, mixer_device.volume / panning (DeviceParameter : value, min, max,
str_for_value), available_input_routing_types / input_routing_type, available_input_routing_channels
/ input_routing_channel, current_monitoring_state (0 = In, 1 = Auto, 2 = Off) ;
Device.name, class_name, type (1 = instrument) ; Browser.instruments, plugins, load_item ;
BrowserItem.name, children, is_device, is_loadable, uri.

Dans Live, les NOTES ne passent pas par le script : BIND regle l'entree MIDI de la piste sur le port
« Uppwind Link », canal n, et son monitoring sur In -- Live joue lui-meme (timing natif, pedale,
pitch bend et CC compris). Le script ne fait que la liste, les liens, le mixage, les plugins.

24 oct. : le tempo (cap 'tempo'). TEMPO -> Song.tempo ; un changement de Song.tempo dans Live (le
rappel add_tempo_listener, et tick() qui relit le tempo toutes les ~100 ms) -> TEMPO_CHANGED, au plus
un toutes les 50 ms, seulement a un studio qui a dit 'tempo' dans son HELLO. Le tempo pose par le
studio n'est jamais renvoye (TEMPO_HOLD_MS). HELLO_ACK.bpm : le tempo de Live.

25 oct. (v1.3.0, sans loopMIDI) : DEUX TRANSPORTS pour les memes messages. Le SysEx sur loopMIDI (receive, puis
send_midi), et le canal local du compagnon (handle_link : la commande LINK de editor_channel, les memes morceaux SysEx
en hexadecimal ; ce que le script envoie attend dans une boite que le compagnon releve). Une session (HELLO) vit sur
le transport de son HELLO : les reponses repartent par le transport de la requete, ce que le script pousse (CHANGED,
TEMPO_CHANGED) par celui de la session. Le dernier HELLO gagne, d'ou qu'il vienne.
BIND.via = 'plugin' (cap 'notes') : la piste visee prend ses notes d'une piste d'aide qui porte « Uppwind Link
Notes » (notes_route.py), au lieu du port « Uppwind Link » ; la piste d'aide est creee si besoin (une etape
d'annulation), retiree apres UNBIND (HELPER_GRACE_S plus tard : un studio qui se recharge la reprend) seulement si le
script l'a creee. BIND_ACK.notes : le numero du Notes, que le studio donne au compagnon.
"""

import math

from . import notes_route as NR
from . import uwlink_protocol as P

DAW_NAME = "Ableton Live"
CAPS = ["list", "bind", "set", "plugins", "add", "remove", "audition", "tempo", "notes"]
PENDING_S = 5.0             # s : un BIND par le plugin attend au plus ca que Live offre la piste d'aide et son Notes
HELPER_GRACE_S = 15.0       # s : une piste d'aide liberee reste ce temps (un studio qui se recharge la reprend)
UDP_OUT_MAX = 4096          # morceaux en attente pour le compagnon (au-dela : il ne releve plus, on vide)
LINK_IN_MAX = 32            # morceaux par requete LINK
LINK_OUT_BUDGET = 48000     # caracteres hexadecimaux par reponse LINK (sous MAX_OUT du canal)
LIVE_TEMPO_MIN = 20.0       # Song.tempo : 20..999 BPM
LIVE_TEMPO_MAX = 999.0
LIST_BUDGET = 20000
POLL_EVERY = 1.0            # s : la liste des pistes a-t-elle change ?
OUT_PER_TICK = 16           # morceaux SysEx par appel de tick (update_display : toutes les 100 ms)
MONITOR_IN = 0
MONITOR_AUTO = 1            # (26 oct.) le monitoring d'une piste MIDI neuve dans Live
FALLBACK_INPUT = "all ins"  # (26 oct.) l'entree d'une piste MIDI neuve (« All Ins / All Channels »)
LINK_WAIT_MAX_S = 1.0       # (26 oct.) un releve vide du compagnon (« wait ») attend au plus ca dans le canal
INSTRUMENT = 1             # Live.Device.DeviceType.instrument
PLUGIN_CLASSES = ("PluginDevice", "AuPluginDevice", "Vst3PluginDevice", "PluginDevice3")
PLUGIN_FOLDERS = ("vst", "vst2", "vst3", "audio units", "au", "plug-ins", "plugins", "clap")
MAX_PLUGINS = 4000
ROUTING_HINT = ("Live does not offer the 'Uppwind Link' MIDI input on this track: in Preferences > "
                "Link, Tempo & MIDI, switch 'Track' on for the Uppwind Link input port.")


def _key(obj):
    return getattr(obj, "_live_ptr", None) or id(obj)


def _int(v, default=0):
    try:
        return int(v)
    except Exception:
        return default


def _db_of(text):
    """'-6.0 dB' -> -6.0 ; '-inf dB' -> -inf ; illisible -> None."""
    s = str(text).strip().lower().replace("db", "").strip()
    if s in ("-inf", "-∞", "-infinity"):
        return -math.inf
    try:
        return float(s)
    except ValueError:
        return None


def gain_to_live_volume(param, gain):
    """Le gain lineaire du studio -> la valeur du fader de Live (0.85 = 0 dB, 1.0 = +6 dB).

    Cherche la valeur dont str_for_value affiche le bon nombre de dB (la courbe exacte de Live,
    sans la deviner) ; a defaut, une approximation lineaire en dB autour de 0.85."""
    lo = float(getattr(param, "min", 0.0))
    hi = float(getattr(param, "max", 1.0))
    if gain <= 0:
        return lo
    want = 20.0 * math.log10(min(1.0, gain))
    fmt = getattr(param, "str_for_value", None)
    if fmt is not None:
        try:
            a, b = lo, hi
            ok = _db_of(fmt(b)) is not None
            for _ in range(40):
                if not ok:
                    break
                m = (a + b) / 2.0
                d = _db_of(fmt(m))
                if d is None:
                    ok = False
                    break
                if d < want:
                    a = m
                else:
                    b = m
            if ok:
                return (a + b) / 2.0
        except Exception:
            pass
    return max(lo, min(hi, 0.85 + want / 40.0)) if want > -34 else lo


class LinkCore(object):
    def __init__(self, song, send, log=None, now=None, browser=None, version=None):
        self._song = song            # () -> Song
        self._send_raw = send        # (tuple d'octets) -> None
        self._log = log or (lambda m: None)
        self._now = now or (lambda: 0.0)
        self._browser = browser or (lambda: None)
        self._version = version or (lambda: "")
        self.rx = P.Reassembler()
        self.out = []
        self.session = False
        self.binds = {}              # slot -> cle de la piste
        self.waiting = {}            # slot deplace par un autre BIND sur sa piste -> cle (il la reprend ensuite)
        self.saved = {}              # cle -> {"type", "channel", "monitor"} (entree d'avant)
        self.mix = {}                # cle -> {"mute", "solo", ...}
        self.orig = {}               # cle -> (volume, pan, mute)
        self.added = set()           # cles des pistes creees par ADD (les seules que REMOVE supprime)
        self.plugin_cache = None
        self.last_poll = 0.0
        self.list_sig = None
        # le tempo : ce que le studio sait faire (HELLO.caps) ; le tempo de Live tel que le studio le
        # sait (pose par lui, ou deja dit) ; le dernier TEMPO_CHANGED ; jusqu'a quand le tempo pose par
        # le studio n'est pas un changement de Live ; le rappel (un seul objet : add / remove)
        self.studio_caps = []
        self.tempo_known = None
        self.tempo_sent_at = -1e9
        self.tempo_hold_until = 0.0
        self._tempo_cb = self.on_tempo
        self._tempo_song = None
        # (25 oct.) les transports : celui de la session ; celui du message en cours ; la boite du compagnon
        self.transport = "midi"
        self._via_now = None
        self.udp_out = []
        self.udp_seen = None
        self.last_failure = ""       # (26 oct.) la derniere commande qui a leve (UppwindLink.py : la lecture rapide)
        # (25 oct.) les notes par le plugin : le mode de chaque slot ('port' | 'plugin') ; piste visee -> piste
        # d'aide (la derniere connue : un nouveau BIND la reprend) ; les pistes d'aide creees par le script ; celles a
        # retirer (cle -> heure) ; les BIND qui attendent Live (slot -> {...})
        self.via = {}
        self.helper_of = {}
        self.made = set()
        self.doomed = {}
        self.pending = {}

    # -- Les pistes --------------------------------------------------------------------------
    def tracks(self):
        try:
            return list(self._song().tracks)
        except Exception:
            return []

    def index_of(self, key):
        for i, t in enumerate(self.tracks()):
            if _key(t) == key:
                return i
        return -1

    def track_by_key(self, key):
        for t in self.tracks():
            if _key(t) == key:
                return t
        return None

    @staticmethod
    def instrument(track):
        if not getattr(track, "has_midi_input", False):
            return None
        for d in getattr(track, "devices", []) or []:
            if _int(getattr(d, "type", 0)) == INSTRUMENT:
                return d
        return None

    def item(self, i, track):
        dev = self.instrument(track)
        if dev is None or NR.is_helper(track):        # (25 oct.) une piste d'aide n'est pas un instrument a jouer
            return None
        cls = str(getattr(dev, "class_name", ""))
        kind = "plugin" if cls in PLUGIN_CLASSES else ("rack" if getattr(dev, "can_have_chains", False) else "instrument")
        it = {"id": i, "name": str(getattr(track, "name", ""))[:64], "plugin": str(getattr(dev, "name", ""))[:64],
              "kind": kind}
        try:
            it["color"] = "#%06x" % (int(track.color) & 0xFFFFFF)
        except Exception:
            pass
        k = _key(track)
        slots = sorted(s for s, key in self.binds.items() if key == k)
        if slots:
            it["slots"] = slots
        return it

    def list_page(self, start):
        tracks = self.tracks()
        out = []
        size = 40
        i = max(0, start)
        while i < len(tracks):
            it = self.item(i, tracks[i])
            if it is None:
                i += 1
                continue
            s = len(P.canonical_json(it)) + 1
            if out and size + s > LIST_BUDGET:
                break
            out.append(it)
            size += s
            i += 1
        data = {"items": out}
        if i < len(tracks):
            data["next"] = i
        return data

    def signature(self):
        # (25 oct.) l'index compte : une piste d'aide retiree au milieu decale les suivantes (leurs ITEM.id changent)
        sig = []
        for i, t in enumerate(self.tracks()):
            if NR.is_helper(t):
                continue
            dev = self.instrument(t)
            sig.append((_key(t), str(getattr(t, "name", "")), str(getattr(dev, "name", "")) if dev else None,
                        _int(getattr(t, "color", 0)), i))
        return tuple(sig)

    # -- Routage d'entree (BIND) -------------------------------------------------------------
    @staticmethod
    def _is_link_port(name):
        # (26 oct.) le port loopMIDI, quel que soit son libelle (« Uppwind Link », « UppwindLink In ») ; jamais le retour
        # (« … Back »), ni une piste d'aide (« Uppwind Link · Lead »), ni les plugins (Notes, Return)
        s = " ".join(str(name or "").split()).lower()
        if s.startswith(NR.HELPER_PREFIX.lower()):
            return False
        return "uppwindlink" in s.replace(" ", "") and not any(w in s for w in ("back", "notes", "return"))

    # -- (26 oct.) Les pistes restees sur le port loopMIDI ----------------------------------------------------------
    def fallback_input(self, track):
        """L'entree d'une piste MIDI neuve (All Ins / All Channels, Monitor Auto) : celle d'avant le BIND quand le
        script ne la connait plus (un set d'une autre session, un script recharge)."""
        types = list(getattr(track, "available_input_routing_types", []) or [])
        pick = None
        for rt in types:
            if " ".join(str(getattr(rt, "display_name", "")).split()).lower() == FALLBACK_INPUT:
                pick = rt
                break
        if pick is None:
            for rt in types:
                name = str(getattr(rt, "display_name", "")).strip()
                if name and not self._is_link_port(name) and not name.lower().startswith(NR.HELPER_PREFIX.lower()):
                    pick = rt
                    break
        if pick is None:
            return False
        track.input_routing_type = pick
        chans = list(getattr(track, "available_input_routing_channels", []) or [])
        for rc in chans:
            if str(getattr(rc, "display_name", "")).strip().lower() == "all channels":
                track.input_routing_channel = rc
                break
        else:
            if chans:
                track.input_routing_channel = chans[0]
        track.current_monitoring_state = MONITOR_AUTO
        return True

    def off_link_port(self, track):
        """Une piste encore sur « Uppwind Link / Ch. n » (le chemin loopMIDI d'avant) retrouve son entree d'avant le BIND :
        celle que le script a gardee, sinon celle d'une piste neuve. -> True si elle a change."""
        rt = getattr(track, "input_routing_type", None)
        if rt is None or not self._is_link_port(getattr(rt, "display_name", "")):
            return False
        k = _key(track)
        s = self.saved.pop(k, None)
        ok = False
        try:
            if s is not None and s.get("type") is not None and not self._is_link_port(getattr(s["type"], "display_name", "")):
                track.input_routing_type = s["type"]
                if s.get("channel") is not None:
                    track.input_routing_channel = s["channel"]
                if s.get("monitor") is not None:
                    track.current_monitoring_state = s["monitor"]
                ok = True
            else:
                ok = self.fallback_input(track)
        except Exception as e:
            self._log("restore routing: %s" % e)
        if ok:
            self._log("moved '%s' off the Uppwind Link MIDI port (left from the loopMIDI route)" % getattr(track, "name", ""))
        return ok

    def sweep_link_port(self):
        """(26 oct.) Une session sans loopMIDI commence : aucune piste ne doit rester sur le port (les notes viennent des
        pistes d'aide). Celles qu'une session d'avant y a laissees retrouvent leur entree."""
        n = 0
        for t in self.tracks():
            if NR.is_helper(t):
                continue
            try:
                if self.off_link_port(t):
                    n += 1
            except Exception as e:
                self._log("restore routing: %s" % e)
        return n

    def route(self, track, slot):
        types = list(getattr(track, "available_input_routing_types", []) or [])
        port = None
        for rt in types:
            if self._is_link_port(getattr(rt, "display_name", "")):
                port = rt
                break
        if port is None:
            return False
        k = _key(track)
        before_type = getattr(track, "input_routing_type", None)
        before_chan = getattr(track, "input_routing_channel", None)
        fresh = k not in self.saved
        if fresh:
            self.saved[k] = {"type": before_type, "channel": before_chan,
                             "monitor": getattr(track, "current_monitoring_state", None)}
        track.input_routing_type = port
        want = "%d" % slot
        chan = None
        for rc in list(getattr(track, "available_input_routing_channels", []) or []):
            name = str(getattr(rc, "display_name", "")).strip()
            digits = name.split(".")[-1].strip() if "." in name else name.split(" ")[-1]
            if digits == want:
                chan = rc
                break
        if chan is None:
            # Sans « Ch. n », la piste ecouterait les 16 slots (All Channels) : on remet son entree
            try:
                if before_type is not None:
                    track.input_routing_type = before_type
                if before_chan is not None:
                    track.input_routing_channel = before_chan
            except Exception as e:
                self._log("restore routing: %s" % e)
            if fresh:
                self.saved.pop(k, None)
            return False
        track.input_routing_channel = chan
        track.current_monitoring_state = MONITOR_IN
        return True

    # -- (25 oct.) Les notes par le plugin : la piste d'aide « Uppwind Link · <piste> » -----------------------------
    def helper_for(self, track):
        """La piste d'aide de `track` : celle que son entree designe deja, la derniere qu'on lui a donnee, ou celle
        qui porte son nom. None : il faut la creer."""
        tracks = self.tracks()
        src = NR.source_of(track, tracks)
        if src is not None:
            return src
        k = self.helper_of.get(_key(track))
        if k is not None:
            t = self.track_by_key(k)
            # celle que le script vient de creer compte meme si son Notes se charge encore : sinon chaque nouvel
            # essai (retry_pending) en creerait une autre
            if t is not None and (NR.is_helper(t) or (k in self.made and NR.untouched_helper(t))):
                return t
        want = NR.helper_name(getattr(track, "name", ""))
        for t in tracks:
            if t is not track and NR.is_helper(t) and str(getattr(t, "name", "")) == want:
                return t
        return None

    def _undo(self, begin):
        try:
            f = getattr(self._song(), "begin_undo_step" if begin else "end_undo_step", None)
            if f is not None:
                f()
        except Exception as e:
            self._log("undo step: %s" % e)

    def make_helper(self, track):
        """La piste d'aide neuve, au bout du set, avec son Uppwind Link Notes -> (piste, None) ou (None, erreur).
        Une seule etape d'annulation ; la piste choisie dans Live est rendue telle quelle."""
        b = self._browser()
        item = NR.find_notes_item(b)
        if item is None:
            return None, NR.NOT_INSTALLED
        song = self._song()
        view = getattr(song, "view", None)
        before = getattr(view, "selected_track", None) if view is not None else None
        self._undo(True)
        try:
            index = len(self.tracks())
            song.create_midi_track(index)
            helper = self.tracks()[index]
            view.selected_track = helper
            b.load_item(item)
            helper.name = NR.helper_name(getattr(track, "name", ""))
        finally:
            self._undo(False)
            if before is not None:
                try:
                    view.selected_track = before
                except Exception:
                    pass
        self.made.add(_key(helper))
        self._log("created the helper track '%s' (Uppwind Link Notes)" % getattr(helper, "name", ""))
        return helper, None

    def route_plugin(self, track):
        """L'entree de `track` sur « <piste d'aide> / Uppwind Link Notes », la piste d'aide creee s'il le faut.
        -> ("ok", numero) | ("wait", raison) (Live n'offre pas encore la piste, le plugin se charge) | ("error", msg)."""
        k = _key(track)
        # (26 oct.) encore sur le port loopMIDI (une session d'avant, un script recharge) : son entree d'avant d'abord,
        # pour que UNBIND la rende, et jamais « Uppwind Link / Ch. n »
        self.off_link_port(track)
        helper = self.helper_for(track)
        if helper is None:
            helper, err = self.make_helper(track)
            if helper is None:
                return "error", err
        hk = _key(helper)
        self.helper_of[k] = hk
        self.doomed.pop(hk, None)
        dev = NR.notes_device(helper)
        if dev is None:
            return "wait", "the plugin is loading"
        tag = NR.read_tag(dev)
        if tag is None:
            return "wait", NR.NO_TAG
        others = NR.tags_in_use(self.tracks(), skip=dev)
        if tag == 0 or tag in others:
            # pas encore de numero, ou une copie (piste dupliquee) : un numero libre
            tag = NR.free_tag(self.tracks(), skip=dev)
            if not tag or not NR.write_tag(dev, tag):
                return "wait", "the Link tag does not stick"
        if not NR.points_to(track, helper):
            rt = NR.routing_to(track, helper)
            if rt is None:
                return "wait", "Live does not offer the helper track as an input yet"
            if k not in self.saved:
                self.saved[k] = {"type": getattr(track, "input_routing_type", None),
                                 "channel": getattr(track, "input_routing_channel", None),
                                 "monitor": getattr(track, "current_monitoring_state", None)}
            track.input_routing_type = rt
            ch = NR.notes_channel(track, dev)
            if ch is None:
                return "wait", "Live does not offer the Uppwind Link Notes output yet"
            track.input_routing_channel = ch
        if getattr(track, "current_monitoring_state", None) != MONITOR_IN:
            if k not in self.saved:
                self.saved[k] = {"type": None, "channel": None, "monitor": getattr(track, "current_monitoring_state", None)}
            track.current_monitoring_state = MONITOR_IN
        return "ok", tag

    def tag_of(self, key):
        h = self.helper_of.get(key)
        t = self.track_by_key(h) if h is not None else None
        dev = NR.notes_device(t) if t is not None else None
        return (NR.read_tag(dev) or 0) if dev is not None else 0

    def free_helper(self, key):
        """La piste visee ne prend plus ses notes du plugin : sa piste d'aide (si le script l'a creee et que personne
        d'autre ne la prend) part dans HELPER_GRACE_S."""
        hk = self.helper_of.get(key)
        if hk is None or hk not in self.made:
            return
        for k, h in self.helper_of.items():
            if h == hk and k != key and k in self.binds.values():
                return
        self.doomed[hk] = self._now() + HELPER_GRACE_S

    def sweep_helpers(self, force=False):
        """Les pistes d'aide a retirer, l'heure venue (force : toutes, maintenant) — sauf celles qu'une piste prend
        encore (a la main, ou par un BIND arrive entre-temps)."""
        now = self._now()
        due = [hk for hk, at in self.doomed.items() if force or now >= at]
        if not due:
            return
        tracks = self.tracks()
        busy = set()
        for t in tracks:
            src = NR.source_of(t, tracks)
            if src is not None:
                busy.add(_key(src))
        bound_helpers = set(self.helper_of.get(k) for k in self.binds.values())
        for hk in due:
            del self.doomed[hk]
            if hk in busy or hk in bound_helpers:
                continue
            i = self.index_of(hk)
            if i < 0 or not NR.untouched_helper(self.tracks()[i]):
                # partie, ou plus la meme : renommee, un autre device, des clips, une cle reprise par Live pour une
                # autre piste — on n'y touche pas
                self.made.discard(hk)
                for k in [k for k, h in self.helper_of.items() if h == hk]:
                    del self.helper_of[k]
                continue
            self._undo(True)
            try:
                self._song().delete_track(i)
                self._log("removed the helper track it had created")
            except Exception as e:
                self._log("remove helper track: %s" % e)
            finally:
                self._undo(False)
            self.made.discard(hk)
            for k in [k for k, h in self.helper_of.items() if h == hk]:
                del self.helper_of[k]

    def unroute(self, key):
        s = self.saved.pop(key, None)
        t = self.track_by_key(key)
        if s is None or t is None:
            return
        try:
            if s["type"] is not None:
                t.input_routing_type = s["type"]
            if s["channel"] is not None:
                t.input_routing_channel = s["channel"]
            if s["monitor"] is not None:
                t.current_monitoring_state = s["monitor"]
        except Exception as e:
            self._log("restore routing: %s" % e)

    # -- Mixage (SET) --------------------------------------------------------------------------
    def capture(self, key, track):
        if key not in self.orig:
            md = track.mixer_device
            self.orig[key] = (md.volume.value, md.panning.value, bool(track.mute))

    def apply_mutes(self):
        bound = set(self.binds.values())
        any_solo = any(self.mix.get(k, {}).get("solo") for k in bound)
        for k in bound:
            t = self.track_by_key(k)
            if t is None:
                continue
            m = self.mix.get(k, {})
            if "mute" in m or any_solo:
                self.capture(k, t)
                want = bool(m.get("mute")) or (any_solo and not m.get("solo"))
                if bool(t.mute) != want:
                    t.mute = want
            elif k in self.orig and bool(t.mute) != self.orig[k][2]:
                t.mute = self.orig[k][2]

    def restore(self, key):
        o = self.orig.pop(key, None)
        self.mix.pop(key, None)
        t = self.track_by_key(key)
        if o is None or t is None:
            return
        try:
            md = t.mixer_device
            md.volume.value = o[0]
            md.panning.value = o[1]
            t.mute = o[2]
        except Exception as e:
            self._log("restore mix: %s" % e)

    def set(self, d):
        tracks = self.tracks()
        t = tracks[d["id"]]
        k = _key(t)
        self.capture(k, t)
        m = self.mix.setdefault(k, {})
        md = t.mixer_device
        if "vol" in d:
            md.volume.value = gain_to_live_volume(md.volume, d["vol"])
        if "pan" in d:
            md.panning.value = max(md.panning.min, min(md.panning.max, d["pan"]))
        for f in ("mute", "solo"):
            if f in d:
                m[f] = d[f]
        if "mute" in d or "solo" in d:
            self.apply_mutes()

    # -- Liens ---------------------------------------------------------------------------------
    def bind(self, slot, track, via="port"):
        """-> False (rien n'a change) si Live n'offre pas le canal « Ch. n » sur cette piste. (25 oct.) via 'plugin' :
        -> ("ok", numero) | ("wait", raison) | ("error", msg) (route_plugin) ; rien ne change dans les liens avant « ok »."""
        k = _key(track)
        res = True
        if via == "plugin":
            res = self.route_plugin(track)
            if res[0] != "ok":
                return res
        elif not self.route(track, slot):
            return False
        self.link_slot(slot, k, via)
        return res

    def link_slot(self, slot, k, via):
        old = self.binds.get(slot)
        self.waiting.pop(slot, None)
        # une piste n'a qu'un canal d'entree : l'autre slot attend (ITEM.slots ne le montre plus) et
        # reprend la piste quand celui-ci s'en va — elle n'est jamais rendue a son entree d'avant tant
        # qu'un slot la veut encore
        for s, key in list(self.binds.items()):
            if key == k and s != slot:
                del self.binds[s]
                self.waiting[s] = k
        if old is not None and old != k:
            del self.binds[slot]
            self.release(old)
        self.binds[slot] = k
        self.via[slot] = via
        self.apply_mutes()
        return True

    def release(self, key):
        """La piste n'a plus de slot actif : le dernier slot deplace la reprend, sinon elle retrouve son entree."""
        if key in self.binds.values():
            return
        t = self.track_by_key(key)
        for s in reversed(list(self.waiting)):
            if self.waiting[s] != key:
                continue
            del self.waiting[s]
            via = self.via.get(s, "port")
            ok = False
            if t is not None:
                ok = self.route_plugin(t)[0] == "ok" if via == "plugin" else self.route(t, s)
            if ok:
                self.binds[s] = key
                return
            self.via.pop(s, None)
        self.unroute(key)
        self.restore(key)
        self.free_helper(key)

    def unbind(self, slot):
        self.waiting.pop(slot, None)
        self.pending.pop(slot, None)
        k = self.binds.pop(slot, None)
        if k is None:
            self.via.pop(slot, None)
            return
        self.release(k)
        if slot not in self.binds and slot not in self.waiting:
            self.via.pop(slot, None)
        self.apply_mutes()

    def reset(self):
        self.waiting.clear()                    # tout s'en va : personne ne reprend les pistes
        self.pending.clear()
        for slot in list(self.binds):
            self.unbind(slot)
        for k in list(self.orig):
            self.restore(k)
        self.mix.clear()
        self.via.clear()
        self.plugin_cache = None

    # -- Plugins ---------------------------------------------------------------------------
    def plugins(self):
        if self.plugin_cache is not None:
            return self.plugin_cache
        out = []
        b = self._browser()
        if b is None:
            self.plugin_cache = out
            return out

        def walk(node, depth, vendor, kind, path):
            for child in list(getattr(node, "children", []) or []):
                if len(out) >= MAX_PLUGINS:
                    return
                name = str(getattr(child, "name", ""))
                p = path + "/" + name
                # 14 oct. (premier essai dans Live 12.4.6) : un plugin se charge (is_loadable) ; is_device seul ne
                # suffit pas a le reconnaitre partout — un element chargeable qui n'est pas un dossier en est un aussi
                loadable = getattr(child, "is_loadable", False)
                if loadable and (getattr(child, "is_device", False) or not getattr(child, "is_folder", False)):
                    uri = getattr(child, "uri", None)
                    out.append({"id": str(uri or p)[:200], "name": name[:80], "vendor": vendor[:60], "kind": kind,
                                "_item": child})
                elif depth < 4 and not loadable:
                    v = vendor
                    if kind == "plugin" and name.lower() not in PLUGIN_FOLDERS:
                        v = name
                    walk(child, depth + 1, v, kind, p)

        try:
            # Les instruments de Live : un niveau (les dossiers de presets en dessous, on ne descend pas)
            for child in list(getattr(b.instruments, "children", []) or []):
                if getattr(child, "is_device", False) and getattr(child, "is_loadable", False):
                    name = str(getattr(child, "name", ""))
                    uri = getattr(child, "uri", None)
                    out.append({"id": str(uri or ("instruments/" + name))[:200], "name": name[:80],
                                "vendor": "Ableton", "kind": "instrument", "_item": child})
            walk(b.plugins, 1, "", "plugin", "plugins")
            if not any(x["kind"] == "plugin" for x in out):
                self._log("no plugin found in the browser: %s" % self._tree(b.plugins, 2))
        except Exception as e:
            self._log("browser: %s" % e)
        self.plugin_cache = out
        return out

    def _tree(self, node, depth):
        """Le haut de l'arbre des plugins, pour le Log.txt de Live quand rien n'y est reconnu : nom, dossier,
        appareil, chargeable, nombre d'enfants (les 6 premiers de chaque niveau)."""
        parts = []
        for child in list(getattr(node, "children", []) or [])[:6]:
            kids = list(getattr(child, "children", []) or [])
            s = "%s[f=%d d=%d l=%d n=%d]" % (getattr(child, "name", "?"), bool(getattr(child, "is_folder", False)),
                                            bool(getattr(child, "is_device", False)),
                                            bool(getattr(child, "is_loadable", False)), len(kids))
            if depth > 1 and kids:
                s += " { %s }" % self._tree(child, depth - 1)
            parts.append(s)
        return ", ".join(parts) or "(empty)"

    def plugins_page(self, start):
        items = self.plugins()
        out = []
        size = 40
        i = max(0, start)
        while i < len(items):
            it = dict((k, v) for k, v in items[i].items() if k != "_item")
            s = len(P.canonical_json(it)) + 1
            if out and size + s > LIST_BUDGET:
                break
            out.append(it)
            size += s
            i += 1
        data = {"items": out}
        if i < len(items):
            data["next"] = i
        return data

    def add(self, plugin_id, name):
        entry = None
        for p in self.plugins():
            if p["id"] == plugin_id:
                entry = p
                break
        if entry is None:
            return None, "No plugin with this id in Live's browser."
        song = self._song()
        index = len(self.tracks())
        song.create_midi_track(index)
        track = self.tracks()[index]
        song.view.selected_track = track
        self._browser().load_item(entry["_item"])
        if name:
            track.name = name
        self.added.add(_key(track))
        it = {"id": index, "name": str(getattr(track, "name", name or entry["name"]))[:64],
              "plugin": entry["name"][:64], "kind": entry["kind"]}
        try:
            it["color"] = "#%06x" % (int(track.color) & 0xFFFFFF)
        except Exception:
            pass
        return it, None

    def remove(self, i):
        tracks = self.tracks()
        t = tracks[i]
        k = _key(t)
        if k not in self.added:
            return "Only instruments added from Uppwind Studio can be removed from here."
        for s, key in list(self.binds.items()):
            if key == k:
                del self.binds[s]
        for s, key in list(self.waiting.items()):
            if key == k:
                del self.waiting[s]
        self.saved.pop(k, None)
        self.orig.pop(k, None)
        self.mix.pop(k, None)
        self.added.discard(k)
        self.free_helper(k)                     # (25 oct.) sa piste d'aide part aussi (si le script l'a creee)
        self.helper_of.pop(k, None)
        self._song().delete_track(i)
        return None

    # -- Le tempo (cap 'tempo') ------------------------------------------------------------------
    def get_tempo(self):
        try:
            v = float(self._song().tempo)
        except Exception:
            return None
        return None if (math.isnan(v) or math.isinf(v)) else v

    def watch_tempo(self):
        """Le rappel de Live sur Song.tempo, une fois par Song (tick() relit aussi le tempo : sans
        rappel, rien ne se perd)."""
        try:
            song = self._song()
        except Exception:
            return
        if song is None or song is self._tempo_song:
            return
        self.unwatch_tempo()
        add = getattr(song, "add_tempo_listener", None)
        if add is None:
            return
        try:
            has = getattr(song, "tempo_has_listener", None)
            if has is None or not has(self._tempo_cb):
                add(self._tempo_cb)
            self._tempo_song = song
        except Exception as e:
            self._log("tempo listener: %s" % e)

    def unwatch_tempo(self):
        song, self._tempo_song = self._tempo_song, None
        if song is None:
            return
        try:
            has = getattr(song, "tempo_has_listener", None)
            if has is None or has(self._tempo_cb):
                song.remove_tempo_listener(self._tempo_cb)
        except Exception as e:
            self._log("tempo listener: %s" % e)

    def on_tempo(self):
        """Live : Song.tempo a change (dans Live, ou parce que le studio l'a pose)."""
        try:
            self.tempo_check()
        except Exception as e:
            self._log("tempo: %s" % e)

    def set_tempo(self, bpm):
        """TEMPO du studio : Song.tempo. Ce tempo-la n'est pas renvoye (TEMPO_HOLD_MS)."""
        v = max(LIVE_TEMPO_MIN, min(LIVE_TEMPO_MAX, float(bpm)))
        self.tempo_known = v
        self.tempo_hold_until = self._now() + P.TEMPO_HOLD_MS / 1000.0
        self._song().tempo = v

    def tempo_check(self):
        """Le tempo de Live a-t-il change sans le studio ? -> TEMPO_CHANGED, au plus un par 50 ms (le
        dernier gagne : tick() reprend celui qui a du attendre)."""
        if not self.session or "tempo" not in self.studio_caps:
            return
        t = self.get_tempo()
        if t is None:
            return
        now = self._now()
        if now < self.tempo_hold_until:
            return
        if self.tempo_known is not None and abs(t - self.tempo_known) <= P.TEMPO_EPS:
            return
        if now - self.tempo_sent_at < P.TEMPO_MIN_INTERVAL_MS / 1000.0:
            return
        bpm = P.round_bpm(t)
        if bpm is None:
            return
        self.tempo_known = t
        self.tempo_sent_at = now
        self.send(P.TEMPO_CHANGED, 0, {"bpm": bpm})

    # -- Le protocole ------------------------------------------------------------------------
    def send(self, cmd, req, data, via=None):
        """(25 oct.) via : le transport ('midi' | 'udp') ; par defaut celui du message en cours (une reponse), sinon
        celui de la session (ce que le script pousse)."""
        try:
            chunks = P.encode_message(cmd, req, data)
        except Exception as e:
            self._log("cannot encode %s: %s" % (P.command_name(cmd), e))
            return
        via = via or self._via_now or self.transport
        if via == "udp":
            self.udp_out.extend(chunks)
            if len(self.udp_out) > UDP_OUT_MAX:
                self._log("the companion stopped reading the link (%d messages waiting): dropped" % len(self.udp_out))
                self.udp_out = []
            return
        self.out.extend(chunks)
        self.flush()

    def error(self, req, code, msg=None):
        data = {"code": code}
        if msg:
            data["msg"] = str(msg)[:200]
        self.send(P.ERROR, req, data)

    def flush(self):
        n = 0
        while self.out and n < OUT_PER_TICK:
            chunk = self.out.pop(0)
            n += 1
            try:
                self._send_raw(tuple(bytearray(chunk)))
            except Exception as e:
                self._log("send_midi failed: %s" % e)
                self.out = []
                return

    def receive(self, midi_bytes, via="midi"):
        """Un message MIDI recu par le script -> True s'il etait a nous. (25 oct.) via 'udp' : le meme morceau, arrive
        par le canal du compagnon (handle_link) ; chaque transport a son reassembleur."""
        if not midi_bytes or midi_bytes[0] != 0xF0 or not P.is_link_sysex(midi_bytes):
            return False
        rx = self.rx if via != "udp" else self._udp_rx()
        out = rx.push(midi_bytes, self._now() * 1000.0)
        t = out["type"]
        if t == "ignored":
            return False
        if t == "partial":
            return True
        self._via_now = via
        try:
            if t == "error":
                cmd, req = out.get("cmd"), out.get("req")
                sender = P.COMMANDS[cmd][1] if cmd in P.COMMANDS else None
                if req is not None and sender != "daw" and cmd != P.ERROR:
                    self.error(req, out["code"], "Update the Uppwind Link script." if out["code"] == P.ERR_VERSION else None)
                return True
            try:
                self.dispatch(out["cmd"], out["req"], out["data"])
            except Exception as e:
                self.last_failure = "%s: %s" % (type(e).__name__, e)
                self._log("%s failed: %s" % (out.get("name"), e))
                self.error(out["req"], P.ERR_INTERNAL, "%s: %s" % (type(e).__name__, e))
            return True
        finally:
            self._via_now = None

    def _udp_rx(self):
        r = getattr(self, "_rx_udp", None)
        if r is None:
            r = self._rx_udp = P.Reassembler()
        return r

    # -- (25 oct.) Le canal du compagnon : LINK (les morceaux SysEx en hexadecimal), ROUTES ------------------------
    def handle_link(self, req):
        """{"cmd": "LINK", "in": ["f07d5557…f7", …]} -> {"ok": True, "out": [...], "more": bool} : les morceaux du
        studio, puis ce que le script a pour lui. Un morceau illisible ou qui n'est pas du lien est ignore."""
        self.udp_seen = self._now()
        ins = req.get("in") or []
        if not isinstance(ins, list):
            return {"ok": False, "error": "in: a list of hex chunks"}
        bad = 0
        for h in ins[:LINK_IN_MAX]:
            try:
                b = bytearray.fromhex(str(h))
            except (TypeError, ValueError):
                bad += 1
                continue
            if not b or b[0] != 0xF0 or not self.receive(tuple(b), via="udp"):
                bad += 1
        out = []
        size = 0
        while self.udp_out:
            h = "".join("%02x" % x for x in bytearray(self.udp_out[0]))
            if out and size + len(h) > LINK_OUT_BUDGET:
                break
            out.append(h)
            size += len(h)
            self.udp_out.pop(0)
        rep = {"ok": True, "out": out, "more": bool(self.udp_out)}
        if bad:
            rep["ignored"] = bad
        return rep

    def link_hold(self, req):
        """(26 oct.) Le canal peut-il garder cette requete (-> s, 0 : repondre maintenant) ? Un releve vide du compagnon
        (LINK sans « in », avec « wait » en ms) attend que le script ait quelque chose pour le studio : il repart des que
        la boite se remplit (CHANGED, TEMPO_CHANGED, un BIND_ACK qui attendait Live), sans releves a vide en boucle."""
        if str(req.get("cmd", "")).upper() != "LINK" or req.get("in") or self.udp_out:
            return 0
        try:
            w = float(req.get("wait") or 0) / 1000.0
        except (TypeError, ValueError):
            return 0
        return min(w, LINK_WAIT_MAX_S) if w > 0 else 0

    def take_failure(self):
        """(26 oct.) La derniere commande qui a leve (son texte), une seule fois ; '' sinon."""
        f = getattr(self, "last_failure", "")
        self.last_failure = ""
        return f

    def routes(self):
        """ROUTES : l'entree de chaque piste, ses Notes et leurs numeros (le diagnostic du chemin sans loopMIDI)."""
        tracks = self.tracks()
        return {"ok": True, "tracks": [NR.describe(i, t, tracks) for i, t in enumerate(tracks)],
                "notesInBrowser": NR.find_notes_item(self._browser()) is not None}

    def playable(self, i):
        tracks = self.tracks()
        return 0 <= i < len(tracks) and self.instrument(tracks[i]) is not None and not NR.is_helper(tracks[i])

    def bound(self, req, slot, i, tag, via=None):
        """(25 oct.) Un BIND par le plugin a abouti : BIND_ACK avec le numero du Notes."""
        self.list_sig = self.signature()
        self.send(P.BIND_ACK, req, {"slot": slot, "id": i, "via": "plugin", "notes": int(tag)}, via=via)

    def retry_pending(self):
        """Les BIND par le plugin qui attendaient Live (la piste d'aide, le plugin qui se charge, le menu « MIDI
        From » pas encore a jour) : encore un essai, ou l'ERROR passe le delai."""
        now = self._now()
        for slot, p in list(self.pending.items()):
            t = self.track_by_key(p["key"])
            if t is None:
                del self.pending[slot]
                self.send(P.ERROR, p["req"], {"code": P.ERR_NOT_FOUND, "msg": "The track went away."}, via=p["via"])
                continue
            res = self.bind(slot, t, via="plugin")
            if res[0] == "ok":
                del self.pending[slot]
                self.bound(p["req"], slot, self.index_of(p["key"]), res[1], via=p["via"])
            elif res[0] == "error" or now >= p["until"]:
                del self.pending[slot]
                why = res[1] or p["why"]
                msg = why if res[0] == "error" or why == NR.NO_TAG else "Live did not set up the Uppwind Link Notes route in time (%s)." % why
                self._log("BIND slot %d via the plugin: %s" % (slot, msg))
                self.send(P.ERROR, p["req"], {"code": P.ERR_UNSUPPORTED, "msg": msg[:200]}, via=p["via"])
            else:
                p["why"] = res[1]

    def dispatch(self, cmd, req, data):
        spec = P.COMMANDS[cmd]
        if spec[1] == "daw":
            return
        v = P.validate_message(cmd, data, "studio")
        if not v["ok"]:
            if cmd != P.ERROR:
                self.error(req, v["error"]["code"], v["error"]["msg"])
            return
        d = v["data"]
        if cmd == P.HELLO:
            self.reset()
            self.session = True
            # (25 oct.) la session vit sur le transport de son HELLO ; l'autre boite repart vide
            self.transport = self._via_now or "midi"
            if self.transport == "udp":
                self.out = []
                # (26 oct.) le studio quitte loopMIDI : les pistes qu'il y avait reliees (et que ce script ne connait
                # peut-etre plus) retrouvent leur entree ; ses BIND par le plugin les menent ensuite a leur piste d'aide
                self.sweep_link_port()
            else:
                self.udp_out = []
            self.list_sig = self.signature()
            project = ""
            try:
                project = str(getattr(self._song(), "name", "") or "")[:120]
            except Exception:
                pass
            # le tempo : ce que Live a maintenant ; TEMPO_CHANGED seulement a un studio qui le lit
            self.studio_caps = list(d.get("caps", []))
            self.tempo_known = self.get_tempo()
            self.tempo_hold_until = 0.0
            self.tempo_sent_at = -1e9
            self.watch_tempo()
            ack = {"v": P.LINK_VERSION, "daw": DAW_NAME, "dawVersion": str(self._version())[:60],
                   "project": project, "caps": list(CAPS)}
            bpm = P.round_bpm(self.tempo_known)
            if bpm is not None:
                ack["bpm"] = bpm
            self.send(P.HELLO_ACK, req, ack)
            self._log("Uppwind Studio connected.")
        elif cmd == P.LIST:
            self.send(P.LIST_REPLY, req, self.list_page(d.get("start", 0)))
        elif cmd == P.BIND:
            if not self.playable(d["id"]):
                self.error(req, P.ERR_NOT_FOUND, "No instrument on track %d." % (d["id"] + 1))
                return
            track = self.tracks()[d["id"]]
            if d.get("via") == "plugin":
                # (25 oct.) sans loopMIDI : la piste d'aide et son Uppwind Link Notes ; jamais « Uppwind Link / Ch. n »
                self.pending.pop(d["slot"], None)
                res = self.bind(d["slot"], track, via="plugin")
                if res[0] == "error":
                    self.error(req, P.ERR_UNSUPPORTED, res[1])
                elif res[0] == "wait":
                    self.pending[d["slot"]] = {"req": req, "id": d["id"], "key": _key(track), "why": res[1],
                                               "until": self._now() + PENDING_S, "via": self._via_now or self.transport}
                else:
                    self.bound(req, d["slot"], d["id"], res[1])
                return
            types = [getattr(rt, "display_name", "") for rt in getattr(track, "available_input_routing_types", []) or []]
            if not any(self._is_link_port(n) for n in types):
                self.error(req, P.ERR_NOT_FOUND, ROUTING_HINT)
                return
            if not self.bind(d["slot"], track):
                self.error(req, P.ERR_NOT_FOUND, "Live offers no 'Ch. %d' input channel for the Uppwind Link port "
                                                 "on track %d." % (d["slot"], d["id"] + 1))
                return
            self.list_sig = self.signature()
            self.send(P.BIND_ACK, req, {"slot": d["slot"], "id": d["id"], "via": "port"})
        elif cmd == P.UNBIND:
            self.unbind(d["slot"])
        elif cmd == P.SET:
            if not self.playable(d["id"]):
                self.error(req, P.ERR_NOT_FOUND, "No instrument on track %d." % (d["id"] + 1))
                return
            self.set(d)
        elif cmd == P.PLUGINS:
            if d.get("start", 0) == 0:
                self.plugin_cache = None             # une nouvelle liste : on relit le navigateur
            self.send(P.PLUGINS_REPLY, req, self.plugins_page(d.get("start", 0)))
        elif cmd == P.ADD:
            item, err = self.add(d["plugin"], d.get("name"))
            if err:
                self.error(req, P.ERR_NOT_FOUND, err)
                return
            self.list_sig = self.signature()
            self.send(P.ADDED, req, {"item": item})
        elif cmd == P.REMOVE:
            if not (0 <= d["id"] < len(self.tracks())):
                self.error(req, P.ERR_NOT_FOUND, "No track %d." % (d["id"] + 1))
                return
            err = self.remove(d["id"])
            if err:
                self.error(req, P.ERR_UNSUPPORTED, err)
                return
            self.list_sig = self.signature()
        elif cmd == P.TEMPO:
            self.set_tempo(d["bpm"])
        elif cmd == P.PING:
            self.send(P.PONG, req, {"t": d["t"]})
        elif cmd == P.ERROR:
            self._log("Uppwind Studio reports %s: %s" % (d.get("code"), d.get("msg", "")))

    def tick(self):
        """Appele par update_display (~ toutes les 100 ms)."""
        self.flush()
        self.tempo_check()                     # le tempo qui a du attendre, ou un rappel manque
        if self.pending:
            self.retry_pending()               # (25 oct.) un BIND par le plugin qui attendait Live
        if self.doomed:
            self.sweep_helpers()               # (25 oct.) les pistes d'aide liberees, l'heure venue
        now = self._now()
        if now - self.last_poll < POLL_EVERY:
            return
        self.last_poll = now
        sig = self.signature()
        live = set(x[0] for x in sig)
        dropped = [s for s, k in self.binds.items() if k not in live]
        for s in dropped:                       # la piste a ete supprimee : le lien tombe
            k = self.binds.pop(s)
            self.saved.pop(k, None)
            self.orig.pop(k, None)
            self.mix.pop(k, None)
            self.via.pop(s, None)
            self.free_helper(k)                 # (25 oct.) sa piste d'aide, si le script l'a creee
            self.helper_of.pop(k, None)
        for s in [s for s, k in self.waiting.items() if k not in live]:
            del self.waiting[s]
        self.added &= live                     # une piste supprimee n'est plus « ajoutee par le studio »
        if self.list_sig is None:
            self.list_sig = sig
            return
        if sig != self.list_sig or dropped:
            self.list_sig = sig
            if self.session:
                self.send(P.CHANGED, 0, {})

    def shutdown(self):
        try:
            self.reset()
        except Exception as e:
            self._log("shutdown: %s" % e)
        # (25 oct.) Live ferme le set (ou le script) : les pistes d'aide restent (on ne touche pas au set qui se ferme ;
        # un prochain BIND les reprend par leur nom)
        self.doomed.clear()
        self.made.clear()
        self.unwatch_tempo()                   # le tempo reste celui qu'il est : partage, pas emprunte
