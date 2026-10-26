# Uppwind Link for FL Studio

With this script, FL Studio becomes the plugin host for Uppwind Studio. The studio plays your channel-rack instruments, sets their volume, pan and mute, and hears them back: on Windows through the Uppwind Link companion, on macOS through BlackHole.

**You need:** FL Studio 21.2 or newer (2024, 2025, 2026), on Windows or macOS. You also need the two MIDI ports described in the [main README](../README.md) (loopMIDI on Windows, the IAC Driver on macOS) and, for the sound, the Uppwind Link companion on Windows or BlackHole on macOS.

**The easy way:** in Uppwind Studio, *Settings › Uppwind Link* has a setup guide. Pick *FL Studio*: it shows these steps and checks each one by itself — the ports, FL answering, and the sound coming back (**Test sound**).

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
[Uppwind Link] v1.1.0 ready (FL API 40, input port 77). Waiting for Uppwind Studio.
```

## 3. Audio settings (*Options › Audio settings*)

- **Windows (the default): the Uppwind Link companion brings FL's sound back to the studio**, no virtual cable needed. Set *Device* to **FL Studio ASIO** (then *Show ASIO panel › Output*) or **Primary Sound Driver**, playing to an output you don't listen to (a spare output of your audio interface, a monitor's HDMI output…), or you hear everything twice. Your sound card's own ASIO driver can't be heard by the companion. In the studio, *Settings › Uppwind Link › Audio return*: **Companion (no cable)**.
- **Only if that does not work for you:** install VB-CABLE and play FL into **CABLE Input (VB-Audio Virtual Cable)** (*FL Studio ASIO › Show ASIO panel › Output*, or *CABLE Input* in the device list), at 48 kHz, the cable's rate.
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
| Notes and velocity | Sent to the bound channel with `channels.midiNoteOn`. Nothing reaches FL's *selected* channel. |
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

FL's MIDI scripting API has no function to load a plugin, list the installed plugins or delete a channel. So the script reports only these capabilities: `list`, `bind`, `set`, `audition` and `tempo`. The studio then says *"Add the plugin in FL's channel rack; it appears here instantly."*

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

## Licence

The script is under the MIT licence (`LICENSE-MIT`, shipped with it). Copyright (C) 2026 Uppwind. FL Studio is a trademark of Image-Line Software; this project is not affiliated with Image-Line.
