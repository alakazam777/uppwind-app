# name=Uppwind Link
# supportedDevices=Uppwind Link,IAC Driver Uppwind Link
# SPDX-License-Identifier: MIT
# Copyright (C) 2026 Uppwind
"""Uppwind Link -- le script de controleur FL Studio.

FL Studio devient l'hote de plugins sans tete d'UPPWIND STUDIO : le studio joue les instruments
du rack (vos VST) par MIDI, regle leur volume, leur pan, leur mute ; l'audio revient au studio par
un cable audio virtuel (la sortie audio de FL). Rien ne s'entend deux fois.

Le dossier « Uppwind Link » (ce fichier + uwlink_protocol.py) va dans
    Documents/Image-Line/FL Studio/Settings/Hardware/
puis, dans FL (Options > MIDI settings) :
    Input  « Uppwind Link »       : Enable, Controller type = Uppwind Link, Port = un numero libre (ex. 77)
    Output « Uppwind Link Back »  : Port = le MEME numero (c'est ainsi que device.midiOutSysex trouve le retour)
Voir README.md (meme dossier) pour la mise en place complete (loopMIDI, VB-CABLE).

Ce que fait le script (le protocole : uwlink_protocol.py, a cote) :
  HELLO -> HELLO_ACK  (FL Studio, sa version, le projet, caps)
  LIST  -> LIST_REPLY (le rack : position globale, nom, plugin, couleur ; pagine)
  CHANGED pousse quand le rack change (renommage, ajout, retrait, couleur), apres 250 ms de calme
  BIND / UNBIND : le canal MIDI n (slot n) joue le canal du rack choisi ; le lien SUIT l'instrument
                  quand on deplace ou insere des canaux, et tombe si on le supprime
  SET   : volume (gain lineaire -> loi de volume des canaux de FL), pan, mute ; solo = fait taire
          les AUTRES instruments relies (jamais le reste du projet)
  PING  -> PONG
  TEMPO -> le tempo du projet (general.processRECEvent(midi.REC_Tempo, bpm x 1000, REC_Control |
          REC_UpdateControl), comme le script Novation d'Image-Line) ; jamais renvoye au studio
  TEMPO_CHANGED pousse quand le tempo change dans FL (relu toutes les 50 ms dans OnIdle : FL n'a pas
          de rappel pour le tempo), seulement a un studio qui a dit 'tempo' dans son HELLO
Ce que l'API de script de FL ne permet PAS : charger un nouveau plugin, lister les plugins installes,
supprimer un canal, choisir les ports MIDI d'un canal (getChannelMidiInPort se lit, rien ne l'ecrit).
caps ne contient donc ni 'plugins', ni 'add', ni 'remove' : le studio dit « Add the plugin in FL's
channel rack; it appears here instantly ».

(19 nov., 1.4.0) Les PARAMETRES d'un plugin (cap 'params') : « Link to controller » sur la fenetre du plugin dans le
studio. PARAMS {id, start?} -> PARAMS_REPLY : les parametres NOMMES du plugin du canal id (un VST en annonce 4240 :
4096 normaux, la plupart sans nom, puis ses « MIDI CC » ; seuls les 4096 premiers comptent), pagines. PARAM_SET
{id, p, v} : plugins.setParamValue sans reprise (le studio fait la sienne). PARAM_WATCH {id} : le canal suivi (un a
la fois ; sans id : aucun) ; ses valeurs sont relues quand FL dit qu'une valeur de plugin a change (OnRefresh,
HW_Dirty_ControlValues = 4096, mesure le 19 nov. : il part a chaque bouton tourne dans la fenetre du plugin), au plus
toutes les PARAM_EVERY, et au moins toutes les PARAM_POLL ; ce qui a change part en PARAM_CHANGED, avec la barre
d'aide de FL (ui.getHintMsg : « FX Delay Dry / Wet: 0.38 », le dernier reglage touche). Mesure sur TAL-U-NO-LX : 74
parametres nommes, leurs valeurs relues en 0,03 ms ; un instrument de FL (FLEX) : 44 ms, d'ou la relecture seulement
quand FL le dit. Un reglage pose par PARAM_SET n'est pas renvoye (son echo est ignore).

(19 nov., 1.3.0) EDITOR {id, open} (cap 'editor') : la fenetre de l'instrument s'ouvre dans FL, a la demande du
studio (Follow track, quand le compagnon ne la trouve pas). channels.showEditor(index, 1), pose au prochain OnIdle
(pas dans le traitement du SysEx) ; la reponse EDITOR_ACK part alors. FERMER est refuse (unsupported) : mesure du
19 nov. sur FL 2026 (API 45), showEditor(index, 0) sur une fenetre detachee a bloque FL (« Not responding », a tuer). Une fenetre que l'on a
detachee une fois (son menu, « Detached ») se rouvre detachee : FL s'en souvient.

(27 oct., 1.2.1) Nos propres reponses revenues sur l'entree : le Script output dit pourquoi (la sortie « Uppwind Link »
a le numero de l'entree) et comment reparer.

(26 oct., 1.2.0) Les notes par le plugin « Uppwind Link Notes » (cap 'notes'), le son par « Uppwind Link
Return », les commandes toujours ici (FL refuse les sockets aux scripts, et un script ne vit que sur une
entree MIDI : loopMIDI reste pour ces commandes). Le chemin d'une note :
    studio -> compagnon -> Uppwind Link Notes (un canal du rack) --(son port de SORTIE MIDI, numero P)-->
    l'instrument dont le port d'ENTREE MIDI est P
Le script ne peut ni poser le plugin ni regler ces ports ; il les LIT. La regle, faite une fois a la main :
le canal Uppwind Link Notes a ses ports d'entree ET de sortie sur P (le script ne lit que l'entree),
l'instrument vise son port d'entree sur P. Le script donne alors au Notes le numero « Link tag » P (comme
le script de Live) ; BIND.via 'plugin' -> BIND_ACK.notes = P : le studio envoie a ce Notes. Sans Notes sur
le port de l'instrument (ou sans port) : BIND_ACK.via 'port', le script joue les notes recues comme avant.
Les canaux Uppwind Link Notes ne sont pas listes (ce ne sont pas des instruments).

Notes : channels.midiNoteOn(index global, note, velocite) -- 0 = note off (comme les scripts
Novation d'Image-Line). Pedale (CC64) : emulee ici (les note off attendent le relachement), pour
TOUS les instruments. CC 1/7/10/11 : envoyes aux VST par leurs parametres « MIDI CC » (4096 + n,
voir plugins.getParamCount) ; les instruments natifs de FL ne les ont pas. Pitch bend : le bouton
de hauteur du canal (channels.setChannelPitch, en fraction de la plage). CC 120/123 : tout relacher.

Bac a sable de FL : ni fichiers ni reseau ; json, math, time seulement (cf. uwlink_protocol.py).

(24 nov., 1.5.0, audit M3) LE CHEMIN MIDI EST AUTHENTIFIE (cap 'auth', voir uwlink_protocol.py) : tout programme peut
ecrire sur le port loopMIDI, et une page web a qui l'on a donne « MIDI avec SysEx » aussi. Le script ne fait donc rien
pour un studio qui ne prouve pas le secret :
  - le secret vient de uwl_secret.py (a cote de ce fichier, ecrit par le compagnon quand il installe le script : le
    studio appaire au compagnon le recoit de lui, rien a faire) ; sans lui, le script en tire un au lancement de FL et
    affiche son CODE au Script output (et dans la barre d'aide de FL quand un studio le demande) : on le colle dans
    Uppwind Studio. Sans uwl_secret.py, ce code change a chaque lancement de FL (le script ne peut rien ecrire) ;
  - HELLO ne coupe plus la session en cours : il donne un nonce (HELLO_ACK.auth) ; seul un AUTH juste ouvre la
    session (ce que faisait HELLO : liens defaits, reglages rendus) ;
  - ensuite chaque commande est signee (P.verify_signed) ; une commande sans signature juste est refusee (ERROR auth).
Les notes et les CC du port ne sont pas des SysEx : ils ne portent pas de preuve (le studio les envoie aux slots lies).
"""

import math
import time

import channels
import device
import general
import midi
import mixer
import plugins
import ui

import uwlink_protocol as P

SCRIPT_VERSION = "1.5.0"  # (24 nov.) 1.5.0 : le chemin MIDI authentifie ; 1.4.1 : PARAM_SET lie ou suivi, file bornee
TAG = "[Uppwind Link]"
DAW_NAME = "FL Studio"
CAPS = ["list", "bind", "set", "audition", "tempo", "notes", "editor", "params", "auth"]
ADD_HINT = "Add the plugin in FL's channel rack; it appears here instantly."
# (26 oct.) le plugin des notes, son parametre « Link tag » (0..255, 0 : aucun), les ports MIDI de FL
NOTES_NAME = "Uppwind Link Notes"
TAG_PARAM = "Link tag"
TAG_MAX = 255
PARAM_SCAN = 64             # le « Link tag » est parmi les premiers parametres (les noms vides au-dela)

LIST_BUDGET = 20000         # octets de JSON par page de LIST_REPLY (100 morceaux, sous 25 600)
CHANGE_SETTLE = 0.25        # s de calme avant de pousser CHANGED
MUTE_RETRY = 0.12           # s entre deux bascules de mute d'un meme canal (FL les regroupe par tick)
MUTE_CHECK = 0.06           # s entre deux verifications des mutes
OUT_PER_TICK = 8            # morceaux SysEx envoyes par appel (le reste au prochain OnIdle)
# (24 nov., audit F3 des scripts) la file de sortie est bornee : une rafale de LIST (100 morceaux par reponse) la faisait
# grossir sans fin. Au-dela, le message entier est jete (jamais un morceau sur deux : le studio ne saurait pas le
# recoller). 512 : quatre pages de LIST d'avance, et un message de MAX_CHUNKS (128) passe toujours dans une file vide.
OUT_MAX = 512
OUT_WARN_EVERY = 10.0       # s entre deux lignes « file pleine » au Script output
# (24 nov., 1.5.0) l'authentification : le code redit au plus toutes les CODE_EVERY ; les refus au journal au plus une
# ligne toutes les AUTH_LOG_EVERY (une rafale ne noie pas le Script output)
CODE_EVERY = 20.0
AUTH_LOG_EVERY = 10.0
# (24 nov., relecture) un AUTH juste sur une session deja ouverte ne remet plus le DAW a zero : les liens restent, et
# ceux que le studio ne reprend pas (BIND) dans STALE_AFTER s sont defaits (un onglet ferme, un autre projet)
STALE_AFTER = 10.0
NO_SECRET = ("This FL Studio Python has no safe random source: install the Uppwind Link script with the Uppwind Link "
             "companion, which gives it its code.")
WHERE_CODE = "View > Script output"
STRUCT_CHECK = 2.0          # s : filet de securite (le nombre de canaux a-t-il change sans qu'on le dise ?)
VST_CC_BASE = 4096          # parametres des VST : 4096 normaux, puis 128 « MIDI CC », puis 16 aftertouch
PARAMS_PAGE = 100           # parametres par PARAMS_REPLY (~ 50 octets chacun)
PARAM_EVERY = 0.04          # s : au plus un PARAM_CHANGED toutes les 40 ms
PARAM_POLL = 0.5            # s : relue meme sans OnRefresh (une automation ne le dit pas toujours)
PARAM_EPS = 0.0005          # un changement plus petit ne compte pas (l'arrondi du protocole est 1e-4)
HW_DIRTY_CONTROL_VALUES = getattr(midi, "HW_Dirty_ControlValues", 4096)
TEMPO_CHECK = P.TEMPO_MIN_INTERVAL_MS / 1000.0   # s entre deux lectures du tempo de FL (et deux TEMPO_CHANGED)
FL_TEMPO_MIN = 10.0         # le tempo de FL va de 10 a 522 BPM
FL_TEMPO_MAX = 522.0

# Types de canaux (les CT_ de midi.py) : on ne joue ni les clips audio ni les clips d'automation
CT_SAMPLER = getattr(midi, "CT_Sampler", 0)
CT_HYBRID = getattr(midi, "CT_Hybrid", 1)
CT_GENPLUG = getattr(midi, "CT_GenPlug", 2)
CT_LAYER = getattr(midi, "CT_Layer", 3)
CT_AUDIOCLIP = getattr(midi, "CT_AudioClip", 4)
CT_AUTOCLIP = getattr(midi, "CT_AutoClip", 5)
KIND = {CT_SAMPLER: "sampler", CT_HYBRID: "hybrid", CT_GENPLUG: "plugin", CT_LAYER: "layer"}
TYPE_LABEL = {CT_SAMPLER: "Sampler", CT_HYBRID: "Sampler", CT_GENPLUG: "Plugin", CT_LAYER: "Layer"}
PIM_NONE = getattr(midi, "PIM_None", 0)

# Ce qui, dans OnRefresh, peut changer la liste (noms, couleurs, rack)
REFRESH_FLAGS = (getattr(midi, "HW_Dirty_Names", 16384) | getattr(midi, "HW_Dirty_Colors", 8192)
                 | getattr(midi, "HW_Dirty_ChannelRack", 262144)
                 | getattr(midi, "HW_Dirty_ChannelRackGroup", 32768))
CE_SELECT = getattr(midi, "CE_Select", 4)
PL_START = getattr(midi, "PL_Start", 0)
PL_LOAD_OK = getattr(midi, "PL_LoadOk", 100)
PL_LOAD_ERROR = getattr(midi, "PL_LoadError", 101)
# Le tempo : l'evenement global REC_Tempo (BPM x 1000) ; les drapeaux du script Novation d'Image-Line
# (Novation/script/fl.py, set_tempo) : la valeur, son controle, une etape d'annulation de FL
REC_TEMPO = getattr(midi, "REC_Tempo", 0x4000 * 0x10000 + 5)
REC_TEMPO_FLAGS = getattr(midi, "REC_Control", 973) | getattr(midi, "REC_UpdateControl", 16)


def _now():
    return time.time()


# -- Etat ---------------------------------------------------------------------------------
class _State(object):
    def __init__(self):
        self.api = 0
        self.g_chan = False          # useGlobalIndex accepte par channels.* (API >= 33)
        self.g_plug = False          # ... par plugins.* (API >= 26)
        self.send = None             # device.midiOutSysex
        self.rx = P.Reassembler()
        self.session = False         # un HELLO recu
        self.last_rx = 0.0
        self.binds = {}              # slot -> {"index", "sig"}
        self.held = {}               # (slot, note) -> index qui a recu le note on
        self.pedal = {}              # slot -> pedale enfoncee
        self.sustained = set()       # (slot, note) : note off retenus par la pedale
        self.bent = {}               # index -> slot qui a tordu la hauteur
        self.mix = {}                # index -> {"vol", "pan", "mute", "solo"} demandes par le studio
        self.orig = {}               # index -> (vol, pan, mute) d'avant le studio
        self.mute_want = {}          # index -> [mute voulu, permanent]
        self.toggled_at = {}         # index -> instant de la derniere bascule
        self.cc_params = {}          # index -> nombre de parametres du plugin (cache)
        self.out = []                # morceaux SysEx en attente
        self.dirty = False
        self.dirty_at = 0.0
        self.count = -1
        self.list_sig = None
        self.last_mute_check = 0.0
        self.last_struct_check = 0.0
        self.last_sx = (b"", 0.0)
        self.warned_out = False
        self.out_dropped = 0         # (24 nov.) messages jetes, file pleine (dits au plus toutes les OUT_WARN_EVERY)
        self.out_warned_at = -1e9
        self.vol_checked = False
        # le tempo : ce que le studio sait faire (HELLO.caps), le tempo de FL tel que le studio le sait,
        # jusqu'a quand le tempo pose par le studio n'est pas un changement de FL, le dernier envoi
        self.studio_caps = []
        self.tempo_known = None
        self.tempo_hold_until = 0.0
        self.tempo_sent_at = -1e9
        self.last_tempo_check = 0.0
        # (26 oct.) les notes par Uppwind Link Notes : index d'un Notes -> index de son « Link tag » (-1 : aucun) ;
        # les numeros annonces (index -> tag) ; ce qui a ete dit au journal (pas deux fois la meme ligne)
        self.tag_param = {}
        self.tags = {}
        self.said = {}
        # (19 nov., 1.3.0) EDITOR en attente du prochain OnIdle : (req, index, open) ; le dernier demande gagne
        self.editor = None
        # (19 nov., 1.4.0) les parametres : index -> [(p, nom)] (cache, oublie quand le rack change) ; le canal suivi,
        # ses dernieres valeurs (p -> v), FL a-t-il dit qu'une valeur a change, la derniere lecture
        self.pnames = {}
        self.pw = None
        self.pw_last = {}
        self.pw_dirty = False
        self.pw_at = 0.0
        self.pw_sig = None
        # (24 nov., 1.5.0) l'authentification : le secret (15 octets), d'ou il vient ('companion' : uwl_secret.py ;
        # 'session' : tire au lancement), son kid ; l'offre en cours (HELLO -> {n, t, caps}) ; la cle de la session et le
        # dernier compteur accepte ; quand le code a ete dit, quand un refus a ete dit (et combien depuis)
        self.secret = b""
        self.secret_from = ""
        self.kid = ""
        self.offer = None
        self.key = None
        self.ctr = 0
        self.code_said_at = -1e9
        self.refused_at = -1e9
        self.refused = 0
        self.error_said_at = -1e9
        # (24 nov., relecture) le code ecrit en entier au Script output une fois par lancement (et seulement le code
        # de ce lancement) ; les slots d'avant un nouvel AUTH -> l'heure ou ils tombent s'ils ne sont pas repris ;
        # le message en cours etait-il signe par la session
        self.code_logged = False
        self.stale = {}
        self.rx_signed = False


S = _State()


def _log(msg):
    try:
        print("%s %s" % (TAG, msg))
    except Exception:
        pass


# -- Index globaux (API 33+) ou relatifs au groupe affiche (avant) --------------------------
def _g(*args):
    return args + (True,) if S.g_chan else args


def _pg(*args):
    return args + (True,) if S.g_plug else args


def _count_all():
    try:
        return int(channels.channelCount(True))
    except TypeError:
        return int(channels.channelCount())


def _exists(i):
    return isinstance(i, int) and 0 <= i < _count_all()


def _type(i):
    try:
        return int(channels.getChannelType(*_g(i)))
    except Exception:
        return CT_GENPLUG


def _playable(i):
    return _exists(i) and _type(i) not in (CT_AUDIOCLIP, CT_AUTOCLIP)


def _name(i):
    try:
        return str(channels.getChannelName(*_g(i)) or "")
    except Exception:
        return ""


def _plugin_valid(i):
    try:
        return bool(plugins.isValid(*_pg(i, -1)))
    except Exception:
        return False


def _plugin_name(i, ctype=None):
    t = _type(i) if ctype is None else ctype
    if _plugin_valid(i):
        try:
            n = plugins.getPluginName(*_pg(i, -1, False))
            if n:
                return str(n)
        except Exception:
            pass
    return TYPE_LABEL.get(t, "")


def _color(i):
    # 0x--RRGGBB, comme utils.ColorToRGB / ColorToHex d'Image-Line
    try:
        return "#%06x" % (int(channels.getChannelColor(*_g(i))) & 0xFFFFFF)
    except Exception:
        return None


def _sig(i):
    return (_name(i), _plugin_name(i))


def _item(i):
    t = _type(i)
    if t in (CT_AUDIOCLIP, CT_AUTOCLIP):
        return None
    if _is_notes(i):
        return None                 # (26 oct.) un Uppwind Link Notes n'est pas un instrument
    it = {"id": i, "name": _name(i)[:64], "plugin": _plugin_name(i, t)[:64], "kind": KIND.get(t, "plugin")}
    c = _color(i)
    if c:
        it["color"] = c
    slots = sorted(s for s, b in S.binds.items() if b["index"] == i)
    if slots:
        it["slots"] = slots
    return it


# -- (26 oct.) Les notes par Uppwind Link Notes ---------------------------------------------
def _midi_port(i):
    """Le port d'entree MIDI du canal (son wrapper : Settings > MIDI > Input port), ou -1 (aucun, clavier...)."""
    try:
        p = int(channels.getChannelMidiInPort(*_g(i)))
    except Exception:
        return -1
    return p if p >= 0 else -1


def _is_notes(i):
    """Un canal qui porte Uppwind Link Notes (le nom du plugin, pas celui du canal : renomme, il le reste)."""
    if not _plugin_valid(i):
        return False
    try:
        n = str(plugins.getPluginName(*_pg(i, -1, False)) or "")
    except Exception:
        return False
    return NOTES_NAME.lower() in n.lower()


def _notes_channels():
    return [j for j in range(_count_all()) if _type(j) == CT_GENPLUG and _is_notes(j)]


def _tag_param(j):
    """L'index du parametre « Link tag » du Notes j, ou -1 (un plugin d'avant le 25 oct.)."""
    if j not in S.tag_param:
        k = -1
        try:
            n = min(int(plugins.getParamCount(*_pg(j, -1))), PARAM_SCAN)
            for p in range(n):
                if str(plugins.getParamName(*((p,) + _pg(j, -1)))).strip().lower() == TAG_PARAM.lower():
                    k = p
                    break
        except Exception:
            k = -1
        S.tag_param[j] = k
    return S.tag_param[j]


def _read_tag(j):
    p = _tag_param(j)
    if p < 0:
        return None
    try:
        v = float(plugins.getParamValue(*((p,) + _pg(j, -1))))
    except Exception:
        return None
    return max(0, min(TAG_MAX, int(round(v * TAG_MAX))))


def _write_tag(j, n):
    """Le numero n dans « Link tag » (la valeur normalisee d'un VST, 0..1) -> True si on le relit."""
    p = _tag_param(j)
    if p < 0:
        return False
    try:
        plugins.setParamValue(*((float(n) / TAG_MAX, p) + _pg(j, -1, PIM_NONE)))
    except Exception as e:
        _log("cannot set the Link tag of '%s': %s" % (_name(j), e))
        return False
    return _read_tag(j) == n


def _say(key, msg):
    """Une ligne au journal (Script output), pas deux fois de suite la meme."""
    if S.said.get(key) != msg:
        S.said[key] = msg
        _log(msg)


def _sync_tags():
    """Chaque Uppwind Link Notes prend pour « Link tag » son port MIDI (1..255 ; 0 sans port) : le studio le vise par
    ce numero. -> {port: [index des Notes]}"""
    by_port = {}
    for j in _notes_channels():
        port = _midi_port(j)
        want = port if 1 <= port <= TAG_MAX else 0
        have = _read_tag(j)
        if have is None:
            _say(("old", j), "'%s' is an older Uppwind Link Notes (no Link tag): install the new Uppwind Link plugins."
                 % _name(j))
            continue
        if have != want:
            if _write_tag(j, want):
                _say(("tag", j), "'%s': Link tag %d (MIDI port %s)." % (_name(j), want, port if port >= 0 else "none"))
            else:
                continue
        S.tags[j] = want
        if want:
            by_port.setdefault(want, []).append(j)
    for port, js in by_port.items():
        if len(js) > 1:
            _say(("twins", port), "%d Uppwind Link Notes share MIDI port %d (%s): one per port, or the studio picks "
                 "the first." % (len(js), port, ", ".join("'%s'" % _name(j) for j in js)))
    return by_port


def _notes_route(i, by_port=None):
    """(tag, pourquoi) : le Notes qui joue le canal i (son port de sortie = le port d'entree de i), ou (0, la raison).
    by_port : ce que _sync_tags vient de rendre (sinon il est refait)."""
    port = _midi_port(i)
    if not 1 <= port <= TAG_MAX:
        return 0, ("'%s' has no MIDI input port: in its wrapper, Settings > MIDI > Input port, pick a free number "
                   "(1 to 255), and give an Uppwind Link Notes the same Input and Output port." % _name(i))
    if by_port is None:
        by_port = _sync_tags()
    if port not in by_port:
        return 0, ("no Uppwind Link Notes on MIDI port %d (the Input port of '%s'): give one the same Input and "
                   "Output port %d." % (port, _name(i), port))
    return port, ""


# -- Volume : la loi des canaux de FL -----------------------------------------------------
# utils.VolTodB (Shared/Python/Lib/utils.py) : gain = (11^v - 1) / 10 ; v = 1 -> 0 dB, la valeur
# par defaut midi.ChannelDefaultVolume (0.78125) -> -5.2 dB. Le studio envoie un gain lineaire.
def gain_to_fl_volume(gain):
    g = max(0.0, min(1.0, float(gain)))
    return math.log(10.0 * g + 1.0) / math.log(11.0)


def fl_volume_to_gain(v):
    v = max(0.0, min(1.0, float(v)))
    return (11.0 ** v - 1.0) / 10.0


# -- (24 nov., 1.5.0) L'authentification -------------------------------------------------------
def _load_secret():
    """uwl_secret.py (le compagnon l'ecrit a cote du script) ; sinon un secret neuf, pour ce lancement de FL."""
    try:
        import uwl_secret
        s = P.secret_of(getattr(uwl_secret, "SECRET", ""))
        if s:
            return s, "companion"
        _log("uwl_secret.py holds no valid code: using a code for this FL session only.")
    except ImportError:
        pass
    except Exception as e:
        _log("uwl_secret.py: %s" % e)
    s = P.random_bytes(P.SECRET_BYTES, ("%r" % time.time()).encode("ascii"))
    if P.random_is_weak():
        # (24 nov., relecture) un code tire de l'horloge se devine hors ligne (le kid le verifie) : pas de lien MIDI
        return b"", "weak"
    return s, "session"


def _say_code(now, force=False):
    """Le code de CE lancement (sans le compagnon) : la barre d'aide, au plus toutes les CODE_EVERY ; en entier au
    Script output une fois par lancement. Celui du compagnon ne s'affiche jamais : le compagnon le donne au studio."""
    if S.secret_from != "session":
        return
    if not force and now - S.code_said_at < CODE_EVERY:
        return
    S.code_said_at = now
    code = P.code_of(S.secret)
    if not S.code_logged:
        S.code_logged = True
        _log("MIDI link code for this FL session: %s -- paste it in Uppwind Studio (Settings > Uppwind Link, step 4). "
             "It changes each time FL starts; the Uppwind Link companion installs a script that keeps one." % code)
    try:
        ui.setHintMsg("Uppwind Link code: %s" % code)
    except Exception:
        pass


def _refuse(req, msg, why):
    """Une commande sans preuve juste : ERROR auth (la file est bornee), le journal au plus toutes les AUTH_LOG_EVERY."""
    S.refused += 1
    now = _now()
    if now - S.refused_at >= AUTH_LOG_EVERY:
        S.refused_at = now
        _log("refused %d unproven message(s) on the MIDI port (%s). Only Uppwind Studio with this script's code is "
             "obeyed." % (S.refused, why))
        S.refused = 0
    if req:
        _error(req, P.ERR_AUTH, msg)


def _hello(req, d):
    """HELLO : un nonce, rien d'autre. La session en cours (s'il y en a une) continue."""
    now = _now()
    if len(S.secret) != P.SECRET_BYTES:
        _refuse(req, NO_SECRET, "no code")
        return
    n = P.new_nonce(S.secret)
    S.offer = {"n": n, "t": now, "caps": list(d.get("caps", []))}
    ack = {"v": P.LINK_VERSION, "daw": DAW_NAME, "dawVersion": _daw_version(), "caps": list(CAPS),
           "auth": {"kid": S.kid, "n": n}}
    _send(P.HELLO_ACK, req, ack)
    if "auth" not in S.offer["caps"]:
        _say(("old-studio",), "this Uppwind Studio does not know the MIDI link code yet: reload its page.")
    else:
        _say_code(now)          # (24 nov., relecture) seulement a un studio qui sait le demander


def _auth(req, d):
    """AUTH juste : la session s'ouvre (ce que faisait HELLO avant le 24 nov.) ; sinon ERROR auth."""
    now = _now()
    o, S.offer = S.offer, None
    if len(S.secret) != P.SECRET_BYTES:
        _refuse(req, NO_SECRET, "no code")
        return
    if o is None or (now - o["t"]) * 1000.0 > P.AUTH_PENDING_MS:
        _refuse(req, "Connect again (HELLO first).", "AUTH without HELLO")
        return
    if not P.proof_ok(d["p"], P.studio_proof(S.secret, o["n"], d["n"])):
        _refuse(req, "Wrong MIDI link code: paste the code shown in FL (%s)." % WHERE_CODE, "wrong code")
        _say_code(now)
        return
    if S.key is not None:
        # (24 nov., relecture) une session existait : on change de cle sans rendre le mix. Le studio relie ses pistes
        # (BIND) ; un slot qu'il ne reprend pas tombe apres STALE_AFTER s (OnIdle). Les notes tenues sont relachees.
        _release_all()
        S.stale = dict((slot, now + STALE_AFTER) for slot in S.binds)
        S.pw = None
        S.pw_last = {}
    else:
        _reset_session(restore=True)
    S.session = True
    S.key = P.session_key(S.secret, o["n"], d["n"])
    S.ctr = 0
    S.last_rx = now
    S.count = _count_all()
    S.list_sig = _list_signature(S.count)
    # le tempo : celui de FL maintenant ; TEMPO_CHANGED seulement a un studio qui le lit
    S.studio_caps = o["caps"]
    S.tempo_known = _tempo()
    S.tempo_hold_until = 0.0
    S.tempo_sent_at = -1e9
    ack = {"p": P.daw_proof(S.secret, o["n"], d["n"]), "project": _project()}
    bpm = P.round_bpm(S.tempo_known)
    if bpm is not None:
        ack["bpm"] = bpm
    _send(P.AUTH_ACK, req, ack)
    _log("Uppwind Studio connected (MIDI link code checked).")
    _sync_tags()                # (26 oct.) les numeros des Uppwind Link Notes, des maintenant


# -- Sortie ---------------------------------------------------------------------------------
def _send(cmd, req, data):
    try:
        chunks = P.encode_message(cmd, req, data)
    except Exception as e:
        _log("cannot encode %s: %s" % (P.command_name(cmd), e))
        return
    if len(S.out) + len(chunks) > OUT_MAX:
        # (24 nov.) file pleine : ce message ne part pas, en entier
        S.out_dropped += 1
        now = _now()
        if now - S.out_warned_at >= OUT_WARN_EVERY:
            S.out_warned_at = now
            _log("too many replies waiting for Uppwind Studio: %d message(s) dropped (%s last)."
                 % (S.out_dropped, P.command_name(cmd)))
            S.out_dropped = 0
        _flush()
        return
    S.out.extend(chunks)
    _flush()


def _error(req, code, msg=None):
    data = {"code": code}
    if msg:
        data["msg"] = str(msg)[:200]
    _send(P.ERROR, req, data)


def _flush():
    if not S.out:
        return
    if S.send is None:
        if not S.warned_out:
            S.warned_out = True
            _log("no SysEx output in this FL build: replies cannot reach Uppwind Studio.")
        S.out = []
        return
    try:
        assigned = bool(device.isAssigned())
    except Exception:
        assigned = True
    if not assigned:
        if not S.warned_out:
            S.warned_out = True
            _log("no output port linked. In Options > MIDI settings, give the 'Uppwind Link Back' "
                 "OUTPUT the same Port number as the 'Uppwind Link' input (%s)." % _port())
        S.out = []
        return
    n = 0
    while S.out and n < OUT_PER_TICK:
        chunk = S.out.pop(0)
        n += 1
        try:
            S.send(chunk)
        except Exception as e:
            _log("midiOutSysex failed: %s" % e)
            S.out = []
            return


def _port():
    try:
        return str(device.getPortNumber())
    except Exception:
        return "?"


# -- Notes ----------------------------------------------------------------------------------
def _note(i, note, vel):
    try:
        channels.midiNoteOn(i, note, vel)
    except Exception as e:
        _log("midiNoteOn failed: %s" % e)


def _note_on(slot, note, vel):
    b = S.binds.get(slot)
    if b is None:
        return
    key = (slot, note)
    prev = S.held.get(key)
    if prev is not None:
        _note(prev, note, 0)            # meme touche rejouee : on relache la precedente
    S.sustained.discard(key)
    S.held[key] = b["index"]
    _note(b["index"], note, vel)


def _note_off(slot, note):
    key = (slot, note)
    i = S.held.get(key)
    if i is None:
        return
    if S.pedal.get(slot):
        S.sustained.add(key)
        return
    del S.held[key]
    _note(i, note, 0)


def _release_slot(slot, send_off=True):
    for key in [k for k in S.held if k[0] == slot]:
        i = S.held.pop(key)
        S.sustained.discard(key)
        if send_off:
            _note(i, key[1], 0)
    S.pedal[slot] = False
    for i, s in list(S.bent.items()):
        if s == slot:
            del S.bent[i]
            _set_pitch(i, 0.0)


def _release_all():
    for slot in set([k[0] for k in S.held] + list(S.pedal.keys()) + list(S.bent.values())):
        _release_slot(slot)


def _pedal(slot, down):
    S.pedal[slot] = down
    if down:
        return
    for key in [k for k in S.sustained if k[0] == slot]:
        S.sustained.discard(key)
        i = S.held.pop(key, None)
        if i is not None:
            _note(i, key[1], 0)


def _set_pitch(i, amount):
    try:
        channels.setChannelPitch(*_g(i, amount, 0, PIM_NONE))
    except TypeError:
        try:
            channels.setChannelPitch(i, amount)
        except Exception:
            pass
    except Exception:
        pass


def _bend(slot, value14):
    b = S.binds.get(slot)
    if b is None:
        return
    amount = max(-1.0, min(1.0, (value14 - 8192) / 8192.0))
    i = b["index"]
    if amount == 0.0:
        S.bent.pop(i, None)
    else:
        S.bent[i] = slot
    _set_pitch(i, amount)


def _param_count(i):
    if i not in S.cc_params:
        n = 0
        if _plugin_valid(i):
            try:
                n = int(plugins.getParamCount(*_pg(i, -1)))
            except Exception:
                n = 0
        S.cc_params[i] = n
    return S.cc_params[i]


def _param_names(i):
    """(19 nov., 1.4.0) Les parametres nommes du plugin du canal i : [(p, nom)] (cache)."""
    if i not in S.pnames:
        out = []
        if _plugin_valid(i):
            n = min(_param_count(i), VST_CC_BASE)
            for p in range(n):
                try:
                    nm = str(plugins.getParamName(*((p,) + _pg(i, -1))) or "").strip()
                except Exception:
                    nm = ""
                if nm:
                    out.append((p, nm))
        S.pnames[i] = out
    return S.pnames[i]


def _param_value(i, p):
    v = float(plugins.getParamValue(*((p,) + _pg(i, -1))))
    return min(1.0, max(0.0, v)) if v == v else 0.0


def _params_page(i, start):
    names = _param_names(i)
    items = []
    k = start
    while k < len(names) and len(items) < PARAMS_PAGE:
        p, nm = names[k]
        try:
            v = _param_value(i, p)
        except Exception:
            v = 0.0
        items.append({"p": p, "name": nm[:64], "v": round(v, 4)})
        k += 1
    data = {"id": i, "plugin": _plugin_name(i)[:64], "items": items}
    if k < len(names):
        data["next"] = k
    return data


def _param_set(i, p, v):
    plugins.setParamValue(*((float(v), p) + _pg(i, -1, PIM_NONE)))
    if S.pw == i:
        S.pw_last[p] = float(v)        # pas d'echo : le studio sait ce qu'il a pose


def _param_watch(i):
    S.pw = i
    S.pw_sig = (_name(i), _plugin_name(i)) if i is not None else None
    S.pw_last = {}
    S.pw_dirty = False
    S.pw_at = 0.0
    if i is None:
        return
    for p, _ in _param_names(i):
        try:
            S.pw_last[p] = _param_value(i, p)
        except Exception:
            pass


def _param_check(now):
    i = S.pw
    if i is None or now - S.pw_at < PARAM_EVERY:
        return
    if not S.pw_dirty and now - S.pw_at < PARAM_POLL:
        return
    S.pw_at = now
    S.pw_dirty = False
    if not _plugin_valid(i):
        S.pw = None
        return
    changed = []
    for p, _ in _param_names(i):
        try:
            v = _param_value(i, p)
        except Exception:
            continue
        old = S.pw_last.get(p)
        S.pw_last[p] = v
        if old is not None and abs(v - old) > PARAM_EPS and len(changed) < P.MAX_PARAM_CHANGES:
            changed.append({"p": p, "v": round(v, 4)})
    if changed:
        data = {"id": i, "items": changed}
        try:
            hint = str(ui.getHintMsg() or "").strip()
        except Exception:
            hint = ""
        if hint:
            data["hint"] = hint[:80]
        _send(P.PARAM_CHANGED, 0, data)


def _cc(slot, cc, value):
    if cc in P.ALL_OFF_CCS:
        _release_slot(slot)
        return
    if cc == 64:
        _pedal(slot, value >= 64)
        return
    b = S.binds.get(slot)
    if b is None or cc not in P.PASS_CCS:
        return
    i = b["index"]
    p = VST_CC_BASE + cc
    if _param_count(i) > p:      # un VST : ses parametres « MIDI CC »
        try:
            plugins.setParamValue(*((value / 127.0, p) + _pg(i, -1, PIM_NONE)))
        except Exception as e:
            _log("MIDI CC %d not forwarded: %s" % (cc, e))


def _on_channel_message(status, d1, d2):
    kind = status & 0xF0
    slot = (status & 0x0F) + 1
    if kind == 0x90 and d2 > 0:
        _note_on(slot, d1, d2)
    elif kind == 0x80 or kind == 0x90:
        _note_off(slot, d1)
    elif kind == 0xB0:
        _cc(slot, d1, d2)
    elif kind == 0xE0:
        _bend(slot, (d1 & 0x7F) | ((d2 & 0x7F) << 7))
    # aftertouch, program change : ignores (mais consommes : rien ne part au canal selectionne)


# -- Mixage (SET) -------------------------------------------------------------------------
def _get_vol(i):
    try:
        return float(channels.getChannelVolume(*_g(i, False)))
    except TypeError:
        return float(channels.getChannelVolume(i))


def _set_vol(i, v):
    try:
        channels.setChannelVolume(*_g(i, v, PIM_NONE))
    except TypeError:
        channels.setChannelVolume(i, v)


def _get_pan(i):
    try:
        return float(channels.getChannelPan(*_g(i)))
    except Exception:
        return 0.0


def _set_pan(i, p):
    try:
        channels.setChannelPan(*_g(i, p, PIM_NONE))
    except TypeError:
        channels.setChannelPan(i, p)


def _is_muted(i):
    try:
        return bool(channels.isChannelMuted(*_g(i)))
    except Exception:
        return False


def _toggle_mute(i):
    # Une BASCULE (value = -1) : la forme explicite n'est pas fiable d'une version a l'autre
    try:
        channels.muteChannel(*_g(i, -1))
    except TypeError:
        channels.muteChannel(i)


def _capture(i):
    if i not in S.orig and _exists(i):
        try:
            S.orig[i] = (_get_vol(i), _get_pan(i), _is_muted(i))
        except Exception:
            pass


def _bound_indexes():
    return set(b["index"] for b in S.binds.values())


def _apply_mutes():
    bound = _bound_indexes()
    any_solo = any(S.mix.get(i, {}).get("solo") for i in bound)
    for i in bound:
        m = S.mix.get(i, {})
        if "mute" in m or any_solo:
            _capture(i)
            S.mute_want[i] = [bool(m.get("mute")) or (any_solo and not m.get("solo")), True]
        elif i in S.mute_want and S.mute_want[i][1]:
            if i in S.orig:
                S.mute_want[i] = [S.orig[i][2], False]
            else:
                del S.mute_want[i]


def _restore(i):
    """Rend au canal le volume / pan / mute d'avant le studio (s'il n'est plus relie)."""
    o = S.orig.pop(i, None)
    S.mix.pop(i, None)
    if o is None:
        S.mute_want.pop(i, None)
        return
    if not _exists(i):
        S.mute_want.pop(i, None)
        return
    try:
        _set_vol(i, o[0])
        _set_pan(i, o[1])
    except Exception:
        pass
    S.mute_want[i] = [o[2], False]


def _reconcile_mutes(now):
    for i, (want, keep) in list(S.mute_want.items()):
        if not _exists(i):
            del S.mute_want[i]
            continue
        if _is_muted(i) == want:
            if not keep:
                del S.mute_want[i]
            continue
        if now - S.toggled_at.get(i, 0.0) >= MUTE_RETRY:
            try:
                _toggle_mute(i)
            except Exception as e:
                _log("mute failed: %s" % e)
            S.toggled_at[i] = now


def _set(d):
    i = d["id"]
    _capture(i)
    m = S.mix.setdefault(i, {})
    if "vol" in d:
        m["vol"] = d["vol"]
        v = gain_to_fl_volume(d["vol"])
        _set_vol(i, v)
        if not S.vol_checked:
            S.vol_checked = True
            try:
                want = -120.0 if d["vol"] <= 0 else 20.0 * math.log10(d["vol"])
                got = float(channels.getChannelVolume(*_g(i, True)))
                if abs(got - want) > 0.5 and want > -60:
                    _log("volume law check: asked %.1f dB, FL reports %.1f dB." % (want, got))
            except Exception:
                pass
    if "pan" in d:
        m["pan"] = d["pan"]
        _set_pan(i, d["pan"])
    if "mute" in d:
        m["mute"] = d["mute"]
    if "solo" in d:
        m["solo"] = d["solo"]
    if "mute" in d or "solo" in d:
        _apply_mutes()
        _reconcile_mutes(_now())


# -- Liens slot -> canal ------------------------------------------------------------------
def _bind(slot, i, notes=0):
    """notes (26 oct.) : le « Link tag » du Uppwind Link Notes qui joue ce canal (0 : les notes passent par ici)."""
    old = S.binds.get(slot)
    _release_slot(slot)
    S.binds[slot] = {"index": i, "sig": _sig(i), "notes": notes}
    if old is not None and old["index"] != i and old["index"] not in _bound_indexes():
        _restore(old["index"])
    _apply_mutes()


def _unbind(slot, restore=True, send_off=True):
    b = S.binds.get(slot)
    _release_slot(slot, send_off)
    if b is None:
        return
    del S.binds[slot]
    if b["index"] not in _bound_indexes():
        if restore:
            _restore(b["index"])
        else:
            S.mix.pop(b["index"], None)
            S.orig.pop(b["index"], None)
            S.mute_want.pop(b["index"], None)
    _apply_mutes()


def _reset_session(restore=True):
    S.stale.clear()
    _release_all()
    for slot in list(S.binds):
        _unbind(slot, restore=restore)
    for i in list(S.orig):
        if restore:
            _restore(i)
    S.mix.clear()
    if not restore:
        S.orig.clear()
        S.mute_want.clear()
    S.cc_params.clear()
    S.tag_param.clear()
    S.tags.clear()
    S.pnames.clear()
    S.pw = None
    S.pw_last = {}


# -- La structure du rack a change ----------------------------------------------------------
def _remap(mapping):
    """mapping : ancien index -> nouvel index (None = supprime) pour les canaux relies."""
    if not mapping:
        return

    def mv(i):
        return mapping.get(i, i)

    for slot, b in list(S.binds.items()):
        if b["index"] in mapping:
            new = mapping[b["index"]]
            if new is None:
                _release_slot(slot, send_off=False)      # le canal n'existe plus
                del S.binds[slot]
            else:
                b["index"] = new
    S.held = dict((k, mv(i)) for k, i in S.held.items() if mv(i) is not None)
    S.bent = dict((mv(i), s) for i, s in S.bent.items() if mv(i) is not None)
    for d in (S.mix, S.orig, S.mute_want, S.toggled_at):
        moved = dict((mv(i), v) for i, v in d.items() if mv(i) is not None)
        d.clear()
        d.update(moved)
    S.cc_params.clear()
    S.tag_param.clear()
    S.pnames.clear()


def _rack(n):
    """Une lecture du rack : [(type, nom, plugin, couleur)] -- 4 a 6 appels a l'API par canal."""
    out = []
    for i in range(n):
        t = _type(i)
        out.append((t, _name(i), _plugin_name(i, t), _color(i)))
    return out


def _list_signature(n):
    return tuple(_rack(n))


def _reconcile_structure():
    n = _count_all()
    rack = _rack(n)
    sigs = [(name, plugin) for (_t, name, plugin, _c) in rack]
    # Meme nombre de canaux : le rack d'avant dit qui n'a PAS bouge. Un canal qui portait deja ce
    # nom a cette place est un jumeau, pas le notre (le notre a ete renomme sur place).
    prev = S.list_sig if (n == S.count and S.list_sig is not None and len(S.list_sig) == n) else None
    mapping = {}
    for slot, b in S.binds.items():
        old = b["index"]
        if old < n and sigs[old] == b["sig"]:
            continue
        cands = [i for i, s in enumerate(sigs) if s == b["sig"]
                 and not (prev is not None and i != old and (prev[i][1], prev[i][2]) == s)]
        if cands:
            mapping[old] = min(cands, key=lambda i: (abs(i - old), i))
        elif n == S.count and old < n:
            b["sig"] = sigs[old]                         # renomme ou remplace sur place
        else:
            mapping[old] = None                          # supprime
    if n != S.count:
        # les restaurations en attente pour des canaux NON relies ne savent plus ou aller
        bound = _bound_indexes()
        for d in (S.orig, S.mute_want, S.mix):
            for i in [i for i in d if i not in bound and i not in mapping]:
                del d[i]
    _remap(mapping)
    S.count = n
    S.cc_params.clear()
    S.tag_param.clear()
    S.tags.clear()
    S.pnames.clear()
    if S.pw is not None:
        # (1.4.0) le canal suivi, comme un lien : retrouve par son nom et son plugin (le plus proche), sinon plus rien
        pw = S.pw
        if pw >= n or sigs[pw] != S.pw_sig:
            cands = [i for i, s in enumerate(sigs) if s == S.pw_sig]
            pw = min(cands, key=lambda i: (abs(i - S.pw), i)) if cands else None
        _param_watch(pw)
    if S.session:
        _sync_tags()                # (26 oct.) un Notes ajoute, deplace : son numero suit son port
    sig = tuple(rack)
    first = S.list_sig is None
    if sig != S.list_sig or mapping:
        S.list_sig = sig
        if S.session and not first:
            _send(P.CHANGED, 0, {})
    _apply_mutes()


def _check_routes():
    """(26 oct.) Toutes les STRUCT_CHECK s : les numeros des Notes suivent leurs ports ; un lien par un Notes dont la
    route a change (un port bouge, le Notes retire) tombe, et CHANGED le dit : le studio relie a nouveau (le Notes
    du nouveau port, ou les notes par ici)."""
    if not S.session:
        return
    by_port = _sync_tags()
    lost = []
    for slot, b in list(S.binds.items()):
        if b.get("notes") and _notes_route(b["index"], by_port)[0] != b["notes"]:
            lost.append(slot)
    for slot in lost:
        _log("the Uppwind Link Notes route of '%s' changed: the studio links it again." % _name(S.binds[slot]["index"]))
        del S.binds[slot]
        _release_slot(slot, send_off=False)
    if lost:
        _send(P.CHANGED, 0, {})


def _mark_dirty():
    # Pas un anti-rebond pur : si FL rafraichit sans cesse (lecture), on recompte quand meme toutes
    # les CHANGE_SETTLE secondes au plus.
    if not S.dirty:
        S.dirty = True
        S.dirty_at = _now()


# -- Le tempo (cap 'tempo') -----------------------------------------------------------------
def _tempo():
    """Le tempo de FL en BPM, ou None. mixer.getCurrentTempo(asInt) : la doc dit 1 -> BPM x 1000 en entier ;
    les scripts d'Image-Line (Novation) et de NI constatent l'inverse (1 -> BPM). FL va de 10 a 522 BPM : une
    valeur de 1000 ou plus est donc en milliemes, quel que soit le sens."""
    try:
        v = float(mixer.getCurrentTempo(1))
    except Exception:
        try:
            v = float(mixer.getCurrentTempo())
        except Exception:
            return None
    if math.isnan(v) or math.isinf(v) or v <= 0:
        return None
    return v / 1000.0 if v >= 1000.0 else v


def _set_tempo(bpm):
    """TEMPO du studio : le tempo du projet. Ce tempo-la n'est pas renvoye (TEMPO_HOLD_MS : FL peut ne
    l'appliquer qu'au tick suivant)."""
    v = max(FL_TEMPO_MIN, min(FL_TEMPO_MAX, float(bpm)))
    S.tempo_known = v
    S.tempo_hold_until = _now() + P.TEMPO_HOLD_MS / 1000.0
    general.processRECEvent(REC_TEMPO, int(round(v * 1000.0)), REC_TEMPO_FLAGS)


def _tempo_check(now):
    """Le tempo de FL a-t-il change sans le studio ? -> TEMPO_CHANGED (au plus un par TEMPO_CHECK)."""
    if not S.session or "tempo" not in S.studio_caps or now < S.tempo_hold_until:
        return
    t = _tempo()
    if t is None:
        return
    if S.tempo_known is not None and abs(t - S.tempo_known) <= P.TEMPO_EPS:
        return
    if now - S.tempo_sent_at < TEMPO_CHECK:
        return
    bpm = P.round_bpm(t)
    if bpm is None:
        return
    S.tempo_known = t
    S.tempo_sent_at = now
    _send(P.TEMPO_CHANGED, 0, {"bpm": bpm})


# -- Les commandes du studio ----------------------------------------------------------------
def _project():
    try:
        title = str(ui.getProgTitle() or "")
    except Exception:
        return ""
    parts = [p.strip() for p in title.split(" - ")]
    keep = [p for p in parts if p and "FL Studio" not in p]
    name = keep[0] if keep else ""
    name = name.lstrip("*").strip()
    if name.lower().endswith(".flp"):
        name = name[:-4]
    return name[:120]


def _daw_version():
    try:
        nums = [int(ui.getVersion(k)) for k in (0, 1, 2, 3)]
        v = "%d.%d.%d build %d" % tuple(nums)
    except Exception:
        try:
            v = str(ui.getVersion())
        except Exception:
            v = ""
    return ("%s, API %d" % (v, S.api))[:60] if v else ("API %d" % S.api)


def _list_page(start):
    n = _count_all()
    out = []
    i = max(0, start)
    size = 40
    while i < n:
        it = _item(i)
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
    if i < n:
        data["next"] = i
    return data


def _dispatch(cmd, req, data):
    spec = P.COMMANDS[cmd]
    if spec[1] == "daw":
        # nos propres reponses revenues par une boucle : on les ignore. (27 oct.) La boucle a une cause connue : la
        # sortie « Uppwind Link » a le numero de port de l'entree (FL le lui redonne quand on rechoisit le type de
        # controleur de l'entree) ; les reponses partent la, jamais vers « Uppwind Link Back » : le studio attend.
        _say(("loop",), "FL Studio sends this script's replies into the 'Uppwind Link' OUTPUT: the studio never gets "
             "them. In Options > MIDI settings, set that output's Port to none (---); only 'Uppwind Link Back' keeps "
             "port %s." % _port())
        return
    v = P.validate_message(cmd, data, "studio")
    if not v["ok"]:
        if cmd != P.ERROR:
            _error(req, v["error"]["code"], v["error"]["msg"])
        return
    d = v["data"]
    cap = spec[3]
    if cap and cap not in CAPS:
        _error(req, P.ERR_UNSUPPORTED, ADD_HINT)
        return
    if cmd == P.HELLO:
        _hello(req, d)              # (24 nov., 1.5.0) un nonce ; la session s'ouvre sur AUTH
    elif cmd == P.AUTH:
        _auth(req, d)
    elif cmd == P.LIST:
        _send(P.LIST_REPLY, req, _list_page(d.get("start", 0)))
    elif cmd == P.BIND:
        if not _playable(d["id"]) or _is_notes(d["id"]):
            _error(req, P.ERR_NOT_FOUND, "No instrument at position %d of the channel rack." % (d["id"] + 1))
            return
        # (26 oct.) via 'plugin' : le Uppwind Link Notes du port d'entree du canal, s'il y en a un ; sinon par ici
        tag, why = (0, "")
        if d.get("via") == "plugin":
            tag, why = _notes_route(d["id"])
        _bind(d["slot"], d["id"], tag)
        S.stale.pop(d["slot"], None)
        ack = {"slot": d["slot"], "id": d["id"]}
        if d.get("via") == "plugin":
            ack["via"] = "plugin" if tag else "port"
            if tag:
                ack["notes"] = tag
                _say(("route", d["slot"]), "'%s' plays from the Uppwind Link Notes on MIDI port %d." % (_name(d["id"]), tag))
            else:
                _say(("route", d["slot"]), "notes for '%s' come through loopMIDI: %s" % (_name(d["id"]), why))
        _send(P.BIND_ACK, req, ack)
    elif cmd == P.UNBIND:
        S.stale.pop(d["slot"], None)
        _unbind(d["slot"])
    elif cmd == P.SET:
        if not _playable(d["id"]):
            _error(req, P.ERR_NOT_FOUND, "No instrument at position %d of the channel rack." % (d["id"] + 1))
            return
        _set(d)
    elif cmd == P.TEMPO:
        _set_tempo(d["bpm"])
    elif cmd == P.EDITOR:
        if not _playable(d["id"]) or _is_notes(d["id"]):
            _error(req, P.ERR_NOT_FOUND, "No instrument at position %d of the channel rack." % (d["id"] + 1))
            return
        if not d["open"]:
            # (19 nov.) fermer une fenetre detachee par showEditor(index, 0) a bloque FL 2026 : jamais
            _error(req, P.ERR_UNSUPPORTED, "Close the plugin's window in FL Studio itself.")
            return
        S.editor = (req, d["id"], True)
    elif cmd == P.PARAMS:
        if not _playable(d["id"]) or not _plugin_valid(d["id"]):
            _error(req, P.ERR_NOT_FOUND, "No plugin at position %d of the channel rack." % (d["id"] + 1))
            return
        _send(P.PARAMS_REPLY, req, _params_page(d["id"], d.get("start", 0)))
    elif cmd == P.PARAM_SET:
        if not _playable(d["id"]) or not _plugin_valid(d["id"]):
            _error(0, P.ERR_NOT_FOUND, "No plugin at position %d of the channel rack." % (d["id"] + 1))
            return
        # (24 nov., audit M3) seulement l'instrument que le studio a lie (BIND) ou qu'il suit (PARAM_WATCH) : jamais un
        # plugin quelconque du projet
        if d["id"] != S.pw and d["id"] not in _bound_indexes():
            _error(0, P.ERR_UNSUPPORTED, "Link the instrument at position %d to a track in Uppwind Studio first."
                   % (d["id"] + 1))
            return
        _param_set(d["id"], d["p"], d["v"])
    elif cmd == P.PARAM_WATCH:
        i = d.get("id")
        if i is not None and (not _playable(i) or not _plugin_valid(i)):
            i = None
        _param_watch(i)
    elif cmd == P.PING:
        _send(P.PONG, req, {"t": d["t"]})
    elif cmd == P.ERROR:
        # (24 nov.) un ERROR n'a pas a etre prouve : au journal au plus toutes les AUTH_LOG_EVERY. (Relecture) Son texte
        # seulement s'il est signe par la session : sinon n'importe quel programme ecrirait ici ce qu'il veut
        now = _now()
        if now - S.error_said_at >= AUTH_LOG_EVERY:
            S.error_said_at = now
            if S.rx_signed:
                _log("Uppwind Studio reports %s: %s" % (d.get("code"), d.get("msg", "")))
            else:
                _log("an unsigned error report came in on the MIDI port (not shown: anything can send one).")


def _handle_sysex(raw):
    """True si le SysEx etait a nous (event.handled), False sinon."""
    if raw is None:
        return False
    try:
        raw = bytes(raw)
    except Exception:
        return False
    if not P.is_link_sysex(raw):
        return False
    now = _now()
    if S.last_sx[0] == raw and now - S.last_sx[1] < 0.05:
        return True                 # deja traite (OnMidiMsg puis OnSysEx)
    S.last_sx = (raw, now)
    out = S.rx.push(raw, now * 1000.0)
    t = out["type"]
    if t == "ignored":
        return False
    if t == "partial":
        return True
    if t == "error":
        cmd, req = out.get("cmd"), out.get("req")
        sender = P.COMMANDS[cmd][1] if cmd in P.COMMANDS else None
        if req is not None and sender != "daw" and cmd != P.ERROR:
            msg = "Update the Uppwind Link script." if out["code"] == P.ERR_VERSION else None
            _error(req, out["code"], msg)
        return True
    # (24 nov., 1.5.0) sans preuve : HELLO, AUTH et ERROR seulement (et nos propres reponses revenues, que _dispatch
    # signale) ; tout le reste doit etre signe avec la cle de la session
    cmd, req, data = out["cmd"], out["req"], out["data"]
    S.rx_signed = False
    if cmd == P.ERROR and S.key is not None:
        # (24 nov., relecture) un ERROR signe par la session : son texte peut aller au journal
        v = P.verify_signed(cmd, req, out.get("text"), S.key, S.ctr)
        if v["ok"]:
            S.ctr = v["c"]
            data = v["data"]
            S.rx_signed = True
    if cmd not in (P.HELLO, P.AUTH, P.ERROR) and P.COMMANDS[cmd][1] != "daw":
        if S.key is None:
            _refuse(req, "Reload Uppwind Studio: this script needs its MIDI link code (%s)." % WHERE_CODE, "no session")
            return True
        v = P.verify_signed(cmd, req, out.get("text"), S.key, S.ctr)
        if not v["ok"]:
            _refuse(req, "This command was not signed by your Uppwind Studio session.", v["why"])
            return True
        S.ctr = v["c"]
        data = v["data"]
        S.rx_signed = True
        S.last_rx = now
    out = {"cmd": cmd, "req": req, "data": data, "name": out.get("name")}
    try:
        _dispatch(out["cmd"], out["req"], out["data"])
    except Exception as e:
        _log("%s failed: %s" % (out.get("name"), e))
        _error(out["req"], P.ERR_INTERNAL, "%s: %s" % (type(e).__name__, e))
    return True


# -- Les rappels de FL ----------------------------------------------------------------------
def OnInit():
    global S
    S = _State()
    # (24 nov., relecture) le secret d'abord : rien ne doit trouver S.secret vide (le HELLO le refuse de toute facon)
    S.secret, S.secret_from = _load_secret()
    S.kid = P.key_id(S.secret) if S.secret else ""
    try:
        S.api = int(general.getVersion())
    except Exception:
        S.api = 0
    S.g_chan = S.api >= 33
    S.g_plug = S.api >= 26
    S.send = getattr(device, "midiOutSysex", None) or getattr(device, "midiOutSysEx", None)
    try:
        S.count = _count_all()
    except Exception:
        S.count = -1
    _log("v%s ready (FL API %d, input port %s). Waiting for Uppwind Studio." % (SCRIPT_VERSION, S.api, _port()))
    if S.secret_from == "companion":
        _log("MIDI link code: the one the Uppwind Link companion installed (id %s). Uppwind Studio gets it from the "
             "companion: start the companion and pair the studio. The code is also in uwl_secret.py, next to this "
             "script." % S.kid)
    elif S.secret_from == "weak":
        _log("no system random source in this FL Python: the MIDI link stays closed (a code drawn from the clock "
             "could be guessed). Install the script with the Uppwind Link companion: it gives it a code.")
    _say_code(_now(), force=True)
    if not P.SHA256_NATIVE:
        _log("no hashlib in this FL Python: the MIDI link code is checked in pure Python (slower, same result).")
    try:
        if not device.isAssigned():
            _log("no output port linked yet: give the 'Uppwind Link Back' OUTPUT the same Port number (%s)." % _port())
    except Exception:
        pass


def OnDeInit():
    try:
        _release_all()
        for i, o in list(S.orig.items()):
            if _exists(i):
                _set_vol(i, o[0])
                _set_pan(i, o[1])
                if _is_muted(i) != o[2]:
                    _toggle_mute(i)
        S.orig.clear()
    except Exception as e:
        _log("shutdown: %s" % e)
    _log("stopped.")


def OnMidiMsg(event):
    try:
        status = int(event.status)
    except Exception:
        return
    if not 0x80 <= status < 0xF0:
        # SysEx (status 0xF0) -- ou tout ce qui n'est pas un message de canal : on regarde s'il
        # porte notre en-tete ; horloge, start/stop... : FL en fait ce qu'il veut.
        try:
            if _handle_sysex(getattr(event, "sysex", None)):
                event.handled = True
        except Exception as e:
            _log("SysEx: %s" % e)
        return
    try:
        _on_channel_message(status, int(event.data1), int(event.data2))
    except Exception as e:
        _log("MIDI: %s" % e)
    event.handled = True            # rien ne doit partir au canal selectionne de FL


def OnSysEx(event):
    # Certaines versions de FL ne passent le SysEx qu'ici (apres OnMidiMsg)
    try:
        if _handle_sysex(getattr(event, "sysex", None)):
            event.handled = True
    except Exception as e:
        _log("SysEx: %s" % e)


def _show_editor():
    """(19 nov.) EDITOR : ouvrir la fenetre du canal (jamais la fermer : voir l'en-tete), puis EDITOR_ACK."""
    req, i, want = S.editor
    S.editor = None
    if not _playable(i):
        _error(req, P.ERR_NOT_FOUND, "No instrument at position %d of the channel rack." % (i + 1))
        return
    try:
        channels.showEditor(*_g(i, 1 if want else 0))
    except Exception as e:
        _error(req, P.ERR_INTERNAL, ("FL Studio did not open the window: %s" % e)[:200])
        return
    _send(P.EDITOR_ACK, req, {"id": i, "open": want})


def OnIdle():
    try:
        now = _now()
        if S.editor is not None:
            _show_editor()
        if S.pw is not None:
            _param_check(now)
        if S.stale:
            _drop_stale(now)
        _flush()
        if S.dirty and now - S.dirty_at >= CHANGE_SETTLE:
            S.dirty = False
            _reconcile_structure()
        elif now - S.last_struct_check >= STRUCT_CHECK:
            S.last_struct_check = now
            if _count_all() != S.count:
                _mark_dirty()
            else:
                _check_routes()     # (26 oct.) les ports MIDI ne previennent pas quand ils changent
        if S.mute_want and now - S.last_mute_check >= MUTE_CHECK:
            S.last_mute_check = now
            _reconcile_mutes(now)
        if S.session and now - S.last_tempo_check >= TEMPO_CHECK:
            S.last_tempo_check = now
            _tempo_check(now)
        if S.held and S.last_rx and (now - S.last_rx) * 1000.0 > P.LINK_TIMEOUT_MS:
            _log("Uppwind Studio went quiet: releasing held notes.")
            _release_all()
            S.last_rx = 0.0
    except Exception as e:
        _log("idle: %s" % e)


def _drop_stale(now):
    """(24 nov., relecture) les slots d'avant le dernier AUTH que le studio n'a pas repris : defaits, le mix rendu."""
    gone = [slot for slot, t in S.stale.items() if now >= t]
    for slot in gone:
        del S.stale[slot]
        if slot in S.binds:
            _unbind(slot)
    if gone:
        _log("released %d channel link(s) the studio did not take back after it connected again." % len(gone))


def OnRefresh(flags):
    try:
        if int(flags) & HW_DIRTY_CONTROL_VALUES:
            S.pw_dirty = True
        if int(flags) & REFRESH_FLAGS:
            _mark_dirty()
    except Exception:
        pass


def OnDirtyChannel(index, flag=-1):
    if flag != CE_SELECT:
        _mark_dirty()


def OnProjectLoad(status):
    try:
        if status == PL_START:
            _release_all()
        elif status in (PL_LOAD_OK, PL_LOAD_ERROR):
            # un autre projet : les liens d'avant ne veulent plus rien dire
            _reset_session(restore=False)
            S.count = _count_all()
            S.list_sig = _list_signature(S.count)
            if S.session:
                _send(P.CHANGED, 0, {})
    except Exception as e:
        _log("project load: %s" % e)
