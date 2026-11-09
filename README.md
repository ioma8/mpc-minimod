# MPC Sample MINIMOD

I was annoyed by the complete reworks by other custom firmwares. I want the
original MPC Sample experience, with just small fixes that enhance it for better flow. Thus I made this minimal MINIMOD for the **Akai MPC Sample** (firmware
**1.3.0.12 only**). It adds only a few features which help me enjoy it more. Can be easily reverted by removing from SD card.

▶ **[Watch the intro](https://youtu.be/bnnkfryecPY)**

[![minimod — intro video](https://img.youtube.com/vi/bnnkfryecPY/maxresdefault.jpg)](https://youtu.be/bnnkfryecPY)

## Built on the open Rust SDK

minimod is built on **[mpc-sample-sdk](https://github.com/ioma8/mpc-sample-sdk)**,
a memory-safe Rust SDK for the Akai MPC Sample made by the same author. The SDK
owns the firmware finding and verification, the in-memory hooks, the event
plumbing, audio/MIDI, drawing and typed access to the app's own objects, so
custom mods can be written in **safe Rust** without raw pointers, C ABIs or
`unsafe` in the feature code.

The SDK is public and documented for anyone who wants to build their own MPC
Sample mod:
**https://github.com/ioma8/mpc-sample-sdk** ·
[hands-on guide](https://github.com/ioma8/mpc-sample-sdk/blob/main/docs/README.md).

| what | how |
| --- | --- |
| **AutoTrim** — trims the selected sample to its silent edges; each further press tightens it (3 %, 6 %, 9 %), the fourth removes the trim | `SHIFT` + `B3` |
| **Octave shift** of pads in 16 Levels mode — by half the grid | `PLUS` / `MINUS` keys (both = reset) |
| **External MIDI chromatic playback** of the selected sample while in 16 Levels, in the sample's own key (C3 when it has no root note) | Enable **Pad MIDI In** in MIDI Config, then play a MIDI keyboard |
| **HPSS** — splits the selected sample into harmonic and percussive copies on two empty pads; the source sample is left untouched | `SHIFT` + `DATA`, choose **HPSS**, `DATA` |
| **Spectral Chop** — cuts the selected sample into musical regions — a new chord, a new phrase, a different drum — and chops that same pad at those boundaries (Manual slice markers, up to 16) | `SHIFT` + `DATA`, choose **Spectral Chop**, `DATA` |
| **Add Reverb** — adds the firmware's stereo reverb and a rendered tail to a new pad copy | `SHIFT` + `DATA`, choose **Add Reverb**, `DATA` |
| **Vinyl / Lo-Fi** — applies the firmware's Vinyl or Decimator effect to a new pad copy | `SHIFT` + `DATA`, choose **Vinyl** or **Lo-Fi**, `DATA` |
| **Bass Synth** — previews C2–D#3 from pads 1–16 and saves a shaped C2 pluck to a new pad | `SHIFT` + `DATA`, choose **Bass Synth**; K1/K2/K3 shape it |
| **Live Looper** — a fullscreen 16-pad looper: each pad records a loop from the selected input source and plays it back; overdub, per-pad clear, stop-all, sync pulse with loop-length quantization and self-calibrated latency | `SHIFT` + `DATA`, choose **Live Looper**; pads record/play/stop, `SHIFT`+pad overdubs, `ERASE`+pad clears, `STOP` stops all, `LOOP` syncs, `K1` trims latency, `SHIFT`+`DATA` exits |

The trim and pitch changes confirm themselves on screen for a second. HPSS,
Spectral Chop, Add Reverb, Vinyl, Lo-Fi and Bass Synth are in the small
**SAMPLE OPTIONS** menu; turn the DATA encoder to choose an action, press DATA
to confirm, or press `SHIFT+DATA` to close. HPSS needs two empty pads and
creates `HPSS Harmonic` and `HPSS Percussive`.
Spectral Chop slices the pad into **regions, not notes**: it cuts where the
music changes identity — a new chord, a new phrase, a different drum — so the
pieces stay recombable instead of being one pad per hit. Every boundary still
lands on a real attack: the detector proposes candidates from the attack
function, and a candidate is kept only when the slice is long enough and the
sound has moved on since the last cut (chroma for harmony, log bands for
timbre). The shortest slice follows the material — dense drums give short
slices, a sustained pad gives long ones — so one rule covers both, up to 16
slices on the same pad. It uses the firmware's own slice API (reset,
insert-at-frame, reconcile), sets the chop type to Manual through the binding's
model callback, and verifies every boundary by reading it back before it
reports. It never creates samples or uses free pads. Details:
[`docs/FEATURES.md`](docs/FEATURES.md#spectral-chop); the slice API and chop
binding: [`docs/MECHANISMS.md`](docs/MECHANISMS.md#5-the-slice-container-and-a-slice).

The **Live Looper** is a separate fullscreen view, not a sample action: every pad
records a loop from the source the sampler itself hears (the selected input),
plays it through the app's audio output, and keeps it in RAM until the app
restarts. Pressing a pad records, pressing it again plays, another press stops;
`SHIFT`+pad overdubs and returns to playback; `ERASE`+pad clears one loop and
`STOP` stops them all; `LOOP` starts a sync pulse from the last loop's length,
after which new loops are quantized to whole pulse lengths (the metronome click
is off by default, `SHIFT`+`LOOP` toggles it). Recording latency comes from the
firmware's own ALSA ring geometry (one buffer per direction, read from the
device), so overdubs line up with headphones, USB audio or an instrument input;
`K1` trims it and the value is kept in `/data/minimod-looper.cfg`; the loops
follow `MAIN VOLUME` like the rest of the app. Pads are blocked from the app while the view is open and
`SHIFT`+`DATA` closes it. Details: [`docs/LIVE-LOOPER.md`](docs/LIVE-LOOPER.md).

External MIDI plays the selected sample chromatically over the whole keyboard
while 16 Levels is active, measured from the sample's own root note (C3 when it
has none): exact semitones, no wrapping. Enable **Pad MIDI In** in MIDI Config —
it resets to Off on every start. Details:
[`docs/FEATURES.md`](docs/FEATURES.md#external-midi-chromatic-playback).

## Install

1. Download **`minimod.zip`** from the
   [latest release](https://github.com/ioma8/mpc-minimod/releases/latest).
2. Unzip it and copy the `autorun` folder to the **top level** of the SD card —
   not inside another folder.
3. Put the card in the MPC Sample and power it on.

To go back to stock, delete `autorun` from the card (or boot without the card).

## Building

```sh
cd mod && ./build.sh && ./pack.sh
```

That writes `mod/build/minimod.zip` — the same package as the release.

The mod is Rust throughout: [`mpc-sample-sdk`](https://github.com/ioma8/mpc-sample-sdk) is the universal SDK
(firmware image, hooks, input/audio/MIDI, drawing, typed platform objects),
`rust/crates/minimod` is the feature code built only on that SDK, and
`rust/crates/dsp` is the host-testable audio DSP.  The preload library is a
`cdylib` — no C or C++ is compiled. `rust/check.sh` runs formatting, strict clippy (all +
pedantic + cargo, warnings denied), a rustdoc build with warnings denied, the
host tests including the golden DSP fixtures, and the aarch64 cross build. Layout, lint policy and the proof method:
[`docs/RUST-PORT.md`](docs/RUST-PORT.md).
Release description format and the complete feature overview are in
[`RELEASE-NOTES.md`](RELEASE-NOTES.md).

## Docs

- [`docs/README.md`](docs/README.md) — index, reading order and conventions
- [`docs/FEATURES.md`](docs/FEATURES.md) — what every feature does, how it is wired, what was measured, known limits
- [`docs/MECHANISMS.md`](docs/MECHANISMS.md) — the firmware internals we mapped: loader, screen/paint, pad state, sample, slices, chop binding, properties, input, MIDI, sample pool/Extract, effects, threading
- [`docs/RE-METHOD.md`](docs/RE-METHOD.md) — how the reverse engineering is done, what paid off, blind paths, hardening rules
- [`docs/TOOLING.md`](docs/TOOLING.md) — Ghidra bridge and scripts, kuna/r2, emulator lab, in-process probes, harnesses
- [`docs/MODDING.md`](docs/MODDING.md) — boot chain, autorun protocol, hooking technique, packaging, ecosystem
- [`docs/EMULATOR.md`](docs/EMULATOR.md) — the virtual MPC Sample
- [`docs/DEVICE-NOTES.md`](docs/DEVICE-NOTES.md) — hardware, Bluetooth, WiFi, the hidden touch Preferences UI
- [`docs/HISTORY.md`](docs/HISTORY.md) — investigations, reversals and open work
- [`docs/ADDIN-ECOSYSTEM.md`](docs/ADDIN-ECOSYSTEM.md) — the 2026 addin projects, their loading rules and what we can reuse
- [`docs/LIVE-LOOPER.md`](docs/LIVE-LOOPER.md) — the Live Looper: state machine, latency, sync
- [`docs/RUST-PORT.md`](docs/RUST-PORT.md) — the Rust rewrite: layout, lint policy, what is ported and how it is proven
- [`mod/README.md`](mod/README.md) — controls, anchors, build
- [`tools/hpss/`](tools/hpss/) — Python reference implementations and WAV helpers
- [`converter/`](converter/) — separate tool: project → per-pad stems
