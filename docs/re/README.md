# Decompiled fragments (Ghidra, firmware 1.3.0.12)

Address convention: **Ghidra VA = file VA + 0x100000** (the loader's image base is
`0x100000`; the file VAs used everywhere else in these docs are the ones the mod
computes as `base + offset`).

| file | Ghidra | file VA | what it is |
|---|---|---|---|
| `FUN_02902e60__pad_sync_loop.c` | `0x2902e60` | `0x2802e60` | loops over the 8 pad-state blocks and applies each |
| `FUN_0290dca0__pad_dispatch.c` | `0x290dca0` | `0x280dca0` | per-pad dispatch; `param_2 == 0` resets the page |

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

## HPSS investigation

[HPSS research](../HPSS-RE.md) traces the stock Chop Extract command and its
empty-pad selection. [Captured evidence](hpss/README.md) includes the native
sample creation and assignment paths, and
[runtime validation](hpss/runtime-validation.md) covers the shipped
SAMPLE OPTIONS flow.
