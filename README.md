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
| **Live Looper** — a fullscreen 16-pad looper: each pad records a loop from the selected input source and plays it back; overdub, per-pad stop, sync pulse with loop-length quantization | `SHIFT` + `DATA`, choose **Live Looper**; pads record/play/stop, `SHIFT`+pad overdubs, `LOOP` syncs, `SHIFT`+`DATA` exits |

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
[`docs/SPECTRAL-CHOP.md`](docs/SPECTRAL-CHOP.md); the model, the mechanisms and
the evidence: [`docs/re/SPECTRAL-CHOP-SOLVED.md`](docs/re/SPECTRAL-CHOP-SOLVED.md).

The **Live Looper** is a separate fullscreen view, not a sample action: every pad
records a loop from the source the sampler itself hears (the selected input),
plays it through the app's audio output, and keeps it in RAM until the app
restarts. Pressing a pad records, pressing it again plays, another press stops;
`SHIFT`+pad overdubs and returns to playback; `LOOP` starts a sync pulse from the
last loop's length, after which new loops are quantized to whole pulse lengths.
Pads are blocked from the app while the view is open and `SHIFT`+`DATA` closes
it. Details: [`docs/LIVE-LOOPER.md`](docs/LIVE-LOOPER.md).

External MIDI plays the selected sample chromatically over the whole keyboard
while 16 Levels is active (C3 root, exact semitones, no wrapping). Enable **Pad
MIDI In** in MIDI Config — it resets to Off on every start. Details:
[`docs/MIDI-CHROMATIC.md`](docs/MIDI-CHROMATIC.md).

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

- [`mod/README.md`](mod/README.md) — details, controls, hooks
- [`AUTOTRIM-HANDOFF.md`](AUTOTRIM-HANDOFF.md) — how AutoTrim works
- [`HPSS-HANDOFF.md`](HPSS-HANDOFF.md) — how SAMPLE OPTIONS / HPSS works and what was validated
- [`docs/SPECTRAL-CHOP.md`](docs/SPECTRAL-CHOP.md) — the region detector and the in-place chop write
- [`docs/re/SPECTRAL-CHOP-SOLVED.md`](docs/re/SPECTRAL-CHOP-SOLVED.md) — the chop model, the write recipe, the path there and the probe toolkit
- [`docs/re/FIRMWARE-MECHANISMS.md`](docs/re/FIRMWARE-MECHANISMS.md) — the firmware internals we mapped: loader, screen/paint, layers, properties, commands, input, sample pool, threading
- [`docs/MIDI-16LEVELS-METHOD.md`](docs/MIDI-16LEVELS-METHOD.md) — what actually solved the chromatic MIDI problem, and the reusable method
- [`docs/`](docs/) — device notes, emulator, reverse-engineering writeups
- [`tools/hpss/`](tools/hpss/) — Python reference, algorithm notes and WAV helper scripts
- [`converter/`](converter/) — separate tool: project → per-pad stems
