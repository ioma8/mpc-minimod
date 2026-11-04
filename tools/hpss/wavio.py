"""Small stdlib-only WAV reader/writer for the HPSS helper scripts."""
import struct


def read_wav(path):
    """Return (interleaved float samples, sample_rate, channels)."""
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
        values = struct.unpack("<%df" % (len(data) // 4), data)
    elif tag == 3 and bits == 64:
        values = struct.unpack("<%dd" % (len(data) // 8), data)
    elif tag == 1 and bits == 16:
        values = [v / 32768.0 for v in struct.unpack("<%dh" % (len(data) // 2), data)]
    elif tag == 1 and bits == 24:
        values = [
            int.from_bytes(data[i:i + 3], "little", signed=True) / 8388608.0
            for i in range(0, len(data), 3)
        ]
    elif tag == 1 and bits == 32:
        values = [v / 2147483648.0 for v in struct.unpack("<%di" % (len(data) // 4), data)]
    else:
        raise SystemExit(f"{path}: unsupported format tag {tag} / {bits} bit")
    return list(values), sr, ch


def write_wav24(path, samples, sr, ch):
    """Write interleaved float samples as 24-bit PCM."""
    raw = bytearray()
    for v in samples:
        n = max(-8388608, min(8388607, int(round(v * 8388607.0))))
        raw += (n & 0xFFFFFF).to_bytes(3, "little")
    data = bytes(raw)
    fmt = struct.pack("<HHIIHH", 1, ch, sr, sr * ch * 3, ch * 3, 24)
    chunks = (
        b"fmt " + struct.pack("<I", len(fmt)) + fmt
        + b"data" + struct.pack("<I", len(data)) + data
    )
    with open(path, "wb") as f:
        f.write(b"RIFF" + struct.pack("<I", 4 + len(chunks)) + b"WAVE" + chunks)


def peak(samples):
    return max((abs(v) for v in samples), default=0.0)


def normalize(samples, target=0.891):
    p = peak(samples)
    if p <= 0:
        return list(samples)
    scale = target / p
    return [v * scale for v in samples]


def fade_edges(samples, ch, frames, fade_frames):
    out = list(samples)
    if fade_frames <= 0 or frames <= 1:
        return out
    fade_frames = min(fade_frames, frames // 2)
    for i in range(fade_frames):
        g = i / fade_frames
        for c in range(ch):
            out[i * ch + c] *= g
            out[(frames - 1 - i) * ch + c] *= g
    return out
