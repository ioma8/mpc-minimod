# Task — Live humanise

**Hold a key: your playing loosens. Release: it snaps back tight.**

Part of [M1](README.md). The momentary sibling of [per-hit variation](per-hit-variation.md):
variation is a per-pad setting, humanise is a performance gesture.

---

## Button

Hold `ERASE` (`0x09`) — while held, every pad hit is humanised; on release the
effect stops instantly. Toast on press: `Humanise`, no toast on release.

If `ERASE` is unsafe to intercept (it deletes events in record), fall back to
holding `NOTE REPEAT` (`0x0b`) or a second press of `SHIFT`+`MUTE`.

## Behaviour

1. While armed, each `note_on` gets:
   * velocity `×(1 + U(−0.15, +0.15))`,
   * a timing offset of `0 … +18 ms` chosen per hit.
2. Timing is applied only if it can be done without blocking the input thread: the
   mod schedules the original `note_on` on a short timer. If that proves unstable,
   **timing is dropped and the feature ships velocity-only** — still audible, still
   useful, and honest in the toast (`Humanise (vel)`).
3. Releasing the key cancels pending timing offsets (they are ≤18 ms, so at most one
   is in flight).
4. No state persists and nothing is written to the project.

## Implementation

* Button hook `0x159f2f0`: `code == 0x09`, `down` → arm, `!down` → disarm.
* Note hook `0x1e27360`: the same one used by per-hit variation; humanise and
  variation compose (both scale velocity, so the factors multiply, clamped).
* Timing: a `timer_create`/`nanosleep` worker or `usleep`-free design — **never**
  sleep on the UI thread. Queue at most 8 pending notes.

### Unknowns

* Whether the app tolerates a `note_on` delivered from a non-UI thread. If not,
  timing is impossible without the app's own scheduler, and velocity-only is the
  answer. **Test this before building the timing part.**
* Interaction with `note_off` ordering (a delayed on followed by an immediate off).

## Acceptance criteria

- [ ] While held, 16 hits vary in velocity; on release, 16 hits are identical.
- [ ] No dropped or doubled notes (count the recorded events — must match the hits).
- [ ] No audible latency on the *press* itself (only the humanise offset).
- [ ] Releasing mid-hit does not leave a hanging voice.
- [ ] Works with two pads played simultaneously.

## Test plan

1. Hold `ERASE`, play 16 hits, inspect recorded velocities and positions.
2. Release, play 16 hits, confirm identical positions.
3. Fast rolls: 20 hits in 2 s with humanise on — none lost (compare event count).
4. Hold `ERASE` through a bar change and stop; no stuck notes.

## Risks

| risk | mitigation |
|---|---|
| off-thread `note_on` corrupts input state | test first; ship velocity-only if it does |
| delayed notes land after stop | cancel the queue on any STOP / transport event |
| user leaves `ERASE` held by accident | the effect is audible and ends on release by construction |

## Out of scope

Quantise-off, groove templates, swing changes. Humanise only ever nudges the note
it is given.
