# Decompiled fragments (Ghidra, firmware 1.3.0.12)

Address convention: **Ghidra VA = file VA + 0x100000** (the loader's image base is
`0x100000`; the file VAs used everywhere else in these docs are the ones the mod
computes as `base + offset`).

| file | Ghidra | file VA | what it is |
|---|---|---|---|
| `FUN_02902e60__pad_sync_loop.c` | `0x2902e60` | `0x2802e60` | loops over the 8 pad-state blocks and applies each |
| `FUN_0290dca0__pad_dispatch.c` | `0x290dca0` | `0x280dca0` | per-pad dispatch; `param_2 == 0` resets the page |

## SAMPLE OPTIONS input

[Native DATA-event trace](SAMPLE-OPTIONS-INPUT.md): the signed integer emitter,
control-surface identity, modal ownership, why float-property inference lost
turns, and the host/emulator regression checks.

## The pad-state blocks

`FUN_02902e60` is the clearest statement of where the per-pad state lives:

```c
lVar1 = *(long *)(param_1 + 0x22e8) + lVar3;   /* destination pad block */
lVar2 = *(long *)(param_2 + 0x22e8) + lVar3;   /* source pad block      */
FUN_02918be0(lVar1, lVar2);
```

* the array pointer sits at `object + 0x22e8`
* blocks are `0xb30` bytes apart, and the loop runs `0x5980 / 0xb30 = 8` times (8 pads)
* `FUN_02918be0` then reads the knob floats from the source block at
  `+0x1a8`, `+0x220`, `+0x298`, `+0x310`, `+0x368`, `+0x3c0` (stride `0x78`)

So **the internal trim state is `padblock + 0x1a8` (Start) and `padblock + 0x220`
(End)** for the selected pad. Everything the mod wrote before this finding - the
parameter value, its applied copy, the property cache, the layer frames and the
slider - is downstream of that state, which is why none of it made the app's own
view change.

## Firmware mechanisms

[FIRMWARE-MECHANISMS.md](FIRMWARE-MECHANISMS.md) is the reference for what we
learned about the firmware itself: loader and address spaces, the screen/page
and paint model, pad state and the layer array, the sample, the slice container
and its native API, the chop-type binding, the observable property system, the
slice command/undo API, the control-surface input path, the sample pool and
Extract, persistence, threading rules and the limits. Every entry carries its
address, an evidence marker and a pointer to the deeper write-up.

## Spectral Chop

[The solve](SPECTRAL-CHOP-SOLVED.md) is the record: the chop model, the native
slice API (`0x2950aa0` reset, `0x2950d20` insert-at-frame, `0x2950bb0`
reconcile, `0x16680a0` select), the chop-type binding, the UI-thread phase
machine, a catalogue of every mechanism used (hooking, `/proc/self/mem`,
snapshots, page watches, value search, the panel harness, the model read-back
test) and how each was verified. [The history](spectral-chop-handoff.md) is the
short chronological record of the six passes and the leads that were disproved.
The acceptance test is `mod/tests/emulator_spectral.py`.

Note for anyone working here: this index states the address rule below, and the
Spectral Chop investigation still lost a pass to ignoring it.

## HPSS investigation

[HPSS research](../HPSS-RE.md) traces the stock Chop Extract command and its
empty-pad selection. [Captured evidence](hpss/README.md) includes the native
sample creation and assignment paths,
[runtime validation](hpss/runtime-validation.md) covers the shipped
SAMPLE OPTIONS flow, and
[findings and method](../HPSS-RE-FINDINGS.md) records the encoder discovery,
the useful tooling and the blind paths.

The HPSS Extract writer boundary also supports Add Reverb's appended tail:
[writer-tail mechanism and emulator evidence](../REVERB-HANDOFF.md).
The firmware's built-in `Air::H3M::StereoReverb` class, factory, vtable and
current integration boundary are documented in [BUILTIN-REVERB.md](BUILTIN-REVERB.md).
The Sample Options Bass Synth renderer and the H3 host interface requirement
that blocks direct FMBass construction are documented in
[BASS-SAMPLE.md](BASS-SAMPLE.md).
