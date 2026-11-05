# Task — Per-hit variation

**Every hit of a pad is slightly different: round-robin feel without extra samples.**

Part of [M1](README.md).

---

## Button

`SHIFT` + `MUTE` (`0x2b`) toggles it for the **selected pad**. Toast: `Variation on`
/ `Variation off`. If the combo is taken, fall back to `SHIFT`+`LOOP` (`0x4a`).

## Behaviour

1. On every `note_on` for a pad with variation enabled, the velocity is scaled by
   `1 + U(−0.12, +0.12)` and clamped to `0.05 … 1.0`.
2. If a per-voice pitch and pan offset can be written (see the investigation), they
   get `±12 cents` and `±0.15` respectively.
3. Nothing else changes: the sample, the trim and the envelope stay put.
4. The variation is per pad and survives until toggled off or the project changes.

## Why velocity first

Velocity is the only per-hit value the firmware passes us directly — the
`0x1e27360` hook signature is `(input, src, pad, props, vel)`, and AutoTrim's
neighbour `note_off` proves the stream is live. Pitch and pan are probably *not*
per-voice parameters (they look like pad-level properties), so they are a stretch
goal, not the deliverable. The feature ships as velocity-only and says so.

## Implementation

* Hook: `0x1e27360` — `note_on(input, src, pad, props, vel)`. Already used by the
  mod for pad recording; add the scaling there, then call the original with the
  modified velocity.
* State: a 128-byte array of flags, index = pad. No persistence.
* Toggle: button hook `0x159f2f0`, `code == 0x2b` with `g_shift`.
* The RNG is a plain xorshift seeded once at load — no need for anything better, and
  it keeps the behaviour reproducible if the seed is logged.

### Unknowns

* Whether anything downstream reads the *original* velocity from `props` and would
  disagree with the modified one. Test: record a sequence with variation on and
  read back the recorded velocities (if they differ, the sequence is capturing the
  original — still acceptable, but worth documenting).
* Per-voice pitch/pan: probe whether `props` carries a voice object with a writeable
  pitch. If not, ship velocity-only.

## Acceptance criteria

- [ ] With variation on, 16 hits of the same pad produce visibly different
      velocities in the sequence recording (or audibly different levels).
- [ ] With it off, 16 hits are identical.
- [ ] The pad's static settings (level, filter, envelope) are unchanged.
- [ ] Toggling does not affect any other pad.
- [ ] No audio dropout or extra latency (the hook is synchronous and cheap).

## Test plan

1. Enable on pad 1, hit it 16× in record, inspect the recorded velocities.
2. Disable, hit 16× again, confirm identical.
3. Enable on a pad with a loop and on a one-shot; both must stay usable.
4. Check CPU: the hook runs per note event, not per audio block.

## Risks

| risk | mitigation |
|---|---|
| the app caches velocity elsewhere and the modified value is ignored | test 1 above reveals it immediately; fall back to scaling at the sound level (pad gain), which is not per-hit but still creates movement |
| ±12 % is too much on already-loud pads | clamp and, if needed, a fixed `MINIMOD_VAR=0.08` override for taste |

## Out of scope

Sample switching (true round-robin with several samples), pitch per hit unless the
investigation says it is free, and any file writes.
