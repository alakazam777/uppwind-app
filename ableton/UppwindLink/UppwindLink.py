# -*- coding: utf-8 -*-
# SPDX-License-Identifier: MIT
# Copyright (C) 2026 Uppwind
"""Uppwind Link -- la surface de controle Ableton Live (coquille autour de link_core.LinkCore).

NON TESTE dans Live (Ableton n'etait pas installe la ou il a ete ecrit) : la logique est testee
contre un faux Live (tests/test_ableton.py) ; cette coquille, elle, ne fait que brancher Live.

Live : Preferences > Link, Tempo & MIDI
    Control Surface = UppwindLink, Input = Uppwind Link, Output = Uppwind Link Back
    Ports MIDI : « Uppwind Link » en entree -> Track ET Remote allumes ; « Uppwind Link Back » en
    sortie -> Remote allume.

24 oct. (v1.1.0) : le meme script ouvre aussi les fenetres des plugins pour le compagnon (editor_core.EditorCore,
par le canal local editor_channel.LocalChannel : UDP sur 127.0.0.1, jeton dans ~/.uppwind-link-dev.json). Plus de
deuxieme Control Surface pour cela. Un echec du canal n'arrete jamais le lien MIDI.

24 oct. (v1.2.0) : le tempo partage avec le studio (cap 'tempo', link_core : TEMPO / TEMPO_CHANGED).

25 oct. (v1.3.0) : SANS loopMIDI quand le compagnon est la. Le lien passe aussi par le canal local : LINK (les memes
morceaux SysEx, en hexadecimal) et ROUTES (le diagnostic des entrees) vont a link_core ; le reste aux fenetres des
plugins. Les notes passent par le plugin « Uppwind Link Notes » (BIND.via = 'plugin', notes_route.py). Les ports
loopMIDI restent le chemin de secours (FL Studio, un studio sans compagnon).

26 oct. (v1.3.1) : LA LATENCE DES COMMANDES. update_display ne vient que toutes les ~100 ms (mesure dans Live 12 :
99-100 ms entre deux reponses) ; le canal est maintenant lu aussi par un Live.Base.Timer (FAST_POLL_MS, sur le fil de
Live, comme le minuteur d'affichage du script Move d'Ableton) : une commande du studio repart en quelques ms au lieu
d'attendre le prochain update_display. Le releve vide du compagnon (« wait ») attend dans le canal que le script ait
quelque chose a dire (link 2 dans le fichier d'etat). Sans Timer, ou s'il echoue : update_display seul, comme avant.
"""

import time
import weakref

try:
    import Live
except ImportError:            # hors de Live (tests)
    Live = None

try:
    from _Framework.ControlSurface import ControlSurface as _Base
    _FRAMEWORK = True
except ImportError:
    _FRAMEWORK = False

    class _Base(object):
        """L'interface brute qu'attend Live, quand _Framework n'est pas la."""

        def __init__(self, c_instance):
            self._c_instance = c_instance

        def disconnect(self):
            pass

        def connect_script_instances(self, instanciated_scripts):
            pass

        def can_lock_to_devices(self):
            return False

        def refresh_state(self):
            pass

        def build_midi_map(self, midi_map_handle):
            pass

        def update_display(self):
            pass

        def receive_midi(self, midi_bytes):
            pass

        def suggest_input_port(self):
            return ""

        def suggest_output_port(self):
            return ""

        def suggest_map_mode(self, cc_no, channel):
            return -1

        def song(self):
            return self._c_instance.song()

        def application(self):
            return Live.Application.get_application() if Live else None

from .link_core import LinkCore
from .editor_core import EditorCore, EDITOR_VERSION
from .editor_channel import LocalChannel

SCRIPT_VERSION = "1.3.1"
TAG = "[Uppwind Link]"
CHANNEL_PRIORITY = 2          # le vrai script sert le canal local avant le script de dev (priorite 1)
LINK_TRANSPORT = 2            # (25 oct.) le fichier d'etat le dit au compagnon : LINK et ROUTES sont servis ici ;
                              # (26 oct.) 2 : le releve vide peut attendre (« wait »), le canal est lu vite
FAST_POLL_MS = 5              # (26 oct.) Live.Base.Timer : le canal lu toutes les 5 ms (un recvfrom qui ne bloque pas)


def channel_handler(core, editor):
    """(25 oct.) Une requete du canal local -> sa reponse : LINK et ROUTES au lien, le reste aux fenetres des plugins."""
    def handle(req):
        cmd = str(req.get("cmd", "")).upper()
        if cmd == "LINK":
            return core.handle_link(req)
        if cmd == "ROUTES":
            return core.routes()
        return editor.handle(req)
    return handle


class UppwindLinkSurface(_Base):
    # Tous nos attributs et methodes commencent par _uw_ : rien ne doit masquer ceux de
    # _Framework.ControlSurface.
    def __init__(self, c_instance):
        _Base.__init__(self, c_instance)
        self._uw_c = c_instance
        self._uw_core = LinkCore(song=self._uw_song, send=self._uw_send, log=self._uw_log, now=time.time,
                             browser=self._uw_browser, version=self._uw_version)
        # les fenetres des plugins (le compagnon) : a part, et jamais au prix du lien MIDI
        self._uw_editor = None
        self._uw_chan = None
        self._uw_fast = None
        self._uw_fast_cb = None
        self._uw_stopped = False
        try:
            self._uw_editor = EditorCore(song=self._uw_song, log=self._uw_log_raw, now=time.time,
                                         version=self._uw_version, tag=TAG)
            self._uw_chan = LocalChannel(channel_handler(self._uw_core, self._uw_editor), log=self._uw_editor.log,
                                         priority=CHANNEL_PRIORITY,
                                         info={"script": "UppwindLink", "version": SCRIPT_VERSION,
                                               "editor": EDITOR_VERSION, "live": self._uw_version(),
                                               "link": LINK_TRANSPORT},
                                         park=self._uw_core.link_hold)
            self._uw_chan.open()
        except Exception as e:
            self._uw_log("plugin windows (companion channel) off: %s" % e)
            self._uw_chan = None
        self._uw_start_fast()
        self._uw_log("v%s ready. Waiting for Uppwind Studio." % SCRIPT_VERSION)

    # -- (26 oct.) La lecture rapide du canal ------------------------------------------------------------------
    def _uw_start_fast(self):
        """Un Live.Base.Timer (fil de Live) lit le canal toutes les FAST_POLL_MS. Absent, ou refuse : update_display."""
        if self._uw_chan is None:
            return
        base = getattr(Live, "Base", None) if Live is not None else None
        timer = getattr(base, "Timer", None) if base is not None else None
        if timer is None:
            self._uw_log("no Live.Base.Timer: the companion channel is read every ~100 ms (update_display)")
            return
        me = weakref.ref(self)          # le minuteur ne garde pas le script en vie (comme le script Move d'Ableton)

        def on_timer():
            s = me()
            if s is not None:
                s._uw_fast_poll()
        try:
            self._uw_fast_cb = on_timer
            self._uw_fast = timer(callback=on_timer, interval=FAST_POLL_MS, repeat=True)
            self._uw_fast.start()
        except Exception as e:
            self._uw_fast = None
            self._uw_log("fast channel reads off (%s): every ~100 ms instead" % e)

    def _uw_stop_fast(self, why=None):
        t, self._uw_fast = self._uw_fast, None
        if t is not None:
            try:
                t.stop()
            except Exception:
                pass
            if why:
                self._uw_log("fast channel reads off (%s): every ~100 ms instead" % why)

    def _uw_fast_poll(self):
        if self._uw_stopped or self._uw_chan is None or self._uw_fast is None:
            return
        try:
            self._uw_chan.poll()
        except Exception as e:
            self._uw_stop_fast(e)
            return
        # Live refuserait un changement depuis ce minuteur (« Changes cannot be triggered by notifications ») : on
        # revient a update_display, ou tout passe (la commande qui a echoue a deja eu son ERROR)
        f = self._uw_core.take_failure()
        if f and "notification" in f.lower():
            self._uw_stop_fast(f)

    # -- Branchements ------------------------------------------------------------------------
    def _uw_song(self):
        return self._uw_c.song()

    def _uw_browser(self):
        try:
            return Live.Application.get_application().browser
        except Exception:
            return None

    def _uw_version(self):
        try:
            app = Live.Application.get_application()
            return "%d.%d.%d" % (app.get_major_version(), app.get_minor_version(), app.get_bugfix_version())
        except Exception:
            return ""

    def _uw_send(self, midi_bytes):
        self._uw_c.send_midi(midi_bytes)

    def _uw_log(self, msg):
        self._uw_log_raw("%s %s" % (TAG, msg))

    def _uw_log_raw(self, msg):
        try:
            self._uw_c.log_message(msg)
        except Exception:
            pass

    # -- Ce que Live appelle -----------------------------------------------------------------
    def receive_midi(self, midi_bytes):
        try:
            if self._uw_core.receive(midi_bytes):
                return
        except Exception as e:
            self._uw_log("receive: %s" % e)
            return
        _Base.receive_midi(self, midi_bytes)

    def update_display(self):
        _Base.update_display(self)
        try:
            self._uw_core.tick()
        except Exception as e:
            self._uw_log("tick: %s" % e)
        if self._uw_chan is not None:
            try:
                self._uw_chan.poll()
                self._uw_editor.tick()
            except Exception as e:
                self._uw_log("plugin windows: %s" % e)

    def disconnect(self):
        self._uw_stopped = True
        self._uw_stop_fast()
        self._uw_core.shutdown()
        if self._uw_chan is not None:
            try:
                self._uw_chan.close()
            except Exception:
                pass
        self._uw_log("stopped.")
        _Base.disconnect(self)

    def suggest_input_port(self):
        return "Uppwind Link"

    def suggest_output_port(self):
        return "Uppwind Link Back"
