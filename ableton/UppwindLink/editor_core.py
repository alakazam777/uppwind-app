# -*- coding: utf-8 -*-
# SPDX-License-Identifier: MIT
# Copyright (C) 2026 Uppwind
"""Uppwind Link -- les fenetres des plugins de Live pour le compagnon, sans Live (teste contre le faux Live de
tests/test_ableton.py).

Depuis le 24 oct., cette logique fait partie du script UppwindLink : un seul Control Surface. Le compagnon (companion/)
s'en sert pour ouvrir, refermer et lister les fenetres d'editeur des plugins (`device.is_editor_open`, Live 12.4.3+ ;
avant, chaque reponse le dit).

Les commandes (le canal : editor_channel.py ; l'outil : dev/uwlink_dev.py) :
  HELLO                          -> la version de Live, celle du script, ce que l'API offre
  LIST    [track]                -> les pistes et leurs devices, chacun avec son chemin « piste.device »
                                    (dans un Rack : « piste.device.chaine.device »)
  PROBE   [path]                 -> tout ce qu'on sait lire d'un device (ou de tous les plugins), note dans Log.txt
  EDITOR_OPEN  path [select]     -> device.is_editor_open = True (select : choisir d'abord la piste et le device)
  EDITOR_CLOSE path              -> device.is_editor_open = False
  EDITOR_STATE path              -> la valeur lue maintenant
  PRESET_SELECT path index       -> device.selected_preset_index = index (VST2 / AU)
  EVENTS                         -> ce que le script a vu changer tout seul (une fenetre fermee a la main...)

Rien ici n'envoie de MIDI ni ne cree, renomme ou supprime quoi que ce soit dans le projet. Seuls EDITOR_OPEN /
EDITOR_CLOSE (une fenetre), PRESET_SELECT (le programme d'un plugin VST2/AU) et `select` (la piste choisie) changent
quelque chose, et seulement quand on les demande.
"""

EDITOR_VERSION = "0.2.0"
DEV_VERSION = EDITOR_VERSION   # (le nom d'avant, pour le script de dev et ses outils)
PLUGIN_CLASSES = ("PluginDevice", "AuPluginDevice", "Vst3PluginDevice", "PluginDevice3")
MAX_DEPTH = 3               # Rack dans un Rack dans un Rack : on s'arrete la
MAX_PRESETS = 200           # dans une reponse (Log.txt les a toutes)
LOG_PRESETS_CHARS = 4000
WATCH_EVERY = 0.5           # s : les fenetres ouvertes par le script, relues
MAX_EVENTS = 50
TAG = "[Uppwind Link]"         # le script de dev passe le sien : "[Uppwind Link Dev]"


def _read(obj, attr):
    """-> (True, valeur) ou (False, "TypeErreur: message") ; jamais d'exception."""
    try:
        return True, getattr(obj, attr)
    except Exception as e:
        return False, "%s: %s" % (type(e).__name__, e)


def _has(obj, attr):
    """hasattr, sans laisser passer une autre erreur que AttributeError (le LOM peut lever RuntimeError) : la
    propriete existe alors, meme si on ne peut pas la lire maintenant."""
    try:
        getattr(obj, attr)
        return True
    except AttributeError:
        return False
    except Exception:
        return True


def _plain(v):
    """Une valeur du LOM -> du JSON simple (les vecteurs de Live deviennent des listes de chaines)."""
    if v is None or isinstance(v, (bool, int, float, str)):
        return v
    try:
        return [str(x) for x in list(v)]
    except Exception:
        return str(v)


def parse_path(p):
    """'2.0' / '2.0.1.0' / [2, 0] -> [2, 0] ; illisible -> None."""
    if isinstance(p, (list, tuple)):
        parts = list(p)
    elif isinstance(p, int) and not isinstance(p, bool):
        parts = [p]
    elif isinstance(p, str):
        parts = [x for x in p.strip().split(".")]
    else:
        return None
    out = []
    for x in parts:
        try:
            n = int(x)
        except Exception:
            return None
        if n < 0:
            return None
        out.append(n)
    if len(out) < 2 or len(out) % 2 != 0 or len(out) > 2 * MAX_DEPTH:
        return None
    return out


def path_str(parts):
    return ".".join(str(x) for x in parts)


def is_plugin(dev):
    cls = str(getattr(dev, "class_name", "") or "")
    return cls in PLUGIN_CLASSES or "plugin" in cls.lower()


class EditorCore(object):
    def __init__(self, song, log=None, now=None, version=None, tag=TAG):
        self._song = song                        # () -> Song
        self._log_raw = log or (lambda m: None)
        self._tag = tag
        self._now = now or (lambda: 0.0)
        self._version = version or (lambda: "")
        self.watch = []                          # [{"dev", "path", "name", "open"}] : les fenetres ouvertes par nous
        self.events = []
        self.last_watch = 0.0

    def log(self, msg):
        try:
            self._log_raw("%s %s" % (self._tag, msg))
        except Exception:
            pass

    # -- Trouver ------------------------------------------------------------------------------
    def tracks(self):
        try:
            return list(self._song().tracks)
        except Exception:
            return []

    def resolve(self, parts):
        """[piste, device, chaine, device...] -> (track, device) ; ValueError si le chemin ne mene a rien."""
        tracks = self.tracks()
        t = parts[0]
        if t >= len(tracks):
            raise ValueError("no track %d (the set has %d tracks; tracks count from 0)" % (t, len(tracks)))
        track = tracks[t]
        devices = list(getattr(track, "devices", []) or [])
        dev = None
        i = 1
        while i < len(parts):
            d = parts[i]
            if d >= len(devices):
                raise ValueError("no device %d at %s" % (d, path_str(parts[:i]) or "?"))
            dev = devices[d]
            i += 1
            if i < len(parts):
                c = parts[i]
                chains = list(getattr(dev, "chains", []) or [])
                if c >= len(chains):
                    raise ValueError("no chain %d in '%s'" % (c, getattr(dev, "name", "?")))
                devices = list(getattr(chains[c], "devices", []) or [])
                i += 1
        return track, dev

    def walk(self, only_track=None):
        """-> [(chemin, device, profondeur)] de tous les devices, Racks compris."""
        out = []

        def rec(devs, prefix, depth):
            for di, dev in enumerate(devs):
                p = prefix + [di]
                out.append((p, dev, depth))
                if depth + 1 < MAX_DEPTH and getattr(dev, "can_have_chains", False):
                    for ci, ch in enumerate(list(getattr(dev, "chains", []) or [])):
                        rec(list(getattr(ch, "devices", []) or []), p + [ci], depth + 1)

        for ti, t in enumerate(self.tracks()):
            if only_track is not None and ti != only_track:
                continue
            rec(list(getattr(t, "devices", []) or []), [ti], 0)
        return out

    # -- Lire un device -------------------------------------------------------------------------
    def describe(self, dev, parts):
        return {"path": path_str(parts), "name": str(getattr(dev, "name", "")), "class_name": str(getattr(dev, "class_name", "")),
                "plugin": is_plugin(dev), "editor": _has(dev, "is_editor_open")}

    def probe_one(self, dev, parts):
        info = self.describe(dev, parts)
        for attr in ("class_display_name", "type", "can_have_chains", "is_active"):
            ok, v = _read(dev, attr)
            info[attr] = _plain(v) if ok else None
        ok, v = _read(dev, "parameters")
        try:
            info["parameters"] = len(list(v)) if ok else None
        except Exception:
            info["parameters"] = None
        ok, v = _read(dev, "presets")
        if ok:
            names = _plain(v)
            if not isinstance(names, list):
                names = [str(names)]
            info["preset_count"] = len(names)
            info["presets"] = names[:MAX_PRESETS]
            all_names = names
        else:
            info["preset_count"] = None
            info["presets_error"] = v
            all_names = None
        ok, v = _read(dev, "selected_preset_index")
        info["selected_preset_index"] = _plain(v) if ok else None
        if not ok:
            info["selected_preset_index_error"] = v
        ok, v = _read(dev, "is_editor_open")
        info["is_editor_open"] = _plain(v) if ok else None
        if not ok:
            info["is_editor_open_error"] = v
        # Log.txt : une ligne par device, la liste entiere des presets (coupee a LOG_PRESETS_CHARS)
        if all_names is None:
            pres = "presets=<%s>" % info["presets_error"]
        else:
            listing = " | ".join(all_names)
            if len(listing) > LOG_PRESETS_CHARS:
                listing = listing[:LOG_PRESETS_CHARS] + " ...(cut)"
            pres = "presets=%d [%s]" % (len(all_names), listing)
        self.log("PROBE %s '%s' class=%s (%s) type=%s params=%s %s selected_preset_index=%s is_editor_open=%s" % (
            info["path"], info["name"], info["class_name"], info.get("class_display_name"), info.get("type"),
            info.get("parameters"), pres,
            info["selected_preset_index"] if "selected_preset_index_error" not in info else "<%s>" % info["selected_preset_index_error"],
            info["is_editor_open"] if "is_editor_open_error" not in info else "<%s>" % info["is_editor_open_error"]))
        return info

    # -- Les commandes ----------------------------------------------------------------------------
    def handle(self, req):
        cmd = str(req.get("cmd", "")).upper()[:32]
        fn = None
        if cmd and all(c.isalpha() or c == "_" for c in cmd):
            fn = getattr(self, "cmd_" + cmd.lower(), None)
        if fn is None:
            return {"ok": False, "error": "unknown command '%s'" % cmd}
        try:
            return fn(req)
        except ValueError as e:
            return {"ok": False, "error": str(e)}

    def _target(self, req):
        parts = parse_path(req.get("path"))
        if parts is None:
            raise ValueError("give a device path like 2.0 (track 2, device 0; both count from 0) or 2.0.1.0 inside a Rack")
        track, dev = self.resolve(parts)
        return parts, track, dev

    def cmd_hello(self, req):
        devs = self.walk()
        plugins = [d for (_p, d, _x) in devs if is_plugin(d)]
        rep = {"ok": True, "script": EDITOR_VERSION, "live": str(self._version() or ""), "tracks": len(self.tracks()),
               "plugins": len(plugins), "editor_api": any(_has(d, "is_editor_open") for d in plugins) if plugins else None,
               "events": len(self.events)}
        self.log("HELLO live=%s plugins=%d editor_api=%s" % (rep["live"], rep["plugins"], rep["editor_api"]))
        return rep

    def cmd_list(self, req):
        only = req.get("track")
        if only is not None:
            try:
                only = int(only)
            except Exception:
                raise ValueError("track must be a number")
        out = []
        tracks = self.tracks()
        for ti, t in enumerate(tracks):
            if only is not None and ti != only:
                continue
            out.append({"index": ti, "name": str(getattr(t, "name", "")), "devices": []})
        by = dict((x["index"], x) for x in out)
        for parts, dev, depth in self.walk(only):
            d = self.describe(dev, parts)
            d["depth"] = depth
            by[parts[0]]["devices"].append(d)
        return {"ok": True, "tracks": out}

    def cmd_probe(self, req):
        if req.get("path") is not None:
            parts, _t, dev = self._target(req)
            return {"ok": True, "devices": [self.probe_one(dev, parts)]}
        res = [self.probe_one(dev, parts) for (parts, dev, _d) in self.walk() if is_plugin(dev)]
        if not res:
            self.log("PROBE: no plugin device in this set")
        return {"ok": True, "devices": res}

    def _set_editor(self, req, want):
        parts, track, dev = self._target(req)
        name = str(getattr(dev, "name", ""))
        if not _has(dev, "is_editor_open"):
            msg = ("'%s' has no is_editor_open here (Live %s): the property needs Live 12.4.3 or later"
                   % (name, self._version() or "?"))
            self.log("EDITOR_%s %s: %s" % ("OPEN" if want else "CLOSE", path_str(parts), msg))
            return {"ok": False, "error": msg, "path": path_str(parts)}
        selected = False
        if want and req.get("select"):
            view = self._song().view
            view.selected_track = track
            sel = getattr(view, "select_device", None)
            if sel is not None:
                sel(dev)
            selected = True
        _ok, before = _read(dev, "is_editor_open")
        try:
            dev.is_editor_open = bool(want)
        except Exception as e:
            msg = "setting is_editor_open failed: %s: %s" % (type(e).__name__, e)
            self.log("EDITOR_%s %s '%s': %s" % ("OPEN" if want else "CLOSE", path_str(parts), name, msg))
            return {"ok": False, "error": msg, "path": path_str(parts), "selected": selected}
        _ok, after = _read(dev, "is_editor_open")
        self.log("EDITOR_%s %s '%s' select=%s before=%s after=%s" % ("OPEN" if want else "CLOSE", path_str(parts), name,
                                                                     selected, _plain(before), _plain(after)))
        self._watch(dev, parts, name, after)
        return {"ok": True, "path": path_str(parts), "device": name, "selected": selected,
                "before": _plain(before), "after": _plain(after)}

    def cmd_editor_open(self, req):
        return self._set_editor(req, True)

    def cmd_editor_close(self, req):
        return self._set_editor(req, False)

    def cmd_editor_state(self, req):
        parts, _t, dev = self._target(req)
        ok, v = _read(dev, "is_editor_open")
        rep = {"ok": ok, "path": path_str(parts), "device": str(getattr(dev, "name", ""))}
        if ok:
            rep["is_editor_open"] = _plain(v)
        else:
            rep["error"] = v
        self.log("EDITOR_STATE %s '%s' -> %s" % (rep["path"], rep["device"], rep.get("is_editor_open", rep.get("error"))))
        return rep

    def cmd_preset_select(self, req):
        parts, _t, dev = self._target(req)
        try:
            index = int(req.get("index"))
        except Exception:
            raise ValueError("index must be a number")
        name = str(getattr(dev, "name", ""))
        ok, before = _read(dev, "selected_preset_index")
        if not ok:
            self.log("PRESET_SELECT %s '%s': %s" % (path_str(parts), name, before))
            return {"ok": False, "error": before, "path": path_str(parts)}
        try:
            dev.selected_preset_index = index
        except Exception as e:
            msg = "%s: %s" % (type(e).__name__, e)
            self.log("PRESET_SELECT %s '%s' %d failed: %s" % (path_str(parts), name, index, msg))
            return {"ok": False, "error": msg, "path": path_str(parts)}
        _ok, after = _read(dev, "selected_preset_index")
        self.log("PRESET_SELECT %s '%s' %s -> %s" % (path_str(parts), name, _plain(before), _plain(after)))
        return {"ok": True, "path": path_str(parts), "device": name, "before": _plain(before), "after": _plain(after)}

    def cmd_events(self, req):
        return {"ok": True, "events": list(self.events)}

    # -- Ce qui change tout seul ----------------------------------------------------------------
    def _watch(self, dev, parts, name, value):
        for w in self.watch:
            if w["dev"] is dev:
                w["open"] = _plain(value)
                return
        self.watch.append({"dev": dev, "path": path_str(parts), "name": name, "open": _plain(value)})

    def _event(self, text):
        self.events.append({"t": round(float(self._now()), 2), "text": text})
        del self.events[:-MAX_EVENTS]
        self.log(text)

    def tick(self):
        now = self._now()
        if now - self.last_watch < WATCH_EVERY:
            return
        self.last_watch = now
        keep = []
        for w in self.watch:
            ok, v = _read(w["dev"], "is_editor_open")
            if not ok:
                self._event("%s '%s' is gone (%s)" % (w["path"], w["name"], v))
                continue
            v = _plain(v)
            if v != w["open"]:
                self._event("%s '%s' is_editor_open changed by itself: %s -> %s" % (w["path"], w["name"], w["open"], v))
                w["open"] = v
            keep.append(w)
        self.watch = keep


DevCore = EditorCore          # (le nom d'avant, pour le script de dev et ses tests)
