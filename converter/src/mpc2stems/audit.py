"""Report project features that are present but not modelled by this renderer.

Parsing an `.xpj` is only half the job: a project can legitimately use features
the renderer does not implement (an LFO, a modulation matrix, filter envelopes,
looping layers...).  Silently ignoring them produces a mix that looks fine and
is quietly wrong, so `convert` surfaces them instead of staying silent.
"""

from __future__ import annotations

from typing import Any

from . import filters
from .model import Project

#: ``kind -> human description``; order is the order warnings are reported in.
_WARNINGS: list[tuple[str, str]] = [
    ("modlink", "the modulation matrix has an active target (source/target enums are unknown)"),
    ("padfx", "pad effects are active (proprietary DSP, not modelled)"),
    ("arp", "the arpeggiator is enabled (generated notes are not modelled)"),
    ("slice", "a layer's sliceIndex is not 129, so it may play a chop region we do not resolve"),
    (
        "filter",
        "a pad uses a filter type this renderer does not model (MPC3000 LPF, Vocal, Model), "
        "so that pad is rendered without its filter",
    ),
    (
        "insert",
        "a track or submix carries an enabled insert other than the Color Compressor "
        "(Pad-Controlled FXs, Flex Beat, LP Filter, ...), which this renderer does not model",
    ),
]

#: The only insert whose effect this renderer models.
_MODELLED_INSERTS = {"Color Compressor"}

#: ``sliceIndex`` 129 means "the playable region is this layer's own
#: ``sliceInfo``" — the only value in the reference project, and the value
#: MPCTK writes for a full-file region.  128 occurs on hardware with unknown
#: semantics; anything else plausibly names a chop in the WAV's ``slic`` chunk.
SLICE_INDEX_LAYER_REGION = 129


def _instruments(project: Project) -> list[dict[str, Any]]:
    return project.data["tracks"][0]["program"]["drum"]["instruments"]


def _lfo_active(synth: dict[str, Any]) -> bool:
    for slot in (synth.get("lfoData") or {}).values():
        if not isinstance(slot, dict):
            continue
        if any(abs(float(slot.get(k, 0.0))) > 1e-9 for k in ("lfoPitch", "lfoAmpLevel", "lfoPan")):
            return True
        cutoff = slot.get("lfoFilterCutOff")
        if isinstance(cutoff, dict) and abs(float(cutoff.get("value0", 0.0))) > 1e-9:
            return True
    return False


def unmodelled_features(project: Project) -> list[str]:
    """Active features of `project` that this renderer does not model."""
    found: dict[str, set[int]] = {}
    used = sorted(project.pads[0]) if project.pads else []
    instruments = _instruments(project)

    for index in used:
        raw = instruments[index]

        if any(int(link.get("link.target", 0)) != 0 for link in raw.get("modLinks") or []):
            found.setdefault("modlink", set()).add(index)

        effects = (raw.get("padEffects") or {}).get("padEffectsData") or {}
        for entry in effects.values():
            if isinstance(entry, dict) and abs(float(entry.get("fxValue", 0.0))) > 1e-9:
                found.setdefault("padfx", set()).add(index)
                break

        synth = raw.get("synthSection") or {}
        slot0 = (synth.get("filterData") or {}).get("value0") or {}
        filter_type = int(slot0.get("filterType", 0) or 0)
        engaged = float(slot0.get("filterCutoff", 1.0)) < 0.999 or float(
            slot0.get("filterResonance", 0.0)
        ) > 0.001
        if engaged and filter_type and filter_type not in filters.MODELLED_FILTER_TYPES:
            found.setdefault("filter", set()).add(index)

        for layer in raw.get("layersv") or []:
            if not layer.get("sampleFile"):
                continue
            if int(layer.get("sliceIndex", SLICE_INDEX_LAYER_REGION)) != SLICE_INDEX_LAYER_REGION:
                found.setdefault("slice", set()).add(index)
                break

    for track in project.data["tracks"]:
        inserts = ((track.get("program") or {}).get("mixable") or {}).get("inserts") or {}
        if not inserts.get("insertsEnabled", True):
            continue
        for effect in inserts.get("effects") or []:
            if not effect.get("enable", True):
                continue
            name = ((effect.get("plugin") or {}).get("description") or {}).get("name", "?")
            if name not in _MODELLED_INSERTS:
                found.setdefault("insert", set()).add(-1)
                break

    arp = project.data.get("arpeggiatorProperties") or {}
    if isinstance(arp, dict) and arp.get("enabled"):
        found.setdefault("arp", set()).add(0)

    out: list[str] = []
    for kind, description in _WARNINGS:
        pads = found.get(kind)
        if not pads:
            continue
        if kind == "insert":
            # project-wide, not tied to a pad
            out.append(description)
            continue
        where = ", ".join(str(p) for p in sorted(pads)[:8])
        extra = "" if len(pads) <= 8 else f" (+{len(pads) - 8} more)"
        out.append(f"{description} — pad(s) {where}{extra}")
    return out
