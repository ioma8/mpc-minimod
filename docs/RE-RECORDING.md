# Recording path: creating and registering a sample

Firmware: MPC Sample 1.3.0.12.
Address rule: Ghidra VA = mod offset + `0x100000`.

This note records what was learned while trying to reuse the recording path for
the zero-populated-pads synth case. It is evidence, not a shipped feature.

## Recording completion chain

| Ghidra VA | Role |
|---|---|
| `FUN_015929b0` | `SampleRecordCompleteCommand::Do` |
| `FUN_01a95c10` | record-complete command constructor |
| `FUN_01a967d0` | command execute; calls the pool insert |
| `FUN_02a31440` | insert a sample object into the app's sample pool |

`FUN_015929b0` receives a command whose `+0x50` holds the recorded sample pair.
It builds `FUN_01a95c10(command, sample_pair, pool)` and executes it with
`FUN_01a967d0`. Later in the same `Do`, the recording path assigns through:

```text
FUN_01a95f90(command, view, program, &holder, pad, layer, callback)
```

The recorded sample object is built **before** this command by the recording
engine; the command registers/assigns it.

## Raw-audio sample builder

`FUN_01ef4ac0` takes:

```text
out sample pair
app / sample context
{ float32* begin, float32* end }
channels
sample rate
```

It copies the float32 range into an in-memory audio reader, calls
`FUN_01ef3710`, and returns a native sample pair. It needs no file and no source
pad. This is the only raw-audio constructor found; `FUN_02a42150` is called only
from `FUN_02a43b40`, whose callers are reader-based.

## Live context captured at runtime

Temporary hooks were installed at `FUN_01a95c10` and `FUN_01a95f90` and one
recording was made in the emulator with `AUDIO IN -> tone`.

Record-complete constructor:

```text
sample = 0xffffe4077e00
pool   = 0xaaaab1f25b30
```

Assignment constructor:

```text
view     = 0xaaaab1f4cd00
program  = 0xffffe30a2f90   (= g_surface in that boot)
callback = 0xaaaab1e7bf60
pad      = 0
layer     = 0
```

The sample-pool pointer was also located at:

```text
[g_surface + 0x1fc8] = pool
```

`g_surface + 0x80` held the assignment callback value in that boot:

```text
[g_surface + 0x80] = 0xaaaab1e7bf60
```

The `view` pointer was not found in a direct scan of the first `0x3000` bytes of
`g_surface`; it is a record-screen-owned object, not a stable surface field.

## Experiments and outcomes

1. **Direct layer write after `FUN_01ef4ac0` only**

   Result: the app showed:

   ```text
   Sample not found
   Sample 002.wav
   ```

   The sample object existed but was not registered in the sample pool.

2. **`FUN_01ef4ac0` + `FUN_01a95c10`/`FUN_01a967d0` registration**

   Result: the missing-file modal disappeared. The sample was accepted by the
   app and appeared on the pad.

3. **Audio geometry check**

   The produced sample was:

   ```text
   channels = 1
   frames   = 80000
   waveform = constant / DC
   ```

   The source synth take was stereo. The exact audio layout expected by
   `FUN_01ef4ac0` is still unresolved: interleaved vs planar, the correct
   `app`/pool argument, and the frame/channel fields.

4. **Record-UI bootstrap**

   With `AUDIO IN -> tone`, the real record UI can record into a pad. A synthetic
   pad press cannot drive it in the lab because the pad event carries no input
   object:

   ```text
   record-provider raw valid=1 input=0x0
   ```

   `last_press_obj` was also zero. `send_note_on` was not called with a null
   input.

## Current status

The normal current-pad synth flow is shipped and unchanged. The zero-pad path
is not shipped. The next test is to feed `FUN_01ef4ac0` the exact audio layout
the recording engine uses, then verify the registered sample's frames, channels
and waveform before assigning it to the selected pad.
