"""Minimal RIFF/WAVE reader that preserves the exact stored sample values.

We deliberately avoid a general audio library for reading: the validation goal is
bit-exact reconstruction, so the integer<->float mapping has to be explicit.

Supported encodings (everything the MPC writes for its own samples):
  * WAVE_FORMAT_PCM with 16/24/32 bit little endian integers
  * WAVE_FORMAT_IEEE_FLOAT with 32/64 bit floats
"""

from __future__ import annotations

import struct
from dataclasses import dataclass
from pathlib import Path

import numpy as np

WAVE_FORMAT_PCM = 0x0001
WAVE_FORMAT_IEEE_FLOAT = 0x0003
WAVE_FORMAT_EXTENSIBLE = 0xFFFE

#: 24 bit integer -> float divisor used by libsndfile / the MPC.
PCM24_SCALE = 8388608.0
PCM16_SCALE = 32768.0
PCM32_SCALE = 2147483648.0


@dataclass(frozen=True)
class WavData:
    sample_rate: int
    #: always shape (frames, channels), float64 in [-1, 1)
    samples: np.ndarray
    format_tag: int
    bits_per_sample: int
    #: remaining chunks, keyed by fourcc (first occurrence wins)
    chunks: dict[str, bytes]

    @property
    def channels(self) -> int:
        return self.samples.shape[1]

    @property
    def frames(self) -> int:
        return self.samples.shape[0]

    def mono(self) -> np.ndarray:
        return self.samples.mean(axis=1)


def _chunks(blob: bytes) -> list[tuple[str, bytes]]:
    if blob[:4] != b"RIFF" or blob[8:12] != b"WAVE":
        raise ValueError("not a RIFF/WAVE file")
    out: list[tuple[str, bytes]] = []
    pos = 12
    n = len(blob)
    while pos + 8 <= n:
        fourcc = blob[pos : pos + 4].decode("latin-1")
        (size,) = struct.unpack("<I", blob[pos + 4 : pos + 8])
        start = pos + 8
        out.append((fourcc, blob[start : start + size]))
        pos = start + size + (size & 1)
    return out


def read_wav(path: str | Path) -> WavData:
    blob = Path(path).read_bytes()
    chunks = _chunks(blob)
    fmt = None
    data = b""
    extras: dict[str, bytes] = {}
    for fourcc, body in chunks:
        if fourcc == "fmt ":
            fmt = body
        elif fourcc == "data":
            data = body
        else:
            extras.setdefault(fourcc, body)

    if fmt is None:
        raise ValueError(f"{path}: missing fmt chunk")
    if len(fmt) < 16:
        raise ValueError(f"{path}: short fmt chunk")

    tag, channels, rate, _byte_rate, block_align, bits = struct.unpack("<HHIIHH", fmt[:16])
    if tag == WAVE_FORMAT_EXTENSIBLE and len(fmt) >= 26:
        # cbSize(2) validBits(2) channelMask(4) subFormat GUID(16, first 2 bytes = tag)
        (sub_tag,) = struct.unpack("<H", fmt[24:26])
        tag = sub_tag

    width = block_align // channels
    frames = len(data) // block_align

    if tag == WAVE_FORMAT_PCM:
        if bits == 16:
            raw = np.frombuffer(data[: frames * block_align], dtype="<i2")
            samples = raw.astype(np.float64) / PCM16_SCALE
        elif bits == 24:
            raw = np.frombuffer(data[: frames * block_align], dtype=np.uint8).reshape(-1, 3)
            ints = (
                raw[:, 0].astype(np.int32)
                | (raw[:, 1].astype(np.int32) << 8)
                | (raw[:, 2].astype(np.int32) << 16)
            )
            ints = np.where(ints >= 0x800000, ints - 0x1000000, ints)
            samples = ints.astype(np.float64) / PCM24_SCALE
        elif bits == 32:
            raw = np.frombuffer(data[: frames * block_align], dtype="<i4")
            samples = raw.astype(np.float64) / PCM32_SCALE
        else:
            raise ValueError(f"{path}: unsupported PCM bit depth {bits}")
    elif tag == WAVE_FORMAT_IEEE_FLOAT:
        dtype = "<f4" if bits == 32 else "<f8"
        samples = np.frombuffer(data[: frames * block_align], dtype=dtype).astype(np.float64)
    else:
        raise ValueError(f"{path}: unsupported WAVE format tag 0x{tag:04x}")

    samples = samples.reshape(frames, channels)
    return WavData(
        sample_rate=rate,
        samples=samples,
        format_tag=tag,
        bits_per_sample=bits,
        chunks=extras,
    )


def write_pcm24(path: str | Path, samples: np.ndarray, sample_rate: int) -> None:
    """Write 24 bit PCM. ``samples`` is (frames, channels) float in [-1, 1]."""
    samples = np.asarray(samples, dtype=np.float64)
    if samples.ndim == 1:
        samples = samples[:, None]
    frames, channels = samples.shape
    ints = np.clip(np.rint(samples * PCM24_SCALE), -PCM24_SCALE, PCM24_SCALE - 1).astype("<i4")
    packed = np.empty((frames, channels * 3), dtype=np.uint8)
    for c in range(channels):
        packed[:, c * 3 + 0] = (ints[:, c] & 0xFF).astype(np.uint8)
        packed[:, c * 3 + 1] = ((ints[:, c] >> 8) & 0xFF).astype(np.uint8)
        packed[:, c * 3 + 2] = ((ints[:, c] >> 16) & 0xFF).astype(np.uint8)
    body = packed.tobytes()

    block_align = channels * 3
    byte_rate = sample_rate * block_align
    fmt = struct.pack("<HHIIHH", WAVE_FORMAT_PCM, channels, sample_rate, byte_rate, block_align, 24)
    riff_size = 4 + (8 + len(fmt)) + (8 + len(body))
    with open(path, "wb") as fh:
        fh.write(b"RIFF")
        fh.write(struct.pack("<I", riff_size))
        fh.write(b"WAVE")
        fh.write(b"fmt ")
        fh.write(struct.pack("<I", len(fmt)))
        fh.write(fmt)
        fh.write(b"data")
        fh.write(struct.pack("<I", len(body)))
        fh.write(body)


def read_pcm24_ints(path: str | Path) -> np.ndarray:
    """Read a 24 bit PCM WAV as exact ``int32`` sample values (frames, channels).

    Needed for bit-exact stem arithmetic: two 24 bit values must be added as
    integers, never through float64, or rounding breaks the equality.
    """
    blob = Path(path).read_bytes()
    chunks = _chunks(blob)
    fmt = None
    data = b""
    for fourcc, body in chunks:
        if fourcc == "fmt ":
            fmt = body
        elif fourcc == "data":
            data = body
    if fmt is None:
        raise ValueError(f"{path}: missing fmt chunk")
    tag, channels, _rate, _byte_rate, block_align, bits = struct.unpack("<HHIIHH", fmt[:16])
    if tag == WAVE_FORMAT_EXTENSIBLE and len(fmt) >= 26:
        (tag,) = struct.unpack("<H", fmt[24:26])
    if tag != WAVE_FORMAT_PCM or bits != 24:
        raise ValueError(f"{path}: not 24 bit PCM (tag={tag}, bits={bits})")
    frames = len(data) // block_align
    raw = np.frombuffer(data[: frames * block_align], dtype=np.uint8).reshape(-1, 3)
    ints = (
        raw[:, 0].astype(np.int32)
        | (raw[:, 1].astype(np.int32) << 8)
        | (raw[:, 2].astype(np.int32) << 16)
    )
    ints = np.where(ints >= 0x800000, ints - 0x1000000, ints)
    return ints.reshape(frames, channels).astype(np.int32)


def read_slices(wav: "WavData") -> list[tuple[int, int]]:
    """Slice boundaries from the RIFF ``slic`` chunk.

    The chunk is ``u16 version, u16 count`` followed by slice points.  The
    payload interleaves 32-bit sample offsets with small bookkeeping values, so
    we keep the offsets that are in range and strictly increasing.  A value of
    a couple of samples is bookkeeping, not a slice boundary.
    """
    raw = wav.chunks.get("slic")
    if not raw or len(raw) < 8:
        return []
    import struct

    body = raw[4:]
    count = (len(body) // 4) * 4
    values = struct.unpack(f"<{count // 4}I", body[:count])
    bounds: list[int] = []
    for v in values:
        if v > wav.frames:
            continue
        if v != 0 and v < 64:
            continue
        if not bounds or v > bounds[-1]:
            bounds.append(v)
    if len(bounds) < 2:
        return []
    # the last slice point is usually frames-1; only extend on a real gap
    if bounds[-1] < wav.frames - 64:
        bounds.append(wav.frames)
    return [(bounds[i], bounds[i + 1]) for i in range(len(bounds) - 1)]
