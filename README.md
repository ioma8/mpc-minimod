# mpc-minimod

I was annoyed by the too-complete reworks other custom firmwares do. I want the
original MPC Sample experience — I love it — with just small fixes that enhance
the existing flow. So I made this minimal MINIMOD, which does only those things.

It is a small mod for the **Akai MPC Sample**, firmware **1.3.0.12 only**. It
loads into the stock app and adds two things. Nothing on disk is changed, so a
reboot restores stock.

| what | how |
| --- | --- |
| **AutoTrim** — trims the selected sample to its silent edges; each further press tightens it (3 %, 6 %, 9 %), the fourth removes the trim | `SHIFT` + `B3` |
| **Octave shift** in 16 Levels — 8 semitones per press | `PLUS` / `MINUS` (both = reset) |

Both confirm themselves with a short on-screen message, so you always know what
the sample or the row is doing. **Work in progress:** chromatic play from an
external MIDI keyboard — see [`docs/MIDI-CHROMATIC.md`](docs/MIDI-CHROMATIC.md).

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
