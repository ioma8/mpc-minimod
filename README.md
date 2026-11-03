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

1. Download **`mpc-mini.zip`** from the
   [latest release](https://github.com/ioma8/mpc-minimod/releases/latest).
2. Unzip it. You get a folder called `autorun`.
3. Copy `autorun` onto the **SD card**, next to the folders already there — not
   inside one of them. The card should end up with `autorun` at its top level.
4. Put the card into the MPC Sample and switch it on. When it asks, choose
   **Start**.
5. That's it. The two features are active.

Nothing on the device is changed: to go back to stock, delete the `autorun` folder
from the card (or boot without the card).

<details>
<summary>Building it yourself</summary>

```sh
cd mod && ./build.sh && ./pack.sh
```

That writes `mod/build/mpc-mini.zip` — the same package as the release.

</details>

## Docs

- [`mod/README.md`](mod/README.md) — details, controls, hooks
- [`AUTOTRIM-HANDOFF.md`](AUTOTRIM-HANDOFF.md) — how AutoTrim works
- [`docs/`](docs/) — device notes, emulator, reverse-engineering writeups
- [`converter/`](converter/) — separate tool: project → per-pad stems
