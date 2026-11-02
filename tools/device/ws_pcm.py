"""Record the lab's PCM WebSocket (raw S16LE stereo 44.1 kHz) to a WAV.

The virtual lab's audio bridge pushes everything the app plays into the
`mpclab.pcm` virtio-serial port, which QEMU exposes as a WebSocket.  There is no
audio device in QEMU itself, so this socket is the only place the app's output
exists on the host -- and it is exactly the signal the device would send to its
jacks, i.e. the same bus the Song export captures.

No third-party client is used: a client handshake plus frame decoding is about
fifty lines, and the frames are unmasked server-to-client text/binary.
"""

from __future__ import annotations

import base64
import hashlib
import os
import socket
import struct
import sys
import wave
from pathlib import Path

MAGIC = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"


def connect(host: str, port: int, path: str = "/", timeout: float = 5.0) -> socket.socket:
    sock = socket.create_connection((host, port), timeout=timeout)
    key = base64.b64encode(os.urandom(16)).decode()
    request = (
        f"GET {path} HTTP/1.1\r\n"
        f"Host: {host}:{port}\r\n"
        "Upgrade: websocket\r\nConnection: Upgrade\r\n"
        f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
    )
    sock.sendall(request.encode())
    buf = b""
    while b"\r\n\r\n" not in buf:
        chunk = sock.recv(4096)
        if not chunk:
            raise RuntimeError("websocket handshake closed")
        buf += chunk
    head, _, rest = buf.partition(b"\r\n\r\n")
    if b"101" not in head.split(b"\r\n")[0]:
        raise RuntimeError(f"handshake failed: {head[:120]!r}")
    expected = base64.b64encode(hashlib.sha1((key + MAGIC).encode()).digest())
    if expected.lower() not in head.lower():
        raise RuntimeError("handshake accept mismatch")
    sock.setblocking(False)
    return sock, rest


def frames(sock, rest: bytes, seconds: float, sample_rate: int = 44100):
    """Yield raw PCM payloads for ``seconds``, skipping non-binary frames."""
    import time

    buf = rest
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        while len(buf) < 2:
            try:
                buf += sock.recv(65536)
            except BlockingIOError:
                time.sleep(0.002)
                break
        if len(buf) < 2:
            continue
        b0, b1 = buf[0], buf[1]
        opcode = b0 & 0x0F
        length = b1 & 0x7F
        offset = 2
        if length == 126:
            while len(buf) < 4:
                buf += sock.recv(65536)
            (length,) = struct.unpack(">H", buf[2:4])
            offset = 4
        elif length == 127:
            while len(buf) < 10:
                buf += sock.recv(65536)
            (length,) = struct.unpack(">Q", buf[2:10])
            offset = 10
        while len(buf) < offset + length:
            chunk = sock.recv(65536)
            if not chunk:
                return
            buf += chunk
        payload, buf = buf[offset : offset + length], buf[offset + length :]
        if opcode == 8:
            return
        if opcode == 2:
            yield payload


def record(out: Path, seconds: float, port: int = 5657, host: str = "127.0.0.1") -> Path:
    sock, rest = connect(host, port)
    chunks = list(frames(sock, rest, seconds))
    sock.close()
    data = b"".join(chunks)
    out.parent.mkdir(parents=True, exist_ok=True)
    with wave.open(str(out), "wb") as wav:
        wav.setnchannels(2)
        wav.setsampwidth(2)
        wav.setframerate(44100)
        wav.writeframes(data)
    return out


def main(argv: list[str]) -> int:
    out = Path(argv[1]) if len(argv) > 1 else Path("capture.wav")
    seconds = float(argv[2]) if len(argv) > 2 else 5.0
    port = int(argv[3]) if len(argv) > 3 else 5657
    path = record(out, seconds, port)
    size = path.stat().st_size
    print(f"{path}  {size} bytes  {seconds:g}s")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
