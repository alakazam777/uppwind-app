# Uppwind Link

Play the instruments of your DAW (your own VST / AU plugins) from [Uppwind Studio](https://uppwind.lucasmoreau.fr/atelier), see and play their windows inside the studio, and hear them through the studio.

Your DAW (Ableton Live or FL Studio) runs in the background as the plugin host. The studio sends it notes and settings over two virtual MIDI ports, and the sound comes back to the studio, so nothing reaches your speakers twice.

```
 Uppwind Studio (Chrome / Edge)                          Your DAW (Ableton Live, FL Studio)
 ───────────────────────────────                         ──────────────────────────────────
 piano roll, keys, MIDI keyboard ──► MIDI "Uppwind Link" ──────► Uppwind Link script ──► your instruments
 instrument list, status        ◄── MIDI "Uppwind Link Back" ◄── replies, "list changed"
 plugin window, mixer › External ◄── ws://127.0.0.1 ◄── Uppwind Link companion ◄── the plugin's window, the DAW's sound
```

## What is in this repository

| Folder | What |
|---|---|
| [`companion/`](companion/README.md) | **The Uppwind Link companion** for Windows (Rust). A small local program that shows the windows of the plugins open in Ableton Live inside the studio and passes your clicks and keys to them, brings the DAW's sound back to the studio without a virtual cable, and installs the DAW scripts (`--setup`). |
| `plugin/proto/` | The small protocol (Rust, no dependency) between the companion and the Uppwind Link plugins, which are not published yet; the companion is built with it. |
| [`ableton/UppwindLink/`](ableton/README.md) | The Ableton Live Remote Script (Live 11 and 12). |
| [`fl-studio/`](fl-studio/README.md) | The FL Studio MIDI script (FL Studio 21.2 and newer). |
| `shared/` | The MIDI protocol in Python (`uwlink_protocol.py`); identical copies live in `ableton/UppwindLink/` and `fl-studio/`. |
| `fixtures/`, `tests/` | Golden protocol fixtures and the tests (a fake Live, a fake FL, the protocol). |
| `dev/` | Developer tools: `uwlink_dev.py` (talks to the Live script's local channel), `third_party_notices.py`, `release_check.py`. |

Uppwind Studio itself is a website and is not part of this repository.

## Install

The easiest way is the setup guide in the studio: open Uppwind Studio in Chrome or Edge, then **Settings › Uppwind Link**. It checks each step by itself and says exactly what to fix. The steps:

1. **Your browser.** Chrome or Edge on a computer. Click **Allow MIDI** (Web MIDI with SysEx).
2. **Two MIDI ports**, named exactly `Uppwind Link` and `Uppwind Link Back`: [loopMIDI](https://www.tobias-erichsen.de/software/loopmidi.html) on Windows, the IAC Driver on macOS (*Audio MIDI Setup › Window › Show MIDI Studio › IAC Driver*: *Device is online*, add the two buses).
3. **The companion (Windows).** Build it (see *Build from source*) or download a release, then **double-click `uppwind-companion.exe`**: a setup window copies it to `%LOCALAPPDATA%\Uppwind Link\`, installs the DAW scripts you tick (step 4), adds it to the Start Menu and to *Installed apps*, and shows the pairing code; an icon near the clock then opens its status window ([details](companion/README.md#setup-window-and-notification-icon)). From a terminal instead: `uppwind-companion.exe --install` copies it and adds **Uppwind Link Companion** to the Start Menu; optional: `"%LOCALAPPDATA%\Uppwind Link\uppwind-companion.exe" --autostart on` (add `--background` for no window).
4. **The script in your DAW.** On Windows, `uppwind-companion.exe --setup` shows what it would install and `--setup --apply` installs it (a copy of anything it replaces is kept). By hand: copy `ableton/UppwindLink/` into `<User Library>/Remote Scripts/`, or `fl-studio/` into `Documents/Image-Line/FL Studio/Settings/Hardware/Uppwind Link/`, then restart the DAW. Then set the DAW's MIDI settings as its README says ([Ableton Live](ableton/README.md), [FL Studio](fl-studio/README.md)).
5. **Pair the studio with the companion** (once): see *How pairing works*.
6. **The sound coming back.**
   - **Windows: the companion brings it back, no virtual cable needed** (Windows 10 version 2004 or later). In your DAW, play through Windows rather than ASIO, to an output you don't listen to (a spare output of your audio interface, a monitor's HDMI output…), otherwise you hear everything twice. Ableton Live: *Settings › Audio › Driver Type* **MME/DirectX**. FL Studio: **FL Studio ASIO** (*Show ASIO panel › Output*) or **Primary Sound Driver**. In the studio: *Settings › Uppwind Link › Audio return* › **Companion (no cable)**.
   - **Only if that does not work for you** once everything else is set up (your DAW has to use an ASIO interface, or Windows is older than version 2004): install [VB-CABLE](https://vb-audio.com/Cable/) as a fallback, play the DAW into *CABLE Input* at 48 kHz, and pick *CABLE Output* as the studio's return.
   - **macOS:** install [BlackHole 2ch](https://existential.audio/blackhole/), set it and your DAW to 48 kHz, play the DAW into it, and pick it as the studio's return.
7. **Ready to play.** Add an instrument from *Sounds › Your plugins*, run the latency calibration once (*Latency › Calibrate*), and freeze plugin tracks before export or sharing: a browser cannot render your DAW offline, and other people don't own your plugins.

> **Your MIDI keyboard.** On Windows, a USB MIDI device can only be opened by one application at a time. If your DAW holds the keyboard, the browser can't see it: disable it in the DAW's MIDI settings and restart the browser. The studio forwards what you play.

## Security model, in plain words

The companion is a program on your computer that a web page can talk to. It is built so that only the studio you paired can, and only for what the studio needs.

- **Local only.** It listens on `127.0.0.1`, port 47410 (or the next free one up to 47419), so nothing from the network can reach it. It has no telemetry, no update check and no web client: it never connects to the internet.
- **Who can connect.** A browser page can only connect if it comes from `https://uppwind.lucasmoreau.fr` (the *Origin* check), talks to `127.0.0.1:<port>` exactly (the *Host* check, against DNS rebinding), and sends the **pairing code** in its first message. The code holds a 256-bit random secret, compared in constant time; a wrong code is refused, and after five wrong codes the next wrong ones are slowed down.
- **What pairing allows.** While it runs and the studio is paired, the studio's page can: list the plugin windows open in Ableton Live; see and click, drag, scroll and type inside **those plugin windows and their own menus**; open and close plugin editors in Live; hear the sound of **Ableton Live or FL Studio** (and the plugins they host). Pairing means trusting that website: use the official studio only, and keep the code to yourself.
- **What it cannot do.** It never shows your desktop or any other window, never shows or clicks a dialog box, file browser or captioned window that a plugin opens (those stay in the DAW, on your screen), never reads or writes your files, never runs commands, never captures the sound of any program other than the DAW, never records anything, and never changes your audio settings. Typed text never goes to its log.
- **Ending it.** *Settings › Uppwind Link › Plugin windows › Unpair* in the studio; `uppwind-companion.exe --new-token` makes a new code (every paired browser then has to pair again); `--stop` stops a companion running in the background; `--uninstall` removes it.
- **On your computer.** The pairing code is stored in `%USERPROFILE%\.uppwind-companion.json`, and the token of the Live script's local channel in `%USERPROFILE%\.uppwind-link-dev.json`. Any program running under your Windows account can read them, as with any local app: the companion trusts the programs you run yourself.
- **Logs.** The background log (`%LOCALAPPDATA%\Uppwind Link\companion.log`) never contains the pairing code (older versions wrote it there; the companion removes such lines when it starts; if you ever shared an old log, run `--new-token`). It does contain the names of your plugins and tracks: look at it before you attach it to a bug report.
- **Never run Live, FL Studio or the companion as administrator.** The companion needs no administrator rights, and an elevated companion would let the paired page drive elevated windows.
- **Developer options.** `--dev-origins`, `--allow-origin` and `--studio-origin` make the companion accept other pages (a local development server). Use them only for a server of your own; the companion prints a warning while they are on.
- **The executable is not signed yet.** Windows SmartScreen warns about downloaded copies. Download releases from this repository only and compare their SHA-256 (`Get-FileHash uppwind-companion.exe`) with the one published with the release.

## How pairing works

1. Start the companion (Start Menu › **Uppwind Link Companion**). Its console shows a line `UWC-47410-…` (64 characters after the port): that is your pairing code. A companion running in the background never prints it; `uppwind-companion.exe --pairing-code` shows it.
2. In the studio: *Settings › Uppwind Link › Plugin windows (Uppwind Companion)*, paste the code, **Pair**.
3. The studio keeps the code in that browser only, and connects by itself from then on. The companion keeps the same code from one start to the next, until you run `--new-token`.
4. On `https://`, Chrome may ask whether the page may *access other apps and services on this device* (Local Network Access): allow it, or the studio cannot reach the companion.

## Browser permissions

- **MIDI devices, with SysEx** (*Allow MIDI*): lets the page use every MIDI device, hardware included. The studio uses the two Uppwind Link ports and the keyboard you play.
- **Local network access** (Chrome's *apps and services on this device*): lets the page reach programs on your computer. The studio only connects to the companion on `127.0.0.1`.
- **The microphone** (only with a virtual cable, or on macOS): the cable's output is opened as an audio input. The permission covers every input; the studio only opens the device you choose.
- **What happens to the sound.** The studio reads your DAW's sound only while Uppwind Link is on. It records it only when you **Freeze** a track, and it leaves your computer only if you export or share.

## What it can and cannot do

| DAW | Status |
|---|---|
| **Ableton Live** 11 / 12 | Instruments listed, added (on a new MIDI track) and removed, notes, volume / pan / mute / solo, shared tempo. Plugin windows open and close from the studio with **Live 12.4.3 or later**. |
| **FL Studio** 21.2 or newer | Channel-rack instruments listed, notes, volume / pan / mute / solo, shared tempo. With the Uppwind Link plugins, notes go through *Uppwind Link Notes* (sample-accurate) and the sound through *Uppwind Link Return*; the script's commands still use loopMIDI (FL runs scripts only on a MIDI input and refuses them network sockets). Without loopMIDI at all, "plugins only": each Uppwind Link Notes is an instrument, the tempo follows FL, no remote channel volume / pan / mute ([details](fl-studio/README.md)). FL's scripting API cannot load a plugin, delete a channel or set its MIDI ports: add plugins in FL's channel rack. Plugin windows in the studio: not yet (the companion follows Live's windows). |
| Bitwig Studio, REAPER | Not yet; both have APIs that would allow it. |
| Logic Pro | Not possible: Logic has no comparable scripting API. |

The companion runs on **Windows 10 / 11**. The audio return without a cable needs Windows 10 version 2004 (build 19041) or later, and a DAW that plays through Windows: an ASIO driver bypasses the Windows mixer, so the companion cannot hear it.

## Build from source

You need [Rust](https://rustup.rs) (stable, MSVC toolchain) on Windows, and Python 3.7 or later for the tests.

```
cargo build --release --locked --manifest-path companion/Cargo.toml
```

The program is `companion/target/release/uppwind-companion.exe`. Copy only the `.exe`; never share the `.pdb` file next to it (it holds your local paths).

Tests:

```
cargo test --release --locked --manifest-path companion/Cargo.toml -- --skip the_real_registry_in_a_test_key
python -I tests/test_protocol.py
python -I tests/test_fl_script.py
python -I tests/test_ableton.py
```

The skipped test writes a temporary key under `HKCU\Software\UppwindLinkTest` (and deletes it); run it on its own if you want it. The Rust tests open small windows of their own and real connections on `127.0.0.1` (random ports); they never touch Live, FL Studio or a companion that is running.

Release builds: see [RELEASING.md](RELEASING.md).

## The protocol

Studio ↔ DAW: notes on MIDI channels 1–16 (one per studio slot), and control messages as SysEx `F0 7D 55 57 01 <cmd> <req> <seq> <last> <JSON…> F7`, at most 200 bytes of 7-bit JSON per SysEx message and up to 128 messages per command. `shared/uwlink_protocol.py` is the reference in Python; `fixtures/*.json` are shared with the studio's JavaScript implementation, byte for byte. Manufacturer ID `0x7D` is the MIDI specification's ID for non-commercial and research use; the `55 57` that follows keeps other software's messages apart.

Studio ↔ companion: JSON messages and binary frames over the local WebSocket, described in [companion/README.md](companion/README.md#protocol).

## Reporting a vulnerability

Please do not open a public issue. Use GitHub's private reporting: the repository's **Security** tab › **Report a vulnerability**. See [SECURITY.md](SECURITY.md).

## Licence

Copyright (C) 2026 Uppwind. Two licences, by folder:

| Folder | Licence |
|---|---|
| `ableton/UppwindLink/`, `fl-studio/`, `shared/` (the scripts for Ableton Live and FL Studio, and the Python protocol they share) | **MIT** ([LICENSE-MIT](LICENSE-MIT)) |
| everything else: `companion/` (the companion), `plugin/proto/`, `tests/`, `dev/`, `fixtures/` | **GNU General Public License, version 3 or (at your option) any later version** ([LICENSE](LICENSE)), with an additional permission for the companion to be linked with Microsoft's WebView2 loader ([LICENSE-EXCEPTION.txt](LICENSE-EXCEPTION.txt)) |

The companion embeds copies of the MIT scripts (`--setup` installs them); they keep their MIT licence there. Both come with ABSOLUTELY NO WARRANTY.

The companion is built with Rust crates under the MIT licence (some also under Apache-2.0 or the Unlicense; a few under Apache-2.0, Unicode-3.0 or CC0-1.0 only) and is linked statically with Microsoft's WebView2 loader (`WebView2LoaderStatic.lib`, under the additional permission above), listed with their notices in [THIRD-PARTY-NOTICES.txt](THIRD-PARTY-NOTICES.txt) (`uppwind-companion.exe --license` prints them). This software is based in part on the work of the Independent JPEG Group.

Ableton and Ableton Live are trademarks of Ableton AG; FL Studio is a trademark of Image-Line Software; VST is a trademark of Steinberg Media Technologies GmbH. This project is not affiliated with them.
