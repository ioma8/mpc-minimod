# Release notes

## Standard release-description format

Every GitHub release description must use this order:

1. **Features** — a concise, complete list of the mod's user-facing features,
   including features that shipped in earlier releases.
2. **Install** — brief instructions to download `minimod.zip`, copy the
   `autorun` folder to the root of an SD card, and boot the MPC Sample.
3. **Changes in `<version>`** — only the additions and fixes for that specific
   release.
4. **Verification and compatibility** — what was checked and the supported
   firmware version.

Keep the feature summary and install steps in every release description. Update
the version-specific section for each release; don't replace the overview with
only the latest changes.

## Features

- **16 Levels:** shift the pad row by ±8 semitones with PLUS/MINUS; external
  MIDI plays the selected sample chromatically over the whole keyboard from a
  C3 root, with exact semitones and no wrapping (enable Pad MIDI In in MIDI
  Config).
- **AutoTrim:** trim the selected sample's silent edges with SHIFT+B3; the
  shifted Sample page shows the AutoTrim caption.
- **SAMPLE OPTIONS:** open with SHIFT+DATA, select with DATA, then press DATA
  to confirm.
  - **HPSS** splits a sample into harmonic and percussive layers on two free
    pads.
  - **Spectral Chop** detects musical changes and places slice boundaries on
    the selected sample.
  - **Add Reverb** creates a reverberated copy with an appended tail.
  - **Vinyl** and **Lo-Fi** apply the firmware's built-in H3 effects to a new
    copy.
  - **Bass Synth** previews chromatic C2–D#3 from pads 1–16, shapes it with
    K1/K2/K3, and saves a C2 pluck to a new pad.
  - **Live Looper** opens a fullscreen 16-pad looper: each pad records from the
    selected input source, plays, overdubs, clears with `ERASE`, and can be
    synced to a pulse built from the last loop's length.

## Install

1. Download `minimod.zip` from the release assets.
2. Unzip it and copy the `autorun` folder to the top level of an SD card.
3. Insert the card and power on the MPC Sample. Remove `autorun` and reboot to
   unload the mod.

The release supports MPC Sample firmware **1.3.0.12** only.

## Changes in v0.8.0

- Add a fullscreen **Live Looper** with one loop per pad: press to record from
  the selected input source, press again to play, press a playing loop to stop.
- `ERASE` + pad clears one loop, `STOP` stops every loop, and the sync click is
  off by default (`SHIFT` + `LOOP` toggles it).
- `SHIFT` + pad overdubs into the loop and the next press returns to playback;
  `SHIFT` + `LOOP` clears everything; `SHIFT` + `DATA` closes the view.
- `LOOP` starts an audible sync pulse whose period is the most recent loop's
  length, and every new loop is quantized to a whole multiple of it (padded with
  silence, never truncated silently).
- Record input by interposing the app's own `snd_pcm_readi`/`snd_pcm_readn`
  (RW_INTERLEAVED S32, 4 channels, 44.1 kHz) and mix loop playback into the S32
  block just before the app's device write, so no second audio device is opened
  and the app's own rendering path is untouched.
- Measure the round-trip latency when the view opens (a short chirp through the
  output, cross-correlated against the capture), keep measuring it while loops
  play whenever the input hears the output, and apply it to every take so
  overdubs and synced loops line up; a buffer-based estimate is used when no
  return path exists, `K1` trims it, and the result is stored in
  `/data/minimod-looper.cfg` so it survives a restart. The status strip shows
  the source and value (`CAL`, `AUTO`, `SAVED`, `EST`).
- Draw the loop state, length, playhead and input level as a fullscreen view,
  and block pad notes from the app while it is open.
- Add `mod/tests/emulator_looper.py` and an in-process output tap for audio
  acceptance.

## Changes in v0.7.0

- External MIDI in 16 Levels now plays the selected pad across the **entire
  keyboard**: exact semitones, no wrapping, and no fallback to other pads
  outside the 16-pad row.
- Rebuild the note event for the selected pad and call the app's own pitch
  setter (`w3 = note - 60`) before dispatching with the physical-pad source, so
  routing, note-offs and sustain stay in the firmware's native path.
- Document the captured setter contract, the dispatcher/router/16 Levels
  decompilation and the measurement method
  ([`docs/RE-MIDI-16LEVELS-FINDINGS.md`](docs/RE-MIDI-16LEVELS-FINDINGS.md),
  [`docs/MIDI-16LEVELS-METHOD.md`](docs/MIDI-16LEVELS-METHOD.md)).
- Verified on a real device and in the emulator: 48→110.4, 60→220.7,
  65→293.4 Hz on a 220 Hz sample; 16 Levels off leaves the dispatcher
  byte-identical to stock.

## Changes in v0.6.0

- Add the firmware's built-in StereoReverb, Vinyl and Decimator effects to
  SAMPLE OPTIONS, with native sample extraction and pad assignment.
- Append a rendered reverb tail so the reverberated sample does not cut off at
  the source sample's original length.
- Add Bass Synth: a compact DaisySP physical-model C2 renderer, live chromatic
  pad preview and three live sound-shaping controls.
- Add external MIDI chromatic playback in 16 Levels through the firmware's
  native note-event and pitch-setting path.
- Replace the competing raw MIDI reader with the native MIDI event hook and
  document the reverse-engineered firmware paths.

## Verification

- Live Looper verified in the emulator: opening calibrates the round trip from
  the chirp (1808 frames, repeatable over five open/close cycles), continuous
  correlation locks while a loop plays, the value is seeded from
  `/data/minimod-looper.cfg` after a restart, and clicks recorded through the
  cable land within 45 frames (1 ms) of the pulse grid; a 1.17 s loop recorded from the tone input plays
  at 0.15 rms / 441 Hz after the input is unplugged; stop silences it; overdub
  keeps the loop length; a short take under a 1.27 s pulse becomes exactly one
  pulse; `STOP` silences everything, `ERASE`+pad clears one, `SHIFT`+`LOOP`
  toggles the click; closing keeps the loops and does not restart the app.
- Built and packaged the release for the MPC Sample's ARM64 firmware.
- Emulator checks cover the native reverb, Vinyl, Lo-Fi and Bass Synth paths,
  plus C3/C#3/G3 MIDI transposition, pass-through outside 16 Levels, and no
  firmware restart.
