# MPC Sample MINIMOD

I was annoyed by the complete reworks by other custom firmwares. I want the
original MPC Sample experience, with just small fixes that enhance it for better flow. Thus I made this minimal MINIMOD for the **Akai MPC Sample** (firmware
**1.3.0.12 only**). It adds only a few features which help me enjoy it more. Can be easily reverted by removing from SD card.

▶ **[Watch the intro](https://youtu.be/bnnkfryecPY)**

[![minimod — intro video](https://img.youtube.com/vi/bnnkfryecPY/maxresdefault.jpg)](https://youtu.be/bnnkfryecPY)

| what | how |
| --- | --- |
| **AutoTrim** — trims the selected sample to its silent edges; each further press tightens it (3 %, 6 %, 9 %), the fourth removes the trim | `SHIFT` + `B3` |
| **Octave shift** of pads in 16 Levels mode — by half the grid | `PLUS` / `MINUS` keys (both = reset) |
| **External MIDI chromatic playback** of the selected sample while in 16 Levels (C3 root) | Enable **Pad MIDI In** in MIDI Config, then play a MIDI keyboard |
| **HPSS** — splits the selected sample into harmonic and percussive copies on two empty pads; the source sample is left untouched | `SHIFT` + `DATA`, choose **HPSS**, `DATA` |
| **Spectral Chop** — cuts the selected sample into musical regions — a new chord, a new phrase, a different drum — and chops that same pad at those boundaries (Manual slice markers, up to 16) | `SHIFT` + `DATA`, choose **Spectral Chop**, `DATA` |
| **Add Reverb** — adds the firmware's stereo reverb and a rendered tail to a new pad copy | `SHIFT` + `DATA`, choose **Add Reverb**, `DATA` |
| **Vinyl / Lo-Fi** — applies the firmware's Vinyl or Decimator effect to a new pad copy | `SHIFT` + `DATA`, choose **Vinyl** or **Lo-Fi**, `DATA` |
| **Bass Synth** — previews C2–D#3 from pads 1–16 and saves a shaped C2 pluck to a new pad | `SHIFT` + `DATA`, choose **Bass Synth**; K1/K2/K3 shape it |

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

External MIDI plays the selected sample chromatically over the whole keyboard
while 16 Levels is active (C3 root, exact semitones, no wrapping). Enable **Pad
MIDI In** in MIDI Config — it resets to Off on every start. Details:
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
- [`mod/README.md`](mod/README.md) — controls, anchors, build
- [`tools/hpss/`](tools/hpss/) — Python reference implementations and WAV helpers
- [`converter/`](converter/) — separate tool: project → per-pad stems
