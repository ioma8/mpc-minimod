# Task — Jitter

**One press: move the sound you already like 5–15 % somewhere else.**

Part of [M1](README.md). The companion of [preset roulette](preset-roulette.md):
the roulette replaces, jitter *nudges*.

---

## Button

`SHIFT` + `PAD BANK` (`0x23`), **single tap**.

Double-tap is the roulette. If the combo is taken in stock, fall back per
[README](README.md#tasks--m1-free-wins).

## Behaviour

1. Reads the selected pad's **current** filter and amp-envelope values.
2. Multiplies each normalised value by `1 + U(−s, +s)` with `s = 0.10` (10 %).
3. Clamps to the parameter's legal range and dispatches each value back through
   `0x1968020`.
4. Toast: `Jitter ±10%`.
5. Pressing again re-jitters from the new value — a random walk. To keep it from
   drifting away forever, the walk is remembered: a press within 2 s of the
   previous one uses a **smaller** step (5 %), so repeated taps settle instead of
   running off.
6. Triple-tap returns to the value before the first jitter of the session.

## Why jitter and not more randomness

A pad you already chose is a set of decisions you made by ear. Re-rolling it throws
them away; nudging it explores the neighbourhood you were already in. That is the
whole feature: **±10 % is small enough to stay musical and large enough to be
audible on every pad.**

## Implementation

* Read: the parameter objects are reachable from the pad chain
  (`surface + 0x2808 → state + 0x22e8 → layer + 0x4a8`); the trim parameters'
  layout is partly known (`T_VALUE 0x19c`). If a control's current value cannot be
  read reliably, fall back to the value the mod last wrote for that control
  (kept in a small per-pad table).
* Write: `orig_setctl(context, surface, pad, controlId, value)`.
* The jitter must run **after** the control-ID table from the roulette task exists.

### Unknowns

* Read path for filter/envelope values (the trim ones are known, these are not).
* Whether the app quantises values (a 10 % move on a 3-value enum like filter type
  must be handled as "no change", not as an out-of-range write).

## Acceptance criteria

- [ ] Single tap changes the pad audibly, and the page's readout changes with it.
- [ ] Ten taps in a row keep the pad usable — it does not drift to silence or to a
      fully closed filter.
- [ ] Triple-tap restores the starting values.
- [ ] The jitter never touches level, trim, or the sample.
- [ ] On a pad with no filter/envelope (empty pad) it reports `empty pad`.

## Test plan

1. Pick a pad, note the filter/amp values from the screen.
2. Tap 5× with 3 s gaps, screenshot after each: every value must differ by roughly
   5–15 %, and each step must stay inside the visible range.
3. Tap 10× quickly: the steps must get smaller, not walk off.
4. Triple-tap and compare with the first screenshot.

## Risks

| risk | mitigation |
|---|---|
| read path unknown | fall back to a per-pad cache of written values; it makes jitter "relative to what the mod set", which is still useful |
| enum parameters (filter type) | jitter only continuous parameters (cutoff, resonance, attack, decay); leave type alone |
| random walk drifting | the 2 s smaller-step rule above |

## Out of scope

Randomising level, pan, sample choice or trim. Jitter is a *sound* feature, not a
kit-level one.
