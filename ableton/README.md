# Uppwind Link for Ableton Live

> **Tested in Ableton Live 12.4.6 Suite on Windows (October 2026):** HELLO, PING, LIST, PLUGINS (after a plug-in rescan and a restart of Live), ADD, BIND, SET, UNBIND and REMOVE work. The script is written from the Live Object Model, and its logic is also tested against a fake Live (`tests/test_ableton.py`).

**You need:** Ableton Live 11 or 12. Live 10 isn't supported: its scripts run Python 2. Then either the **Uppwind Link companion and its plugins** (Windows; no MIDI ports at all, see *Without MIDI ports* below), or the two MIDI ports described in the [main README](../README.md) and, for the sound, the companion on Windows (BlackHole on macOS).

## Without MIDI ports (version 1.3.0 and later)

With the Uppwind Link companion running and the **Uppwind Link plugins** installed (Live: *Settings › Plug-Ins*, VST3 folder on, **Rescan**), the studio needs no loopMIDI port:

- **Commands** (LIST, BIND, SET, TEMPO…) go through the companion and the script's local channel (`LINK`: the same messages, UDP on `127.0.0.1` with the token). Select **UppwindLink** as the Control Surface; *Input* and *Output* can stay on *None*.
- **Notes**: for each Live track the studio plays, the script makes a helper track **Uppwind Link · <track name>** at the end of the set, loads **Uppwind Link Notes** on it (one undo step) and sets the played track's *MIDI From* to that helper track's *Uppwind Link Notes* output, *Monitor In*. It never touches `Uppwind Link / Ch. n` then. Live merges the 16 MIDI channels of a *MIDI From* input, hence one helper track per played track.
- **Which Notes is which**: the plugin has a *Link tag* parameter. The script gives each Notes in the set its own number (a duplicated track gets a new one) and tells the studio; the plugin tells the companion; the studio sends that track's notes to `tag:<number>`. The helper tracks don't show in the studio's list.
- **UNBIND** puts the track's input back and removes the helper track 15 s later, only if the script made it (a studio that reloads takes it back meanwhile). Helper tracks you made yourself are never removed. When Live closes the set, nothing is removed.
- **Sound**: put **Uppwind Link Return** on the Master; it works with ASIO.
- The loopMIDI ports stay the fallback (no companion, or *via loopMIDI* chosen in the studio).
- `ROUTES` (companion → script) says, per track, its input, the helper track it plays from and the Notes numbers: the diagnostic of this route.

**The easy way:** in Uppwind Studio, *Settings › Uppwind Link* has a setup guide. Pick *Ableton Live*: it shows these steps and checks each one by itself.

## 1. Install the script

Copy the **`UppwindLink`** folder (`__init__.py`, `UppwindLink.py`, `link_core.py`, `notes_route.py`, `uwlink_protocol.py`, `editor_core.py`, `editor_channel.py`) into the *Remote Scripts* folder of your User Library: `<User Library>/Remote Scripts/UppwindLink/`. Updating? Replace the whole folder, then restart Live.

**Where is your User Library?** Live shows it in *Settings › Library › Location of User Library* (Live 11: *Preferences › Library*). By default:

- **Windows:** `Documents\Ableton\User Library\Remote Scripts\UppwindLink\`
- **macOS:** `~/Music/Ableton/User Library/Remote Scripts/UppwindLink/`

Create the *Remote Scripts* folder if it isn't there. Then **restart Live**: it reads Remote Scripts only when it starts.

## 2. Settings › Tempo & MIDI

Live 12 calls it *Settings › Tempo & MIDI* (*Options › Settings* on Windows, *Live › Settings* on macOS). Live 11 calls it *Preferences › Link, Tempo & MIDI*.

**Control Surface:** choose **UppwindLink**, with *Input* **Uppwind Link** and *Output* **Uppwind Link Back**. On macOS, Live lists the IAC buses as *IAC Driver (Uppwind Link)* and *IAC Driver (Uppwind Link Back)*. **UppwindLink only appears in the list after a restart of Live** with the folder in place.

**One Control Surface is all you need.** Since version 1.1.0 the same script also opens your plugins' windows for the Uppwind Link companion (see *Plugin windows* below).

**MIDI Ports:**

| Port | Track | Remote |
|---|---|---|
| Input `Uppwind Link` | **On** (the notes reach your tracks) | **On** (the script's commands) |
| Output `Uppwind Link Back` | Off | **On** (the script's replies) |
| Input `Uppwind Link Back`, Output `Uppwind Link` | Off | Off |

## 3. Settings › Audio

**Windows (the default): the Uppwind Link companion brings Live's sound back to the studio**, no virtual cable needed. Set *Driver Type* to **MME/DirectX** (the companion cannot hear an ASIO driver), then the *Audio Output Device* to an output you don't listen to (a spare output of your audio interface, a monitor's HDMI output…), or you hear everything twice. In the studio, *Settings › Uppwind Link › Audio return*: **Companion (no cable)**.

**Only if that does not work for you** (Live has to use your ASIO interface, or Windows is older than version 2004): install VB-CABLE, keep *Driver Type* on MME/DirectX and set the *Audio Output Device* to **CABLE Input (VB-Audio Virtual Cable)** at 48 kHz. VB-CABLE is not an ASIO device: with *Driver Type* ASIO, CABLE Input is not in the list.

**macOS:** set the *Audio Output Device* to **BlackHole 2ch**, at 48 kHz.

A buffer of 256 samples keeps the round trip short. Then, in the studio's setup guide, **Test sound** plays one short, quiet note on one of your Live instruments and shows the level that comes back.

## 4. Your plugins

*Sounds › Your plugins › Browse plugins* lists Live's instruments and your plug-ins. If your VST plugins are missing there: *Settings › Plug-Ins*, turn on your VST3 (or VST2) folder, click **Rescan**, then restart Live.

## What the script does

| Command | What happens in Live |
|---|---|
| LIST | Lists the MIDI tracks that hold an instrument: track name, instrument name and colour. |
| BIND slot *n* | Sets the track's MIDI input to **Uppwind Link / Ch. *n*** and *Monitor* to **In**. Live plays the notes itself, with native timing, pedal, pitch bend and CCs. A track has one input, so it follows only its latest slot. With `via: plugin` (1.3.0): *MIDI From* = the helper track's **Uppwind Link Notes** instead (see above); the reply carries the Notes number. |
| UNBIND, studio reload, disconnect | Puts back the track's own input, monitoring, volume, pan and mute. |
| SET | Sets the mixer volume, using Live's own dB scale via `str_for_value`, so −6 dB in the studio is −6 dB in Live. Also sets pan and the track activator (mute). Solo silences the other linked tracks only. |
| PLUGINS | Reads Live's browser: Live's instruments, and the *Plug-ins* tree with vendors. Effect plugins appear too, so pick an instrument. |
| ADD | Creates a MIDI track at the end of the set, loads the plugin on it and names the track. |
| REMOVE | Deletes a track, but only one that the studio added this session. |
| TEMPO | Sets the set's tempo (`song.tempo`). Since version 1.2.0. |
| Tempo changed in Live | Pushed to the studio as `TEMPO_CHANGED` (a listener on `song.tempo`, at most one every 50 ms), so the studio follows. The tempo the studio set is never sent back. The tempo stays as it is when the studio disconnects. |
| Changes | The track list is checked every second. Renames, new tracks and deletions are pushed to the studio as `CHANGED`. A deleted track loses its link. |

## Plugin windows (Uppwind Companion)

With the Uppwind Link companion running on the same computer, selecting a plugin track in the studio shows that plugin's own window inside the studio. The script opens and closes the plugin's editor in Live for it (`is_editor_open`, **Live 12.4.3 or later**; older versions say so, and the companion can still show a window you open yourself).

The companion talks to the script over UDP on `127.0.0.1` only (port 47391, or the next free one up to 47398). Each request carries a random token that the script writes when Live starts in `~/.uppwind-link-dev.json` (your home folder) and deletes when Live quits. Nothing listens on the network, and a request without the token gets no answer. The script never sends MIDI for this: it only lists your set's plugins and opens or closes their windows.

| Command (companion → script) | What happens in Live |
|---|---|
| LIST | The tracks and their devices, each with its path (`track.device`, or `track.device.chain.device` inside a Rack). |
| EDITOR_OPEN / EDITOR_CLOSE | Opens or closes a plugin's window. |
| EDITOR_STATE, EVENTS | Whether a window is open; what changed by itself (a window you closed by hand). |

## Troubleshooting

- **Log.txt.** Look for lines starting with `[Uppwind Link]` in `Log.txt`:
  - **Windows:** `%AppData%\Ableton\Live x.y\Preferences\Log.txt`
  - **macOS:** `~/Library/Preferences/Ableton/Live x.y/Log.txt`
- **BIND fails with "switch 'Track' on".** The `Uppwind Link` input port's **Track** switch is off.
- **BIND fails with "no 'Ch. n' input channel".** Live didn't offer the per-channel inputs for that port. The script refuses rather than leave the track on *All Channels*, where it would play every slot's notes. Please report it with your Live version.
- **The control surface isn't in the list.** The folder must be named exactly `UppwindLink` and must sit directly in *Remote Scripts*. Then restart Live: the list is read when Live starts.
- **Live doesn't answer the studio.** Is **UppwindLink** selected as a Control Surface, with Input `Uppwind Link` and Output `Uppwind Link Back`, and **Remote** on for the `Uppwind Link` input and the `Uppwind Link Back` output?
- **The studio says Live's script does not answer (plugin windows).** Is Live open, with UppwindLink as a Control Surface? `Log.txt` has a line `[Uppwind Link] listening on udp://127.0.0.1:47391`. If it says `cannot listen`, another program holds ports 47391 to 47398.
- **Your VST plugins are missing in *Browse plugins*.** *Settings › Plug-Ins*: Rescan, then restart Live. If the list stays empty, `Log.txt` has a line `no plugin found in the browser` with the top of Live's plug-in tree: please send it.

## Licence

The Remote Script is under the MIT licence (`LICENSE-MIT`, shipped with it). Copyright (C) 2026 Uppwind. Ableton and Ableton Live are trademarks of Ableton AG; this project is not affiliated with Ableton.
