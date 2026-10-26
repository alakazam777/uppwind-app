# -*- coding: utf-8 -*-
# SPDX-License-Identifier: MIT
# Copyright (C) 2026 Uppwind
"""Uppwind Link -- les notes par le plugin « Uppwind Link Notes », sans port MIDI (sans Live : teste contre le faux
Live de tests/test_ableton.py).

Le chemin d'une note, sans loopMIDI :
    studio --(WebSocket)--> compagnon --(TCP 127.0.0.1)--> « Uppwind Link Notes » sur une piste d'aide
           --(MIDI From de la piste visee : <piste d'aide> / Uppwind Link Notes)--> l'instrument de la piste visee

Live fusionne les 16 canaux d'une entree « MIDI From » : une piste d'aide par piste visee. Elle s'appelle
« Uppwind Link · <nom de la piste visee> » et ne porte qu'un device, Uppwind Link Notes.

Qui est qui (le plugin ne sait pas sur quelle piste il est) : le plugin a un parametre « Link tag » (0..TAG_MAX,
0 = pas encore choisi). Le script choisit un numero libre pour chaque Notes du set et l'ecrit dans ce parametre (le
set le garde) ; le plugin l'annonce au compagnon (trame TAG). Le script le dit au studio (BIND_ACK.notes) ; le studio
envoie les notes de ce slot au compagnon « vers tag:<n> ». Une piste dupliquee porte le meme numero : le script le
voit (deux Notes, un numero) et en donne un neuf a la copie.

Ce module ne fait que lire et regler : creer la piste d'aide (Song.create_midi_track, puis le plugin par
Browser.load_item sur la piste choisie), son numero (DeviceParameter.value), l'entree de la piste visee
(input_routing_type / input_routing_channel, monitoring In). link_core.LinkCore decide quand.
"""

NOTES_NAME = "Uppwind Link Notes"
HELPER_PREFIX = u"Uppwind Link · "      # « Uppwind Link · <piste visee> »
TAG_PARAM = "Link tag"
TAG_MAX = 255
SEARCH_DEPTH = 7                             # Plug-Ins / VST3 / <editeur> / <plugin> : 4 suffisent, de la marge
SEARCH_NODES = 20000                         # un navigateur enorme : on s'arrete la
NOT_INSTALLED = ("The Uppwind Link Notes plugin is not in Live's browser: install the Uppwind Link plugins, then in "
                 "Live open Settings > Plug-Ins and press Rescan (or pick 'via loopMIDI' in Settings > Uppwind Link).")
NO_TAG = ("This Uppwind Link Notes has no 'Link tag' parameter: install the new Uppwind Link plugins, then rescan "
          "them in Live.")


def _low(s):
    return " ".join(str(s or "").split()).lower()


def _devices(track):
    try:
        return list(getattr(track, "devices", []) or [])
    except Exception:
        return []


def _params(dev):
    try:
        return list(getattr(dev, "parameters", []) or [])
    except Exception:
        return []


def tag_param(dev):
    for p in _params(dev):
        if _low(getattr(p, "name", "")) == _low(TAG_PARAM):
            return p
    return None


def is_notes_device(dev):
    """Un Uppwind Link Notes : son nom (Live donne au device le nom du plugin) ou son parametre « Link tag » (un
    device renomme a la main)."""
    if _low(NOTES_NAME) in _low(getattr(dev, "name", "")):
        return True
    return tag_param(dev) is not None


def notes_device(track):
    for d in _devices(track):
        if is_notes_device(d):
            return d
    return None


def is_helper(track):
    """Une piste qui porte un Uppwind Link Notes : une piste d'aide (le studio ne la liste pas)."""
    return notes_device(track) is not None


def untouched_helper(track):
    """Une piste d'aide telle que le script l'a faite, que l'on peut retirer sans rien perdre : son nom
    « Uppwind Link · … », rien d'autre que le Notes (ou rien, s'il ne s'est pas charge), aucun clip."""
    if not str(getattr(track, "name", "") or "").startswith(HELPER_PREFIX):
        return False
    if any(not is_notes_device(d) for d in _devices(track)):
        return False
    try:
        if any(getattr(s, "has_clip", False) for s in list(getattr(track, "clip_slots", []) or [])):
            return False
        if list(getattr(track, "arrangement_clips", []) or []):
            return False
    except Exception:
        return False
    return True


# -- Le numero (« Link tag ») ----------------------------------------------------------------------------------
def _int_text(s):
    s = str(s or "").strip()
    return int(s) if s.isdigit() else None


def read_tag(dev):
    """-> le numero (0 : pas encore choisi), ou None (pas de parametre « Link tag » : un plugin d'avant)."""
    p = tag_param(dev)
    if p is None:
        return None
    try:
        v = float(p.value)
    except Exception:
        return None
    # le texte que le plugin donne a sa valeur (un entier) : le plus sur, quelle que soit l'echelle de Live
    fmt = getattr(p, "str_for_value", None)
    if fmt is not None:
        try:
            n = _int_text(fmt(v))
            if n is not None and 0 <= n <= TAG_MAX:
                return n
        except Exception:
            pass
    lo, hi = float(getattr(p, "min", 0.0)), float(getattr(p, "max", 1.0))
    if hi <= lo:
        return 0
    if hi - lo >= TAG_MAX - 0.5:               # une echelle en pas entiers (0..255)
        n = int(round(v - lo))
    else:                                      # la valeur normalisee du VST3 (0..1)
        n = int(round((v - lo) / (hi - lo) * TAG_MAX))
    return max(0, min(TAG_MAX, n))


def write_tag(dev, n):
    """Le numero n dans « Link tag » -> True si on le relit."""
    p = tag_param(dev)
    if p is None:
        return False
    lo, hi = float(getattr(p, "min", 0.0)), float(getattr(p, "max", 1.0))
    if hi - lo >= TAG_MAX - 0.5:
        v = lo + n
    else:
        v = lo + (hi - lo) * float(n) / TAG_MAX
    p.value = max(lo, min(hi, v))
    return read_tag(dev) == n


def tags_in_use(tracks, skip=None):
    """{numero: [pistes]} de tous les Notes du set (skip : un device a ne pas compter)."""
    out = {}
    for t in tracks:
        for d in _devices(t):
            if d is skip or not is_notes_device(d):
                continue
            n = read_tag(d)
            if n:
                out.setdefault(n, []).append(t)
    return out


def free_tag(tracks, skip=None):
    used = tags_in_use(tracks, skip)
    for n in range(1, TAG_MAX + 1):
        if n not in used:
            return n
    return 0


# -- Le plugin dans le navigateur de Live ------------------------------------------------------------------------
def find_notes_item(browser):
    """L'element « Uppwind Link Notes » sous Browser.plugins (VST3 d'abord), ou None."""
    if browser is None:
        return None
    try:
        root = browser.plugins
    except Exception:
        return None
    seen = [0]
    found = []

    def walk(node, depth, path):
        for child in list(getattr(node, "children", []) or []):
            seen[0] += 1
            if seen[0] > SEARCH_NODES:
                return
            name = _low(getattr(child, "name", ""))
            loadable = getattr(child, "is_loadable", False)
            if loadable and name == _low(NOTES_NAME):
                found.append((path + [name], child))
                continue
            if depth < SEARCH_DEPTH and not loadable:
                walk(child, depth + 1, path + [name])

    try:
        walk(root, 1, [])
    except Exception:
        return None
    if not found:
        return None
    for path, item in found:
        if "vst3" in path:
            return item
    return found[0][1]


# -- Les entrees « MIDI From » --------------------------------------------------------------------------------
def helper_name(target_name):
    return (HELPER_PREFIX + str(target_name or "track"))[:120]


def _names_match(display, name):
    """Le nom qu'affiche le menu « MIDI From » pour une piste, et le nom de la piste : le meme, ou « 3-<nom> »."""
    d, n = _low(display), _low(name)
    if not n:
        return False
    if d == n:
        return True
    if d.endswith(n):
        head = d[:len(d) - len(n)].rstrip("- ").strip()
        return head.isdigit()
    return False


def routing_to(track, helper):
    """Le type d'entree de `track` qui designe la piste `helper`, ou None (Live ne l'offre pas encore)."""
    name = getattr(helper, "name", "")
    for rt in list(getattr(track, "available_input_routing_types", []) or []):
        if _names_match(getattr(rt, "display_name", ""), name):
            return rt
    return None


def notes_channel(track, dev=None):
    """Le canal d'entree « Uppwind Link Notes » (la sortie MIDI du plugin), une fois le type pose ; ou None."""
    want = [_low(NOTES_NAME)]
    if dev is not None and getattr(dev, "name", ""):
        want.append(_low(dev.name))
    for rc in list(getattr(track, "available_input_routing_channels", []) or []):
        d = _low(getattr(rc, "display_name", ""))
        if any(w and w in d for w in want):
            return rc
    return None


def points_to(track, helper):
    """L'entree de `track` est-elle deja « <helper> / Uppwind Link Notes » ?"""
    rt = getattr(track, "input_routing_type", None)
    rc = getattr(track, "input_routing_channel", None)
    if rt is None or not _names_match(getattr(rt, "display_name", ""), getattr(helper, "name", "")):
        return False
    return _low(NOTES_NAME) in _low(getattr(rc, "display_name", "")) or (
        notes_device(helper) is not None and _low(notes_device(helper).name) == _low(getattr(rc, "display_name", "")))


def source_of(track, tracks):
    """La piste d'aide que l'entree de `track` designe (son « MIDI From »), ou None."""
    rt = getattr(track, "input_routing_type", None)
    if rt is None:
        return None
    for t in tracks:
        if t is not track and is_helper(t) and _names_match(getattr(rt, "display_name", ""), getattr(t, "name", "")):
            return t
    return None


def describe(i, track, tracks):
    """Une ligne de ROUTES : ce que Live dit de l'entree d'une piste et de son Notes."""
    rt = getattr(track, "input_routing_type", None)
    rc = getattr(track, "input_routing_channel", None)
    out = {"index": i, "name": str(getattr(track, "name", ""))[:64],
           "input": str(getattr(rt, "display_name", "") if rt is not None else "")[:64],
           "channel": str(getattr(rc, "display_name", "") if rc is not None else "")[:64]}
    dev = notes_device(track)
    if dev is not None:
        out["notes"] = read_tag(dev)
    src = source_of(track, tracks)
    if src is not None:
        out["from"] = str(getattr(src, "name", ""))[:64]
        sd = notes_device(src)
        if sd is not None:
            out["fromNotes"] = read_tag(sd)
    return out
