# -*- coding: utf-8 -*-
# SPDX-License-Identifier: MIT
# Copyright (C) 2026 Uppwind
"""Uppwind Link -- le canal local entre le compagnon et le script de Live.

Le transport le plus simple que l'etude propose (« canal local entre compagnon et script, comme AbletonOSC ») :
des datagrammes UDP en JSON, sur 127.0.0.1 SEULEMENT, lus sans jamais bloquer depuis update_display (toutes les
~100 ms, dans le fil de Live : on peut y toucher au Live Object Model).

Securite : on n'ecoute que la machine elle-meme ; chaque requete porte un jeton aleatoire que le
script ecrit au demarrage dans un fichier local (STATE_FILE, lu par le compagnon et par
dev/uwlink_dev.py) ; une requete sans le bon jeton, trop grosse, illisible ou venue d'ailleurs que
127.0.0.1 est ignoree sans reponse.

    requete  : {"token": "...", "id": 7, "cmd": "EDITOR_OPEN", "path": "2.0"}
    reponse  : {"id": 7, "ok": true, ...}   ou   {"id": 7, "ok": false, "error": "..."}

24 oct. : UN SEUL SERVEUR PAR LIVE. Le vrai script (UppwindLink, priorite 2) porte ce canal ; le script de dev de
l'etape 0 (priorite 1, le meme fichier a l'octet pres) peut rester choisi a cote le temps de la transition. Les deux
tournent dans le meme Python : ils se trouvent par un registre commun (un attribut de `sys`, REGISTRY). Le plus
prioritaire sert ; l'autre attend (« stands by ») sans socket, et reprend le canal si le premier s'arrete. Le fichier
d'etat garde son nom d'avant (STATE_NAME) : un compagnon d'avant le 24 oct. trouve le vrai script sans rien changer.
"""

import binascii
import errno
import hmac
import json
import os
import socket
import sys
import time

DEFAULT_PORT = 47391          # puis 47392... si le port est pris (le fichier d'etat dit lequel)
PORT_TRIES = 8
HOST = "127.0.0.1"
MAX_IN = 8192                 # octets : une requete est petite
MAX_OUT = 60000               # octets : sous la limite d'un datagramme UDP (65 507)
PER_POLL = 16                 # requetes traitees au plus par appel de poll (toutes les ~100 ms)
STATE_NAME = ".uppwind-link-dev.json"
ENV_FILE = "UWLINK_DEV_FILE"  # un autre chemin pour le fichier d'etat (les tests)
ENV_PORT = "UWLINK_CHANNEL_PORT"   # un autre premier port (les tests : 0, un port libre au hasard)
REGISTRY = "_uppwind_link_local_channel"
RETAKE_POLLS = 10             # en attente : toutes les 10 lectures (~1 s), le canal est-il libre ?


def state_file():
    p = os.environ.get(ENV_FILE)
    if p:
        return p
    return os.path.join(os.path.expanduser("~"), STATE_NAME)


def new_token():
    return binascii.hexlify(os.urandom(32)).decode("ascii")       # 256 bits


def shared_registry():
    """Le registre commun a toutes les copies de ce fichier dans le Python de Live : {"owner": canal | None}."""
    reg = getattr(sys, REGISTRY, None)
    if not isinstance(reg, dict):
        reg = {"owner": None}
        try:
            setattr(sys, REGISTRY, reg)
        except Exception:
            pass
    return reg


WOULD_BLOCK = (errno.EAGAIN, errno.EWOULDBLOCK, 10035)        # rien a lire (10035 : WSAEWOULDBLOCK)
RESET = (errno.ECONNRESET, 10054)   # Windows : le dernier destinataire n'ecoute plus (un outil deja parti)


def _code(e):
    c = getattr(e, "errno", None)
    if c is None and getattr(e, "args", None):
        c = e.args[0]
    return c


class LocalChannel(object):
    def __init__(self, handle, log=None, port=DEFAULT_PORT, path=None, info=None, priority=0, registry=None,
                 park=None, now=None):
        """handle(request_dict) -> reply_dict ; port=0 : un port libre au hasard (les tests) ;
        priority : le plus haut sert quand deux scripts portent ce canal ; registry : un dict a part (les tests).
        (26 oct.) park(request_dict) -> s : une requete qui peut attendre (le releve vide du compagnon) est gardee au
        plus ce temps, et repondue des que park dit 0 (le script a quelque chose a dire) ; une seule a la fois."""
        self._handle = handle
        self._park = park
        self._now = now or time.time
        self._parked = None           # (requete, adresse, jusqu'a quand)
        self._log = log or (lambda m: None)
        self.path = path or state_file()
        self.token = new_token()
        self.sock = None
        self.port = None
        env = os.environ.get(ENV_PORT, "")
        if port == DEFAULT_PORT and env.isdigit() and int(env) < 65536:
            port = int(env)
        self._want = int(port)
        self.info = dict(info or {})
        self.name = str(self.info.get("script") or "a Live script")
        self.priority = int(priority)
        self.rejected = 0
        self.standby = False          # un autre script sert : on attend, sans socket
        self.closed = False           # disconnect : plus jamais de reprise
        self._reg = registry if isinstance(registry, dict) else shared_registry()
        self._polls = 0

    @property
    def serving(self):
        return self.sock is not None

    # -- Ouvrir, fermer ----------------------------------------------------------------------
    def open(self):
        if self.closed:
            return False
        cur = self._reg.get("owner")
        if cur is not None and cur is not self and getattr(cur, "serving", False):
            if getattr(cur, "priority", 0) >= self.priority:
                if not self.standby:
                    self._log("%s already serves the local channel (udp://%s:%s): standing by"
                              % (getattr(cur, "name", "another script"), HOST, getattr(cur, "port", "?")))
                self.standby = True
                return False
            try:
                cur.step_aside(self)
            except Exception as e:
                self._log("cannot take the local channel over: %s" % e)
        last = None
        ports = [0] if self._want == 0 else range(self._want, self._want + PORT_TRIES)
        for p in ports:
            s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            try:
                s.bind((HOST, p))
            except Exception as e:
                last = e
                s.close()
                continue
            s.setblocking(False)
            self.sock = s
            self.port = s.getsockname()[1]
            break
        if self.sock is None:
            self._log("cannot listen on %s:%s (%s)" % (HOST, self._want, last))
            return False
        self.standby = False
        self._reg["owner"] = self
        self._write_state()
        self._log("listening on udp://%s:%d (token in %s)" % (HOST, self.port, self.path))
        return True

    def step_aside(self, other):
        """Un script plus prioritaire prend le canal : on ferme le socket et on attend (il reecrit le fichier d'etat)."""
        if self.sock is not None:
            try:
                self.sock.close()
            except Exception:
                pass
            self.sock = None
        self.standby = True
        self._log("%s takes the local channel over: standing by" % getattr(other, "name", "another script"))

    def _write_state(self):
        data = {"port": self.port, "token": self.token, "host": HOST, "started": int(time.time())}
        data.update(self.info)
        try:
            tmp = self.path + ".tmp"
            fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
            with os.fdopen(fd, "w") as f:
                json.dump(data, f)
            if os.path.exists(self.path):
                os.remove(self.path)
            os.rename(tmp, self.path)
        except Exception as e:
            self._log("cannot write %s: %s" % (self.path, e))

    def close(self):
        self.closed = True
        self.standby = False
        if self.sock is not None:
            try:
                self.sock.close()
            except Exception:
                pass
            self.sock = None
        if self._reg.get("owner") is self:
            self._reg["owner"] = None
        try:
            with open(self.path) as f:
                mine = json.load(f).get("token") == self.token
            if mine:
                os.remove(self.path)
        except Exception:
            pass

    # -- Lire, repondre ----------------------------------------------------------------------
    def poll(self):
        """A appeler souvent (update_display ; 26 oct. : et la lecture rapide d'UppwindLink.py) : traite toutes les
        requetes en attente, sans bloquer, puis la requete gardee si son heure est venue. -> nombre traite.
        En attente (un autre script servait) : reprend le canal des qu'il est libre."""
        n = self._read()
        self._wake()
        return n

    def _read(self):
        if self.sock is None:
            if self.standby and not self.closed:
                self._polls += 1
                if self._polls % RETAKE_POLLS == 0:
                    cur = self._reg.get("owner")
                    if cur is None or cur is self or not getattr(cur, "serving", False):
                        self.open()
            return 0
        n = 0
        while n < PER_POLL:
            try:
                raw, addr = self.sock.recvfrom(MAX_IN + 1)
            except Exception as e:
                c = _code(e)
                if c in RESET:
                    n += 1
                    continue
                if c not in WOULD_BLOCK:
                    self._log("recv: %s" % e)
                return n
            n += 1
            reply = self.accept(raw, addr)
            if reply is not None:
                self.send(reply, addr)
        return n

    def accept(self, raw, addr):
        """Un datagramme recu -> la reponse a envoyer, ou None (ignore sans repondre)."""
        if not addr or addr[0] != HOST or len(raw) > MAX_IN:
            self.rejected += 1
            return None
        try:
            req = json.loads(raw.decode("utf-8"))
        except Exception:
            self.rejected += 1
            return None
        if not isinstance(req, dict):
            self.rejected += 1
            return None
        tok = req.get("token")
        if not isinstance(tok, str) or not hmac.compare_digest(tok.encode("utf-8"), self.token.encode("ascii")):
            self.rejected += 1
            return None
        hold = self._hold(req)
        if hold > 0:
            old = self._parked
            self._parked = (req, addr, self._now() + hold)
            if old is not None:
                self.send(self._reply(old[0]), old[1])     # la plus ancienne repart (vide) : une seule gardee
            return None
        return self._reply(req)

    def _hold(self, req):
        if self._park is None:
            return 0
        try:
            return max(0.0, float(self._park(req) or 0))
        except Exception:
            return 0

    def _wake(self):
        """La requete gardee repart quand le script a quelque chose a dire, ou a son heure."""
        p = self._parked
        if p is None:
            return
        if self.sock is None:
            self._parked = None
            return
        if self._hold(p[0]) > 0 and self._now() < p[2]:
            return
        self._parked = None
        self.send(self._reply(p[0]), p[1])

    def _reply(self, req):
        rid = req.get("id")
        try:
            rep = self._handle(req)
        except Exception as e:
            rep = {"ok": False, "error": "%s: %s" % (type(e).__name__, e)}
        rep = dict(rep or {})
        rep["id"] = rid
        return rep

    def send(self, reply, addr):
        try:
            out = json.dumps(reply).encode("utf-8")
        except Exception as e:
            out = json.dumps({"id": reply.get("id"), "ok": False, "error": "cannot encode the reply: %s" % e}).encode("utf-8")
        if len(out) > MAX_OUT:
            out = json.dumps({"id": reply.get("id"), "ok": False,
                              "error": "reply too large (%d bytes): ask for one track or one device" % len(out)}).encode("utf-8")
        try:
            self.sock.sendto(out, addr)
        except Exception as e:
            self._log("send: %s" % e)


DevChannel = LocalChannel     # (le nom d'avant, pour le script de dev et ses tests)
