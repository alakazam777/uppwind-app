# Uppwind Link companion

A small Windows program that lets [Uppwind Studio](https://uppwind.lucasmoreau.fr/atelier) (a web page) work with the DAW on the same computer:

- **Plugin windows.** It finds the windows of the plugins open in Ableton Live, streams the one the studio shows (JPEG frames, only the changed part while you play), and passes your clicks, drags, wheel and keys back to that window.
- **The DAW's sound, without a virtual cable.** It captures the sound of Ableton Live or FL Studio only (Windows' per-process loopback, the DAW and the plugins it runs) and streams it to the studio in 5 ms packets.
- **Opening and closing plugin editors in Live**, through the UppwindLink Remote Script's local channel (UDP on `127.0.0.1`, with a token).
- **Installing the DAW scripts** (`--setup`), and optionally FL Studio's MIDI settings, with a backup.
- **Installing itself**: double-click the downloaded exe and a setup window does the rest; afterwards an icon in the notification area.

It talks to the studio over a WebSocket on `127.0.0.1` only. See the main [README](../README.md) for installation, the security model and pairing.

Requirements: Windows 10 or 11, 64-bit. The audio return needs Windows 10 version 2004 (build 19041) or later and a DAW that plays through Windows (not ASIO). Opening plugin editors from the studio needs Ableton Live 12.4.3 or later with the UppwindLink Remote Script. The setup and status windows need Microsoft Edge WebView2 Runtime (part of Windows 10 and 11); without it, the setup runs in the console.

## Build

```
cargo build --release --locked --manifest-path companion/Cargo.toml
```

Run it from the repository folder so that cargo reads `.cargo/config.toml`. The program is `companion/target/release/uppwind-companion.exe`. Release builds: [RELEASING.md](../RELEASING.md). **Raise `version` in `Cargo.toml` for every published release**: the setup window compares it with the installed copy to offer an update.

## Setup window and notification icon

1. **SmartScreen** may warn, since the program is not signed yet: *More info* › *Run anyway*. Nothing asks for administrator rights.
2. **The setup window** (a console flashes for a fraction of a second, then hides): what was found (Live, FL Studio, the MIDI ports), **Install** and **Customize**. Install stops only where it must: FL Studio open while its settings are ticked (the companion never closes a DAW), or missing MIDI ports. Customize shows each step: the DAW scripts (one box per Live User Library, one for FL Studio, and FL Studio's MIDI settings, backed up first), the MIDI ports (a **Get loopMIDI** button opens its author's page; nothing is downloaded by the companion), **Start with Windows** and **Open the studio when done**. At the end: the pairing code with **Copy** and **Open the studio** (the code is never put in an address), then what is left to do in Live, FL Studio and for the sound.
3. **Afterwards**: an icon near the clock. Click: the **status window** (the companion and its port, the studio paired or not, the pairing code with Copy, Live and FL Studio and their scripts, MIDI ports, the audio return). Right-click: *Open Uppwind Link*, *Open the studio*, *Start with Windows*, *Quit*. Starting it again from the Start Menu shows the running one (a single instance).
4. **Uninstall**: Settings › Apps › *Uppwind Link* › Uninstall. A window says what goes, what comes back and what stays.

Double-clicking the downloaded exe again: the same version installed opens the status window; an older one opens the setup window in *Update* mode. From a terminal, without options, nothing changes: the companion's console.

| Step | Undone by `--uninstall` |
|---|---|
| the exe in `%LOCALAPPDATA%\Uppwind Link`, the Start Menu shortcut (`--app`) | yes (a running exe is removed just after it exits) |
| the DAW scripts, after a backup | yes: only folders that carry the companion's marker; the folder that was there before is put back |
| FL Studio's MIDI settings (FL Studio closed) | yes, first (FL Studio open: nothing is removed) |
| *Start with Windows* (`--background`) | yes |
| the *Installed apps* entry, `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\UppwindLink` | yes |
| the status window's WebView2 data | yes |

What stays: the backups (`%LOCALAPPDATA%\Uppwind Link\backups`) and the pairing code file (`%USERPROFILE%\.uppwind-companion.json`: reinstalled, the studio stays paired). loopMIDI stays (it is not ours). Never administrator, never `HKLM`; the only registry key written outside FL Studio's is that entry, and its writer refuses any other key.

```
uppwind-companion.exe                    double-click: setup, update or the status window; in a terminal: the console
uppwind-companion.exe --wizard           the setup window (also "Set up again…" in the status window)
uppwind-companion.exe --wizard --quiet   install without a window or a question, with the default choices
uppwind-companion.exe --app              the icon and the status window (the Start Menu shortcut); --background: the icon only
uppwind-companion.exe --uninstall [--quiet]   undo everything (a confirmation window; a question in a terminal)
uppwind-companion.exe --version
--sandbox <folder>                       with any of the above: a trial that changes nothing outside <folder>
--smoke-test [wizard|status|uninstall] [--shot <file.jpg>]   open the real window for 3 s, then close it
--ui-preview <folder>                    the pages as HTML files with a fake companion (for a headless browser)
```

**`--sandbox <folder>`**: the program, its shortcuts (not the real Start Menu), its pairing file (a new token: yours is not read) and its log go into the folder; the *Installed apps* entry goes under `HKCU\Software\UppwindLinkTest\Sandbox\…` (so it does not show in Settings). Live and FL Studio are read but never changed: their steps are simulated. The companion started at the end takes the next free port and has its own single-instance names. To finish: `--sandbox <folder> --uninstall`, then delete the folder.

**The windows' security**: no remote page (the HTML of `src/ui/web/` is a string inside the exe, loaded with `NavigateToString`, origin `null`); CSP `default-src 'none'`, scripts and styles only with this launch's nonce, images as `data:`; no text inserted as HTML. No navigation, new window, download, permission, dropped file, context menu, browser shortcut or developer tools. The page talks to the program through wry's in-process `window.ipc.postMessage`: **no port, no HTTP endpoint**. `src/ui/ipc.rs` accepts a closed list of commands per window, 16 KB at most, values checked; links are named (studio, loopMIDI, VB-CABLE — the last one only in the status window and only after a failed capture), never an address coming from the page; *Copy* copies the code the companion knows. The pairing code is shown in the window only; it is never written to `companion.log`. The windows run on a thread of their own (STA, as WebView2 requires); the server keeps its own limits (16 connections, the studio origin only unless `--dev-origins`/`--allow-origin`).

## Install, start, stop (command line)

```
uppwind-companion.exe --install                 copy to %LOCALAPPDATA%\Uppwind Link\ and add the Start Menu shortcut
uppwind-companion.exe --autostart on            start with Windows, minimized (add --background: no window)
uppwind-companion.exe --autostart off
uppwind-companion.exe --stop                    stop the one with the icon, else the companion named in the state file (only an uppwind-companion*.exe)
uppwind-companion.exe --pairing-code            print the pairing code (a background companion never logs it)
uppwind-companion.exe --uninstall [--quiet]     see above
```

Everything stays in your user profile: no administrator rights, nothing under `HKLM`. An installed copy that is running is renamed aside and removed at the next install. The pairing code survives reinstalls (`%USERPROFILE%\.uppwind-companion.json`). With `--background` or `--app`, the output goes to `%LOCALAPPDATA%\Uppwind Link\companion.log` (restarted past 1 MB), which never contains the pairing code. `UWC_INSTALL_ROOT=<folder>` replaces `%LOCALAPPDATA%` and `%APPDATA%` (tests).

## Run

Start it from the Start Menu (**Uppwind Link Companion**) or run the exe from a terminal. In a terminal it prints the address it listens on, the studio origin it accepts, the pairing code, the plugin windows open in Live and whether Live's script channel is there. Keep the window open; **Ctrl+C** stops it.

| Option | |
|---|---|
| `--port N` | listen on this port (0: any free port); by default 47410, or the next free one up to 47419 |
| `--new-token` | make a new pairing code (every paired browser pairs again) |
| `--state-file PATH` | another state file than `%USERPROFILE%\.uppwind-companion.json` |
| `--capture wgc\|dwm\|print` | force a capture method (by default: Windows.Graphics.Capture, then the window's DWM surface, then PrintWindow) |
| `--dev-origins` | also accept `http://localhost` and `http://127.0.0.1` on ports 5174 and 5747 (a local development server of the studio) |
| `--allow-origin URL` | also accept this origin (`https://…` or a local address only) |
| `--studio-origin URL\|none` | replace the studio origin built into the program (`UPPWIND_STUDIO_ORIGIN` at build time changes the built-in one) |
| `--demo` | a fake plugin window and a test sine instead of Live (no DAW needed) |
| `--test-window [--test-window-at X,Y]` | a test window of the companion itself, offered to the studio like a plugin |
| `--test-tone` | start a test tone in another process, which the studio may capture as target `test-tone` |
| `--plugin-port N` / `--no-plugins` | the port of the listener for the Uppwind Link plugins (by default a random free one, on `127.0.0.1`, with its own secret; the frames are defined in `../plugin/proto`; the plugins are not published yet), or no listener |
| `--license` | the licence, its additional permission for the WebView2 loader, the DAW scripts' MIT licence, the third-party notices |

The companion prints a warning while `--dev-origins`, `--allow-origin` or `--studio-origin` is on. Use them for a server of your own only.

## Diagnostics (console only, nothing is sent anywhere)

```
uppwind-companion.exe --list                       the plugin windows open in Live
uppwind-companion.exe --probe NAME [--seconds S]   capture a plugin window for a few seconds and measure (--any: any visible window)
uppwind-companion.exe --bench NAME                 where the latency goes (capture cost, the plugin's redraw rate)
uppwind-companion.exe --describe NAME              a plugin window's child windows and their DPI
uppwind-companion.exe --capture-test live|fl|PID|NAME|tone [--seconds N]   the audio levels of a process, per second
uppwind-companion.exe --tone [--seconds N]         play a test tone (sine and clicks)
uppwind-companion.exe --setup [--apply] [--remove] [--json] [--daw ableton|fl|all] [--fl-settings]
```

Environment variables for diagnostics: `UWC_TRACE=1` (each capture and its decision in the log), `UWC_MOUSE=send` (mouse messages by SendMessage), `UWC_DPI=logical` (convert pointer coordinates for DPI-unaware plugins), `UWC_LOOPBACK=…` (the format asked from Windows for the audio capture), `UWC_DWM_DUMP=<folder>` (writes captured DWM surfaces there: never set it on a machine whose plugin windows you would not share).

## `--setup`

`--setup` looks at the computer and says what it would do; nothing changes without `--apply`. It reads the installed DAWs and their versions, Live's User Library (`Library.cfg`) and the settings Live last applied (`Log.txt`), FL Studio's data folder and its MIDI and audio settings (registry, read only), the MIDI ports, the audio devices and the installed scripts. With `--apply` it installs or updates the scripts (a copy of what was there goes to `%LOCALAPPDATA%\Uppwind Link\backups` first, and is put back if a write fails). With `--fl-settings` it also writes FL Studio's MIDI settings and its *Auto close* box under `HKCU\Software\Image-Line\FL Studio <NN>` only, FL closed, after exporting a `.reg` backup of FL's `Devices` key; `--remove --fl-settings` and `--uninstall` put back the values it wrote. It never touches Live's preferences, a driver or `HKLM`.

Audio: since the companion brings the sound back itself, `--setup` asks for a DAW that plays through Windows (Live: MME/DirectX; FL Studio: FL Studio ASIO or Primary Sound Driver) to an output you don't listen to. It offers VB-CABLE only on a Windows that cannot capture a single app.

## Protocol

JSON text messages, binary frames for images and sound, over `ws://127.0.0.1:<port>`. The handshake is refused (HTTP 403) unless `Host` is exactly `127.0.0.1:<port>` and `Origin` is an allowed origin. The first message must be `hello` with the pairing token, or the connection closes (4400 not hello, 4401 wrong token, 4429 too many wrong tokens).

Studio → companion: `hello{version,token}`, `list`, `devices`, `live`, `open{path}`, `close{path}`, `watch{id,fps,quality}`, `unwatch`, `ack{seq}`, `ping{t}`, `mode{mode: direct|focus}`, `input{kind: pointer|wheel|key|text, id, n, t, …}` (x, y in pixels of the image, with its size iw, ih; buttons as in the DOM), `audio{action: start|stop, target: auto|live|fl|test-tone|<pid>, rate, format: f32|i16, ms}`, `instances`, `midi{to: <id>|tag:<n>, events}`, `emitted{id}`, `link{sysex?: [hex…], stop?}`, `routes`, and `place{x,y,w,h,dpr}` (read, not used yet).

Companion → studio: `hello` (with `input`, `modes`, `audio`, `audioFormats`, `audioError`, `plugins`, `linkTransport`), `windows`, `devices`, `live` (with `script`, `version`, `link`), `opened`, `closed`, `watching`, `unwatched`, `pong`, `mode`, `inputack{n,t,recv,inj,ok,error?}`, `audio{state: on|off, …}`, `instances`, `emitted`, `link{sysex}` / `link{state: up|down|off}`, `routes`, `error`.

**Ableton Live without MIDI ports** (`src/link_relay.rs`): `link` carries the Uppwind Link protocol's SysEx chunks (hex; only `F0 7D 55 57 … F7` chunks of at most 212 bytes are relayed) to the Remote Script's local channel (`LINK`, with the script's token) and brings back what the script has for the studio. The script reads its channel every ~100 ms and cannot send on its own, so a thread per studio connection collects its box every 40 ms, at once when the studio speaks or the script says `more`. `live.link` (the script's state file says `"link": 1`, script 1.3.0 and later) tells the studio this route is there. Notes go to the Notes plugin of the right Live track with `midi{to: "tag:<n>"}`: the Remote Script gives each Uppwind Link Notes a number (its *Link tag* parameter), the plugin announces it (frame `TAG`), the companion finds the instance. `routes` returns the script's `ROUTES` (each track's input, helper track, Notes numbers) for diagnostics.

An image: `UWF1`, the header length (u32), a JSON header `{seq,id,w,h,t,enc,cap,sw,sh,q}` (a tile adds `{x,y}`), then the JPEG. The studio sends `ack` after each image; at most 2 images are in flight. A sound packet: `UWA1`, a 36-byte little-endian header (`seq` u32, `pos` u64, `t` f64 capture time in ms since 1970, `rate` u32, `frames` u16, `ch` u8, `fmt` u8: 1 float32 / 2 int16, `flags` u8: 1 silence / 2 discontinuity), then interleaved samples. Details: `src/protocol.rs`, `src/audio/packet.rs`.

Limits: text messages up to 16 KB, at most 1000 messages per second, a connection closed after 50 malformed messages, any binary message from the studio refused, 16 connections at a time, 32 MB of pending output per connection.

## How it stays inside its lane

- **Windows**: `watch` accepts only a plugin window of Ableton Live (a tool window of a `Ableton Live*.exe` process, owned by Live, titled `Plugin/Track`) or the companion's own test window. Every input checks that the same window of the same process is still shown. Input goes to that window, its child windows and its menus: windows on the plugin's thread that it owns, or tool or topmost pop-ups that overlap it, never Live's main window, another plugin, a dialog box (`#32770`), a window with a title bar or a window hosting an Explorer view. Keys go only to a menu the plugin owns, or to the plugin. In Focus mode the real cursor is checked over the plugin before each click and never leaves it during a drag.
- **Keys**: the Windows key is never sent; Alt+F4 is posted to the plugin's own child window only.
- **Sound**: only a process whose program is `Ableton Live*.exe`, `FL64.exe` or `FL.exe` (and its child processes), or the test tone the companion started. A refused process id gets the same answer whether it exists or not, without its name.
- **Errors sent to the studio** carry no file path and no other program's name.
- **Logs** never hold typed text or the pairing code; they hold window, plugin and track names, counts and timings.

Known limits: the Live and companion state files are readable by any program running under your account (like your other files); the program is not signed yet; a program that names itself `Ableton Live….exe` and imitates Live's windows would be treated as Live (it already runs as you).

## macOS (first milestone, 26 Oct 2026)

`sh dev/build-macos.sh` (from `tools/uppwind-link`) builds the companion and the plugins for Apple silicon and Intel, joins them with `lipo` and signs them ad hoc. The console companion works the same way as on Windows, with these differences:

| Part | macOS | Status |
|---|---|---|
| Plugin windows of Live | `CGWindowListCopyWindowInfo`, Live's windows titled `plugin/track` (`src/macos.rs`, `plugins.rs`) | built; titles need **Screen Recording** |
| Capture | `CGWindowListCreateImage` for one window, looked up at run time | built; needs **Screen Recording**. TODO: ScreenCaptureKit once Apple removes it |
| Clicks, wheel, keys | `CGEventPostToPid` to Live, in screen coordinates (`input.rs`) | built; needs **Accessibility**; Focus mode does not exist on Mac yet |
| MIDI ports | the companion creates `Uppwind Link` and `Uppwind Link Back` itself (CoreMIDI, `src/midi_ports.rs`) while it runs; `--no-midi-ports` turns them off | built, not yet tried with FL or Chrome |
| DAW's sound | the **Uppwind Link Return** plugin on the master (tested end to end with the test host) | works. TODO: Core Audio process taps (macOS 14.2+) to capture an app without the plugin |
| `--install`, `--autostart`, `--background`, `--stop` | `~/Library/Application Support/Uppwind Link/uppwind-companion`, a LaunchAgent `~/Library/LaunchAgents/com.uppwind.link.companion.plist` (written, never loaded by the companion: it starts at the next login) | built and tested in a temporary folder |
| `--setup` | Live: `~/Library/Preferences/Ableton`, `~/Music/Ableton/User Library`; FL: `/Applications/FL Studio *.app`, `~/Documents/Image-Line` | built; FL's MIDI settings are not read on Mac (no registry) |
| Setup window, notification icon | `src/ui` builds on WKWebView; the icon (`src/tray`) and the setup window's install steps stay Windows-only | TODO |

`--list` prints whether Screen Recording and Accessibility are allowed, without asking (no macOS prompt). The first capture makes macOS ask.

## Files

`src/main.rs` (options, state file, listening), `src/session.rs` (a connection: handshake, pairing, messages, frames, audio), `src/security.rs` (Host, Origin, token), `src/protocol.rs`, `src/plugins.rs` (Live's plugin windows), `src/popups.rs` (a plugin's menus), `src/capture.rs` (WGC, DWM surface, PrintWindow), `src/stream.rs` (the capture thread: tiles, gestures), `src/encode.rs` (JPEG), `src/input.rs` (Direct and Focus input), `src/live.rs` (the Live script's local channel), `src/link_relay.rs` (the studio's link to the Live script without MIDI ports), `src/audio/` (process loopback, packets, the allow-list), `src/plugin_link/` (the Uppwind Link plugins' listener: Return's sound, Notes' MIDI), `src/install.rs`, `src/setup/` (`--setup`), `src/wizard/` (first launch, the setup window's steps, install and uninstall, the *Installed apps* entry), `src/ui/` (the WebView2 window, its IPC, the pages in `web/`), `src/tray/` (the icon, the single instance, the status window), `src/bench.rs`, `src/testwin.rs`, `build.rs` (embeds the DAW scripts and the UI assets: explicit lists), `build/winres.rs` (the exe's icon and version block, written without `rc.exe`).

## Tests

```
cargo test --release --locked --manifest-path companion/Cargo.toml -- --skip the_real_registry_in_a_test_key
```

Security checks, the protocol, Windows messages and coordinates, tiles, real WebSocket connections in demo mode (pairing, refusals, the connection cap, audio after pairing only), a real test window that receives clicks, drags, wheel and keys, menus in their own windows, `--setup` on fake machines, the setup window's steps, install and uninstall in a temporary folder. The skipped test writes and then deletes a key under `HKCU\Software\UppwindLinkTest`.
