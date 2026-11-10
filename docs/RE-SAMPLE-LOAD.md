# Native sample loading and pad assignment

Firmware: MPC Sample 1.3.0.12.
Address rule: Ghidra VA = mod offset + `0x100000`.

## Validated chain

| Ghidra VA | Role | Evidence |
|---|---|---|
| `FUN_01ecaf20` | High-level loader; builds a request and calls the load executor | decompile: constructs `FUN_01ee0580`, sets `request+0x40 = -1`, calls `FUN_01a5bd00` |
| `FUN_01ee0580` | Load-request constructor | decompile: writes 10 qwords, path at `+0x38`, pad/layer pair at `+0x40`, flags at `+0x48` |
| `FUN_01a5bd00` | Load executor; loads the request and optionally assigns a sample to a pad | decompile: reads path `+0x38`, pad `+0x40`, layer `+0x44`; calls `LoadSampleCommand` and `AssignSampleCommand` |
| `FUN_01a95ab0` | Native `Load Sample` command constructor | decompile: copies request fields into command |
| `FUN_01a95f90` | Native assignment command constructor | decompile: called only when request pad `!= -1` |

## Request layout (static, validated)

| Offset | Meaning |
|---|---|
| `+0x00..+0x30` | Loader context/service fields filled by `FUN_01ee0580` |
| `+0x38` | Sample key/path string |
| `+0x40` | Destination pad index; `-1` means load only |
| `+0x44` | Destination layer/index for the assignment |
| `+0x48` | Load flags / mode |

`FUN_02a2ed50` confirms `+0x38` is the lookup key: it searches the sample pool comparing `param_3` against each sample's `+0xa8` string.

## Important correction

`+0x38` is not a raw C++ `File` object; it is a sample key/path string consumed by the pool lookup. `FUN_01a5bd00` assigns only when `*(int *)(request+0x40) != -1`.

## Why the final path uses Extract

The file-load chain requires a live `AudioFormatReader`, sample-pool manager and
loader view. Those objects are only constructed by the Sample Select browser;
fabricating them crashes the sampler.

Extract already creates a sample from an audio buffer without a file path. Its
command keeps the destination pad at `command + 0x58`; patching that word before
execute sends the new sample to the selected pad instead of the next free pad.

## Common assignment path (validated)

Both native flows converge on `FUN_01a95f90`:

- `FUN_0153f0b0` (Extract command `Do`) calls `FUN_01a95f90` at `0x153f248`.
- `FUN_01a95ab0` (Load Sample command) and `FUN_01a5bd00` also use the same assignment machinery.

`FUN_01a95f90` builds the `AssignSampleCommand`; its virtual slot `+0x40` executes `FUN_01a9a3b0`, which applies the per-layer change through `FUN_01a9a290` and `FUN_029c6790`.

Extract call arguments:

```text
FUN_01a95f90(command, new_sample, program, &sample_holder,
             destination_pad, destination_layer, view/context)
```

The destination pad is `*(extract_command + 0x58)`; the sample is the command's new sample at `+0xb8`.

## Final implementation

The synth path does not construct a load request or call the assignment constructor:

1. Render the synth buffer.
2. Run the stock Extract command with an empty source provider when needed.
3. Patch the Extract command’s destination at `command + 0x58` to the selected pad before execute.
4. Extract writes the new sample directly to that pad.

The destination is restored after execute. `Extract::Request.destination` exposes this; `None` keeps the app’s free-pad behavior.

Verified in the emulator on empty pads `D04`, `D10`, `D11` and by overwriting `D10`; the selected pad received the named synth sample and no extra scratch pad was used.

## Zero populated pads

The zero-pad case is not yet supported. `borrow_provider` needs one populated
pad to seed the Extract source; with none it reports `Synth: no source sample`.

The missing object is a native source sample with a valid provider at
`sample + 0x130 + 0x10`. Creating one without an existing pad requires:

```text
FUN_01ef6140 / FUN_01ef3710
  -> FUN_02a43b40
  -> FUN_02a42150
```

or a native recording command that builds the same sample object from the
rendered buffer. Zeroed metadata alone is insufficient because Extract reads the
provider object, not only the frame count.


## Recording path

The native recording completion chain and the raw-audio sample builder are
documented in [RE-RECORDING.md](RE-RECORDING.md). It covers
`FUN_01ef4ac0`, `FUN_01a95c10`, `FUN_01a967d0`, `FUN_02a31440`, the captured live
contexts and the unresolved audio-layout issue.
