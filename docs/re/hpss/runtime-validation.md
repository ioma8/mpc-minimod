# HPSS runtime validation (emulator)

Date: 2026-11-04. Firmware 1.3.0.12. Emulator `virtual-mpc-sample`, ac50
`NRestarts=0` throughout. The release build is `mod/build/minimod.so`; the
logging build is `mod/build/minimod-hpss.so` with `MINIMOD_LOG=1`.

## Native bridge milestones

1. **Unchanged extraction.** The stock `SHIFT+B1` Chop Extract still creates a
   normal sample; the constructor/execute/writer hooks pass it through when no
   HPSS output is armed.
2. **Scoped audio substitution.** A probe wrote `source * 0.5` through the stock
   Extract writer. The resulting 24-bit source and 32-bit-float output matched
   `source * 0.5` with **max error 0** over 63 906 frames.
3. **Full-sample override.** The screen's extraction range is copied from
   `screen+0x9ba0` by the command's execute method. The execute hook writes
   `0 .. frames` for exactly that copy and restores the live values afterwards.
   With a live chop/trim range of `0..5083` or `0..7204`, HPSS still wrote the
   whole 63 906-frame sample.

## Audio parity against `tools/hpss/hpss.py`

The settled constants were used: 2048/512, Hann, kt 41, kf 17, power 6, margin
0.55, no gate, no normalisation. Native outputs are interleaved float32;
Python output was decoded from 24-bit and compared sample by sample.

| case | frames x channels | harmonic max error | percussive max error | `h+p` vs source |
|---|---|---|---|---|
| long mono, 44.1 kHz | 63 906 x 1 | 7.2e-8 | 8.9e-8 | 4.0e-7 |
| short mono (`chop130`), 44.1 kHz | 5 083 x 1 | 8.9e-8 | 7.8e-8 | 4.7e-8 |
| stereo, 44.1 kHz | 44 100 x 2 | 8.9e-7 | 7.2e-7 | 7.4e-7 |
| digital silence | 4 410 x 1 | max abs 0 | max abs 0 | exact zero |

Python reported reconstruction error between **-138.4 dB and -141.0 dB**
relative to the input. The native `h+p` errors above are float32 rounding, not
missing energy.

## Household / project checks

- Both outputs are playable: the pads show `HPSS Harmonic` and
  `HPSS Percussive` and select/play normally.
- The original object is untouched: `/data/UserContent/Samples/User/Sample
  002.wav` kept SHA-256
  `2028b9087ab1892a9b7116ec89d471065424b6953183a8e39636cc921f408124`.
  The app rewrites its **project working copy** on save with 24-bit dither; the
  largest observed change is 2 LSB (about -110 dBFS), spread as +2/-2/0.
- The live Sample-page mode is restored: a run started in Full mode left Chop
  mode off; if the native handler flips it, the mod replays the CHOP button
  through the app's own dispatcher.
- Live trim/chop values survive: a trim of `0..7204` was still in the settings
  after HPSS, while the extraction used `0..63906`.
- Pads persisted across an ac50 restart (`systemctl restart ac50`); no missing
  sample dialog appeared.
- Ordinary `SHIFT+B1` Extract created `Chop 1 - Sample 001.wav` unchanged.
  Ordinary `SHIFT+B3` AutoTrim still wrote bounds `0..7204`.
- Insufficient pads: exercised with a test build that forced the preflight count
  to one; the UI reported `HPSS: need 2 pads` and wrote nothing.
- Second-output failure: exercised with a test build that forced the second
  extraction to fail; the UI reported `HPSS: 1 of 2` and kept the harmonic
  output.
- A destructive pad-clear rollback was tested and **removed**: clearing the
  first pad outside the native command/history path made the app delete the file
  and then report a missing sample on reload. The shipped path never clears
  pads and never deletes files.

## Output naming and depth

- Names are `HPSS Harmonic` and `HPSS Percussive`; the app appends `1`, `2`, ...
  when a name already exists in the project.
- On this firmware the stock Extract writer emits **float32** WAVs even for a
  24-bit source. The docs' earlier "16 or 24-bit" expectation was wrong for this
  build.

## Not covered on hardware

Real MPC Sample 1.x hardware, native undo grouping, a genuinely full 128-pad
project, large-sample memory pressure, and navigating away while the worker is
running. These are listed in [HPSS-HANDOFF.md](../../../HPSS-HANDOFF.md).
