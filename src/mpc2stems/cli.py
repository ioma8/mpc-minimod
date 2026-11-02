"""Command line interface for mpc2stems."""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

import numpy as np

from . import audit, exact, master, model, render, separate, xpj
from .wav import read_pcm24_ints, read_wav, write_pcm24


def _slug(text: str, limit: int = 40) -> str:
    slug = re.sub(r"[^A-Za-z0-9]+", "-", text).strip("-").lower()
    return (slug or "pad")[:limit]


def _load_reference(path: Path) -> np.ndarray:
    ref = read_wav(path)
    if ref.sample_rate != 44100:
        raise SystemExit(f"reference must be 44100 Hz, got {ref.sample_rate}")
    return ref.samples


def _align(model_arr: np.ndarray, ref: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
    n = min(len(model_arr), len(ref))
    return model_arr[:n], ref[:n]


def _correlation(a: np.ndarray, b: np.ndarray) -> float:
    if len(a) == 0:
        return 0.0
    denom = float(np.sqrt(np.sum(a * a) * np.sum(b * b)))
    return float(np.sum(a * b) / denom) if denom else 0.0


def cmd_info(args: argparse.Namespace) -> int:
    proj = model.load(args.project)
    track = proj.drum_track_indices()[0]
    print(f"container      : {proj.container.version} ({proj.container.platform})")
    print(f"project        : {args.project}")
    print(f"sample root    : {proj.root}")
    print(f"song           : {proj.song}  ({proj.bpm:g} BPM)")
    print(f"sequences      : {[(s.index, s.bpm, s.length_pulses, len(s.notes)) for s in proj.sequences]}")
    print(f"drum track     : {track}  {proj.data['tracks'][track]['name']!r}")
    print(f"master fader   : {proj.master_volume:.6f}")
    print(f"track fader    : {proj.track_mixes[track].volume:.6f}")
    pads = sorted(p for p, pad in proj.pads[track].items() if pad.unmuted_layers)
    by_index = proj.sequence_by_index
    used = sorted({n.pad_index for i in proj.song for n in by_index[i].notes})
    print(f"pads w/ sample : {len(pads)}")
    print(f"pads in song   : {used}")
    print(f"{'pad':>4} {'file':40} {'cents':>7} {'vol':>6} {'pan':>6} {'warp':>5} {'str%':>5} {'dir':>3}")
    for p in used:
        pad = proj.pads[track][p]
        layers = pad.unmuted_layers
        if not layers:
            # solo projects silence every pad but one, so a note can still exist
            # for a pad that has no sample
            print(f"{p:4d} {'(no sample)':40}")
            continue
        layer = layers[0]
        print(
            f"{p:4d} {layer.sample_file[:40]:40} {layer.cents:7.1f} {pad.volume:6.3f} "
            f"{pad.pan:6.3f} {str(pad.warp_enable):>5} {pad.stretch_percentage:5d} {layer.direction:3d}"
        )
    return 0


def cmd_convert(args: argparse.Namespace) -> int:
    proj = model.load(args.project)
    for warning in audit.unmodelled_features(proj):
        print(f"warning: {warning} (not modelled by this renderer)", file=sys.stderr)
    track = proj.drum_track_indices()[0]
    params = render.RenderParams(latency=args.latency, fidelity=args.fidelity)
    stems, n_frames = render.render_pad_stems(proj, track, params)

    out_dir = Path(args.out)
    stems_dir = out_dir / "stems"
    stems_dir.mkdir(parents=True, exist_ok=True)

    if not args.no_master:
        params_c = master.CompressorParams()
        stems = master.apply_to_stems(stems, params_c)

    reference = _load_reference(Path(args.reference)) if args.reference else None
    reference_ints = None
    if reference is not None:
        n_frames = len(reference)
        if args.exact:
            reference_ints = read_pcm24_ints(args.reference)

    if getattr(args, "refine", False) and reference is not None:
        stems = separate.refine(stems, reference)

    gains: dict[int, float] = {}
    if args.calibrate and reference is not None:
        gains = exact.calibrate_pad_gains(stems, reference)
        stems = exact.apply_gains(stems, gains)

    def trimmed(pad_index: int) -> np.ndarray:
        stem = stems[pad_index][:n_frames]
        if len(stem) < n_frames:
            stem = np.vstack([stem, np.zeros((n_frames - len(stem), 2))])
        return stem

    exact_ints = None
    if args.exact:
        if reference_ints is None:
            raise SystemExit("--exact needs --reference (the device mixdown)")
        exact_ints = exact.distribute_residual(
            {p: trimmed(p) for p in stems}, reference_ints, n_frames
        )

    names = {}
    for pad_index in sorted(stems):
        pad = proj.pads[track][pad_index]
        layer = pad.unmuted_layers[0]
        name = f"{pad_index:02d}-{_slug(layer.sample_name or layer.sample_file)}.wav"
        if exact_ints is not None:
            exact.write_int24(stems_dir / name, exact_ints[pad_index], 44100)
        else:
            write_pcm24(stems_dir / name, np.clip(trimmed(pad_index), -1.0, 0.999999), 44100)
        names[pad_index] = name

    if exact_ints is not None:
        # the written stems are the deliverable: the mix must be their exact sum
        total_ints = sum(exact_ints[p] for p in exact_ints)
        exact.write_int24(out_dir / "mix-from-stems.wav", total_ints, 44100)
        total = exact.from_int24(total_ints)
    else:
        total = np.zeros((n_frames, 2))
        for pad_index, stem in stems.items():
            total[: min(len(stem), n_frames)] += stem[:n_frames]
        write_pcm24(out_dir / "mix-from-stems.wav", np.clip(total, -1.0, 0.999999), 44100)

    report = {
        "project": str(args.project),
        "track": track,
        "song": proj.song,
        "bpm": proj.bpm,
        "frames": int(n_frames),
        "pads": {str(k): v for k, v in names.items()},
        "render_params": params.__dict__,
        "calibrated_gains": {str(k): round(v, 4) for k, v in gains.items()},
    }

    if reference is not None:
        a, b = _align(total, reference)
        report["reference"] = str(args.reference)
        report["correlation"] = {
            "left": _correlation(a[:, 0], b[:, 0]),
            "right": _correlation(a[:, 1], b[:, 1]),
        }
        report["rms"] = {
            "stems": float(np.sqrt(np.mean(a**2))),
            "reference": float(np.sqrt(np.mean(b**2))),
        }
        report["residual_rms"] = float(np.sqrt(np.mean((a - b) ** 2)))

    (out_dir / "report.json").write_text(json.dumps(report, indent=2, default=str))
    print(json.dumps(report, indent=2, default=str))
    return 0


def cmd_solo_projects(args: argparse.Namespace) -> int:
    """Write one project per pad that plays, with every other pad silenced.

    Stock firmware exports a mixdown only -- no stems, no per-track split.  The
    cheapest way to get per-pad audio out of it is to hand the device a project
    that contains one pad, export, and repeat.  Silencing happens *in the file*
    (other layers lose their sample references), so no UI feature is involved,
    and each variant keeps the master chain, so its export is that pad through
    the same bus as the full mix.
    """
    project = Path(args.project)
    container = xpj.read_project(project)
    document = container.document
    source_dir = project.parent / f"{project.stem}_[ProjectData]"
    out_dir = Path(args.out)
    out_dir.mkdir(parents=True, exist_ok=True)

    proj = model.load(project)
    playing = sorted({event[1] for event in render.note_events(proj)})
    instruments = [
        ins
        for track in document["data"]["tracks"]
        if track["program"].get("type") == 0
        for ins in track["program"].get("drum", {}).get("instruments", [])
    ]
    playing = [index for index in playing if index < len(instruments)]
    if not playing:
        print("no pad in this project plays a note", file=sys.stderr)
        return 1

    written = []
    for keep in playing:
        variant = json.loads(json.dumps(document))
        pads = [
            ins
            for track in variant["data"]["tracks"]
            if track["program"].get("type") == 0
            for ins in track["program"].get("drum", {}).get("instruments", [])
        ]
        kept_file = ""
        for index, ins in enumerate(pads):
            layers = ins.get("layersv", [])
            if index == keep:
                kept_file = next((l.get("sampleFile") for l in layers if l.get("sampleFile")), "")
                continue
            for layer in layers:
                if layer.get("sampleFile") or layer.get("sampleName"):
                    layer["sampleName"] = ""
                    layer["sampleFile"] = ""
                    layer["active"] = False

        # keep only the pool entries that name the sample this pad still uses
        for entry in variant["data"].get("samples", [])[:]:
            if entry.get("path") != kept_file:
                variant["data"]["samples"].remove(entry)
        for track in variant["data"]["tracks"]:
            for entry in (track.get("samples") or [])[:]:
                if entry.get("path") != kept_file:
                    track["samples"].remove(entry)

        label = _slug(Path(kept_file).stem if kept_file else f"pad{keep}")
        name = f"solo-{keep:02d}-{label}"
        target = out_dir / f"{name}.xpj"
        xpj.write_project(target, container, variant)

        data_dir = out_dir / f"{name}_[ProjectData]"
        data_dir.mkdir(exist_ok=True)
        sample = source_dir / kept_file
        if kept_file and sample.is_file():
            (data_dir / kept_file).write_bytes(sample.read_bytes())
        else:
            print(f"warning: {kept_file!r} not found for pad {keep}", file=sys.stderr)
        written.append(target)

    print(f"wrote {len(written)} solo project(s) to {out_dir}", file=sys.stderr)
    for target in written:
        print(target.name)
    return 0

def cmd_validate(args: argparse.Namespace) -> int:
    proj = model.load(args.project)
    track = proj.drum_track_indices()[0]
    reference = _load_reference(Path(args.reference))
    params = render.RenderParams(latency=args.latency)
    stems, _ = render.render_pad_stems(proj, track, params)
    if not args.no_master:
        stems = master.apply_to_stems(stems, master.CompressorParams())
    total = np.zeros((len(reference), 2))
    for stem in stems.values():
        n = min(len(stem), len(reference))
        total[:n] += stem[:n]

    out = 20 * np.log10(np.maximum(np.abs(total), 1e-9))
    clip = np.mean(np.abs(total) > 1.0) * 100
    print(f"{'channel':>7} {'corr':>8} {'rmse':>8} {'rms model':>10} {'rms ref':>8}")
    for ch, name in enumerate(("left", "right")):
        print(
            f"{name:>7} {_correlation(total[:, ch], reference[:, ch]):8.4f} "
            f"{np.sqrt(np.mean((total[:, ch] - reference[:, ch]) ** 2)):8.4f} "
            f"{np.sqrt(np.mean(total[:, ch] ** 2)):10.4f} "
            f"{np.sqrt(np.mean(reference[:, ch] ** 2)):8.4f}"
        )
    print(f"model peak {np.abs(total).max():.3f} (clipping {clip:.1f}% before clip)")
    print(f"reference peak {np.abs(reference).max():.3f}")
    return 0


def cmd_verify(args: argparse.Namespace) -> int:
    stems_dir = Path(args.stems)
    files = sorted(stems_dir.glob("*.wav"))
    if not files:
        raise SystemExit(f"no .wav files in {stems_dir}")
    reference = read_pcm24_ints(args.reference)
    total = np.zeros_like(reference, dtype=np.int64)
    for path in files:
        ints = read_pcm24_ints(path)
        n = min(len(ints), len(reference))
        total[:n] += ints[:n]
    ok = bool(np.array_equal(total, reference))
    diff = total - reference.astype(np.int64)
    print(f"stems        : {len(files)} files in {stems_dir}")
    print(f"frames       : {len(reference)}")
    print(f"sum == mix   : {ok}")
    print(f"max |diff|   : {int(np.abs(diff).max())} LSB")
    print(f"different    : {int(np.count_nonzero(diff))} of {diff.size} samples")
    return 0 if ok else 1


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="mpc2stems", description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    p_info = sub.add_parser("info", help="print project summary")
    p_info.add_argument("project", type=Path)
    p_info.set_defaults(func=cmd_info)

    p_conv = sub.add_parser("convert", help="render per-pad stems")
    p_conv.add_argument("project", type=Path)
    p_conv.add_argument("--out", required=True, type=Path)
    p_conv.add_argument("--reference", type=Path, help="device mixdown for trimming/validation")
    p_conv.add_argument("--latency", type=int, default=91)
    p_conv.add_argument("--no-master", action="store_true", help="skip the master compressor")
    p_conv.add_argument(
        "--fidelity",
        action="store_true",
        help="do not gate stretched pads to their note length: the stems then carry the "
        "energy the device puts there (up to 21 dB more), while the model residual rises "
        "because their phase cannot be reproduced offline",
    )
    p_conv.add_argument(
        "--calibrate",
        action="store_true",
        help="fit per-pad gains against --reference before writing stems",
    )
    p_conv.add_argument(
        "--refine",
        action="store_true",
        help="recover each stem from --reference with a model-guided Wiener mask; "
        "this is what fixes the warped pads, whose stretcher phase cannot be rendered",
    )
    p_conv.add_argument(
        "--exact",
        action="store_true",
        help="distribute the device master-FX residual so the stems sum to the mixdown bit-exactly",
    )
    p_conv.set_defaults(func=cmd_convert)

    p_solo = sub.add_parser(
        "solo-projects",
        help="write one project per pad (all other pads silent) for device exports",
    )
    p_solo.add_argument("project", type=Path)
    p_solo.add_argument("--out", required=True, type=Path)
    p_solo.set_defaults(func=cmd_solo_projects)

    p_ver = sub.add_parser("verify", help="check the written stems sum bit-exactly to a mixdown")
    p_ver.add_argument("stems", type=Path, help="directory containing the rendered stem WAVs")
    p_ver.add_argument("reference", type=Path)
    p_ver.set_defaults(func=cmd_verify)

    p_val = sub.add_parser("validate", help="compare the stem sum against a mixdown")
    p_val.add_argument("project", type=Path)
    p_val.add_argument("reference", type=Path)
    p_val.add_argument("--latency", type=int, default=91)
    p_val.add_argument("--no-master", action="store_true")
    p_val.set_defaults(func=cmd_validate)

    args = parser.parse_args(argv)
    if not hasattr(args, "func"):
        parser.print_help()
        return 1
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
