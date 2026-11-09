# Raw evidence

Unedited captures behind [`../MECHANISMS.md`](../MECHANISMS.md) and
[`../HISTORY.md`](../HISTORY.md): Ghidra script output, decompiled function
fragments and screenshots. They are data, not documentation — no claim here is
verified by itself.

**Address convention:** Ghidra VA = mod (file) offset + `0x100000`. Files named
`FUN_<va>__*.c` use the Ghidra VA.

## Decompiled fragments

| file | Ghidra | mod offset | what it shows |
|---|---|---|---|
| `FUN_02902e60__pad_sync_loop.c` | `0x2902e60` | `0x2802e60` | loops over the 8 pad blocks (`0x5980 / 0xb30`) and applies each through `FUN_02918be0` |
| `FUN_0290dca0__pad_dispatch.c` | `0x290dca0` | `0x280dca0` | per-pad dispatch; `param_2 == 0` resets the page |

The pad-block loop is the clearest statement of the pad-state layout:
`object + 0x22e8` is the block array, blocks are `0xb30` bytes, and
`FUN_02918be0` reads the knob floats at `+0x1a8`, `+0x220`, `+0x298`, `+0x310`,
`+0x368`, `+0x3c0` (stride `0x78`). See
[`../MECHANISMS.md`](../MECHANISMS.md#3-pad-state-and-layers).

## HPSS / Extract (`hpss/`)

Ghidra Java script output from the Extract investigation, grouped by area.
Runtime validation of the shipped flow is in
[`../FEATURES.md`](../FEATURES.md#hpss); the writer boundary and the tail trick
are in [`../MECHANISMS.md`](../MECHANISMS.md#the-writer-boundary).

| group | captures |
|---|---|
| Extract command | `extract-constructors.txt`, `extract-and-assign.txt`, `extract-ui.txt`, `command-execution.txt`, `free-pad.txt`, `pad-occupied.txt`, `file-loader.txt`, `load-request.txt` |
| provider / writer boundary | `audio-provider.txt`, `provider-methods.txt`, `provider-read.txt`, `provider-read-assembly.txt`, `provider-writer.txt`, `read-loop.txt`, `writer-consumer.txt`, `wav-write-block.txt`, `reverb-reader-boundary.txt`, `native-controls.txt` |
| assignment | `assign-layer.txt` |
| slices | `slice-create.txt`, `slice-execute.txt`, `slice-vtable.txt` |

## Spectral Chop (`spectral-chop/`)

Screenshots of the detector against real material, used while tuning the
region-vs-onset rule: `b3-change.png`, `b3-onsets.png`, `b3-zoom-0-1s.png`,
`b3-zoom-2-3s.png`, `guitar-change.png`, `regions-break.png`,
`regions-guitar.png`, `regions-vocal.png`.

## Reproducing a capture

All `.txt` files were produced by running the Java scripts in
`tools/ghidra/hpss/` against the imported image; the invocation and the address
convention are in [`../TOOLING.md`](../TOOLING.md#running-the-java-scripts).
Regenerate rather than trust a stale capture when the firmware or the target
address changes.
