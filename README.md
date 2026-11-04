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
| **HPSS** — splits the selected sample into harmonic and percussive copies on two empty pads; the source sample is left untouched | `SHIFT` + `DATA`, choose **HPSS**, `DATA` |
| **Spectral Chop** — detects the onsets in the selected sample and chops that same pad at exactly those frames (Manual slice markers, up to 16) | `SHIFT` + `DATA`, choose **Spectral Chop**, `DATA` |

The trim and pitch changes confirm themselves on screen for a second. HPSS and
Spectral Chop open a small **SAMPLE OPTIONS** menu; turn the DATA encoder to
choose an action, press DATA to confirm, or press `SHIFT+DATA` to close. HPSS
needs two empty pads and creates `HPSS Harmonic` and `HPSS Percussive`.
Spectral Chop detects up to 16 onsets on the same pad and rewrites that pad's
chop so every slice boundary sits on a detected frame. It never creates
samples or uses free pads, and it verifies the result by reading the written
boundaries back before it reports. Details:
[`docs/SPECTRAL-CHOP.md`](docs/SPECTRAL-CHOP.md); how the chop model is
reached: [`docs/re/spectral-chop-handoff.md`](docs/re/spectral-chop-handoff.md).

Chromatic MIDI from an external keyboard is in progress —
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

## Docs

- [`mod/README.md`](mod/README.md) — details, controls, hooks
- [`AUTOTRIM-HANDOFF.md`](AUTOTRIM-HANDOFF.md) — how AutoTrim works
- [`HPSS-HANDOFF.md`](HPSS-HANDOFF.md) — how SAMPLE OPTIONS / HPSS works and what was validated
- [`docs/SPECTRAL-CHOP.md`](docs/SPECTRAL-CHOP.md) — onset detector and the in-place chop write
- [`docs/re/spectral-chop-handoff.md`](docs/re/spectral-chop-handoff.md) — native slice API and the exact remaining steps
- [`docs/`](docs/) — device notes, emulator, reverse-engineering writeups
- [`tools/hpss/`](tools/hpss/) — Python reference, algorithm notes and WAV helper scripts
- [`converter/`](converter/) — separate tool: project → per-pad stems
