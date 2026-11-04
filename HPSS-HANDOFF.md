# HPSS handoff — 2026-11-04

## Goal and workspace

In **Sample mode**, SHIFT + DATA opens **SAMPLE OPTIONS**. DATA confirms
**HPSS**, splitting the current sample into harmonic and percussive outputs on
two empty pads, without modifying the source. **The flow reuses stock Chop
Extract's creation, free-pad selection and assignment mechanism.**

Worktree: `/Users/jakubkolcar/projects/customs/sample-converter-hpss`  
Branch: `feature/sample-options-hpss`. **Changes are uncommitted.**

## Current status

Implemented and validated in the emulator:

- `SHIFT+DATA` opens SAMPLE OPTIONS only on the visible Sample page. DATA
  confirms **HPSS**; `SHIFT+DATA` closes. DATA repeats and the underlying DATA
  action are suppressed while open.
- HPSS runs on a worker thread. The source is read exactly as the app has it
  (PCM16/24/32, float32/64, extensible WAVs; unsaved recordings through a unique
  `TempProjects` match). Source/project identity is checked again before commit.
- Two empty pads are preflighted with the native occupancy predicate. The
  operation stops with `HPSS: need 2 pads` if fewer than two are free.
- Two native Extract commands are submitted, harmonic then percussive. The
  execute hook supplies a full-sample extraction range (`0..frames`) and the
  writer callback substitutes the processed interleaved float32 audio. The
  native command keeps ownership, file bookkeeping and assignment.
- Outputs are named `HPSS Harmonic` and `HPSS Percussive`. The app appends a
  numeral if the name already exists.
- The live trim/chop range, the selected sample, and the Sample page's
  Chop/Full mode are restored after the native handler runs.
- `mod/src/hpss.c` implements the settled algorithm: 2048 FFT, 512 hop,
  symmetric Hann, reflect padding, time/frequency medians 41/17 at 44.1 kHz
  (scaled to odd widths at other rates), power 6, margin 0.55, complementary
  masks, no gate or normalisation.

**Validation evidence** is in
[`docs/re/hpss/runtime-validation.md`](docs/re/hpss/runtime-validation.md).
The host-side DSP regression test is `python3 mod/tests/hpss_dsp.py`.
Key results: native outputs match the Python prototype within 9e-7 (float32
rounding) for long mono, short mono, stereo and silence; reconstruction error
is about -140 dB; a half-amplitude substitution was bit-exact; the original
user-content hash is unchanged; both outputs are playable; pads persist across
an ac50 restart; ordinary Extract and AutoTrim still work.

## Known limits / remaining checks

- Real MPC Sample 1.3.0.12 hardware (only the emulator was used).
- Native undo grouping. The two outputs are two native commands; undo is
  whatever the firmware does for two extracts, and was not exercised.
- A genuinely full 128-pad project. The "fewer than two pads" branch was
  exercised with a test build that forced the preflight count.
- Second-write failure was exercised with a forced test build: the harmonic
  output is kept and the UI reports `HPSS: 1 of 2`. A destructive pad-clear
  rollback was tested and **removed**: clearing a pad outside the native
  command/history path made the app delete the file and report a missing sample
  on reload.
- Navigating away while the worker is running aborts on the next completion
  check. A long-running project change was not timed against a deliberately
  slowed DSP.
- The firmware's stock Extract writer emits **float32** WAVs, not the 16/24-bit
  depth assumed in older notes. The app's project save may rewrite a 24-bit
  working copy with ±2 LSB dither; the user-content source is untouched.
- Very large samples are bounded by `hpss_split`'s allocation checks; memory
  pressure on device was not measured.

## Native integration facts

All addresses here are **Ghidra VAs**; subtract `0x100000` for mod offsets.

| Function | Purpose |
|---|---|
| `0x17580d0` | Stock Sample/Chop Extract handler; argument is the Sample screen |
| `0x296dd00` | Native first-empty-pad finder, indices 0–127, `-1` if full |
| `0x153ed40` | `ExtractAndAssignSampleCommand` constructor |
| `0x153f0b0` | Command execution: creates/assigns the sample, manages file bookkeeping |
| `0x1e8aff0` | Writer callback: block frame count at `+0`, audio pointer at `+8` |
| `0x1e93570` | Concrete writer consumes contiguous interleaved float32 |
| `0x29047b0` | Native pad-occupancy predicate over the layer array |

The extraction settings object is at `screen+0x9ba0`. The execute method copies
the range from `+0x78` (start) and `+0xd0` (end, exclusive) into the slice
command; the mod patches those only for that copy. The constructor stores the
settings pointer, so patching the object around the handler call alone is not
enough.

## Build and emulator

```sh
cd mod
./build.sh          # release build/minimod.so
./pack.sh           # SD autorun package
```

Logging build:

```sh
zig cc -target aarch64-linux-gnu -shared -fPIC -O2 \
  -fno-omit-frame-pointer -mbranch-protection=standard \
  -Wall -Wextra -DMINIMOD_LOG=1 \
  -o build/minimod-hpss.so src/minimod.c src/hpss.c
```

Emulator: `../virtual-mpc-sample`, panel `http://127.0.0.1:8765`.

```sh
cd ../virtual-mpc-sample
python3 lab.py status
python3 lab.py preload /path/to/mod/build/minimod.so
python3 lab.py ssh 'tail -f /run/minimod.log'
```

`MINIMOD_LOG=1` writes `/run/minimod.log`; the release build has no logging.
The stopped-app project state is in `/data/TempProjects`; the emulator's
`lab.py preload` restarts ac50 but keeps that state.

## Research record

[`docs/HPSS-RE.md`](docs/HPSS-RE.md) is the chronological investigation.
Earlier sections describe the rejected provider/loader approach and the probe
milestones; the final shipped hooks are summarised there under "Shipped
integration". Raw decompilation is indexed in
[`docs/re/hpss/README.md`](docs/re/hpss/README.md).
