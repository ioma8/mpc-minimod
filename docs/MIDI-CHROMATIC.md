# External MIDI chromatic playback in 16 Levels

The release build now transposes incoming external MIDI note-ons chromatically
while 16 Levels is active. The firmware still receives and routes every event,
so its selected-pad routing, note-off matching, sustain and audio engine remain
in control. Root is MIDI note 60 (C3), matching SampQOL's documented default.

## User setup

1. In MIDI Config, select the external keyboard's port and enable **Pad MIDI
   In**.
2. Select the sample pad to play and turn on **16 LEVELS**.
3. Play the keyboard. C3 plays at the sample's original pitch; each semitone
   above or below shifts it by one semitone. Chords, key releases and sustain
   continue through the firmware's own MIDI path.

Outside 16 Levels, MIDI events are passed through unchanged. The existing
PLUS/MINUS octave control remains a separate 16 Levels feature.

## Firmware path and evidence

The SampQOL v0.1.5 package supplied for this work documented a MIDI **Keys**
mode that plays the selected pad chromatically, with C3 as the default root.
Its `mpcqol.so` was inspected with the Ghidra bridge and its callback at
firmware file offset `0x211b090` showed the shape of the path. The final
implementation was derived from the firmware itself.

Decompiling the already-analyzed Ghidra project (`mpcsample.bin`, VA = file VA
+ `0x100000`) plus captured register state gave the exact contract:

* `0x211b090` — dispatcher. Routing is keyed by **source**; the note slot comes
  from `event+0x30`; type `3` with velocity `0` is a note-off.
* `0x211aa30` → `0x2215070` — decode the event into the note struct; the slot
  byte is recomputed before the voice call.
* `0x21472f0` — 16 Levels writer. Writes exactly one parameter slot per level
  0..15 (velocity, percentage, filter bits, or the pitch setter).
* `0x27e4400` — property setter. Normalizes a value into `props[index+2]`;
  index 0 has range −120..120 with 241 steps. The app calls it with
  `w3 = level − originalPad` and `event+0x30 = selected pad`.

The mod therefore rebuilds the note for the selected pad instead of trusting the
incoming pad mapping:

1. `event+0x14` mask cleared, `event+0x30 = selected pad`;
2. setter trampoline called with `w3 = note - 60`
   (`orig_pitch_off(event+0x14, 0, 0, note-60)`);
3. dispatch with the physical-pad source `0x7ffffffa`, so the firmware's
   selected-pad routing, note-off matching, sustain and audio engine stay in
   control.

Note-offs take the same path with velocity 0 and the tracked replayed note.

**Pad MIDI In must be On.** The app resets it to Off on every start; with it off
external notes follow the stock pad mapping (some notes silent, others playing
other pads). This was the cause of several false "it does not work" test runs.

The earlier raw-ALSA experiment was wrong: it opened a MIDI device already owned
by the firmware and called the pad `note_on` entry directly, creating competing
input paths. MIDI events are now transformed in the native event path already
used by the firmware. The callback compares the source as a signed 32-bit
integer, because external MIDI source IDs can have the high bit set.

## Current limits

The root is fixed at C3 (MIDI 60); per-pad custom root notes are not exposed in
MiniMod. Pitch transformation applies only while 16 Levels is active; with it
off the dispatcher is left byte-identical to stock (`d503233f`). Pad MIDI In is
not persisted by the app and must be re-enabled after each start. The SampQOL
reference also has a chopped-pad mode; MiniMod applies chromatic pitch to
incoming notes without a separate slice-versus-pitch setting.

## Emulator verification

`mod/tests/emulator_midi_chromatic.py` covers the transform and pass-through.
The final full-range check used a controlled 220 Hz sine on the selected pad,
Pad MIDI In On and 16 Levels on (dispatcher site `155f947c`):

| note | offset | measured | expected |
|---|---|---|---|
| 48 | -12 | 110.4 Hz | 110.0 |
| 60 | 0 | 220.7 Hz | 220.0 |
| 65 | +5 | 293.4 Hz | 293.7 |

Positive and negative offsets are exact; every note 36..96 played the selected
sample (RMS 161..613) instead of falling back to other pads. Turning 16 Levels
off restored the stock dispatcher bytes (`d503233f`). The method behind these
measurements is written up in [MIDI-16LEVELS-METHOD.md](MIDI-16LEVELS-METHOD.md)
and the decompiled facts in [RE-MIDI-16LEVELS-FINDINGS.md](RE-MIDI-16LEVELS-FINDINGS.md).
