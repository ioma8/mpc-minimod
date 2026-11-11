# Recording path: creating and registering a sample

Firmware: MPC Sample 1.3.0.12.
Address rule: Ghidra VA = mod offset + `0x100000`.

This note preserves the reverse-engineering record of the MPC recording path.
Minimod's current zero-pad synth flow uses stock Extract and the SDK seed API;
see [PAD-CREATION.md](PAD-CREATION.md) for its implementation and verification.
The dated experiments below explain why the recording path was not selected.

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
x8: out sample pair (C++ indirect return)
x0: app / sample context
x1: { float32* begin, float32* end }
w2: channels
w3: sample rate
```

It copies the float32 range into an in-memory audio reader, calls
`FUN_01ef3710`, and returns a native sample pair. It needs no file and no source
pad. This is the only raw-audio constructor found; `FUN_02a42150` is called only
from `FUN_02a43b40`, whose callers are reader-based.

## Builder context: static call chain

The recording engine uses a real app-owned sample service, not the pool as the
builder context. The static chain is:

```text
FUN_0149d710: service = new 0x58; FUN_01ef3330(service, ...)
              app[0xa0] = service
              FUN_019dcc50(..., service, ...)  // argument 19
FUN_019dcc50: FUN_01f17c90(..., service, ...)  // argument 8
FUN_01f17c90: [record engine + 0xab0] = service
FUN_01f176f0: FUN_01ef5120([record engine + 0xab0], left, right, ...)
FUN_01ef5120: interleave the two channels, then FUN_01ef4ac0(service, ...)
```

`FUN_01ef3330` constructs the service using the name `Temporary Samples`.
Temporary hooks on both constructors captured the same nonzero service pointer
in one emulator boot, confirming the relationship at runtime. Cross-boot and
lifetime behavior still need validation before relying on it in the mod.

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

   The source synth take was stereo. The reader uses interleaved float32: `FUN_021ceab0` returns a pointer at
   `buffer + channels * frame_offset * 4`. The builder sets channel count
   from its fourth argument and frames to float count divided by channels.
   This static evidence does not explain the earlier mono/DC result; the
   actual call arguments, buffer contents, and context still need live checks.

4. **Record-UI bootstrap**

   With `AUDIO IN -> tone`, the real record UI can record into a pad. A synthetic
   pad press cannot drive it in the lab because the pad event carries no input
   object:

   ```text
   record-provider raw valid=1 input=0x0
   ```

   `last_press_obj` was also zero. `send_note_on` was not called with a null
   input.

## Current implementation

The zero-pad synth path is integrated. It supplies a native seed to stock
Extract, patches the selected destination, removes only temporary provider
copies, and enables the live layer through SDK `pads::set_layer_active`. Ghidra
and emulator evidence are recorded in [PAD-CREATION.md](PAD-CREATION.md). The
raw-builder experiments that follow are historical alternatives, not pending
steps for the shipped synth path.

## Controlled raw-builder probe

On the zero-sample emulator project, a temporary hook captured the live
`Temporary Samples` service. A probe passed 4,096 frames of interleaved
float32 with opposite-polarity left and right ramps through `FUN_01ef4ac0`,
using `x8` for the result and `w2 = 2`, `w3 = 44100`. The returned native
sample reported two channels, 4,096 frames and 44,100 Hz. Its first values
matched the input pattern within roughly 1/32768, indicating quantization.
This validates construction and geometry, not pool registration, assignment,
or the earlier unexplained mono/DC experiment.

A follow-up prototype registered that ramp using `FUN_01a95c10` and
`FUN_01a967d0`, temporarily borrowed it into the empty layer, and ran stock
Extract with the rendered synth buffer. The emulator reported `Synth: copy
created`; the selected sample measured stereo, 33,075 frames at 44,100 Hz,
with RMS 0.092. This is a prototype: modal handling, audible pitch, persistence,
reference ownership, and removal of the extra seed remain unverified.

A second run omitted pool registration for the temporary native provider.
Extract still created the selected `Bass C2` pad with stereo 33,075-frame
audio at 44.1 kHz (RMS 0.092); the screen showed the waveform and no modal.
After the app stopped, the saved project contained exactly one sample-pool
entry, `Bass C2.wav`, with no seed entry. Reload, seed lifetime, cleanup, and a
pitch check remain to be verified. The current experiment intentionally retains its sample
pair and is not production-ready.

The longer maintained-path test exposed two failures hidden by the initial
short check. Releasing the seed after Extract was followed by a firmware
restart. Keeping it alive avoided that immediate restart, but a full pad scan
found the 4,096-frame seed still referenced on A02 while A01 held the synth.
An earlier saved prototype likewise contained an `internal` A02 reference;
the first status/audio assertion did not check all pads. Seed cleanup and
ownership therefore remain open; neither short pass is release evidence.

A follow-up retained one seed for the process lifetime and cleared pad-layer
references that still pointed to it after Extract. The full pad scan then saw
only A01 populated; the saved project contained only `Bass C2.wav`, and A01
showed the sample after an app restart. The 33,075-frame output measured a
69 Hz peak (within 5 Hz of C2). This establishes one successful scenario;
repeated renders, cancellation, alternate destinations, and the safety of
clearing the native layer references still require testing.

Replacing direct zero writes in the cleanup with the app's sample-selection
control (`0x40d`, value `0`) also removed the seed from the extra pad. In one
A01 run, all-pad inspection found only A01, the restart count remained zero,
and the saved project contained only `Bass C2.wav`. The setter is the native
path used by `FUN_01a9a290`; this is preferable to manually clearing layer
pointers. Further destination and repeated-render checks are still needed.

An empty-project A05 test passed with only A05 populated. A subsequent render
to A09 in the same session created A09 but also left an unwanted provider
copy on A01. The extra-copy issue therefore also affects borrowing a populated
pad, not only the synthetic seed. Cleanup must distinguish pads that were
already populated from newly created provider copies.

The next prototype snapshots pads known to be empty before borrowing and uses
the native sample-selection control only for newly occupied provider copies,
excluding the destination. In a fresh empty project, rendering A05 then A09
left exactly those two pads populated with no restart; the saved project had
two pool entries and layers 4 and 8. Source audio preservation, cancellation,
and repeated reloads still require verification.

After restarting the two-pad project, an overwrite of populated A05 left an
unwanted provider copy on A01. A09's WAV SHA-256 did not change. The initial
occupancy-based cleanup applied only when borrowing; the populated-source
path also needs that cleanup.

Applying the same empty-pad snapshot to a populated source fixed the overwrite
case in the emulator: after reloading A05/A09, rendering on A05 left exactly
those two pads populated, preserved A09's WAV SHA-256, and did not restart
the firmware. Cancellation and other timing cases remain open.

An attempt to cancel by immediately sending DATA then SEQ completed the synth
render before cancellation: status was `Synth: copy created`, only A05 was
populated, and the PID stayed unchanged. This is inconclusive for cancellation.

Borrowing A05 as the provider for a render on empty A09 left exactly those two
pads populated. The provider WAV decoded to 66,150 float32 values before and
after, with maximum absolute difference 0.0.

The durable fixture generator and empty-project regression now reproduce A05
from a generated zero-sample project. They verified one populated pad, stereo
33,075 frames at 44.1 kHz, RMS 0.09215, a 69 Hz peak, no modal and no restart.
The screenshot check must wait for the menu to close: checking immediately
after the success status initially misidentified the menu as a modal.

A three-second temporary render delay showed why the prior page-switch
attempt did not exercise cancellation: the menu consumes non-encoder buttons
while open, so SEQ was swallowed and the render completed. This result does
not validate cancellation.

A separate temporary build forced the synth finalize cancellation branch. It
reported `Synth: cancelled`, the 128-pad scan found no samples, the PID and
restart counter stayed unchanged, and the saved project contained zero pool
entries and no sample layers. This verifies cleanup when that branch runs, not
the physical page-change trigger. The forced branch was removed afterward.

The native seed constructor and service-capture hook were moved into
`mpc-sample-sdk::platform::sample::silent_seed`; minimod calls that SDK API.
The SDK owns the hook in its site table and retains one seed per process. The
the workspace then used remote SDK tag `v0.1.9`. An integrated zero-project emulator
run on A05 passed the full pad
scan, stereo geometry, pitch, modal and restart checks.

After publishing SDK `v0.1.9`, minimod was rebuilt from that remote tag
(commit `185c41c6`) and the A05 empty-project regression passed again with
33,075 stereo frames, a 69 Hz peak, no extra pad and no restart.


SDK `v0.1.10` also adds `pads::set_layer_active`. Ghidra identifies the live
`InstrumentLayer::active` byte at `+0xb18`; minimod sets it after Extract. A01
and A05 both produced nonzero PCM, and A01 remained audible after a project
save and reload. See [PAD-CREATION.md](PAD-CREATION.md) for measurements.
