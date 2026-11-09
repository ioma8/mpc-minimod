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
  MIDI plays the selected sample chromatically over the whole keyboard from the
  sample's own root note (C3 when it has none), with exact semitones and no
  wrapping (enable Pad MIDI In in MIDI Config).
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
- The loops follow `MAIN VOLUME`: the knob's CC is tracked and applied as a gain
  on the loop mix, and the level is remembered in `/data/minimod-looper.cfg`.
- Record the latency tail after the closing press, flush the input ring at the
  start of every take, and finish a pad atomically so a take can never be closed
  twice and starve the recording counter.
- Keep the audio device untouched from the repaint thread: the ring sizes and the
  channel count now come from `snd_pcm_hw_params_current` on the stream's own
  audio thread, and the loop mix uses the device's real channel count. This
  removes the cross-thread ALSA calls and the stride guess that could corrupt the
  output buffer and take the app down after closing the view. The MIDI dispatcher
  is hooked once at load instead of being re-patched while it may be executing,
  and the loader refuses to install anything unless every hooked site's first
  instruction matches the firmware it was built for.
- `SHIFT` + pad overdubs into the loop and the next press returns to playback;
  `SHIFT` + `LOOP` toggles the click, `ERASE` + `LOOP` clears everything;
  `SHIFT` + `DATA` closes the view.
- `LOOP` starts an audible sync pulse whose period is the most recent loop's
  length, and every new loop is quantized to a whole multiple of it (padded with
  silence, never truncated silently).
- Record input by interposing the app's own `snd_pcm_readi`/`snd_pcm_readn`
  (RW_INTERLEAVED S32, 4 channels, 44.1 kHz) and mix loop playback into the S32
  block just before the app's device write, so no second audio device is opened
  and the app's own rendering path is untouched.
- Take the record latency from the firmware's own ALSA ring geometry (one
  buffer per direction, read from the device with
  `snd_pcm_hw_params_get_buffer_size`), apply it to every take so overdubs and
  synced loops line up, and keep a `K1` trim in `/data/minimod-looper.cfg`. This
  works with headphones, USB audio and a silent instrument input, unlike a
  probe that needs to hear the output.
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
  ([`docs/FEATURES.md`](docs/FEATURES.md#external-midi-chromatic-playback),
  [`docs/HISTORY.md`](docs/HISTORY.md#4-external-midi--16-levels)).
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

- Live Looper verified in the emulator: the firmware reports 1024 frames per
  ring, so takes use 2048 frames and clicks recorded through the loop cable land
  within 69 frames (1.6 ms) of the pulse grid; a 1.17 s loop recorded from the tone input plays
  at 0.15 rms / 441 Hz after the input is unplugged; stop silences it; overdub
  keeps the loop length; a short take under a 1.27 s pulse becomes exactly one
  pulse; `STOP` silences everything, `ERASE`+pad clears one, `SHIFT`+`LOOP`
  toggles the click; closing keeps the loops and does not restart the app.
- Built and packaged the release for the MPC Sample's ARM64 firmware.
- Emulator checks cover the native reverb, Vinyl, Lo-Fi and Bass Synth paths,
  plus C3/C#3/G3 MIDI transposition, pass-through outside 16 Levels, and no
  firmware restart.
