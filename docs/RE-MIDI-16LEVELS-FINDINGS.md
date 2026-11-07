# MIDI → 16 Levels: decompiled facts (firmware 1.3.0.12)

Ghidra project `~/mcpsamplebin`, program `mpcsample.bin` (already analyzed).
Ghidra VA = file VA + 0x100000. Decompiled with
`tools/ghidra/hpss/ResearchDecompile.java` via analyzeHeadless (`-noanalysis`).

## 1. Dispatcher — FUN_0221b090 (file 0x211b090)

```c
uVar9 = (*(u32 *)(event+8) >> 12) & 0xf;      // channel
if ((*(u32 *)(event+8) & 0xff) == 3) {
    if (*(float *)(event+0x10) == 0.0) {      // note-off
        FUN_02218930(app, ctx, *(u8 *)(event+0x30), source, channel);
        ...
    }
    if (*(int *)(app+0x760) != 3 ||
        (*(u8 *)(*(long *)(app+0x750)+0x78) & 1) == 0) goto route;
}
route:
route_table = *(long *)(app+0x7a8);
list = FUN_0292a920(route_table, source);      // KEYED BY SOURCE
for each route entry:
    if (FUN_0292a9c0(route_table, entry, channel)) {
        FUN_0221aa30(app, ctx, event, entry.p0, entry.p1, source, 1);
        ...
        if (type == 3) {
            FUN_0292e910(pad_slots + channel*0x218, *(u8 *)(event+0x30), source);
            ...
        }
    }
```

* Routing is keyed by **source** (`param_4`). Physical pads use `0x7ffffffa`;
  external MIDI uses its own id (e.g. `0x90b6e9eb`).
* The pad/slot played is `*(u8 *)(event+0x30)`.
* `event+8` type, `event+0x10` velocity, `event+0x14` property block.

## 2. Router — FUN_0221aa30 (file 0x211aa30)

Decodes the event into a local note struct via `FUN_02215070`, then:

```c
if (type == 3 && (PadMidiIn || (app+0x760==3 && [app+0x750]+0x78&1))) return;
...
note = FUN_02214fc0(app, *(u32 *)(event+0x10), source, route_obj);  // slot
...
voice = FUN_021f8da0(*(long *)(app+0x778), route_obj);
if (on) { FUN_021e5460(voice, ctx, &note_struct);
          FUN_02219d00(app, ctx, route_obj, &note_struct, source, f); }
else    { FUN_021e56c0(voice, ctx, &note_struct); }
```

`FUN_02215070` copies the event and defaults `event+0x16..0x17 = 0x78 0x78`,
then `FUN_02214fc0` recomputes the slot byte at struct+0x30.

## 3. 16 Levels value writer — FUN_022472f0 (file 0x21472f0)

```c
type = *(u32 *)(levels_obj + 0x88);   // 16 Levels type
level = param_2;                      // 0..15
switch (type) {
  case 0: *(float *)(event+0x10) = (level*8 + 7)/127.0;      // velocity
  case 1: FUN_028e4400(event+0x14, 0, 0);                    // uses s0 = level
  case 2: *(u8 *)(event+0x31)=2;  FUN_028e4510(event+0x14, 2);
  case 3: *(u8 *)(event+0x31)=0xe; FUN_028e4510(event+0x14, 0xe);
  case 4: *(u8 *)(event+0x31)=7;  FUN_028e4510(event+0x14, 7);
  case 5: *(u8 *)(event+0x31)=8;  FUN_028e4510(event+0x14, 8);
  case 6: *(u8 *)(event+0x32) = percent(level);              // percentage
  case 7: event+0x34 = (event+0x34 & 0xf8) | ((level+1)/2-1);
  case 8: *(u8 *)(event+0x31)=0xf; FUN_028e4510(event+0x14, 0xf);
  case 9: *(u8 *)(event+0x33) = level;
}
```

So the row always writes **exactly one parameter slot** (or velocity) for a
16-value index 0..15. There is no absolute-note path in this function.

## 4. Property setter — FUN_028e4400 (file 0x27e4400)

```c
void FUN_028e4400(u16 *props, int index, ...) {
    range  = FUN_028e3cb0(index, 0);   // packed (min,max) in 32-bit halves
    steps  = FUN_028e3900(index);      // e.g. 241 for index 0/1, 101 for 5/6
    value  = (steps-1) * (s0 / (max - min));
    props[index+2] = clamp((int)value, 0, 255);
    props[0] |= 1 << index;
}
```

Ranges observed:

| index | min | max | steps |
|---|---|---|---|
| 0,1 | -120 | 120 | 241 |
| 5 | -50 | 50 | 101 |
| 6 | 0 | 100 | 101 |
| 15 | 1 | 128 | 255 |
| others | 0 | 127 | 255 |

`props` is the event block at `event+0x14`: u16 mask followed by 16 normalized
bytes (`event+0x16 .. 0x25`). Index 0 is the slot written by the 16 Levels
"Tune" branch (type 1).

## 5. Consequence for the mod

* The 16 Levels row has exactly **16 slot values**; a synthesized event can
  only address those slots.
* Feeding `prop0 = base + (note-60)` for notes outside 0..15 hands the engine
  values outside the row's parameter range. The engine resolves them to other
  pads/parameters — audible as different samples.
* Therefore the 16-key window (MIDI 60..75) is the only range this event path
  can reproduce exactly; full-keyboard chromatic needs the engine's voice pitch
  parameter (`FUN_021e5460` / `FUN_02219d00` path), not the row slot.

## 6. The octave-shift path (already proven in this mod)

`pitch_off` hooks `FUN_028e4400` and adds `8 * g_octave` to the 4th argument:

```c
static void pitch_off(void *a0, uint64_t a1, uint64_t a2, int a3) {
    int off = a3 + 8 * g_octave;
    orig_pitch_off(a0, a1, a2, off);
}
```

PLUS/MINUS bumps `g_octave` by ±1, which shifts the 16 Levels row by 8
semitones per press and audibly pitches outside the 16-slot grid. So the
property setter is a valid arbitrary-pitch control; the row slot is not the
only lever.

Ghidra shows `FUN_028e4400(ushort *props, uint index, int value)` normalizes
the value into `props[index+2]`; the decompiler also shows an `extraout_s0`
float, so the calling convention for the value (w3 vs s0) must be validated.
A direct `orig_pitch_off(event+0x14, 0, 0, row)` call from the MIDI hook
produced silence/noise, so the synthetic event also lacks the pad/slot fields
the setter path expects.

Next implementation: build the event the way the app's 16 Levels row does
(slot + selected pad), then apply `note-60` through the setter exactly as the
octave hook does (decompose into slot + 8*octave if needed).

## 7. Setter contract, captured from the app (instrumented pitch_off)

On a physical 16 Levels pad press the app calls `FUN_028e4400` with:

```
props = event+0x14, index = 0,
w3 = level - originalPad    (signed semitones; e.g. PAD6 level5 orig3 -> 2, PAD1 -> -3)
event+0x30 = selected pad   (0x0e for A15)
```

Reproducing that call for external MIDI with `w3 = note-60` routes **every**
note to the selected sample (RMS 172..639 across 36..96, versus stock routing
4256/16269).

Pitch measured with a 220 Hz sine:

| note | measured | expected |
|---|---|---|
| 60 | 110.4 Hz | root |
| 72 | 220.7 Hz | x2 (exact) |
| 84 | 438.7 Hz | x4 (1.988, exact) |
| 48 | 247.6 Hz | /2 (wrong) |

Positive offsets are pitch-exact through the setter. Negative offsets are the
remaining defect; the last negative sweep was invalid because the app was not
in the Tune/16 Levels state (stock-pad RMS 3000..4600), so it must be repeated
with a cleanly armed state, capturing the app's own MINUS (g_octave=-1) call
for comparison.

## 8. Decisive experiment and final implementation

One session, Pad MIDI In **On** (verified by screenshot), sine sample on A15,
16 Levels on (dispatcher site `155f947c`), setter called with `w3 = note-60`:

| note | w3 | measured | expected (220 Hz) |
|---|---|---|---|
| 48 | -12 | 110.4 Hz | 110.0 |
| 60 | 0 | 220.7 Hz | 220.0 |
| 65 | +5 | 293.4 Hz | 293.7 |

Positive and negative offsets are exact semitones. The earlier "48 = 247.6 Hz"
was PAD 6's tail (220*2^(2/12)=246.9), because the app's row is offset by
`originalPad` while the MIDI path uses absolute `note-60`.

Final implementation (minimod.c, MIDI hook):

1. On an external note in 16 Levels, build the note event for the selected pad:
   `mask=0`, `event+0x30 = selected pad`, `event+0x14` property block.
2. Call the app's own setter with `w3 = note - 60`:
   `orig_pitch_off(event+0x14, 0, 0, note-60)`.
3. Dispatch with the physical-pad source `0x7ffffffa`.
4. Note-off: same path with velocity 0 and the tracked replay note.

Verified: every note 36..96 plays the selected sample (RMS 161..613 vs stock
4256/16269); 16 Levels on installs the hook (`155f947c`), 16 Levels off leaves
the dispatcher stock (`d503233f`).
