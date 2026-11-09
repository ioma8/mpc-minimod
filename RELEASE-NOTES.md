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

- **Rust implementation:** the preload library is a Rust `cdylib` built on the
  public memory-safe [`mpc-sample-sdk`](https://github.com/ioma8/mpc-sample-sdk);
  no C or C++ is compiled, and the feature crate contains no `unsafe`.
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
  - **Synth** has eight selectable characters — bass, piano, pads, lead, bells,
    strings, voice and organ. Turn `DATA` to switch; each character has its own
    three knobs, and `DATA` saves a C2 note to a new pad.
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

### Rust port

- Replace the C preload library with a Rust `cdylib` built on the public
  [`mpc-sample-sdk`](https://github.com/ioma8/mpc-sample-sdk). The workspace
  keeps feature code in `rust/crates/minimod` and host-testable DSP in
  `rust/crates/dsp`; the SDK owns firmware discovery, hooks, events, audio,
  drawing and typed platform objects.
- No C or C++ is compiled. `minimod` contains no `unsafe` and no C ABI; the SDK
  isolates the firmware boundary behind its safe API.
- One SDK build script (`build-mod.sh`) now performs the cross-build, symbol and
  glibc checks, stripping and the SD `autorun` package. minimod only vendors a
  thin wrapper.
- The strict gate covers formatting, clippy (all + pedantic + cargo), rustdoc,
  the no-`unsafe` guard, host tests and the aarch64 release build.

### Synth characters

- Add eight selectable synth characters on the Synth page: bass, piano, pads,
  lead, bells, strings, voice and organ.
- The DATA encoder switches character; the page title shows the selection and
  the three knob labels/defaults change with it.
- Give each character its own synthesis path and controls: BASS
  BRIGHT/DECAY/FILTER, PIANO BRIGHT/DECAY/TONE, PADS ATTACK/BRIGHT/WIDTH, LEAD
  BRIGHT/BITE/FILTER, BELLS BRIGHT/DECAY/RATIO, STRINGS ATTACK/BRIGHT/WIDTH,
  VOICE VOWEL/BRIGHT/FILTER, ORGAN DRAWBAR/BRIGHT/FILTER.
- Shorten the bass from 1.5 s to 0.75 s with a 150 ms body and fast tail, and
  replace the subtle EXCITE knob with DECAY.
- Name rendered pads after the selected character (`Piano C2`, `Organ C2`, …).

### Fixes

- Route non-bass preview notes to the selected synth engine; previously every
  character except the bass was silent when auditioned from the pads.
- Let 16 Levels reach the stock app again; the mod had consumed the key and the
  app never toggled its own state.
- Clear the held PLUS flag on release so MINUS shifts the 16 Levels pitch down
  instead of being mistaken for a two-key reset.
- Accept a single spectral boundary, so pad A16's one-change sample chops into
  two regions instead of failing with “no onsets”.
- Close the SAMPLE OPTIONS menu when a Spectral run fails, making the status
  and toast visible.
- Keep a reverb copy's tail: native Extract starts with the source range, so the
  new pad's trim end is moved to 99% of the rendered frames (1% end trim).
- Repaint after a toast expires, so AutoTrim and other messages disappear
  instead of remaining on screen.
- Draw the looper grid like the hardware: pad 1 bottom-left, pad 16 top-right.

### Live Looper

- Add a fullscreen 16-pad looper with one loop per pad: press to record from
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

## Verification and compatibility

- Supported firmware: **MPC Sample 1.3.0.12 only**.
- Package: `minimod.zip`, built from the Rust tree, with a valid
  `autorun.check`; the loader checks the 36-word firmware fingerprint.
- SDK: **59 tests**, clippy and rustdoc clean.
- minimod: **39 tests**; DSP: **42 tests**, including all eight synth
  characters, every knob, the golden fixtures and the non-bass preview.
- `rust/check.sh`: formatting, strict clippy, rustdoc, no-`unsafe` guard, host
  tests and the aarch64 cross build all pass.
- Emulator checks cover the menu, AutoTrim, HPSS, Spectral Chop, Reverb, Vinyl,
  Lo-Fi, the eight-character Synth page and render path, Live Looper and
  16 Levels.
- Real-hardware verification remains device-specific; the emulator validates
  logic, UI, native object access and the rendered audio, not hardware latency
  or analog audio behaviour.
