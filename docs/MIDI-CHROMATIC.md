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

The SampQOL v0.1.5 package supplied for this work documents a MIDI **Keys** mode
that plays the selected pad chromatically, with C3 as the default root. Its
protected `mpcqol.so` was imported into the project's Ghidra instance and
inspected with the Java Ghidra bridge. The `keys_init` setup installs a callback
at firmware file offset `0x211b090`. The callback:

* accepts note events with event type `3` and positive velocity;
* reads MIDI note from event offset `+0x30`;
* calls firmware offset `0x27e4400` with the event property block at `+0x14`
  and `note - root` as the fourth argument;
* lets the original firmware note-event dispatcher run afterward.

The callback compares its source argument as a **signed 32-bit integer**.
External MIDI source IDs can have the high bit set; treating the value as an
unsigned pointer incorrectly rejects those notes. The physical-pad sentinel
`0x7ffffffa` remains excluded by SampQOL's signed comparison.

The stock firmware functions were independently decompiled through the same
Java bridge. At Ghidra addresses `0x221b090` and `0x28e4400` (firmware file
offsets `0x211b090` and `0x27e4400`), the dispatcher consumes that event and the
pitch setter writes its pitch data into the note-event property block. The mod
uses these exact hooks and setter. It applies the delta only when its existing
16 Levels button-state tracker and Sample page are active, and calls the
setter trampoline to avoid applying the mod's PLUS/MINUS octave wrapper twice.

The earlier raw-ALSA experiment was wrong: it opened a MIDI device already
owned by the firmware, then called the pad `note_on` entry directly. That
created competing input paths and merely triggered pad samples. The experiment
and its optional thread are removed. MIDI events are now transformed in the
native event path already used by the firmware.

## Current limits

The mod uses the firmware/SampQOL default root, C3 (MIDI 60); per-pad custom
root-note editing is not exposed in MiniMod. The firmware must be configured to
receive the controller on its selected pad input. The SampQOL reference also
has a configurable chopped-pad mode; MiniMod currently applies chromatic pitch
to incoming notes in 16 Levels without a separate slice-versus-pitch setting.

## Emulator verification

`mod/tests/emulator_midi_chromatic.py` passed on the virtual MPC with MIDI Port
set to External and Pad MIDI In On. It sent C3, C#3 and G3 through the
`ext_midi` endpoint and observed three note-on transforms; the final event was
MIDI 67 with a `+7` semitone delta. A note sent after leaving 16 Levels passed
through unchanged, and the firmware process stayed alive with no service
restart. The test uses the same external-MIDI route as a controller; the mod
does not open or write the app's ALSA rawmidi nodes.
