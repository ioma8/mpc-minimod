# External MIDI → sequence recording: investigation findings

Status: **not implemented**. This document records what was tested and what the
firmware actually does, so the feature can be picked up without repeating the
work. The chromatic playback shipped in v0.7.0 is unaffected.

## Symptom

With **16 Levels** active and **Pad MIDI In On**, external MIDI plays the
selected pad chromatically over the whole keyboard (v0.7.0). But when the
transport is recording a sequence, those notes play and are **not captured** in
the sequence.

## Decisive tests

1. **Physical pads do record.** SEQ RECORD + PLAY, tap three pads, STOP, then
   play the sequence back: RMS 5318, peak 32767. Recording and playback work.
2. **Chromatic MIDI is not recorded.** Same pass with external notes 60/64/67
   into a fresh sequence (A04): all five sequences have
   `seqEventList.events == []`, and playback adds nothing.
3. **Stock external MIDI is not recorded either.** With 16 Levels off (stock
   pad mapping), the same external notes also produce zero sequence events.
4. **Project file caveat.** `Temp-Project.xpj` is gzip-compressed JSON. Its
   `seqEventList.events` stayed empty even after the pad pass that audibly
   played back, so the saved project is not a reliable record of live sequence
   contents. Playback audio is the reliable signal.

## Where the note path runs (decompiled)

Dispatcher `FUN_0221b090` (file `0x211b090`) caller captured with
`__builtin_return_address(0)` in the mod hook:

| path | LR | function | what it does |
|---|---|---|---|
| physical pad | `…c7554` (file `0x1e27554`) | `FUN_01f27360` | pad queue pump: builds the note, calls the 16 Levels writer `FUN_022472f0`, then the dispatcher |
| external MIDI | `…c5dd0` (file `0x1e25dd0`) | `FUN_01f25cf0` | writes the per-note velocity table, builds the note, calls the dispatcher |

Neither function contains a recorder call. The pad pump itself is invoked from
`FUN_020ce3e0` (call site `0x20ce480`), a generic **observer notification**
loop that calls listener vtable methods — the pad grid observer layer, which is
where the sequencer records.

## Recorder candidates probed

| address | function | result |
|---|---|---|
| file `0x211a940` (SampQOL `keys-recwriter`) | `FUN_0221a800` | **never called** during a recorded pad press |
| file `0x2119d00` | `FUN_02219d00` | called (type 3, source `0x7ffffffa`, `+0x30` 0x50) — note dispatch, not recording |

## Source experiment (ruled out)

Replaying the transformed note with the **original external source** instead of
the physical-pad sentinel `0x7ffffffa` breaks playback: notes 60 and 72 are
silent. Routing depends on the sentinel, so "record via the external source"
is not a usable workaround.

## Conclusion

* The sequencer records a **pad-grid observer event**, upstream of the note
  dispatcher.
* The chromatic transform lives at the dispatcher, i.e. below the recorder, so
  the sequence never sees it.
* External MIDI is not recorded even with the stock mapping, so
  "external MIDI should be recorded" is a **firmware feature gap**, not a
  regression introduced by 16 Levels.

## Next implementation (not started)

1. Hook `FUN_020ce3e0`; log the method index (`param_2`) and each listener
   target (`*(*listener + method)`) for one recorded pad press vs one
   unrecorded press. The listener that only runs while recording is the
   recorder.
2. Feed that recorder from the MIDI path with the chromatic note (pad +
   `note-60` pitch), or replay the pad-grid observer event and apply the pitch
   through the setter.
3. Decide what the recorded event stores so playback reproduces the chromatic
   pitch rather than the bare pad note.

## Address table

| file offset | Ghidra VA | role |
|---|---|---|
| `0x211b090` | `0x221b090` | note dispatcher (mod hook) |
| `0x211aa30` | `0x221aa30` | router |
| `0x2215070` | `0x2215070`* | event decode |
| `0x2119d00` | `0x2219d00` | note dispatch into the engine |
| `0x211a940` | `0x221a940` | `FUN_0221a800`, SampQOL "recwriter" candidate |
| `0x21472f0` | `0x22472f0` | 16 Levels value writer |
| `0x27e4400` | `0x28e4400` | property/pitch setter |
| `0x1e27360` | `0x1f27360` | pad queue pump |
| `0x1e25cf0` | `0x1f25cf0` | external note input |
| `0x20ce3e0` | `0x20ce3e0` | observer notification loop (pad grid layer) |

\* event decode address is already a Ghidra VA in the decompiler output.

## Caveats for the next session

* **Pad MIDI In resets to Off on every app start**; re-enable it before any test.
* The 16 Levels dialog toggles the mode when opened; verify the dispatcher site
  (`d503233f` stock, `155f947c` hook installed) instead of trusting the UI.
* On the SEQ page, DATA selects another sequence; use a fresh sequence for
  recording tests so previous takes do not confound the playback audio.
* The emulator's saved project does not reflect live sequence events; use
  playback audio, or read the live model from memory.
