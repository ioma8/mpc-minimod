"""Reader for Akai MPC project files (`.xpj`).

Container layout (observed on MPC firmware reporting product id ``AC50``):

    gzip stream
      -> ACVS\\n
         <version>\\n            e.g. "1.3.0.12"
         SerialisableProjectData\\n
         json\\n
         <platform>\\n           e.g. "Linux"
         { ... JSON document ... }

The JSON document has a single top level ``data`` object.
"""

from __future__ import annotations

import gzip
import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any

MAGIC = b"ACVS\n"
PAYLOAD_KIND = "SerialisableProjectData"


@dataclass(frozen=True)
class Container:
    """The outer envelope of an ``.xpj`` file."""

    magic: str
    version: str
    payload_kind: str
    encoding: str
    platform: str
    json_bytes: bytes

    @property
    def document(self) -> dict[str, Any]:
        return json.loads(self.json_bytes)


def parse_container(blob: bytes) -> Container:
    """Parse a raw (already gunzipped) project blob."""
    if not blob.startswith(MAGIC):
        raise ValueError(f"not an ACVS project: {blob[:16]!r}")

    lines = blob.split(b"\n", 5)
    if len(lines) < 6:
        raise ValueError("truncated ACVS header")
    if lines[2].decode("ascii", "replace") != PAYLOAD_KIND:
        raise ValueError(f"unexpected payload kind {lines[2]!r}")

    return Container(
        magic=lines[0].decode("ascii"),
        version=lines[1].decode("ascii"),
        payload_kind=lines[2].decode("ascii"),
        encoding=lines[3].decode("ascii"),
        platform=lines[4].decode("ascii"),
        json_bytes=lines[5],
    )


def read_project(path: str | Path) -> Container:
    """Read and gunzip an ``.xpj`` file."""
    raw = Path(path).read_bytes()
    if raw[:2] == b"\x1f\x8b":
        raw = gzip.decompress(raw)
    return parse_container(raw)

def write_project(
    path: str | Path,
    container: Container,
    document: dict[str, Any],
    *,
    mtime: int = 0,
) -> Path:
    """Write an ``.xpj`` back out: the same five-line header, gzipped JSON.

    ``mtime=0`` keeps the gzip header fixed so the same project always produces
    the same bytes, which matters when the device is asked to open 12 variants of
    one file.
    """
    header = "\n".join(
        (
            container.magic,
            container.version,
            container.payload_kind,
            container.encoding,
            container.platform,
        )
    )
    body = json.dumps(document, indent=0, ensure_ascii=False)
    blob = gzip.compress(f"{header}\n{body}".encode("utf-8"), 6, mtime=mtime)
    out = Path(path)
    out.write_bytes(blob)
    return out
