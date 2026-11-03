# Native AutoTrim branch findings

The previous native replay supplied a null command context and normalized fractions.
Real knob calls supply a live dispatcher context and frame positions. The integer
argument formerly called mode is the pad index. Ghidra's FUN_02968450 resolves it
through the program's pointer vector at +0x2808 (end +0x2810). Each pad state holds
its layer array at +0x22e8; the first layer holds its sample at +0x4a8.

The new AutoTrim resolves the sample and dispatches commands using the same pad
index. It opens Start to zero before setting End and then Start, allowing bounds
to expand from a previously narrow trim. It does not call the old property scan,
write guessed model fields, or set overlay markers. The native command handles the
model and renderer. The command capture hook is enabled in release.

Fresh-emulator pad selection works. The old claim that channel-10 pad messages
cannot select the displayed sample is retracted for this emulator build.

Validation: release build, factory demo project, no prior knob learning required.
PAD 1: BoomBap-Kik-BTB MD.wav, 15129 frames, bounds 1..12487.
PAD 3: HipHop-Clp-Hrt OS.wav, 52775 frames, bounds 0..9652.
PAD 7: Trap-Vox-Zen1 91V.wav, 90197 frames, bounds 65..90197.
Native waveform markers and lower controls redraw; two-pad automated test retains
PID 687 and verifies page navigation responds. A03 screenshot shows End at ~18%.

Run from the worktree root after loading its release library:

    python3 mod/tests/emulator_autotrim.py

The test moves knobs through intermediate values before resetting them: sending
an unchanged MIDI value does not necessarily issue a new control change.

Remaining validation: hardware, project save/reload, non-first sample layers,
selection changed through routes other than pad presses, and selection while a
sequence is playing. The current index is the last note-on pad (default A01), so
sequencer playback may require a dedicated UI-selection source before release.
The original low-severity review cleanup is not completed by this branch.

## Float WAV correction

A07 and A09 use IEEE float32 WAV data, not integer PCM32. The earlier decoder
ignored the WAV format tag and interpreted float bit patterns as integer audio,
which made quiet tails appear loud. The decoder now distinguishes PCM and float,
rejects unsupported encodings, and handles full-scale negative PCM without a
16-bit absolute-value overflow. The default threshold remains -40 dB.

Verified in the emulator after this fix:
- A07: 81..60816 of 90197 frames (0.666 seconds removed from the tail).
- A09: 76..83103 of 98280 frames (0.344 seconds removed from the tail).

`python3 mod/tests/wav_edges.py` compiles the production helpers and checks
matching boundaries for PCM16, PCM24, PCM32, and IEEE float32 fixtures with
low-level noise, including a negative full-scale sample.

## Threshold adjustment requested after listening

The default relative amplitude threshold is now 3% of peak (about -30.46 dB),
three times the former 1%. `MINIMOD_TRIM_DB` still overrides it. Verified in the
emulator: ratio 0.0300, threshold 876 for peak 29204; A07 bounds 84..52786,
A09 bounds 89..73431. The PCM/float regression checks still pass.

## Four-step trim cycle (SHIFT held)

`SHIFT` + `B3` now cycles while SHIFT stays held, and the counter resets on
SHIFT release:

| press | threshold | effect on A01 `BoomBap-Kik-BTB MD` (15 129 frames) |
|---|---|---|
| 1 | 3 % of peak (876) | bounds `23..9794` |
| 2 | 6 % of peak (1752) | bounds `62..7731` (tighter) |
| 3 | 9 % of peak (2628) | bounds `71..5358` (tighter still) |
| 4 | none | bounds `0..15129` (trim removed) |

Each step shows a toast for one second — a 240x34 box centred on the screen with
`trimmed by 3% loudness`, `… 6% …`, `… 9% …` or `trim removed` — drawn by the
paint hook with the app's own `drawText`, so it disappears without a timer.

`B3` is button code `0x0e`, the same code the control surface repeats while the
button is held, so the handler acts on the press edge only (`g_b3_down`); without
that, one physical press fired the cycle up to 16 times. `MINIMOD_TRIM_DB` still
overrides the ratio for the first step only.

## Chop mode disables the whole feature (verified)

Chop replaces the Trim group on the Sample page: the page object holds both
groups and simply hides one. Measured in the emulator, `SampleModeScreen` has
two mutually exclusive children (offsets from the screen object, class vtables
in the image):

| offset | vtable | Trim page | Chop page |
|---|---|---|---|
| `+0x15e0` (first of four) | `0x706d428` | hidden | **visible** |
| `+0x1ee0` | `0x6d849e8` | **visible** | hidden |

Toggling the panel `CHOP` key (button code `0x4b`) flips both, and they never
disagree, so `Component::isVisible` (`0x112fd50`) on the first Chop child is the
mode:

```c
static int in_chop_mode(void)   /* only meaningful on SampleModeScreen */
{
    return isVisible(screen + 0x15e0);
}
```

`SHIFT`+`B3` and the "AutoTrim" cell are both gated on this. When Chop is on the
`B3` press is passed to the stock handler untouched (in Chop, `Shift+B3` is the
stock Split), no trim is computed, and the soft-key cell is not overdrawn.

Verified in the emulator after the change: `SHIFT`+`B3` in Trim logs
`native trim pad=0 … bounds=0..7204` plus the toast; the same combo in Chop logs
nothing. Holding `SHIFT` changes 517 pixels of the third soft-key cell in Trim
and **0** pixels in Chop, i.e. the overlay is gone too.

## The toast, and how it is cleared

Every trim raises a one-second box: `320x46` at `y = 194` (over the
Start/End/Loop bar), filled `0xff15151a`, an amber `0xffffb43c` rule 2 px along
the top and bottom (the colour the page already uses for the waveform) and white
18 pt text, drawn by the existing paint hook with the app's own `drawText`:

| step | text |
|---|---|
| 1 | `Trim 3% of peak` |
| 2 | `Trim 6% of peak` |
| 3 | `Trim 9% of peak` |
| 4 | `Trim cleared` |

The number is the gate level (samples quieter than N % of the sample's peak are
cut), which is why the label says "of peak" rather than "trimmed by" — and why
the last step says "cleared" instead of a level.

Three things had to be right, and each cost a round:

1. **Arm before dispatch.** The box is set *before* the three `orig_setctl`
   calls, because the repaint caused by the trim itself is what paints the page.
   Armed afterwards, the box only appeared on the next unrelated repaint — in
   practice the SHIFT release.
2. **Clearing needs a real invalidate.** The app only repaints on change, so the
   last frame kept the box up indefinitely. `A_REPAINT` (`0x1149880`, Ghidra
   `FUN_01249880`) takes **two packed pairs**, not four scalars:
   `param_2 = (x, y)` and `param_3 = (w, h)`, each split with
   `NEON_sshr(param, 0x20)` and scaled by a float. Calling it with `(0, 0)`
   invalidates a zero-size rectangle and does nothing. A detached thread sleeps
   1.12 s and repaints the screen, at which point the deadline check in the paint
   hook skips the box.
3. **The rect is in logical coordinates.** The app's logical width is wider than
   the 320 px framebuffer: a `320x240` rect cleared only the left two thirds and
   left the right third of the box on screen. Passing `2000x2000` clears all of
   it regardless of the scale factor.

The toast costs nothing when idle: it is a string, a deadline and a compare
inside the paint hook that already runs.

## Resolving the sample file

The trim needs the audio, and a sample does not always carry a path where a scan
would find it. Resolution order:

1. **The object field.** `sample+0x30` is the holder object; `holder+0x120` is a
   `std::string` with the file path. This is the normal case and is used first
   (`path from the sample object -> …`).
2. **The generic scan** (`wav_path_of`): walks the sample's pointers and one
   level deeper looking for a printable `*.wav` whose frame count matches.
3. **Frame count in the temp project**: a just-recorded, still-unsaved sample can
   have an empty path field, so the file is located under
   `/data/TempProjects/*/` by matching `frames` exactly (newest wins).

The WAV parser walks the chunk list rather than reading fixed offsets: Akai
recordings put a `JUNK` chunk before `fmt`, so `fmt` sits at 72 instead of 20.
`WAVE_FORMAT_EXTENSIBLE` is resolved through its subformat GUID.

Verified on real recordings: `Sample 001.wav → 80166 frames`,
`Sample 002.wav → 64650 frames`, a 0-byte `Sample 003.wav` rejected.
