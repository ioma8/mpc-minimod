# Pad creation — current status and handoff

**Goal:** let a synth render write its generated audio to the **currently selected
pad**, including a project with **zero populated pads**.

Status: **current-pad rendering works when at least one populated pad exists.
The zero-populated-pads case is not implemented.**

Firmware: MPC Sample **1.3.0.12**.
Address rule: Ghidra VA = mod offset + `0x100000`.

## What works today

The shipped synth path:

1. renders the synth buffer,
2. uses the selected page/state,
3. runs the stock Extract command with its destination patched at
   `command + 0x58`,
4. Extract creates the new sample directly on the selected pad,
5. source length no longer determines synth audio length (`end = rendered`).

Verified in the emulator:

- selected empty pad with another populated pad as provider: works,
- populated selected pad: overwritten correctly,
- generated output length and quality independent of the source sample.

Relevant fixes are in the working tree/history around commits `d29f734` and
`bf35fe9`; SDK is `v0.1.8`. See [RE-SAMPLE-LOAD.md](RE-SAMPLE-LOAD.md) for the
Extract destination patch.

## The zero-pad gap

With zero populated pads, `borrow_provider()` finds no source. The synth stops
safely with:

```text
Synth: no source sample
```

Stock Extract still needs a source sample object to build its command. The
missing piece is a valid source sample object, not the pad assignment.

### Why a naive dummy sample fails

Creating a native sample from memory audio and writing it straight into the
selected layer makes the app show:

```text
Sample not found
Sample 002.wav
```

The sample exists as an object, but the app has not registered it in its sample
pool. Direct layer assignment is not enough.

## Native paths already identified

### Raw-audio sample builder

```text
FUN_01ef4ac0(
    out sample pair,
    app / sample context,
    { float32* begin, float32* end },
    channels,
    sample rate
)
```

It copies float32 audio into a memory reader, calls `FUN_01ef3710`, and returns
a native sample pair. It needs no file and no source pad. This is the most
promising zero-pad cue.

### Record-complete registration

```text
FUN_015929b0  SampleRecordCompleteCommand::Do
  -> FUN_01a95c10  constructor (sample pair + pool)
  -> FUN_01a967d0  execute
  -> FUN_02a31440  insert sample into the pool
```

`FUN_015929b0` later assigns through:

```text
FUN_01a95f90(
    command,
    view,
    program,
    &sample_holder,
    destination_pad,
    destination_layer,
    callback
)
```

### Live contexts captured in the emulator

Temporary hooks at `FUN_01a95c10` and `FUN_01a95f90` captured one real recording:

```text
record ctor:
  sample   = 0xffffe4077e00
  pool     = 0xaaaab1f25b30

assignment:
  view     = 0xaaaab1f4cd00
  program  = 0xffffe30a2f90   (= g_surface in that boot)
  callback = 0xaaaab1e7bf60
  pad      = 0
  layer    = 0
```

The sample-pool pointer was also located at:

```text
[g_surface + 0x1fc8] = pool
```

`[g_surface + 0x80]` held the captured callback in that boot. The `view` pointer
was not a direct field in the first `0x3000` bytes of `g_surface`; it is owned by
the record screen/command.

## What was tried

| experiment | result |
|---|---|
| `FUN_01ef4ac0` then direct layer write | `Sample not found / Sample 002.wav` |
| `FUN_01ef4ac0` then `FUN_01a95c10`/`FUN_01a967d0` registration | missing-file modal disappeared; sample accepted |
| inspect registered sample | channels `1`, frames `80000`, constant/DC waveform; expected stereo synth take |
| record UI with `AUDIO IN -> tone` | real recording works; pad event input object is `0x0` in the lab, so a synthetic pad press cannot drive it |
| direct memory clearing of pad layers as a zero-pad test | writes to `/proc/<pid>/mem` were denied; not a reliable test |

The direct-sample experiment was reverted. The working tree contains the
current-pad Extract implementation, not the experimental zero-pad code.

## Promising cues for the next attempt

1. **Determine `FUN_01ef4ac0`'s exact audio layout.**
   The registered result was mono/80000 frames/DC. The open question is whether
   the function expects interleaved or planar float32, and which object belongs
   in the `app / sample context` argument. Compare:
   - a real recorded sample's metadata and buffer geometry, and
   - a sample built from a known stereo sine buffer.

2. **Use the captured pool pointer.**
   `[g_surface + 0x1fc8]` is a strong candidate for the app's sample pool. Confirm
   it across boots; do not hardcode heap pointers.

3. **Register before assigning.**
   A sample object is not usable on a pad until it passes through
   `FUN_01a95c10`/`FUN_01a967d0`/`FUN_02a31440`.

4. **Assignment contexts.**
   `program` equals `g_surface` in the captured boot. `view` and `callback` are
   command/record-screen objects. If direct layer write is not enough after
   registration, use `FUN_01a95f90` with the captured live values, not a
   fabricated null callback.

5. **Record UI is a fallback, not the primary design.**
   The real recording UI works with audio input, but it needs a valid pad input
   object. In the lab, synthetic pad events carry `input = 0x0`. On real hardware
   the input object should be nonzero; this is not yet verified.

## What remains

- [ ] Find the correct `FUN_01ef4ac0` audio layout/context.
- [ ] Register the created sample in the pool without a file.
- [ ] Assign it to the currently selected pad (direct layer or `FUN_01a95f90`).
- [ ] Verify zero-populated-pads end to end in a project with no sample objects.
- [ ] Verify the resulting sample is stereo, has the rendered frame count, and is
      audibly the selected synth.
- [ ] Remove all probe hooks/logging and ship only the clean path.

## Handoff files

- [RE-RECORDING.md](RE-RECORDING.md) — record chain, raw builder, live contexts,
  experiments, unresolved audio layout.
- [RE-SAMPLE-LOAD.md](RE-SAMPLE-LOAD.md) — native sample load and Extract
  destination patch.
- [HYPOTHESES.csv](../HYPOTHESES.csv) — hypothesis/debug audit trail.
- [FEATURES.md](FEATURES.md) — shipped synth behavior and limits.

## Rules for the next person

- Do not ship a fabricated sample object or null assignment callback; both
  crashed or corrupted the sampler.
- Never hardcode heap pointers from a captured boot.
- Keep the working current-pad Extract path intact while investigating.
- Test on a **fresh emulator** or a genuinely zero-sample project; the factory
  project is fully populated.
- Record every experiment in `HYPOTHESES.csv`.
