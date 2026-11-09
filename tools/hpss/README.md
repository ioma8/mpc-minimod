# HPSS prototype — harmonic / percussive split

Prototype for a possible on-device transform: split one sample into what **rings**
(chords, bass, vocals) and what **hits** (drums, clicks, transients), as two new
samples.

No ML, no model — an STFT, two median filters and a soft mask:

```
harmonic   = what survives a median filter along TIME  (horizontal spectrogram lines)
percussive = what survives a median filter along FREQ  (vertical spikes)
mask_h     = H^p / (H^p + (margin·P)^p)      mask_p = 1 - mask_h
```

`harmonic + percussive` reconstructs the input to about **−140 dB** error, so the
split is energy-preserving — nothing is invented, only routed.

## Run

```sh
python3 tools/hpss/hpss.py IN.wav OUTDIR [--start S] [--dur S] \
        [--nfft 2048] [--hop 512] [--kt 41] [--kf 17] [--power 6] [--margin 0.55] \
        [--gate MS] [--gate-floor F] [--tag S] [--normalize]
```

Writes `OUTDIR/<name>-harmonic.wav` and `OUTDIR/<name>-percussive.wav`, keeping the
input's sample rate, channel count and bit depth (16/24-bit out).

`--normalize` peak-normalises each layer to −1 dBFS so the quieter one is audible
on its own; leave it off to hear the true levels.

The reader is a RIFF chunk walker, not `wave`: Akai WAVs put an `atem` chunk
before `data`, which `wave` mis-parses (the guitar demo failed until this was fixed).

## Demos (in `out/`)

| source | excerpt | harmonic | percussive | note |
|---|---|---|---|---|
| `Project1` mixdown (`Song1.wav`) | 8.0 s from 8 s | −6.5 dBFS | −14.6 dBFS | dense full mix |
| `18-chop-16-seq-a04-1` (resampled seq) | 8.0 s from 4 s | −21.1 dBFS | −22.3 dBFS | drum-forward |
| `GuitPh-Grove1 C#m85 91V` (guitar loop) | 2.8 s | −15.9 dBFS | −33.1 dBFS | tonal: the split stays in the harmonic layer, 17 dB down |

Objective check on the purely tonal guitar loop: the percussive layer sits
**25 dB** below the harmonic one, i.e. a sample with nothing to hit yields
nothing on the drum side.

## The melody-in-the-percussive-layer problem (fixed)

First listen: the harmonic layer was clean, but the percussive layer still carried
chords. Cause: a note with harmonics is not one spectral peak — at 2048/44.1k one
bin is 21.5 Hz, so a 100 Hz note fills ~6 bins inside a 17-bin frequency median.
`P` (the frequency-median estimate) therefore stays high for tonal content and
the percussive mask lets it through.

Two fixes, both measured. The metric is *percussive energy sitting in the 40 %
of frames where the input has the least spectral flux* — i.e. how much of the
drum layer is really melody. Lower is cleaner.

| setting | percussive in quiet frames |
|---|---|
| old: `kt 31 kf 31 power 2 margin 2` | −9.6 dB |
| `kt 41 kf 17 power 4 margin 0.7` | −11.7 dB |
| **`kt 41 kf 17 power 6 margin 0.55` (shipped default)** | **−13.1 dB** |
| the same + transient gate 200 ms, floor 0.05 | −13.4 dB (tail artefacts) |

* `margin < 1` biases the mask toward the harmonic layer; the first default (2.0)
  did the opposite and pushed chords *into* the percussive layer.
* `power` sharpens the mask; 6 was chosen by ear over 4 and 9.
* The **gate** is a decaying envelope started at every onset of the percussive
  estimate: it keeps attacks and removes the sustained content between hits. It
  measured cleaner still, but shortened the drum tails audibly.

**Verdict (listened, 2026-11-04):**

1. masks alone beat the gate — the gate shortens drum tails audibly, so it is off
   by default and kept only as a flag;
2. the **stronger mask won again** — `power 6 margin 0.55` is the default, and the
   transients survived it.

A `-extreme` variant (`--power 9 --margin 0.45`) is exported for A/B; beyond this
the mask starts to act like a hard gate and thins the hits.

| setting | percussive in quiet frames |
|---|---|
| `power 4 margin 0.7` | −11.7 dB |
| `power 6 margin 0.55` (default) | −13.1 dB |
| `power 9 margin 0.45` (extreme) | −14.1 dB |
| `power 2 margin 1` + 200 ms gate | −13.4 dB, but tail artefacts |

## Limits

* 2048-point FFT smears fast cymbals — some "underwater" around hats and rolls.
* Kick and bass overlap by definition: a little low end always crosses over.
* Cheap artefacts when the loop is very dry or has strong reverb (the tail is
  horizontal, so it follows the harmonic layer).
* Offline only: 0.7 s of Python for 8 s of stereo. A C version on device would be
  similar and would run on stop.

## Status

Reference prototype, not the device code. The shipped transform is the C port in
[`rust/crates/dsp/src/hpss.rs`](../../rust/crates/dsp/src/hpss.rs); this Python version was used to settle
the constants and to check the native output sample by sample. The device flow
that writes the two results to empty pads is `jobflow.rs`/`hpss_exec.rs` in
`rust/crates/minimod/src` (SAMPLE OPTIONS → HPSS).

Helper scripts in this directory:

| script | role |
|---|---|
| `hpss.py` | reference implementation and demo renderer |
| `wavio.py` | stdlib WAV reader / 24-bit writer used by the helpers |
| `wavdiff.py` | compare two WAVs (parity/quantisation checks) |
| `excerpt.py` | cut, fade and normalise a WAV excerpt |
| `mix.py` | sum several aligned WAVs into one 24-bit mix |
| `onsets.py` | reference onset detector for Spectral Chop |
| `spectral_chop.py` | the simple onset detector being tuned for Spectral Chop: normalised flux, local median threshold, attack snapping, slice report and a PNG |

`hpss.py` needs numpy; the three helper scripts use only the standard library.
