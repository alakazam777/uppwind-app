# Uppwind Link plugins

One binary, two plugins (VST3), built with [nice-plug](https://codeberg.org/RustAudio/nice-plug):

- **Uppwind Link Return**: an effect for the DAW's **master** track. It sends the DAW's sound to the companion, which passes it to the studio over the WebSocket the studio has already paired (the same `UWA1` packets as the companion's own per-process capture). The sound does not go through the Windows mixer, so it works with **ASIO** too. Option *Mute DAW output*: the DAW goes silent and only the studio plays (no doubled sound).
- **Uppwind Link Notes**: a **silent** instrument with a MIDI output. It receives the studio's notes, CC and pitch bend from the companion and outputs them as MIDI, placed at the right sample. In Live, the track of the instrument to play takes *MIDI From: <Notes track> / Uppwind Link Notes*; in FL Studio, the wrapper's MIDI output port. No virtual MIDI cable is needed for the notes.

```
 DAW (ASIO, MME, WASAPI: any)
   master ── Uppwind Link Return ──(lock-free queue)──► network thread ──TCP 127.0.0.1──┐
   "Link Notes" track ── Uppwind Link Notes ◄──(lock-free queue)── network thread ◄─────┤
        └─ MIDI From ─► an instrument track                                              ▼
 studio (browser) ◄══ ws://127.0.0.1:<port> (paired: sound "UWA1", notes "midi") ══ companion (plugin_link)
```

With Ableton Live, the studio sends its notes to the plugin through the companion (no loopMIDI): the UppwindLink Remote Script (1.3 and later) makes a helper track per played track, loads Uppwind Link Notes on it, gives it a number in its **Link tag** parameter (0..255, kept in the set) and points the played track's *MIDI From* at it. The plugin announces the number to the companion (frame `TAG`); the studio sends that track's notes to `tag:<n>`.

FL Studio: an Uppwind Link Notes in the channel rack outputs through its wrapper's MIDI *Output port*, and the instrument listening on the same *Input port* plays it. The FL script (1.2 and later) writes the Notes' port number into its *Link tag*; the studio sends that channel's notes to the Notes of FL that carries it. Without the script ("plugins only"), each connected Notes is an instrument of its own in the studio. Both plugins also report the DAW's **tempo** and play state in their `STATS` frame (`"tempo"` in BPM or `null`, `"playing"`), sent every 2 s and at once when either changes: the studio follows FL's tempo without its script.

## Install

**The companion installs them**: they are inside `uppwind-companion` (a release build embeds this bundle, see below), and nothing is downloaded.

- **Windows**: into `C:\Program Files\Common Files\VST3\Uppwind Link.vst3`, the only VST3 folder FL Studio scans (Ableton Live reads it too). Writing there needs administrator rights: the companion asks Windows **once** (a UAC prompt) and runs itself elevated for that copy only; everything else it does stays in your user profile. If a DAW has the plugins loaded, the old file is renamed aside (a loaded DLL cannot be replaced, but it can be renamed), the new one written, and the old one removed once the DAW lets go of it: restart the DAW.
- **macOS**: into `~/Library/Audio/Plug-Ins/VST3/Uppwind Link.vst3`, no administrator rights, byte for byte (the bundle's signature stays valid). On a Mac, **Uppwind Link Return** on the master is how the DAW's sound reaches the studio.
- How: the setup window (*Uppwind Link plugins (VST3)*, ticked by default), `uppwind-companion --setup --apply` (with the DAW scripts; `--skip-plugins` leaves them out), or `uppwind-companion --install-plugins` alone (`--dry-run` says what it would do, `--remove` takes them out). Uninstalling the companion removes the plugins it installed (only those: a bundle you put there yourself is left alone).
- Then: FL Studio › *Options › Manage plugins* › **Find installed plugins**; Ableton Live › *Settings › Plug-Ins* › *Use VST3 Plug-In System Folders* on, **Rescan**.

## Build

From the repository's root (so that cargo reads `.cargo/config.toml`):

```
cargo build --release --locked --manifest-path plugin/Cargo.toml
plugin/target/release/uwl-bundle.exe
```

`uwl-bundle` writes the VST3 bundle (`Uppwind Link.vst3\Contents\x86_64-win\Uppwind Link.vst3`) and `Uppwind Link.clap` into `plugin/target/release/bundled/`. To build a companion that carries them, give it that bundle (an absolute path): `UWL_PLUGIN_BUNDLE=<…>\plugin\target\release\bundled\Uppwind Link.vst3`, then build the companion; `companion/build.rs` accepts only this exact layout. A published build adds the path remapping of [RELEASING.md](../RELEASING.md) to both builds.

A development shortcut, without the companion: `uwl-bundle --install` copies the VST3 bundle into `%LOCALAPPDATA%\Programs\Common\VST3` (your own VST3 folder, no administrator rights; Ableton Live reads it as a custom folder, FL Studio never does). To remove it, delete that `Uppwind Link.vst3` folder.

macOS: `sh dev/build-macos.sh` builds the plugins for arm64 and x86_64, joins them with `lipo`, bundles them (`Contents/Info.plist`, `Contents/PkgInfo`, `Contents/MacOS/Uppwind Link`) and signs them (ad hoc, or `UWL_PLUGIN_SIGN_IDENTITY` for a Developer ID, with the hardened runtime and a timestamp), then builds the companion with that signed bundle inside; all in `dist/macos/`. `plugin/target/release/uwl-bundle --sign -` does the bundle alone (`--install`: into `~/Library/Audio/Plug-Ins/VST3`). Steinberg's `validator` (VST3 SDK) passes 94/94 on the universal bundle (macOS, Apple silicon). The test host (`uwl-host`) runs on macOS too (`dlopen`, `bundleEntry`). A release is signed with a Developer ID and notarized in GitHub Actions: [RELEASING.md](../RELEASING.md), `.github/workflows/macos-build.yml`.

## How a plugin finds the companion

The companion listens for the plugins on a second socket, `127.0.0.1` only, on a random port, with a **secret of its own** (not the studio's pairing token: a plugin can neither capture nor click, it only exchanges sound and notes). Both go into the companion's state file (`pluginPort`, `pluginSecret` in `%USERPROFILE%\.uppwind-companion.json`, `~/.uppwind-companion.json` on macOS; `UWL_STATE_FILE` replaces that path). `--no-plugins` turns the listener off. A plugin that finds no companion (or an older one) does nothing and waits.

The secret never travels (protocol 2). The plugin's `HELLO` carries a fresh 32-byte nonce `Np` from the operating system. The companion answers `CHALLENGE` with its own nonce `Nc` and `Pc = HMAC-SHA256(secret, "uwl-companion|" + port + "|" + hex(Np) + "|" + hex(Nc))`; the plugin checks `Pc` (constant time) **before** it sends anything else: a wrong or missing proof, or a `WELCOME` without a checked `CHALLENGE`, and it closes, waits (backoff), sends no sound, plays no note and keeps its identity. Then it sends `AUTH` with `Pp = HMAC-SHA256(secret, "uwl-plugin|" + port + "|" + hex(Nc) + "|" + hex(Np))`; the companion checks it (constant time) before it registers the plugin or reads any frame, and answers `WELCOME`. The port in both proofs is the one the plugin called and the companion listens on, so a program on a stale port from the state file cannot relay the handshake to the real companion. The companion closes a connection that is not from `127.0.0.1`, takes 16 connections at a time, wants `HELLO` within 2 s and 1 KB and `AUTH` within 2 s more, and slows down wrong proofs after 5 of them (the right proof always passes). A plugin of protocol 1 (the secret in clear) is refused (`WELCOME` 2) and the studio says to reinstall the plugins. The plugins' `process()` never allocates, waits or touches the network; the network thread starts at `activate()` only (a plugin that is merely scanned does not connect), and nothing is sent while the DAW renders offline.

## Protocol (`proto/`, uwl-proto, shared with the companion)

TCP on `127.0.0.1`, little-endian frames `[u32 length][u8 type][body]`, version 2: `HELLO` ("UWLP", kind, nonce, instance id, sample rate, block size, pid, name, host exe), `CHALLENGE` (the companion's nonce and proof), `AUTH` (the plugin's proof), `WELCOME` (the verdict), `AUDIO` (position, time in ms since 1970, rate, frames, channels, flags 1 silence / 2 discontinuity / 4 bypassed, interleaved f32), `FORMAT`, `EVENTS` (16 bytes each: time, number, length, data[3]; channel messages only), `EMITTED` (where and when a Notes plugin output its events), `TAG` (a Notes' *Link tag*, at connect and on change), `STATS` (JSON: counters, and the host's `tempo` and `playing`), `BYE`.

Studio → companion, after pairing: `{"type":"instances"}` → the connected plugins; `{"type":"audio","action":"start","target":"plugin"|"plugin:<id>",…}` → a Return's sound (`auto`, `live` and `fl` also prefer a connected Return of that DAW); `{"type":"midi","to":"<id>"|"tag:<n>","events":[{"t":<ms since 1970>,"d":[0x90,60,100],"n":1}]}` → a Notes plugin (without `t`: at once; at most 256 events, 10 s in the past to 30 s ahead); `{"type":"emitted","id":"<id>"}` → its output log.

## Files

- `plugins/`: `ring.rs` (lock-free SPSC queue, all allocated up front), `clock.rs` (QPC and Windows time, a 0.5 Hz delay-locked loop: sub-blocks, jumps, drift), `sched.rs` (the sorted buffer of 2048 messages, held notes, panic), `link.rs` (the network thread: state file, reconnection, clock conversion), `ret.rs`, `notes.rs`.
- `testhost/`: `uwl-host`, a small VST3 test host that loads the real bundle and calls `process()` from a thread clocked like an audio interface's callback (`scan`, `return`, `notes`, `play`), and `uwl-bundle`. `return`, `notes` and `play` need the test companion's state file, `--state-file FILE` or `UWL_STATE_FILE=FILE`, and give it to the plugin they load; they never read the default one on their own. The test host and the bundler are development tools, not part of the bundle.
- Companion side: `companion/src/plugin_link/` (the listener, the hub, one connection per plugin, the resampler), wired into `audio/feed.rs`, `protocol.rs`, `session.rs` and `main.rs` (`--plugin-port N`, `--no-plugins`).

## Tests

```
cargo test --release --locked --manifest-path plugin/Cargo.toml
```

uwl-proto (frames, limits), the plugins (the lock-free queue across two threads, the clock loop against an interface running 100 ppm fast, sub-blocks, placement, late events, panic). The companion's `plugin_link` tests connect fake plugins over real sockets.

## Known limits

- The DAW track's name is not announced (the instance shows as "Uppwind Link Notes 46f4"); in Live the Remote Script knows which track holds which Notes (its Link tag).
- 44.1 kHz to 48 kHz conversion is linear (fine for monitoring, not for a master).
- A program that can listen on `127.0.0.1` can still make a plugin retry (every 2 s at most) or keep the companion's 16 plugin connections busy: a denial of service, not a leak.
- A duplicated track keeps the original's identity: the companion refuses the copy, which takes a new one.
- No plugin window (DAWs show the generic parameters: *Mute DAW output*, *Bypass*). Windows: not code-signed (like the companion); macOS: signed with a Developer ID in a release.

Licence: GPL-3.0-or-later ([../LICENSE](../LICENSE)), with an additional permission: any VST 3 or CLAP host may load the plugins, whatever its licence ([../LICENSE-EXCEPTION-PLUGINS.txt](../LICENSE-EXCEPTION-PLUGINS.txt)); the crates it is built with: [THIRD-PARTY-NOTICES.txt](THIRD-PARTY-NOTICES.txt). VST is a registered trademark of Steinberg Media Technologies GmbH.
