# Tasks — M1 "free wins"

Five small features that need no new reverse engineering: they ride hooks the mod
already owns (button dispatch, note on/off, the paint hook, the `setctl` dispatch).

| task | one line | binding (proposed) | effort |
|---|---|---|---|
| [preset-roulette](preset-roulette.md) | random-but-constrained pad recipe, one press | `SHIFT`+`PAD BANK`, double-tap | 1 session |
| [jitter](jitter.md) | nudge the current settings by ±5–15 % | `SHIFT`+`PAD BANK`, single tap | ½ session |
| [per-hit-variation](per-hit-variation.md) | every hit slightly different in velocity (and pitch/pan if possible) | `SHIFT`+`MUTE` toggle | ½ session |
| [live-humanise](live-humanise.md) | micro timing/velocity variation only while a key is held | hold `ERASE` | ½ session |
| [ghost-notes](ghost-notes.md) | soft off-grid hits around the pads you play | `SHIFT`+`NOTE REPEAT` toggle | 1 session |

Common rules for all five:

* **Nothing destructive.** They change live parameters or note events, never files.
* **Every result is reversible** with one press.
* **Every action reports itself** in the existing one-second toast.
* Bindings are proposals — the first step of each task is to confirm the combo is
  free in firmware 1.3.0.12, and to fall back to the listed alternative if not.

Shared anchors (file VAs; runtime = `base + addr`):

| what | address |
|---|---|
| button dispatch (event: code at `+8`, down at `+9`) | `0x159f2f0` |
| note on `(input, src, pad, props, vel)` | `0x1e27360` |
| note off `(input, src, pad)` | `0x1e270d0` |
| control dispatch `(ctx, surface, pad, controlId, value)` | `0x1968020` |
| repaint `(comp, x, y)` | `0x1149880` |
| paint hook | `SampleModeScreen` vtable `0x6e212a0`, slot 27 |
| toast | `paint::show_toast` in `rust/crates/minimod/src/paint.rs` |

Status of every task: **not started.**
