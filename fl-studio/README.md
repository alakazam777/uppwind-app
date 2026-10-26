# Uppwind Link for FL Studio

With this script, FL Studio becomes the plugin host for Uppwind Studio. The studio plays your channel-rack instruments, sets their volume, pan and mute, and hears them back: on Windows through the Uppwind Link companion, on macOS through BlackHole.

**You need:** FL Studio 21.2 or newer (2024, 2025, 2026), on Windows or macOS. You also need the two MIDI ports described in the [main README](../README.md) (loopMIDI on Windows, the IAC Driver on macOS) and, for the sound, the Uppwind Link companion on Windows or BlackHole on macOS.

**The easy way:** in Uppwind Studio, *Settings › Uppwind Link* has a setup guide. Pick *FL Studio*: it shows these steps and checks each one by itself — the ports, FL answering, and the sound coming back (**Test sound**).

## Three ways to run FL Studio with the studio

| | Commands (channel list, volume, pan, mute, tempo) | Notes | Sound |
|---|---|---|---|
| **Script + plugins** (recommended) | this script, over loopMIDI | **Uppwind Link Notes**, sample-accurate (channels without one: the script) | **Uppwind Link Return** on the Master |
| **Plugins only** (no loopMIDI at all) | none: the studio cannot move FL's channel faders or set its tempo (it follows FL's tempo, and can send CC 7 / CC 10) | Uppwind Link Notes | Uppwind Link Return |
| **Script only** | this script, over loopMIDI | this script (`channels.midiNoteOn`, timed by FL's script tick) | Return, the companion's capture, or a virtual cable |

Why the commands still need loopMIDI: FL Studio runs a script only while it is attached to a MIDI input, and it does not let scripts open a network socket (`socket.socket()` fails inside FL). The plugins talk to the companion themselves, so they need neither.

### The Uppwind Link plugins in FL Studio (once per instrument)

FL's scripting API cannot load a plugin or change a channel's MIDI ports (`channels.getChannelMidiInPort` can only read them), so this part is manual:

1. *Options › Manage plugins*: make sure the VST3 folder with *Uppwind Link.vst3* is scanned.
2. Channel rack › **+** › **Uppwind Link Notes**.
3. In its wrapper (the gear icon) › *Settings* › *MIDI*: set **Input port** and **Output port** to the same free number, e.g. **10**. In *Processing*, untick **Allow smart disable** (otherwise FL may stop calling it, and the notes stop).
4. Your instrument (a VST: Diva, Serum…) › its wrapper › *Settings* › *MIDI* › **Input port: 10**, the same number.
5. One Uppwind Link Notes per instrument, each on its own number (11, 12…). FL's native instruments (3xOsc, FLEX…) have no MIDI port: with the script, they play through loopMIDI.
6. Mixer › **Master** › an effect slot › **Uppwind Link Return** (untick *Allow smart disable* in its wrapper too).

With the script, it writes each Notes' port number into its *Link tag* parameter: the studio sends a channel's notes to the Notes on that channel's Input port. Without the script, every connected Uppwind Link Notes is listed in the studio as an instrument (*Uppwind Link Notes 1, 2…*, renamed in *Settings › Uppwind Link*). Its notes go to whatever instrument listens on its port.

## 1. Install the script

Copy the **`Uppwind Link`** folder, which holds `device_UppwindLink.py` and `uwlink_protocol.py`, into FL's *Hardware* folder:

- **Windows:** `Documents\Image-Line\FL Studio\Settings\Hardware\Uppwind Link\`
- **macOS:** `~/Documents/Image-Line/FL Studio/Settings/Hardware/Uppwind Link/`

If you moved FL's user data folder (*Options › File settings › User data folder*), use `FL Studio/Settings/Hardware` inside that folder instead. Then **restart FL Studio**.

## 2. MIDI settings (*Options › MIDI settings*)

| List | Port | Set it like this |
|---|---|---|
| **Input** | `Uppwind Link` | **Enable**, *Controller type* = **Uppwind Link (user)**, *Port* = a number no other device uses, e.g. **77** |
| **Input** | `Uppwind Link Back` | leave **disabled** |
| **Output** | `Uppwind Link Back` | *Port* = the **same number** (77) |
| **Output** | `Uppwind Link` | leave **without** a port number |

The matching port number is how FL sends the script's replies to `Uppwind Link Back`. **Put the 77 on the `Uppwind Link Back` output, not on the `Uppwind Link` output:** with the number on the wrong output, *Script output* still says "ready" but FL's replies go nowhere useful and the studio keeps waiting. On macOS, FL lists the IAC buses as *IAC Driver Uppwind Link* and *IAC Driver Uppwind Link Back*; set them up the same way.

To check the script, open *View › Script output* and pick the *Uppwind Link* tab. You should see:

```
[Uppwind Link] v1.2.0 ready (FL API 40, input port 77). Waiting for Uppwind Studio.
```

## 3. Audio settings (*Options › Audio settings*)

- **Windows (the default): Uppwind Link Return on FL's Master mixer track**, through the Uppwind Link companion: any audio driver works, ASIO included, no virtual cable. In the studio, *Settings › Uppwind Link › Audio return*: **Companion (no cable)**; the companion picks the Return by itself.
- **Without the Return plugin**, the companion captures FL's sound through Windows: set *Device* to **FL Studio ASIO** (then *Show ASIO panel › Output*) or **Primary Sound Driver**, playing to an output you don't listen to (a spare output of your audio interface, a monitor's HDMI output…), or you hear everything twice. Your sound card's own ASIO driver can't be heard this way.
- **Last resort only:** install VB-CABLE and play FL into **CABLE Input (VB-Audio Virtual Cable)** (*FL Studio ASIO › Show ASIO panel › Output*, or *CABLE Input* in the device list), at 48 kHz, the cable's rate, and pick *Virtual cable* under *Audio return*.
- **macOS:** set the output device to **BlackHole 2ch**, at 48 kHz.
- **Untick *Auto close device*.** Otherwise FL releases the audio device when the browser is in front, and the studio hears nothing.
- Set *Buffer length* to 256 samples, or 512 if it crackles.

## 4. Use it

1. Open Uppwind Studio in **Chrome or Edge**, go to *Settings › Uppwind Link*, and click **Connect**.
2. *SOUNDS › YOUR PLUGINS* lists your channel rack: every channel with an instrument. Automation clips and audio clips aren't listed. Add a plugin in FL's channel rack and it appears in the studio instantly.
3. Add it to the song from the studio. Its notes, volume, pan, mute and solo now come from the studio.

What the script does with each kind of event:

| Event | What happens |
|---|---|
| Notes and velocity | Sent to the bound channel with `channels.midiNoteOn`. Nothing reaches FL's *selected* channel. A channel played by an Uppwind Link Notes gets its notes, sustain, pitch bend and CCs from that plugin instead, as real MIDI. |
| Sustain pedal (CC 64) | Emulated by the script: note-offs wait for pedal-up. Works with every instrument. |
| Pitch bend | Moves the channel's pitch knob, within the channel's pitch range. |
| Mod wheel and expression (CC 1, 11), CC 7, CC 10 | Reach VST/AU plugins through FL's *MIDI CC* plugin parameters. FL's native instruments don't take MIDI CCs. |
| CC 120 / 123 | Release every note of that slot. |
| Volume | The studio sends a linear gain. The script converts it with FL's channel volume law (the one in `utils.VolTodB`), so −6 dB in the studio is −6 dB on the FL channel. |
| Solo | Silences the *other* Link-bound channels only. The rest of your project is untouched. |
| Moving or inserting channels | The link follows the instrument. Renaming keeps it. |
| Deleting a channel | Its link is dropped. |
| Tempo | The studio and FL share one tempo (since version 1.1.0). The studio's tempo sets FL's (`general.processRECEvent(midi.REC_Tempo, …)`); a tempo change in FL is read every 50 ms and sent to the studio, which follows. The tempo the studio set is never sent back. |
| Loading another project | All links are cleared. Its tempo goes to the studio. |
| Disconnect, unbind, studio reload | FL gets back the channel's own volume, pan and mute. |

## What FL's scripting API cannot do

FL's MIDI scripting API has no function to load a plugin, list the installed plugins, delete a channel or set a channel's MIDI ports. So the script reports only these capabilities: `list`, `bind`, `set`, `audition`, `tempo` and `notes` (it can route a channel's notes through an Uppwind Link Notes that is already there). The studio then says *"Add the plugin in FL's channel rack; it appears here instantly."* The *Uppwind Link Notes* channels are not listed: they are not instruments.

## Troubleshooting

- **The studio says FL doesn't answer.**
  - Is the script in *Script output*? If not, check the folder name and location, then restart FL.
  - Are both port numbers the same?
  - Is the output port number on `Uppwind Link Back`? If it is on the `Uppwind Link` output instead, FL's replies never reach the studio, even though *Script output* says "ready". Clear it there and give it to `Uppwind Link Back`.
  - Is loopMIDI running?
- ***Script output* says "no output port linked".** Give the `Uppwind Link Back` **output** the same *Port* number as the `Uppwind Link` input.
- **Notes play on the selected channel instead of the bound one.** The `Uppwind Link` input's *Controller type* isn't *Uppwind Link (user)*.
- **The browser doesn't see your MIDI keyboard, or it is silent in the studio.** FL is holding it: on Windows, a USB MIDI device is open in one app at a time. Disable the keyboard in FL's MIDI settings, in the *Input* list and in the *Output* list, then restart the browser. The studio forwards what you play to FL.
- **A loopMIDI port turns red.** FL is sending into `Uppwind Link` (an output with a port number). Clear that port number.
- **A channel still plays through loopMIDI although it has an Uppwind Link Notes.** *Script output* says why (`notes for '…' come through loopMIDI: …`): the instrument's *Input port* and the Notes' *Input* and *Output* ports must carry the same number (1 to 255), and the Notes must reach the companion.
- **The studio says the Notes gets the notes but FL does not run it.** Untick *Allow smart disable* in its wrapper's *Processing* tab, and check that its channel is not muted.

## Licence

The script is under the MIT licence (`LICENSE-MIT`, shipped with it). Copyright (C) 2026 Uppwind. FL Studio is a trademark of Image-Line Software; this project is not affiliated with Image-Line.
