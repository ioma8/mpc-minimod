# Task — Preset Roulette

**One press: a random, constrained pad recipe. Always musical, never a dead sample.**

Part of [M1](README.md).

---

## Button

`SHIFT` + `PAD BANK` (`0x23`), **double-tap** (two presses within 400 ms).

Single tap is [jitter](jitter.md) — same button, smaller move. If `SHIFT`+`PAD BANK`
is taken in stock, fall back to `SHIFT`+`SAMPLE SELECT`, then `SHIFT`+`ERASE`.

## Behaviour

1. The recipe applies to the **selected pad** only.
2. Parameters are written through the app's own control dispatch (`0x1968020`), the
   same path `SHIFT`+`B3` uses for the trim, so the app's model stays consistent
   and the page repaints itself.
3. A one-second toast names the result: `Roulette: tight kick`.
4. Any following press replaces the last roulette result; the recipe is remembered
   so [jitter](jitter.md) can start from it.
5. One extra press of the same combo within 2 s (i.e. triple-tap) reverts to the
   values from before the roulette — the "bin" action.

## The recipes

Curated, not uniform random. Each recipe is a set of `(controlId, value)` pairs;
the *values* below are normalised 0…1 as the dispatch expects, the *control IDs*
are to be filled in during the investigation step.

| recipe | character | filter | amp env |
|---|---|---|---|
| `tight kick` | short, low, punchy | LP ~400 Hz, res 0.2 | attack 0, decay 0.12 |
| `808` | long sub | LP ~250 Hz, res 0.1 | attack 0, decay 0.85 |
| `dusty loop` | dark, no top | LP ~3 kHz, res 0.3 | attack 0.02, decay 0.6 |
| `wide pad` | soft, open | LP ~9 kHz, res 0.15 | attack 0.45, decay 0.9 |
| `stab` | hard attack, short tail | LP ~5 kHz, res 0.5 | attack 0, decay 0.2 |
| `punchy snare` | mid-forward | BP ~1.8 kHz, res 0.4 | attack 0, decay 0.25 |
| `roomy` | long, dark | LP ~6 kHz, res 0.1 | attack 0.1, decay 0.75 |
| `telephone` | narrow band | BP ~1.2 kHz, res 0.7 | unchanged |
| `air only` | thin, bright | HP ~2 kHz | attack 0.05, decay 0.4 |
| `muffled` | dull, close | LP ~800 Hz, res 0.05 | unchanged |

Every recipe only touches **filter** (type/cutoff/resonance) and **amp envelope**
(attack/decay), never level, never the sample, never the trim. That is the
constraint that keeps a random press from being a dead sample: you cannot randomise
your way to silence, a missing sample, or a wrong trim.

## Implementation

* Button: hook `0x159f2f0`, `code == 0x23`, `down == 0x7f`, track a double-tap
  timestamp; `SHIFT` state is already tracked (`g_shift`).
* Dispatch: `orig_setctl(context, surface, pad, controlId, value)` — the trim path
  in `do_autotrim()` shows the exact call shape.
* Repaint: `0x1149880(screen, 0, screen[0x28])` after the writes.
* Toast: existing `toast()`; it must be armed **before** the dispatch (learned from
  AutoTrim, where arming after meant the toast only appeared on SHIFT release).

### Unknowns to resolve first (½ session)

1. **Control IDs** for filter type, cutoff, resonance, attack, decay on the Sample
   page. Use the existing `setctl_probe` hook: turn each knob, log
   `(controlId, value)` pairs, and map the IDs.
2. **Value scale** per control: normalised 0…1, Hz, ms, or an enum (filter type is
   almost certainly an enum of 29 values — see `CONTEXT.md`).
3. Whether the dispatch needs the *page* object as the first argument or the
   context; AutoTrim already passes `(context, surface, pad, …)` successfully.

## Acceptance criteria

- [ ] Double-tap applies a random recipe; the toast names it.
- [ ] Ten presses give ten audible variations, none silent, none unchanged.
- [ ] The page reflects the change without leaving it (filter/amp display updates).
- [ ] Triple-tap restores the pre-roulette values exactly.
- [ ] The roulette never changes level, trim, or the assigned sample.
- [ ] Works on an empty pad: reports `empty pad`, changes nothing.
- [ ] No crash on 50 consecutive presses.

## Test plan

Emulator, release build, factory project:

1. Select a pad with a loop. Sweep the filter and envelope knobs once with
   `setctl_probe` active to fill in the control-ID table.
2. Double-tap 10×, screenshot each time, confirm the toast differs and the sound
   changes (screenshot the filter/amp page, not just the pad).
3. Triple-tap and compare the page to the pre-roulette screenshot — byte-identical
   is too strict; the parameter readout must match.
4. Empty pad: expect `empty pad`.

## Risks

| risk | mitigation |
|---|---|
| unknown control IDs | the probe already logs them; ½ session |
| a recipe is inaudible on some material (e.g. `air only` on a hat) | recipes are applied to the *pad*, so audition decides; toast names it so the user can immediately roll again |
| dispatch on the wrong thread corrupts state | all writes stay inside the button hook, i.e. the UI thread, as AutoTrim does |

## Out of scope

Randomising level, pan, trim, sample choice, or anything that writes a file. Recipes
never stack — one recipe per press.
