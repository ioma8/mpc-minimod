# Task — Ghost notes

**Soft off-grid hits around what you play — instant pocket, nothing to program.**

Part of [M1](README.md).

---

## Button

`SHIFT` + `NOTE REPEAT` (`0x0b`) toggles ghost mode for the whole kit.
Toast: `Ghosts 35%`. If taken, fall back to `SHIFT`+`LOOP` (`0x4a`).

## Behaviour

1. When armed, every real pad hit may spawn one **ghost**: the same pad, quieter,
   slightly late.
2. Per hit: `p = 0.35` chance of a ghost, ghost velocity `0.35 … 0.6` of the original,
   offset `1/32` of a beat late (± a few ms of jitter).
3. Ghosts never replace the real hit and never fire on their own — they are always
   attached to a hit you played.
4. A ghost is delivered through the same `note_on` entry point, so the sequencer
   records it as a normal (quiet) event when in record. **That is the trick**: the
   printed version needs no sequencer write path, because the app records what we
   play.
5. `SHIFT`+`NOTE REPEAT` again turns it off.

## Why "attached to a hit"

A generator that fires independently fights the part you are playing. Ghosts that
hang off your own hits thicken the groove without changing the rhythm — which is
what a session drummer does.

## Implementation

* Button hook `0x159f2f0`: `code == 0x0b` + `g_shift` → toggle a global `g_ghosts`.
* Note hook `0x1e27360`: on a real hit with `g_ghosts` set, roll `p`; if it fires,
  schedule a second `note_on` with the scaled velocity after the offset.
* Offset timing uses the same mechanism as [live humanise](live-humanise.md) — build
  that first and reuse it. If off-thread triggering turns out to be unsafe, the
  feature degrades to a *doubled* hit (0 ms offset) and says so in the toast
  (`Ghosts 35% tight`).
* Offset must respect the transport tempo; read the project BPM from the same
  place `tap tempo` writes it if available, otherwise fall back to a fixed 30 ms.

### Unknowns

* Off-thread `note_on` safety (shared with humanise — resolve once).
* Whether a ghost fired from inside the `note_on` hook re-enters the hook (it will).
  Guard with a re-entrancy flag so a ghost cannot spawn a ghost.

## Acceptance criteria

- [ ] Armed: roughly 1 hit in 3 produces a quieter, slightly late companion.
- [ ] Disarmed: no ghosts at all, over 100 hits.
- [ ] A ghost never spawns a second ghost (re-entrancy guard).
- [ ] In record, ghosts appear as separate events with lower velocity, and they
      land on the grid you can edit afterwards.
- [ ] Disabling during playback stops ghosts immediately; none are left pending.
- [ ] Works on a pad bank of 16 different pads without cross-talk.

## Test plan

1. Arm, play 20 hits, count recorded events: expect ~20 real + ~7 ghosts, all with
   velocity 0.35–0.6 of their parent.
2. Disarm, play 100 hits, count: exactly 100.
3. Record a 2-bar pattern with ghosts and play it back with ghost mode off — the
   ghosts must still be there (they were recorded, not generated).
4. Fast repetition (16th notes) with ghosts on: no runaway, no stuck voices.

## Risks

| risk | mitigation |
|---|---|
| ghosts make fast hats muddy | probability 0.35 and velocity ≤0.6 are conservative; both are compile-time constants for tuning |
| ghost timing untestable without transport | fixed 30 ms fallback keeps it musical even without BPM |
| recording doubles the event count | that is the intended workflow; documented in the release notes |

## Out of scope

Ghosts generated without a played hit (a background generator), ghosts on other
pads than the one you hit, and any destructive change.
