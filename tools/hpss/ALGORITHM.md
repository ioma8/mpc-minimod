# HPSS — the exact algorithm and the constants we settled on

Reference implementation: [`hpss.py`](hpss.py). This file is the specification
for the Rust port that ships in the mod (`rust/crates/dsp/src/hpss.rs`, pinned to
the original C by golden fixtures); the parity results are
in [`../../docs/FEATURES.md`](../../docs/FEATURES.md#hpss).

Verdict date: **2026-11-04**, tuned by ear on `Project1`'s mixdown and checked
against a drum-forward resampled loop and a purely tonal guitar loop.

---

## 1. What it computes

Two signals from one:

```
x  ──▶  STFT  ──▶  |X|  ──┬─▶ median over TIME   ──▶ H   (harmonic estimate)
                           └─▶ median over FREQ   ──▶ P   (percussive estimate)

mask_p = P^power / (H^power + (margin·P)^power)
mask_h = 1 - mask_p
xh = iSTFT(X · mask_h)        # what rings: chords, bass, vocals
xp = iSTFT(X · mask_p)        # what hits: drums, clicks, transients
```

Because `mask_h = 1 − mask_p`, `xh + xp = x` **exactly** (up to FFT round-off;
measured −140 dB). The split moves energy between the two outputs, it never
creates or loses any. That property is a design constraint, not a side effect:
it means a bad mask can only make a layer sound wrong, never make the pair
sum to something that isn't the input.

---

## 2. Constants

| constant | value | meaning |
|---|---|---|
| `n_fft` | **2048** | FFT size, 46.4 ms at 44.1 kHz |
| `hop` | **512** | 75 % overlap, 11.61 ms per frame |
| window | **Hann**, symmetric | `w[n] = 0.5 − 0.5·cos(2πn/(N−1))`, `n = 0…N−1` |
| zero pad | **`n_fft` samples each side, reflect** | keeps every input sample covered by a full window |
| `kt` | **41 frames** | median along time ≈ **476 ms** |
| `kf` | **17 bins** | median along frequency ≈ **366 Hz** |
| `power` | **6** | mask sharpness |
| `margin` | **0.55** | >1 pushes content *into* percussive, <1 into harmonic — we want <1 |
| gate | **off** (`gate_ms = 0`) | rejected by ear, see §7 |
| normalise for audition | **0.891** peak (−1 dBFS) | listening only, never for the real split |

Derived, at 44.1 kHz: bin width `44100/2048 = 21.53 Hz`, frame period
`512/44100 = 11.61 ms`.

**Sample-rate portability.** `kt` and `kf` are counts, not durations, so scale
them if the device ever runs at another rate:

```
kt = odd(round(0.476 · sr / hop))      # 41 at 44.1 k, 45 at 48 k
kf = odd(round(366.0 · n_fft / sr))    # 17 at 44.1 k, 17 at 48 k (23.4 Hz bins -> 15.6)
```

Either a slightly longer time window or a slightly narrower frequency window is
harmless; what matters is that both stay "long in time" / "narrow in frequency"
relative to the material.

---

## 3. Step by step

### 3.1 Read

RIFF chunk walker, not `wave`: Akai WAVs put an `atem` chunk before `data`, which
Python's `wave` mis-parses (it read 526 bytes short on the guitar loop). Accept
PCM 16/24/32-bit and float 32/64-bit, honour `WAVE_FORMAT_EXTENSIBLE`
(sub-format at `fmt+24`). Audio is float32 in `−1 … +1`, shape `(n, channels)`.

### 3.2 STFT

```
w        = hann(2048)                       # symmetric
x_pad    = reflect_pad(x, 2048, 2048)       # per channel
n_frames = 1 + (len(x_pad) − 2048) // 512
frame_i  = x_pad[i·512 : i·512 + 2048] · w
X_i      = rfft(frame_i, 2048)              # 1025 bins, complex64
```

Result: `X[bins][frames][channels]`, 1025 bins.

Reflect padding (not zero padding) matters: a zero pad would introduce a fake
transient at both ends, and the percussive median would happily believe it.

### 3.3 The two estimates

```
H[b, f, c] = median( |X[b, f−20 … f+20, c] | )     # 41 frames along time
P[b, f, c] = median( |X[b−8 … b+8, f, c] | )       # 17 bins along frequency
```

Both use **reflect padding** at the edges and both work on the **magnitude**,
not the power. Squares are applied later, in the mask.

Why these survive:
* a sustained note is a horizontal ridge → the time median keeps it, the
  frequency median (17 bins = 366 Hz) sees one peak among neighbours and drops it;
* a drum hit is a vertical spike → the frequency median keeps it, the time median
  (476 ms) sees mostly silence and drops it.

The 476 ms time window is the important one. At 31 frames (360 ms) chords were
still leaking into `P`; at 101 frames nothing improved further but note
transitions started to smear. 41 is the sweet spot.

### 3.4 The mask

```
Hp     = H^6
Pp     = (0.55 · P)^6
mask_p = Pp / max(Hp + Pp, 1e−12)
mask_h = 1 − mask_p
```

`margin = 0.55` multiplies the percussive estimate **before** the exponent, so a
value below 1 biases every contested bin toward the harmonic layer. The first
version had `margin = 2`, which did the opposite and pushed chords into the
percussive layer — that was the "I hear melody in the percussive layer" bug.

`power = 6` makes the mask close to a soft binary decision: a bin either belongs
to the harmonic ridge or to the transient, with little in between. Power 4 left
about 1 dB more melody; power 9 was audibly thinning the hits.

The `1e−12` guard makes digital silence split evenly-ish instead of NaNs.

### 3.5 Inverse

```
frame_i = irfft(X_i · mask, 2048) · w      # window applied again
out    += frame_i                          # overlap-add at i·512
wsum   += w²                               # same offsets
out    /= max(wsum, 1e−9)                  # normalise by the window energy
```

Dividing by `Σw²` (rather than assuming constant overlap-add) is what makes the
reconstruction exact even though Hann at 75 % overlap is only *approximately*
COLA. Trim the `n_fft` samples of padding from each end.

### 3.6 Write

Same sample rate, channel count and bit depth as the input (16 or 24-bit out).
`--normalize` peak-scales each layer to **0.891** (−1 dBFS) purely so the quiet
layer is audible when auditioned; it breaks the `xh + xp = x` property, so it is
never used for a real split.

---

## 4. Channels

Everything is per channel: `H`, `P` and the masks are computed independently for
left and right. That preserves the stereo image of each layer but can slightly
move a centred image, because the mask differs between channels at any given
bin. A shared mask (averaging the two channels before the mask) would fix that at
the cost of a little separation; it was not needed for the listening tests and is
not implemented.

---

## 5. Cost

Measured on this Mac, Python + numpy, 8 s stereo at 44.1 kHz:

| stage | wall time |
|---|---|
| STFT + both medians + 2 inverse STFTs | **0.69 s** |
| frames | 694 |
| memory per median (chunked at 64 bins) | ~7 MB |

The sliding median is the only part that needs care in C: `kt = 41` is small
enough for a ring buffer per bin plus an insertion sort (O(kt) per frame, ~41
comparisons), which is how a device port should do it. No streaming is needed —
the split runs once, offline, on a finished sample.

---

## 6. Validation

The tuning metric is deliberately simple and needs no onset detector:

> *percussive energy sitting in the 40 % of frames where the input has the least
> spectral flux* — i.e. how much of the drum layer is really melody. Lower is
> cleaner. Measured on the first 8 s of `Project1`'s mixdown from 8 s.

| check | result |
|---|---|
| reconstruction `xh + xp − x` | −140 dB below input (numerical zero) |
| percussive in quiet frames, final constants (`p6 m0.55`) | **−13.1 dB** |
| the same with `p4 m0.7` | −11.7 dB |
| the same with `p9 m0.45` | −14.1 dB |
| purely tonal guitar loop: percussive level | **25 dB** below harmonic — nothing to hit is correctly reported as nothing |
| drum-forward resampled loop, levels | harmonic −18.7 dBFS, percussive −24.6 dBFS |

---

## 7. What we rejected, and why

| variant | measured | verdict |
|---|---|---|
| `margin 2`, `power 2`, `kt 31`, `kf 31` | −9.6 dB melody leak | the original bug — margin biased **toward** percussive |
| `power 4`, `margin 0.7` | −11.7 dB | good, but `power 6` was better on the same material |
| `power 6`, `margin 0.55` | **−13.1 dB** | **chosen** |
| `power 9`, `margin 0.45` | −14.1 dB | cleaner on paper, hits too thin by ear |
| transient gate, 200 ms decay, floor 0.05 | −13.4 dB | rejected: audible tail truncation, "choppy" |
| cascade (HPSS applied to the percussive layer, 2–3 passes, measured on `p2 m1`) | −10.2 dB after 3 passes | rejected: less separation than the strong mask, triples the cost |

The gate and the cascade remain in the code as flags (`--gate`, and cascading by
calling `hpss()` again) precisely so these comparisons stay reproducible.

---

## 8. Port notes for the device

If this becomes a native transform:

1. **FFT**: a radix-2 real FFT of 2048 is enough; no dependency needed.
2. **Medians**: ring buffer + insertion sort, `kt = 41`, `kf = 17`.
3. **float32 everywhere**; the magnitudes fit comfortably, masks are in `0…1`.
4. **Memory**: 1025 × frames × channels × 8 B for the complex spectrum; process
   one channel at a time to halve the peak.
5. **Not real-time**: a 5 s stereo sample is ~0.4 s of work on the device CPU at
   a pessimistic 10× the desktop speed — it belongs behind a "process" action,
   not in the audio callback.
6. It produces **two new samples**; the blocker is still the write path that adds
   them to the pool and assigns them to pads.

---

## 9. CLI reference

```sh
python3 tools/hpss/hpss.py IN.wav OUTDIR [--start S] [--dur S] \
        [--nfft 2048] [--hop 512] [--kt 41] [--kf 17] \
        [--power 6] [--margin 0.55] [--gate MS] [--gate-floor F] \
        [--tag SUFFIX] [--normalize]
```

Defaults are the constants in §2. `--tag` suffixes the output names so variants
can coexist in one directory.

Function map in `hpss.py`:

| function | role |
|---|---|
| `read_wav` / `write_wav` | RIFF walker, 16/24-bit writer |
| `stft` / `istft` | §3.2 and §3.5 |
| `median_axis` | §3.3, reflect-padded sliding median, chunked |
| `onset_envelope` / `make_gate` | the rejected gate, kept for reproducibility |
| `hpss` | the whole pipeline, returns `(harmonic, percussive)` |
