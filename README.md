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
| [`companion/`](companion/README.md) | **The Uppwind Link companion** for Windows, and in a first version for macOS (Rust). A small local program that shows the windows of the plugins open in Ableton Live inside the studio and passes your clicks and keys to them, brings the DAW's sound back to the studio without a virtual cable, and installs the DAW scripts and the Uppwind Link plugins (`--setup`, its setup window). |
| [`plugin/`](plugin/README.md) | **The Uppwind Link plugins** (Rust, VST3): *Uppwind Link Return* sends the DAW's sound to the companion with any audio driver, ASIO included; *Uppwind Link Notes* plays the studio's notes in the DAW, sample-accurate. One binary; the companion carries it and installs it. Also the protocol they share with the companion (`plugin/proto/`), a small test host and the bundler (`plugin/testhost/`). |
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
3. **The companion (Windows).** Build it (see *Build from source*) or download a release, then **double-click `uppwind-companion.exe`**: a setup window copies it to `%LOCALAPPDATA%\Uppwind Link\`, installs the DAW scripts you tick (step 4) and the Uppwind Link plugins (step 5), adds it to the Start Menu and to *Installed apps*, and shows the pairing code; an icon near the clock then opens its status window ([details](companion/README.md#setup-window-and-notification-icon)). From a terminal instead: `uppwind-companion.exe --install` copies it and adds **Uppwind Link Companion** to the Start Menu; optional: `"%LOCALAPPDATA%\Uppwind Link\uppwind-companion.exe" --autostart on` (add `--background` for no window).
   **macOS** (first version, from Terminal; no setup window yet): copy `uppwind-companion` from the release's .dmg, run `./uppwind-companion --install` (it copies itself to `~/Library/Application Support/Uppwind Link/`; `--autostart on` starts it at every login), then start that copy from Terminal (it prints the pairing code).
4. **The script in your DAW.** `uppwind-companion --setup` shows what it would install and `--setup --apply` installs it, with the plugins of step 5 (a copy of anything it replaces is kept). By hand: copy `ableton/UppwindLink/` into `<User Library>/Remote Scripts/`, or `fl-studio/` into `Documents/Image-Line/FL Studio/Settings/Hardware/Uppwind Link/`, then restart the DAW. Then set the DAW's MIDI settings as its README says ([Ableton Live](ableton/README.md), [FL Studio](fl-studio/README.md)).
5. **The Uppwind Link plugins (VST3)**, needed for FL Studio, and on a Mac for the sound. The companion carries them and installs them (the setup window, `--setup --apply`, or `uppwind-companion --install-plugins` alone): on Windows into `C:\Program Files\Common Files\VST3`, the folder FL Studio and Live read, after **one administrator prompt from Windows**, for that copy only; on macOS into `~/Library/Audio/Plug-Ins/VST3`, without one. Then let the DAW find them: FL Studio › *Options › Manage plugins* › **Find installed plugins**; Live › *Settings › Plug-Ins* › **Rescan**. Their use: [plugin/README.md](plugin/README.md).
6. **Pair the studio with the companion** (once): see *How pairing works*.
7. **The sound coming back.**
   - **Windows: the companion brings it back, no virtual cable needed** (Windows 10 version 2004 or later). In your DAW, play through Windows rather than ASIO, to an output you don't listen to (a spare output of your audio interface, a monitor's HDMI output…), otherwise you hear everything twice. Ableton Live: *Settings › Audio › Driver Type* **MME/DirectX**. FL Studio: **FL Studio ASIO** (*Show ASIO panel › Output*) or **Primary Sound Driver**. In the studio: *Settings › Uppwind Link › Audio return* › **Companion (no cable)**.
   - **Any driver, ASIO too:** put **Uppwind Link Return** on the DAW's master; the companion takes its sound rather than capturing the DAW, and its *Mute DAW output* option keeps you from hearing it twice.
   - **Only if neither works for you** once everything else is set up: install [VB-CABLE](https://vb-audio.com/Cable/) as a last resort, play the DAW into *CABLE Input* at 48 kHz, and pick *CABLE Output* as the studio's return.
   - **macOS:** **Uppwind Link Return** on the DAW's master is the way (the companion cannot capture an app on a Mac yet); in the studio: *Audio return* › **Companion (no cable)**. Without the companion: [BlackHole 2ch](https://existential.audio/blackhole/) at 48 kHz, picked as the studio's return.
8. **Ready to play.** Add an instrument from *Sounds › Your plugins*, run the latency calibration once (*Latency › Calibrate*), and freeze plugin tracks before export or sharing: a browser cannot render your DAW offline, and other people don't own your plugins.

> **Your MIDI keyboard.** On Windows, a USB MIDI device can only be opened by one application at a time. If your DAW holds the keyboard, the browser can't see it: disable it in the DAW's MIDI settings and restart the browser. The studio forwards what you play.

## Security model, in plain words

The companion is a program on your computer that a web page can talk to. It is built so that only the studio you paired can, and only for what the studio needs.

- **Local only.** It listens on `127.0.0.1`, port 47410 (or the next free one up to 47419), so nothing from the network can reach it. It has no telemetry, no update check and no web client: it never connects to the internet.
- **Who can connect.** A browser page can only connect if it comes from `https://uppwind.lucasmoreau.fr` (the *Origin* check), talks to `127.0.0.1:<port>` exactly (the *Host* check, against DNS rebinding), and proves it knows the **pairing code**. The code holds a 256-bit random secret that never travels over the connection: the companion sends a fresh random challenge, the studio answers with an HMAC-SHA256 of the code over both sides' random values and the port, and the companion answers with its own. The studio sends nothing else, and uses nothing it receives, until it has checked the companion's proof, so a program that listens on the companion's port while the companion is stopped learns nothing. Proofs are compared in constant time; a wrong one is refused, and after five wrong ones the next wrong ones are slowed down. Older studio pages that send the code itself are accepted only until a studio has paired the new way with that code. One case remains for older companions (version 0.1, whose codes start with `UWC-`): with such a code, and only on the port it names, a studio that gets no challenge still sends the code itself, so that those companions keep working; the first time it meets a new companion, the studio switches that pairing to the new handshake for good.
- **The DAW plugins.** The Uppwind Link plugins (Return, Notes) connect to the companion on a second local port, with a secret of their own (not the pairing code), and the same kind of handshake: the companion proves it knows the secret before a plugin sends any sound or plays any note, then the plugin proves it too. The secret never travels; a program that does not know it gets nothing. An older plugin, which sent the secret itself, is refused, and the studio tells you to reinstall the plugins.
- **What pairing allows.** While it runs and the studio is paired, the studio's page can: list the plugin windows open in Ableton Live; see and click, drag, scroll and type inside **those plugin windows and their own menus**; open and close plugin editors in Live; hear the sound of **Ableton Live or FL Studio** (and the plugins they host). Pairing means trusting that website: use the official studio only, and keep the code to yourself.
- **What it cannot do.** It never shows your desktop or any other window, never shows or clicks a dialog box, file browser or captioned window that a plugin opens (those stay in the DAW, on your screen), never reads or writes your files, never runs commands, never captures the sound of any program other than the DAW, never records anything, and never changes your audio settings. Typed text never goes to its log.
- **Ending it.** *Settings › Uppwind Link › Plugin windows › Unpair* in the studio; `uppwind-companion.exe --new-token` makes a new code (every paired browser then has to pair again); `--stop` stops a companion running in the background; `--uninstall` removes it.
- **On your computer.** The pairing code and the plugins' secret are stored in `%USERPROFILE%\.uppwind-companion.json` (`~/.uppwind-companion.json` on macOS), and the token of the Live script's local channel in `%USERPROFILE%\.uppwind-link-dev.json`. On Windows, your profile folder is readable only by you and the administrators; on macOS the companion writes its file readable by you only (mode 0600) and sets that mode again at every start. Any program running under your own account can read them, as with any local app: the companion trusts the programs you run yourself. When the companion stops normally, it removes the plugins' port from that file.
- **Logs.** The background log (`%LOCALAPPDATA%\Uppwind Link\companion.log`) never contains the pairing code (older versions wrote it there; the companion removes such lines when it starts; if you ever shared an old log, run `--new-token`). It does contain the names of your plugins and tracks: look at it before you attach it to a bug report.
- **Never run Live, FL Studio or the companion as administrator.** The companion needs no administrator rights, and an elevated companion would let the paired page drive elevated windows. One exception, on Windows: copying the Uppwind Link plugins into `C:\Program Files\Common Files\VST3`. For that, and only that, the companion starts a second copy of itself with Windows' administrator prompt; that copy takes no file name or folder from anyone (the folder comes from Windows, the files from the program itself), writes the plugins' folder and nothing else (a file there that a DAW still has loaded is set to go at the next restart of Windows), then quits. Refuse the prompt and nothing changes.
- **Developer options.** `--dev-origins`, `--allow-origin` and `--studio-origin` make the companion accept other pages (a local development server). Use them only for a server of your own; the companion prints a warning while they are on.
- **The executable is not signed yet.** Windows SmartScreen warns about downloaded copies. Download releases from this repository only and compare their SHA-256 (`Get-FileHash uppwind-companion.exe`) with the one published with the release.

## How pairing works

1. Start the companion (Start Menu › **Uppwind Link Companion**). Its console shows a line `UWC2-47410-…` (64 characters after the port): that is your pairing code. A companion running in the background never prints it; `uppwind-companion.exe --pairing-code` shows it. (`UWC2` tells the studio that this companion proves it knows the code; the studio then never sends the code itself. Codes of older companions start with `UWC-`.)
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

The companion runs on **Windows 10 / 11**, and in a first version (Terminal only) on **macOS 11 or later**. The audio return without a cable needs Windows 10 version 2004 (build 19041) or later, and a DAW that plays through Windows: an ASIO driver bypasses the Windows mixer, so the companion cannot hear it, but **Uppwind Link Return** on the master works with any driver (and is the way on a Mac).

## Build from source

You need [Rust](https://rustup.rs) (stable, MSVC toolchain) on Windows, and Python 3.7 or later for the tests.

```
cargo build --release --locked --manifest-path companion/Cargo.toml
```

The program is `companion/target/release/uppwind-companion.exe`. Copy only the `.exe`; never share the `.pdb` file next to it (it holds your local paths). Built like this, it carries no plugins (its setup says *not in this build*). With the plugins inside, build them first, then give the companion their bundle (an absolute path):

```
cargo build --release --locked --manifest-path plugin/Cargo.toml
plugin\target\release\uwl-bundle.exe
set UWL_PLUGIN_BUNDLE=%CD%\plugin\target\release\bundled\Uppwind Link.vst3
cargo build --release --locked --manifest-path companion/Cargo.toml
```

On macOS, `sh dev/build-macos.sh` does all of it (universal binaries, the plugins signed before the companion embeds them).

Tests:

```
cargo test --release --locked --manifest-path plugin/Cargo.toml
cargo test --release --locked --manifest-path companion/Cargo.toml -- --skip the_real_registry_in_a_test_key
python -I tests/test_protocol.py
python -I tests/test_fl_script.py
python -I tests/test_ableton.py
```

The skipped test writes a temporary key under `HKCU\Software\UppwindLinkTest` (and deletes it); run it on its own if you want it. The Rust tests open small windows of their own and real connections on `127.0.0.1` (random ports); they never touch Live, FL Studio or a companion that is running. The plugins' installation is tested in temporary folders only: the tests never write a system folder and never ask for administrator rights (`--plugins-dir <folder>` does the same by hand).

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
| everything else: `companion/` (the companion), `plugin/` (the Uppwind Link plugins, their protocol, the test host and the bundler), `tests/`, `dev/`, `fixtures/` | **GNU General Public License, version 3 or (at your option) any later version** ([LICENSE](LICENSE)), with an additional permission for the companion to be linked with Microsoft's WebView2 loader ([LICENSE-EXCEPTION.txt](LICENSE-EXCEPTION.txt)), and one for the plugins to be loaded by any VST 3 or CLAP host, whatever its licence ([LICENSE-EXCEPTION-PLUGINS.txt](LICENSE-EXCEPTION-PLUGINS.txt)) |

The companion embeds copies of the MIT scripts (`--setup` installs them); they keep their MIT licence there. A release of the companion also embeds the Uppwind Link plugins (GPL, like it), with their own third-party notices ([plugin/THIRD-PARTY-NOTICES.txt](plugin/THIRD-PARTY-NOTICES.txt), also printed by `--license`). Both come with ABSOLUTELY NO WARRANTY.

The companion is built with Rust crates under the MIT licence (some also under Apache-2.0 or the Unlicense; a few under Apache-2.0, Unicode-3.0 or CC0-1.0 only) and is linked statically with Microsoft's WebView2 loader (`WebView2LoaderStatic.lib`, under the additional permission above), listed with their notices in [THIRD-PARTY-NOTICES.txt](THIRD-PARTY-NOTICES.txt) (`uppwind-companion.exe --license` prints them). This software is based in part on the work of the Independent JPEG Group.

Ableton and Ableton Live are trademarks of Ableton AG; FL Studio is a trademark of Image-Line Software. VST is a registered trademark of Steinberg Media Technologies GmbH. This project is not affiliated with them.
