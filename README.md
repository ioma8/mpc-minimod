# mpc-minimod

I was annoyed by the too-complete reworks other custom firmwares do — I want the
original MPC Sample experience, with just small fixes that enhance the existing
flow. So this is the minimal MINIMOD for the **Akai MPC Sample** (firmware
**1.3.0.12 only**): two additions to the stock app, nothing changed on disk that
a reboot doesn't undo.

| what | how |
| --- | --- |
| **AutoTrim** — trims the selected sample to its silent edges; each further press tightens it (3 %, 6 %, 9 %), the fourth removes the trim | `SHIFT` + `B3` |
| **Octave shift** in 16 Levels — 8 semitones per press | `PLUS` / `MINUS` (both = reset) |

Both confirm themselves on screen for a second. Chromatic MIDI from an external
keyboard is in progress — [`docs/MIDI-CHROMATIC.md`](docs/MIDI-CHROMATIC.md).

## Install

```sh
cd mod && ./build.sh && ./pack.sh
unzip -q build/mpc-mini.zip -d /Volumes/MPCSAMPLE/     # SD card, fresh autorun/
```

Power on and confirm the prompt. To remove it, reboot or delete `autorun/` from the card.

## Docs

- [`mod/README.md`](mod/README.md) — details, controls, hooks
- [`AUTOTRIM-HANDOFF.md`](AUTOTRIM-HANDOFF.md) — how AutoTrim works
- [`docs/`](docs/) — device notes, emulator, reverse-engineering writeups
- [`converter/`](converter/) — separate tool: project → per-pad stems
