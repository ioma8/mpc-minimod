"""Normalised, renderer-oriented view of an MPC project.

Everything here is a direct read of the project JSON; no inference about audio
behaviour lives in this module.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Iterable

from .xpj import Container, read_project

#: MPC's sequencer resolution.  Observed as 960 pulses per quarter note in every
#: sequence of the reference project (lengthPulses 15360 == 4 bars * 4 beats * 960).
PULSES_PER_QUARTER = 960

#: Program ``type`` values observed in ``tracks[].program.type``.
PROGRAM_TYPE_DRUM = 0

#: Track channel ``type`` values (``tracks[].program.type`` / ``mixer`` entries).
CHANNEL_TYPE_SUBMIX = 8
CHANNEL_TYPE_OUTPUT = 9
CHANNEL_TYPE_INPUT = 10


def _values(mapping: dict[str, Any], count: int) -> list[Any]:
    """Read an MPC ``{"value0": ..., "value1": ...}`` array wrapper."""
    return [mapping[f"value{i}"] for i in range(count)]


@dataclass(frozen=True)
class Layer:
    """One of the 8 sample layers of a drum pad."""

    active: bool
    sample_name: str
    sample_file: str
    zone_start: int
    zone_end: int
    volume: float
    pan: float
    coarse_tune: int
    fine_tune: int
    pitch: float
    velocity_start: int
    velocity_end: int
    root_note: int
    key_track: bool
    loop: bool
    loop_start: int
    loop_end: int
    slice_index: int
    #: 0 = forward, non-zero = reversed playback (pads 18 and 27 use 1)
    direction: int
    offset: int

    @property
    def cents(self) -> float:
        return self.coarse_tune * 100.0 + self.fine_tune


@dataclass(frozen=True)
class Pad:
    index: int
    note: int
    layers: tuple[Layer, ...]
    volume: float  # instrument.mixable.volume (pad level)
    pan: float  # instrument.mixable.pan
    mute: bool
    coarse_tune: int
    fine_tune: int
    velocity_sensitivity: float
    velocity_to_pitch: float
    amp_envelope: dict[str, Any]
    filter: dict[str, Any]
    #: sample warping ("time stretch") settings
    warp_enable: bool
    stretch_percentage: int
    sample_tempo: float
    bpm_lock: bool
    trigger_mode: int
    #: pads sharing a non-zero mute group cut each other (manual: "the last pad
    #: played silences any other playing samples in the same group")
    mute_group: int = 0
    #: second filter slot (``filterData.value1``); type 0 means "off"
    filter_slot1: dict[str, Any] = field(default_factory=dict)
    #: true = the two filter slots run in series, false = blended in parallel
    filter_serial: bool = False
    filter_blend: float = 0.5
    pitch_envelope: dict[str, Any] = field(default_factory=dict)
    filter_envelope: dict[str, Any] = field(default_factory=dict)
    aux_envelope: dict[str, Any] = field(default_factory=dict)
    #: 0.5 is neutral (no modulation)
    pitch_envelope_amount: float = 0.5
    #: LFO slots (``synthSection.lfoData``)
    lfo: dict[str, Any] = field(default_factory=dict)
    velocity_to_start: float = 0.0
    velocity_to_pan: float = 0.0

    @property
    def cents(self) -> float:
        return self.coarse_tune * 100.0 + self.fine_tune

    @property
    def unmuted_layers(self) -> tuple[Layer, ...]:
        return tuple(l for l in self.layers if l.sample_file)


#: Per-note playback modifiers (``event.note.modifierValueN``).  The value is
#: normalised 0..1 with 0.5 as neutral; for tuning this maps to +/- 1 octave.
MODIFIER_TUNING_COARSE = 0
MODIFIER_TUNING_FINE = 1
MODIFIER_FILTER_CUTOFF = 2
MODIFIER_RESONANCE = 3
MODIFIER_FILTER_ENV = 4
MODIFIER_PAN = 5
MODIFIER_LEVEL = 6
MODIFIER_ENV_ATTACK = 7
MODIFIER_ENV_DECAY = 8
MODIFIER_ENV_RELEASE = 9
MODIFIER_SAMPLE_LAYER = 14
MODIFIER_SAMPLE_SLICE = 15

#: (value - 0.5) * 24000 == cents.  0.5041666.. is exactly +1 semitone.
TUNING_COARSE_CENTS_PER_UNIT = 24000.0
#: fine tuning spans +/- 100 cents, same normalisation
TUNING_FINE_CENTS_PER_UNIT = 200.0


@dataclass(frozen=True)
class Note:
    pad_index: int
    note: int
    velocity: float
    time_pulses: int
    length_pulses: int
    modifiers: dict[int, float]  # active per-note modifiers only
    #: playback probability in percent (100 = always)
    probability: int = 100
    #: number of repeats inside the note length (1 = no ratchet)
    ratchet: int = 1

    @property
    def slice_index(self) -> int | None:
        """Sample Slice modifier (index 15) mapped to a slice number."""
        if MODIFIER_SAMPLE_SLICE not in self.modifiers:
            return None
        return int(round(self.modifiers[MODIFIER_SAMPLE_SLICE] * 127.0))

    @property
    def tuning_cents(self) -> float:
        """Extra per-note tuning in cents (0 when no active tuning modifier)."""
        cents = 0.0
        coarse = self.modifiers.get(MODIFIER_TUNING_COARSE)
        if coarse is not None:
            cents += (coarse - 0.5) * TUNING_COARSE_CENTS_PER_UNIT
        fine = self.modifiers.get(MODIFIER_TUNING_FINE)
        if fine is not None:
            cents += (fine - 0.5) * TUNING_FINE_CENTS_PER_UNIT
        return cents


@dataclass(frozen=True)
class Clip:
    track_name: str
    start_pulses: int
    end_pulses: int
    notes: tuple[Note, ...]


@dataclass(frozen=True)
class Sequence:
    index: int
    name: str
    bpm: float
    length_pulses: int
    notes: tuple[Note, ...]


@dataclass(frozen=True)
class TrackMix:
    volume: float
    pan: float
    mute: bool
    sends: tuple[float, ...]
    insert_names: tuple[str, ...]


@dataclass
class Project:
    container: Container
    data: dict[str, Any]
    root: Path
    #: track index -> pads keyed by pad index
    pads: dict[int, dict[int, Pad]]
    sequences: list[Sequence]
    track_mixes: dict[int, TrackMix]
    master_volume: float
    song: list[int]
    bpm: float

    @property
    def sequence_by_index(self) -> dict[int, Sequence]:
        return {s.index: s for s in self.sequences}

    # -- helpers ---------------------------------------------------------
    def drum_track_indices(self) -> list[int]:
        out = []
        for i, tr in enumerate(self.data["tracks"]):
            if tr["program"].get("type") == PROGRAM_TYPE_DRUM:
                out.append(i)
        return out

    def notes_for(self, track: int) -> list[tuple[int, Note]]:
        """Every note of a track in song order, with its sequence index."""
        out: list[tuple[int, Note]] = []
        for seq_index in self.song:
            seq = self.sequences[seq_index]
            for note in seq.notes:
                out.append((seq_index, note))
        return out

    def sample_path(self, track: int, pad: int, layer: Layer) -> Path:
        return self.root / layer.sample_file


def load(path: str | Path) -> Project:
    path = Path(path)
    container = read_project(path)
    data = container.document["data"]
    root = path.parent / f"{path.stem}_[ProjectData]"

    tracks = data["tracks"]
    pads: dict[int, dict[int, Pad]] = {}
    for ti, tr in enumerate(tracks):
        prog = tr["program"]
        if prog.get("type") != PROGRAM_TYPE_DRUM:
            continue
        note_for_pad = _values(prog["padNoteMap"]["noteForPad"], 128)
        instruments = prog["drum"]["instruments"]
        pad_map: dict[int, Pad] = {}
        for pad_index, ins in enumerate(instruments):
            layers = []
            for raw in ins["layersv"]:
                si = raw["sliceInfo"]
                layers.append(
                    Layer(
                        active=bool(raw["active"]),
                        sample_name=raw.get("sampleName", ""),
                        sample_file=raw.get("sampleFile", ""),
                        zone_start=int(si["Start"]),
                        # ``sliceInfo.End`` is treated as exclusive.  The device
                        # writes End = n-1 for an untrimmed sample, which is also
                        # consistent with a last-frame index; rendering that way
                        # measures worse (0.21643 vs 0.21627), so keep exclusive
                        # and the +/-1 frame ambiguity documented in FORMAT.md.
                        zone_end=int(si["End"]),
                        volume=float(raw["volume"]["controlValue"]),
                        pan=float(raw["pan"]),
                        coarse_tune=int(raw["coarseTune"]),
                        fine_tune=int(raw["fineTune"]),
                        pitch=float(raw["pitch"]),
                        velocity_start=int(raw["velocityStart"]),
                        velocity_end=int(raw["velocityEnd"]),
                        root_note=int(raw["rootNote"]),
                        key_track=bool(raw["keyTrackEnable"]),
                        loop=bool(raw["loop"]),
                        loop_start=int(raw["loopStart"]),
                        loop_end=int(raw["loopEnd"]),
                        slice_index=int(raw["sliceIndex"]),
                        direction=int(raw.get("direction", 0)),
                        offset=int(raw.get("offset", 0)),
                    )
                )
            synth = ins["synthSection"]
            filt = synth["filterData"]["value0"]
            pad_map[pad_index] = Pad(
                index=pad_index,
                note=int(note_for_pad[pad_index]),
                layers=tuple(layers),
                volume=float(ins["mixable"]["volume"]),
                pan=float(ins["mixable"]["pan"]),
                mute=bool(ins["mixable"]["mute"]),
                coarse_tune=int(ins["coarseTune"]),
                fine_tune=int(ins["fineTune"]),
                velocity_sensitivity=float(synth["velocitySensitivity"]),
                velocity_to_pitch=float(synth["velocityToPitch"]),
                amp_envelope=synth["ampEnvelope"],
                filter=filt,
                warp_enable=bool(ins.get("warpEnable", False)),
                stretch_percentage=int(ins.get("stretchPercentage", 100)),
                sample_tempo=float(ins.get("tempo", 0.0)),
                bpm_lock=bool(ins.get("bpmLock", False)),
                trigger_mode=int(ins.get("triggerMode", 0)),
                mute_group=int(ins.get("whichMuteGroup", 0)),
                filter_slot1=synth["filterData"].get("value1", {}),
                filter_serial=bool(synth.get("filterSerialRouting", False)),
                filter_blend=float(synth.get("filterBlend", 0.5)),
                pitch_envelope=synth.get("pitchEnvelope", {}),
                filter_envelope=synth.get("filterEnvelope", {}),
                aux_envelope=synth.get("auxEnvelope", {}),
                pitch_envelope_amount=float(synth.get("pitchEnvelopeAmount", 0.5)),
                lfo=synth.get("lfoData", {}),
                velocity_to_start=float(synth.get("velocityToStart", 0.0)),
                velocity_to_pan=float(synth.get("velocityToPan", 0.0)),
            )
        pads[ti] = pad_map

    # ---- sequences -----------------------------------------------------
    sequences: list[Sequence] = []
    for entry in data["sequences"]:
        raw = entry["value"]
        notes: list[Note] = []
        pad_lookup = pads.get(0, {})
        note_to_pad = {p.note: p.index for p in pad_lookup.values()}
        for group in raw.get("trackClipMaps", []):
            for clip_entry in group:
                clip = clip_entry["value"]
                for ev in clip["eventList"]["events"]:
                    if ev.get("type") != 3:  # 3 == note-on
                        continue
                    n = ev["note"]
                    note_number = int(n["note"])
                    if note_number not in note_to_pad:
                        continue
                    mods = {}
                    for k in range(16):
                        if n.get(f"modifierActiveState{k}"):
                            mods[k] = float(n.get(f"modifierValue{k}", 0.0))
                    notes.append(
                        Note(
                            pad_index=note_to_pad[note_number],
                            note=note_number,
                            velocity=float(n["velocity"]),
                            time_pulses=int(ev["time"]),
                            length_pulses=int(n["length"]),
                            modifiers=mods,
                            probability=int(n.get("probability", 100)),
                            ratchet=max(1, int(n.get("ratchet", 1))),
                        )
                    )
        notes.sort(key=lambda x: (x.time_pulses, x.pad_index))
        sequences.append(
            Sequence(
                index=int(entry["key"]),
                name=raw["name"],
                bpm=float(raw["bpm"]),
                length_pulses=int(raw["lengthPulses"]),
                notes=tuple(notes),
            )
        )

    # ---- mixer ---------------------------------------------------------
    track_mixes: dict[int, TrackMix] = {}
    for ti, tr in enumerate(tracks):
        mx = tr["program"]["mixable"]
        inserts = mx.get("inserts", {}).get("effects", [])
        track_mixes[ti] = TrackMix(
            volume=float(mx["volume"]),
            pan=float(mx["pan"]),
            mute=bool(mx["mute"]),
            sends=tuple(float(s) for s in mx.get("sends", [])),
            insert_names=tuple(
                e["plugin"]["description"]["name"] for e in inserts if e.get("enable")
            ),
        )

    song = [int(item["item.sequenceIndex"]) for item in data["songs"][0]["items"]]
    current_sequence = int(data.get("currentSequence", 0))
    by_index = {s.index: s for s in sequences}
    bpm = by_index[current_sequence].bpm if current_sequence in by_index else float(data.get("masterTempo", 120.0))

    return Project(
        container=container,
        data=data,
        root=root,
        pads=pads,
        sequences=sequences,
        track_mixes=track_mixes,
        master_volume=float(data["mixer"]["volume"]),
        song=song,
        bpm=bpm,
    )
