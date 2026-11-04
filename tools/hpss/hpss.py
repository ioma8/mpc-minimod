#!/usr/bin/env python3
"""HPSS — harmonic/percussive source separation, prototype.

Splits a loop into what rings (notes, chords, bass, vocals) and what hits
(drums, clicks, transients). No ML, no model: an STFT, two median filters and
a soft mask.

    harmonic  = what survives a median filter along TIME   (horizontal lines)
    percussive = what survives a median filter along FREQ  (vertical spikes)

Usage:
    hpss.py IN.wav OUTDIR [--start S] [--dur S] [--nfft 2048] [--hop 512]
            [--kt 31] [--kf 31] [--power 2] [--margin 2] [--normalize]

Writes OUTDIR/<name>-harmonic.wav and OUTDIR/<name>-percussive.wav, same sample
rate / channel count / bit depth as the input.
"""
import argparse
import os
import sys
import wave

import numpy as np


# --------------------------------------------------------------------- wav io
def read_wav(path):
    """RIFF walker: Akai WAVs carry extra chunks (e.g. `atem`) before `data`,
    which Python's `wave` module mis-handles."""
    blob = open(path, "rb").read()
    if blob[:4] != b"RIFF" or blob[8:12] != b"WAVE":
        raise SystemExit(f"{path}: not a RIFF/WAVE file")
    pos, fmt, data = 12, None, None
    while pos + 8 <= len(blob):
        cid = blob[pos:pos + 4]
        size = int.from_bytes(blob[pos + 4:pos + 8], "little")
        body = blob[pos + 8:pos + 8 + size]
        if cid == b"fmt ":
            fmt = body
        elif cid == b"data":
            data = body
            break
        pos += 8 + size + (size & 1)
    if fmt is None or data is None:
        raise SystemExit(f"{path}: missing fmt/data chunk")
    tag = int.from_bytes(fmt[0:2], "little")
    ch = int.from_bytes(fmt[2:4], "little")
    sr = int.from_bytes(fmt[4:8], "little")
    bits = int.from_bytes(fmt[14:16], "little")
    if tag == 0xFFFE and len(fmt) >= 26:          # WAVE_FORMAT_EXTENSIBLE
        tag = int.from_bytes(fmt[24:26], "little")
    if tag == 3 and bits == 32:
        audio = np.frombuffer(data, dtype="<f4").astype(np.float32)
    elif tag == 3 and bits == 64:
        audio = np.frombuffer(data, dtype="<f8").astype(np.float32)
    elif tag == 1 and bits == 16:
        audio = np.frombuffer(data, dtype="<i2").astype(np.float32) / 32768.0
    elif tag == 1 and bits == 24:
        b = np.frombuffer(data, dtype=np.uint8).reshape(-1, 3).astype(np.int32)
        v = b[:, 0] | (b[:, 1] << 8) | (b[:, 2] << 16)
        v = np.where(v & 0x800000, v - 0x1000000, v)
        audio = v.astype(np.float32) / 8388608.0
    elif tag == 1 and bits == 32:
        audio = np.frombuffer(data, dtype="<i4").astype(np.float32) / 2147483648.0
    else:
        raise SystemExit(f"{path}: unsupported format tag {tag} / {bits} bit")
    sw = 2 if bits <= 16 else 3
    return audio.reshape(-1, ch), sr, sw


def write_wav(path, data, sr, sw):
    n, ch = data.shape
    if sw == 2:
        ints = np.clip(np.round(data * 32767.0), -32768, 32767).astype("<i2")
        raw = ints.tobytes()
    elif sw == 3:
        ints = np.clip(np.round(data * 8388607.0), -8388608, 8388607).astype(np.int32)
        b = np.empty((ints.size, 3), dtype=np.uint8)
        u = ints.ravel().astype(np.uint32) & 0xFFFFFF
        b[:, 0] = u & 0xFF
        b[:, 1] = (u >> 8) & 0xFF
        b[:, 2] = (u >> 16) & 0xFF
        raw = b.tobytes()
    else:
        raise SystemExit("only 16/24-bit output is supported")
    with wave.open(path, "wb") as w:
        w.setnchannels(ch)
        w.setsampwidth(sw)
        w.setframerate(sr)
        w.writeframes(raw)


# ----------------------------------------------------------------------- stft
def stft(x, n_fft, hop):
    """x: (n, ch) float32 -> complex (bins, frames, ch)"""
    win = np.hanning(n_fft).astype(np.float32)
    pad = n_fft
    xp = np.pad(x, ((pad, pad), (0, 0)), mode="reflect")
    n_frames = 1 + (xp.shape[0] - n_fft) // hop
    idx = np.arange(n_fft)[None, :] + hop * np.arange(n_frames)[:, None]
    frames = xp[idx]                       # (frames, n_fft, ch)
    spec = np.fft.rfft(frames * win[:, None], axis=1)   # (frames, bins, ch)
    return np.transpose(spec, (1, 0, 2)), win, pad


def istft(spec, win, hop, pad, length):
    frames = np.transpose(spec, (1, 0, 2))              # (frames, bins, ch)
    time = np.fft.irfft(frames, n=win.size, axis=1)     # (frames, n_fft, ch)
    time *= win[:, None]
    n_frames, n_fft, ch = time.shape
    out = np.zeros((hop * (n_frames - 1) + n_fft, ch), dtype=np.float32)
    wsum = np.zeros_like(out)
    for i in range(n_frames):
        out[i * hop:i * hop + n_fft] += time[i]
        wsum[i * hop:i * hop + n_fft] += (win ** 2)[:, None]
    out /= np.maximum(wsum, 1e-9)
    return out[pad:pad + length]


# ------------------------------------------------------------- median filters
def median_axis(a, k, axis, chunk=64):
    """Sliding median of width k (odd) along `axis`, reflect-padded."""
    if k <= 1:
        return a.copy()
    r = k // 2
    pad = [(0, 0)] * a.ndim
    pad[axis] = (r, r)
    ap = np.pad(a, pad, mode="reflect")
    windows = np.lib.stride_tricks.sliding_window_view(ap, k, axis=axis)
    out = np.empty_like(a)
    step = chunk if axis == 0 else a.shape[axis]
    for start in range(0, a.shape[axis], step):
        stop = min(start + step, a.shape[axis])
        sl = [slice(None)] * a.ndim
        sl[axis] = slice(start, stop)
        out[tuple(sl)] = np.median(windows[tuple(sl)], axis=-1)
    return out


def onset_envelope(P, sr, hop):
    """Frame energy of the percussive estimate -> normalised flux -> gate.

    The gate is 1 at an onset and decays with `tau`; it is what keeps a drum's
    attack while removing the sustained melodic content that leaks into the
    percussive estimate."""
    energy = P.sum(axis=0)                      # (frames, ch)
    energy = energy.mean(axis=1) if energy.ndim > 1 else energy
    flux = np.maximum(0.0, np.diff(energy, prepend=energy[0]))
    if flux.max() > 0:
        flux = flux / flux.max()
    return flux


def make_gate(flux, decay_frames):
    """Running exponential decay from every flux peak (attack 1, then exp)."""
    gate = np.zeros_like(flux)
    level = 0.0
    for i, v in enumerate(flux):
        level = max(v, level * np.exp(-1.0 / max(decay_frames, 1e-6)))
        gate[i] = level
    return gate


def hpss(x, sr, n_fft=2048, hop=512, kt=41, kf=17, power=6.0, margin=0.55,
         gate_ms=0.0, gate_floor=0.0, verbose=True):
    spec, win, pad = stft(x, n_fft, hop)
    mag = np.abs(spec).astype(np.float32)
    H = median_axis(mag, kt, axis=1)   # along time  -> harmonic
    P = median_axis(mag, kf, axis=0)   # along freq  -> percussive
    Hp, Pp = H ** power, (margin * P) ** power
    mask_p = Pp / np.maximum(Hp + Pp, 1e-12)

    if gate_ms > 0:
        flux = onset_envelope(P, sr, hop)
        decay = gate_ms / 1000.0 * sr / hop
        gate = make_gate(flux, decay)
        gate = gate_floor + (1.0 - gate_floor) * gate
        mask_p = mask_p * gate[None, :, None]

    mask_h = 1.0 - mask_p                      # exact reconstruction by design
    if verbose:
        print(f"  stft {n_fft}/{hop}  frames={mag.shape[1]}  "
              f"mean percussive mask={float(np.mean(mask_p)):.3f}"
              + (f"  gate {gate_ms:.0f} ms floor {gate_floor:.2f}" if gate_ms else ""))
    xh = istft(spec * mask_h, win, hop, pad, x.shape[0])
    xp = istft(spec * mask_p, win, hop, pad, x.shape[0])
    return xh, xp


def rms_db(x):
    v = float(np.sqrt(np.mean(np.square(x, dtype=np.float64))))
    return 20.0 * np.log10(max(v, 1e-9))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("input")
    ap.add_argument("outdir")
    ap.add_argument("--start", type=float, default=0.0)
    ap.add_argument("--dur", type=float, default=0.0)
    ap.add_argument("--nfft", type=int, default=2048)
    ap.add_argument("--hop", type=int, default=512)
    ap.add_argument("--kt", type=int, default=41)
    ap.add_argument("--kf", type=int, default=17)
    ap.add_argument("--power", type=float, default=6.0)
    ap.add_argument("--gate", type=float, default=0.0,
                    help="transient gate on the percussive layer, decay in ms (e.g. 250)")
    ap.add_argument("--gate-floor", type=float, default=0.0,
                    help="gate floor 0..1 (0 removes everything between hits)")
    ap.add_argument("--tag", default="", help="suffix for the output files")
    ap.add_argument("--normalize", action="store_true",
                    help="peak-normalise each layer to -1 dBFS (for auditioning)")
    ap.add_argument("--margin", type=float, default=0.55)
    a = ap.parse_args()

    data, sr, sw = read_wav(a.input)
    if a.start or a.dur:
        i0 = int(a.start * sr)
        i1 = i0 + int(a.dur * sr) if a.dur else data.shape[0]
        data = data[i0:i1]
    if sw == 4:
        sw = 3   # write 24-bit

    print(f"{os.path.basename(a.input)}: {data.shape[0] / sr:.2f}s "
          f"{data.shape[1]}ch {sr}Hz  in RMS {rms_db(data):.1f} dBFS")
    h, p = hpss(data, sr, a.nfft, a.hop, a.kt, a.kf, a.power, a.margin,
                a.gate, a.gate_floor)
    rec = h + p
    err = rec - data
    print(f"  reconstruction error: {rms_db(err) - rms_db(data):.1f} dB below input")
    print(f"  harmonic {rms_db(h):.1f} dBFS   percussive {rms_db(p):.1f} dBFS")

    if a.normalize:
        for buf in (h, p):
            peak = float(np.max(np.abs(buf)))
            if peak > 0:
                buf *= 0.891 / peak

    os.makedirs(a.outdir, exist_ok=True)
    stem = os.path.splitext(os.path.basename(a.input))[0].replace(" ", "_") + a.tag
    for tag, buf in (("harmonic", h), ("percussive", p)):
        path = os.path.join(a.outdir, f"{stem}-{tag}.wav")
        write_wav(path, buf, sr, sw)
        print("  wrote", path)


if __name__ == "__main__":
    sys.exit(main())
