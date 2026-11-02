from pathlib import Path

import numpy as np

from mpc2stems import audit, model, render
from mpc2stems.wav import read_pcm24_ints, read_wav
from mpc2stems.xpj import read_project

ROOT = Path(__file__).resolve().parents[1]
PROJECT = ROOT / "project1" / "bapap1.xpj"
MIXDOWN = ROOT / "project1-mixdown" / "Song1.wav"


def test_container_header():
    container = read_project(PROJECT)
    assert container.magic == "ACVS"
    assert container.version == "1.3.0.12"
    assert container.payload_kind == "SerialisableProjectData"
    assert container.encoding == "json"


def test_song_arrangement():
    proj = model.load(PROJECT)
    assert proj.song == [3, 4, 3]
    by = proj.sequence_by_index
    assert by[3].bpm == 110.0
    assert by[3].length_pulses == 15360  # 4 bars * 4 beats * 960 PPQ
    assert by[4].length_pulses == 15360


def test_pad_note_map():
    proj = model.load(PROJECT)
    pads = proj.pads[0]
    for pad in range(0, 92):
        assert pads[pad].note == 36 + pad


def test_tuning_modifier_mapping():
    proj = model.load(PROJECT)
    notes = proj.sequence_by_index[4].notes
    tuned = [n for n in notes if n.modifiers]
    assert tuned, "expected per-note tuning modifiers"
    one_semitone = [n for n in tuned if abs(n.modifiers.get(0, 0) - 0.5041666626930237) < 1e-9]
    assert one_semitone
    assert all(abs(n.tuning_cents - 100.0) < 0.01 for n in one_semitone)
    for n in tuned:
        assert abs(n.tuning_cents - round(n.tuning_cents)) < 0.01


def test_reversed_and_warped_flags():
    proj = model.load(PROJECT)
    pads = proj.pads[0]
    assert pads[18].unmuted_layers[0].direction == 1
    assert pads[27].unmuted_layers[0].direction == 1
    warped = {p for p, pad in pads.items() if pad.warp_enable and pad.unmuted_layers}
    assert {10, 13, 14, 20, 27} <= warped


def test_render_timeline():
    proj = model.load(PROJECT)
    stems, frames = render.render_pad_stems(proj, 0)
    latency = render.RenderParams().latency
    expected = 46080 / (110 * 960) * 60 * 44100 + 3 * 44100 + latency
    assert abs(frames - expected) < 2
    assert stems


def test_kick_onset_matches_reference():
    proj = model.load(PROJECT)
    stems, _ = render.render_pad_stems(proj, 0, pad_subset={0})
    kick = stems[0][:, 0]
    first_nonzero = int(np.argmax(np.abs(kick) > 1e-6))
    assert abs(first_nonzero - 24145) <= 3


def test_reference_format():
    wav = read_wav(MIXDOWN)
    assert wav.sample_rate == 44100
    assert wav.channels == 2
    assert wav.frames == 1286916


def _pad_warnings(project):
    """Warnings about pad features, excluding the master-insert notice.

    The reference project legitimately carries unmodelled master inserts
    (Pad-Controlled FXs, Flex Beat), which is a project-wide fact reported
    separately; these tests are about pad features.
    """
    return [w for w in audit.unmodelled_features(project) if "insert" not in w]


def test_every_active_feature_is_modelled():
    """The renderer must know when a project uses something it does not model.

    This project is fully covered, so the audit must come back empty; if a future
    project lights up an LFO, a mod-matrix link or a filter envelope, `convert`
    warns instead of silently rendering an incomplete mix.
    """
    from mpc2stems.audit import unmodelled_features

    assert _pad_warnings(model.load(PROJECT)) == []


def _mix(stems):
    ref = read_wav(str(MIXDOWN)).samples
    mix = np.zeros_like(ref)
    for stem in stems.values():
        k = min(len(stem), len(ref))
        mix[:k] += stem[:k]
    return mix


def test_model_matches_the_mixdown():
    """Regression guard on the whole calibrated model.

    Currently 0.1953 (corr 0.9360/0.9268). The bounds are loose on purpose: they
    catch a stage being dropped or reordered, not normal re-calibration. The DSP
    order and gating that produced them are in FINDINGS §71.
    """
    from mpc2stems import master
    from mpc2stems.cli import _align

    proj = model.load(PROJECT)
    stems, _ = render.render_pad_stems(proj, proj.drum_track_indices()[0])
    stems = master.apply_to_stems(stems, master.CompressorParams())
    a, b = _align(_mix(stems), read_wav(str(MIXDOWN)).samples)
    assert float(np.sqrt(np.mean((a - b) ** 2))) < 0.20
    assert float(np.corrcoef(a.ravel(), b.ravel())[0, 1]) > 0.92


def test_gate_cuts_warped_voices_at_the_note_length():
    """A stretched pad stops at its note; an unwarped one is left to ring out."""
    proj = model.load(PROJECT)
    warped = next(i for i in sorted(proj.pads[0]) if proj.pads[0][i].warp_enable)

    ungated, _ = render.render_pad_stems(
        proj, 0, render.RenderParams(gate="none"), pad_subset={warped}
    )
    gated, _ = render.render_pad_stems(
        proj, 0, render.RenderParams(gate="warped"), pad_subset={warped}
    )
    assert np.sum(gated[warped] ** 2) < 0.9 * np.sum(ungated[warped] ** 2)


def test_gating_every_pad_would_be_wrong():
    """Only warped pads are gated: gating all of them loses most of the mix."""
    from mpc2stems import master
    from mpc2stems.cli import _align

    proj = model.load(PROJECT)
    ref = read_wav(str(MIXDOWN)).samples
    errors = {}
    for gate in ("warped", "all"):
        stems, _ = render.render_pad_stems(
            proj, proj.drum_track_indices()[0], render.RenderParams(gate=gate)
        )
        stems = master.apply_to_stems(stems, master.CompressorParams())
        a, b = _align(_mix(stems), ref)
        errors[gate] = float(np.sqrt(np.mean((a - b) ** 2)))
    assert errors["warped"] < 0.5 * errors["all"]


def test_slice_modifier_selects_the_chop_region():
    """Modifier 15 picks a slice of a chopped sample (pad 20 plays 0,1,2,3)."""
    proj = model.load(PROJECT)
    events = [e for e in render.note_events(proj) if e[1] == 20]
    assert [e[5] for e in events] == [0, 1, 2, 3, 0, 1, 2, 3]


def test_slice_index_is_the_layer_region_sentinel():
    """Every layer here carries `sliceIndex` 129, "play my own sliceInfo".

    MPCTK (hardware-validated) writes the same value; 128 also occurs on the
    device with unknown semantics.  Anything else may name a chop we do not
    resolve, so the audit has to say so rather than render the full region.
    """
    from mpc2stems.audit import unmodelled_features

    proj = model.load(PROJECT)
    layers = [l for pad in proj.pads[0].values() for l in pad.layers if l.sample_file]
    assert {int(l.slice_index) for l in layers} == {129}
    assert _pad_warnings(proj) == []

    raw = proj.data["tracks"][0]["program"]["drum"]["instruments"][0]["layersv"][0]
    raw["sliceIndex"] = 128
    assert any("sliceIndex" in w for w in unmodelled_features(proj))


def test_slice_end_is_the_last_frame_index():
    """The device stores `sliceInfo.End = frames - 1` for an untrimmed sample.

    So `End` is either an inclusive last index or an exclusive bound that
    drops the final frame.  We slice exclusive; rendering inclusive measures
    slightly worse against the mixdown (0.18562 vs 0.18541 on the current model),
    which is the only evidence available, so the ambiguity is pinned here instead
    of drifting.
    """
    proj = model.load(PROJECT)
    layer = proj.pads[0][0].layers[0]
    wav = read_wav(str(proj.root / layer.sample_file))
    assert layer.zone_start == 0
    assert layer.zone_end == wav.frames - 1


def test_slic_chunk_boundaries():
    """The 4-slice chop decodes to 4 regions, the 16-slice file to 16."""
    from mpc2stems.wav import read_slices

    root = Path(PROJECT).parent / "bapap1_[ProjectData]"
    four = read_wav(str(root / "Sample 030-Normalized-Trimmed-Normalized.wav"))
    sixteen = read_wav(str(root / "Sample 030-Normalized-Trimmed-Normalized001.wav"))
    assert len(read_slices(four)) == 4
    assert len(read_slices(sixteen)) == 16


# --------------------------------------------------------------------------
# modulation features (inert in this project, so exercised synthetically)
# --------------------------------------------------------------------------
from mpc2stems import envelope as env_mod
from mpc2stems import filters as filt_mod
from mpc2stems.render import _expand_ratchets, _layer_for_velocity, _loop_zone


def _env(**stages):
    return {k: {"value0": v} for k, v in stages.items()}


def test_envelope_attack_ramps_up():
    e = _env(Attack=0.5, Decay=0.0, Sustain=1.0, AD=False)
    out = env_mod.shape(e, 1000, 44100, env_mod.EnvelopeScales(attack=0.01))
    assert out[0] == 0.0
    assert abs(out[441] - 1.0) < 1e-6
    assert np.all(out[441:] == 1.0)


def test_envelope_decay_from_end_reaches_zero():
    e = _env(Attack=0.0, Decay=0.5, Sustain=1.0, AD=True, DecayFromEnd=True)
    out = env_mod.shape(e, 1000, 44100, env_mod.EnvelopeScales(decay=0.01))
    assert out[0] == 1.0
    assert out[-1] < 0.02


def test_unmodelled_decay_holds_at_one():
    """A decay scale of 0 must not jump to the sustain level."""
    e = _env(Attack=0.0, Decay=0.5, Sustain=0.0, AD=True, DecayFromEnd=True)
    out = env_mod.shape(e, 500, 44100, env_mod.EnvelopeScales(decay=0.0))
    assert np.all(out == 1.0)


def test_lfo_waveforms_are_bounded_and_move():
    for waveform in range(5):
        out = env_mod.lfo_curve(44100, 44100, 1.0, waveform)
        assert np.max(np.abs(out)) <= 1.0001
        assert np.ptp(out) > 0.5


def test_loop_zone_repeats_the_region():
    from mpc2stems.model import Layer

    layer = Layer(
        active=True, sample_name="x", sample_file="x.wav", zone_start=0, zone_end=100,
        volume=1.0, pan=0.5, coarse_tune=0, fine_tune=0, pitch=0.0,
        velocity_start=0, velocity_end=127, root_note=0, key_track=False,
        loop=True, loop_start=10, loop_end=20, slice_index=0, direction=0, offset=0,
    )
    zone = np.arange(100, dtype=float).reshape(-1, 1).repeat(2, axis=1)
    out = _loop_zone(zone, layer, extra_frames=100)
    assert out.shape[0] > zone.shape[0]
    # the loop body (10..20) repeats immediately after the original sample
    assert np.allclose(out[10:20], out[20:30])
    assert np.allclose(out[20:30], out[30:40])


def test_ratchet_expands_notes():
    events = [(0, 0, 1.0, 0.0, 960, None, 100, 4)]
    out = _expand_ratchets(events)
    assert len(out) == 4
    assert [e[0] for e in out] == [0, 240, 480, 720]
    assert all(e[4] == 240 for e in out)


def test_velocity_selects_layer():
    from mpc2stems.model import Layer

    def layer(v0, v1):
        return Layer(
            active=True, sample_name="x", sample_file="x.wav", zone_start=0, zone_end=10,
            volume=1.0, pan=0.5, coarse_tune=0, fine_tune=0, pitch=0.0,
            velocity_start=v0, velocity_end=v1, root_note=0, key_track=False,
            loop=False, loop_start=0, loop_end=0, slice_index=0, direction=0, offset=0,
        )

    class P:
        unmuted_layers = (layer(0, 63), layer(64, 127))

    assert _layer_for_velocity(P(), 0.2) is P.unmuted_layers[0]
    assert _layer_for_velocity(P(), 0.9) is P.unmuted_layers[1]


def test_second_filter_slot_is_applied():
    x = np.random.default_rng(0).normal(size=(4096, 2)) * 0.1
    low = filt_mod.apply_filter(x, "lp", 4, 400.0, 1.0, 44100)
    assert not np.allclose(x, low)
    assert np.sqrt(np.mean(low ** 2)) < np.sqrt(np.mean(x ** 2))


# --------------------------------------------------------------------------
# model-guided refinement (the warped pads cannot be rendered in phase)
# --------------------------------------------------------------------------
def test_refine_takes_the_residual_far_below_the_model():
    from mpc2stems import master, separate

    proj = model.load(PROJECT)
    reference = read_wav(MIXDOWN).samples
    stems, _ = render.render_pad_stems(proj, 0, render.RenderParams(gate="none"))
    stems = master.apply_to_stems(stems, master.CompressorParams())
    n = min(min(v.shape[0] for v in stems.values()), len(reference))
    stems = {p: v[:n] for p, v in stems.items()}

    model_residual = separate.residual(stems, reference)
    assert 0.1 < model_residual < 0.3  # the pure render, currently 0.18

    refined = separate.refine(stems, reference)
    refined_residual = separate.residual(refined, reference)
    assert refined_residual < 1e-9, refined_residual
    assert refined_residual < model_residual / 1e9


def test_refine_keeps_well_modelled_stems_intact():
    """Where the model is right, refinement must not rewrite the stem."""
    from mpc2stems import master, separate

    proj = model.load(PROJECT)
    reference = read_wav(MIXDOWN).samples
    stems, _ = render.render_pad_stems(proj, 0, render.RenderParams(gate="none"))
    stems = master.apply_to_stems(stems, master.CompressorParams())
    n = min(min(v.shape[0] for v in stems.values()), len(reference))
    stems = {p: v[:n] for p, v in stems.items()}
    refined = separate.refine(stems, reference)

    # kick and bass are reproduced almost exactly, so they must survive
    for pad in (0, 9):
        corr = np.corrcoef(refined[pad][:n, 0], stems[pad][:n, 0])[0, 1]
        assert corr > 0.9, (pad, corr)


def test_filter_enum_matches_firmware():
    """The filterType enum is recovered from the firmware (docs/FINDINGS.md 69)."""
    from mpc2stems import render

    table = dict(render.RenderParams().filter_types)
    # Names and values come from the firmware's own table (0x45bb4b0) and the
    # immediates in its name lookup (0xbb1700): 1 = Lowpass 1 Pole, 2 = Lowpass
    # 2 Pole, 3 = Lowpass 4 Pole ... 29 = MPC3000 LPF (FINDINGS 80).
    assert table[1] == ("lp", 1)    # Lowpass 1 Pole
    assert table[2] == ("lp", 2)    # Lowpass 2 Pole
    assert table[3] == ("lp", 4)    # Lowpass 4 Pole
    assert table[7] == ("hp", 2)    # Highpass 2 Pole
    assert table[11] == ("bp", 2)   # Bandpass 2 Pole
    assert table[15] == ("bs", 2)   # Bandstop 2 Pole
    assert table[19] == ("bb", 2)   # Bandboost 2 Pole
    # 29 is the MPC3000 emulation.  Against the pad-16 capture and the mixdown it
    # behaves as 2-pole (5.7 dB spread / 0.17839), not 4-pole (7.7 dB / 0.18112).
    assert table[29] == ("lp", 2)
    # Vocal/Model are character filters; they stay unmodelled and are reported.
    assert 26 not in table and 23 not in table
    # The device builds biquads with juce::dsp::IIR::Coefficients; its
    # 2-argument form hardcodes Q = 1/sqrt2, and the fitted optimum agrees
    # (FINDINGS 73).  The cutoff scale stays at the natural 1.0 because the
    # measurement cannot distinguish it from 1.75.
    params = render.RenderParams()
    assert abs(params.filter_q_base - 0.70710678) < 1e-8
    assert params.filter_q_scale == 0.0
    assert params.filter_cutoff_scale == 1.0


# --------------------------------------------------------------------------
# the stretcher must not lose level (FINDINGS 72)
# --------------------------------------------------------------------------
def test_time_stretch_preserves_power():
    """Overlap-add with rewritten phases loses energy; it must be restored.

    Measured -3.4 dB on broadband material and -2.7…-4.4 dB on this project's
    stretched pads, which is audible in a stem.
    """
    from mpc2stems import warp

    rng = np.random.default_rng(0)
    sr = 44100
    signals = {
        "noise": rng.standard_normal((sr, 2)) * 0.1,
        "sine440": np.stack([0.3 * np.sin(2 * np.pi * 440 * np.arange(sr) / sr)] * 2, axis=1),
    }
    for rate in (1.1139, 1.2941, 1.5, 0.8):
        for name, x in signals.items():
            y = warp.time_stretch(x, rate)
            edge = int(0.05 * sr)
            ratio = float(np.mean(y[edge:-edge] ** 2) / np.mean(x[edge:-edge] ** 2))
            assert abs(10 * np.log10(ratio)) < 0.2, (rate, name, ratio)


def test_warp_energy_match_flag_reproduces_the_quieter_output():
    """The flag keeps the old behaviour reachable, and it is quieter."""
    proj = model.load(PROJECT)
    warped = next(i for i in sorted(proj.pads[0]) if proj.pads[0][i].warp_enable)
    on, _ = render.render_pad_stems(
        proj, 0, render.RenderParams(warp_energy_match=True), pad_subset={warped}
    )
    off, _ = render.render_pad_stems(
        proj, 0, render.RenderParams(warp_energy_match=False), pad_subset={warped}
    )
    assert np.sum(on[warped] ** 2) > np.sum(off[warped] ** 2)


# --------------------------------------------------------------------------
# solo projects: per-pad capture on stock firmware (no stem export exists)
# --------------------------------------------------------------------------
def test_xpj_round_trip_preserves_the_document(tmp_path):
    """The writer must produce a file the reader (and the device) accepts."""
    from mpc2stems import xpj

    container = xpj.read_project(PROJECT)
    target = tmp_path / "copy.xpj"
    xpj.write_project(target, container, container.document)
    again = xpj.read_project(target)
    assert again.version == container.version
    assert again.payload_kind == container.payload_kind
    assert again.document["data"]["version"] == container.document["data"]["version"]
    assert len(again.document["data"]["tracks"]) == len(container.document["data"]["tracks"])


def test_solo_projects_isolate_one_pad_each(tmp_path):
    """`solo-projects` must leave exactly one pad audible per variant, and the
    variant must render identically to that pad in the full project."""
    import argparse

    from mpc2stems import cli

    args = argparse.Namespace(project=PROJECT, out=tmp_path / "solo")
    assert cli.cmd_solo_projects(args) == 0

    variants = sorted(args.out.glob("*.xpj"))
    playing = sorted({e[1] for e in render.note_events(model.load(PROJECT))})
    assert len(variants) == len(playing)

    for variant in variants:
        pad = int(variant.name.split("-")[1])
        solo = model.load(variant)
        populated = [i for i, p in solo.pads[0].items() if any(l.sample_file for l in p.layers)]
        assert populated == [pad], (variant.name, populated)
        assert len(solo.data["data"]["samples"] if "data" in solo.data else solo.data["samples"]) == 1

        full_stems, _ = render.render_pad_stems(model.load(PROJECT), 0, pad_subset={pad})
        solo_stems, _ = render.render_pad_stems(solo, 0, pad_subset={pad})
        n = min(len(full_stems[pad]), len(solo_stems[pad]))
        assert np.allclose(full_stems[pad][:n], solo_stems[pad][:n]), variant.name


def test_info_survives_a_silenced_pad(tmp_path, capsys):
    """A solo project still carries notes for pads it silenced; `info` must not
    crash on them (it used to index layers[0] unconditionally)."""
    import argparse

    from mpc2stems import cli

    args = argparse.Namespace(project=PROJECT, out=tmp_path / "solo")
    cli.cmd_solo_projects(args)
    variant = sorted((tmp_path / "solo").glob("*.xpj"))[0]
    assert cli.cmd_info(argparse.Namespace(project=variant)) == 0
    out = capsys.readouterr().out
    assert "(no sample)" in out


# --------------------------------------------------------------------------
# fidelity mode and the audit of unmodelled inserts (FINDINGS 77/78)
# --------------------------------------------------------------------------
def test_fidelity_mode_does_not_gate_stretched_pads():
    """The gate is worth ~1.7 dB of the residual but costs pad 10 21 dB of level.

    `fidelity=True` must give back the audio: the same pad must render far louder
    than the shipped configuration, and the difference must be the gate rather
    than anything else.
    """
    proj = model.load(PROJECT)
    shipped, _ = render.render_pad_stems(
        proj, 0, params=render.RenderParams(), pad_subset={10}
    )
    honest, _ = render.render_pad_stems(
        proj, 0, params=render.RenderParams(fidelity=True), pad_subset={10}
    )
    shipped_rms = float(np.sqrt((shipped[10] ** 2).mean()))
    honest_rms = float(np.sqrt((honest[10] ** 2).mean()))
    gain_db = 20 * np.log10(honest_rms / shipped_rms)
    assert 25.0 < gain_db < 35.0, gain_db


def test_audit_reports_unmodelled_master_inserts():
    """Outputs 1/2 carries Pad-Controlled FXs, Flex Beat and Color Compressor.

    Only the compressor is modelled, so the other two must be reported — an
    unnoticed insert would silently invalidate a conversion.
    """
    warnings = audit.unmodelled_features(model.load(PROJECT))
    assert any("Pad-Controlled FXs" in w for w in warnings), warnings


def test_pole_count_changes_the_filter():
    """6- and 8-pole types used to be silently identical to 4-pole.

    `apply_filter` stacked the same biquad exactly once, so every type above
    2 poles produced the same 4-pole response — the wrong slope with no warning.
    """
    from mpc2stems import filters

    x = np.random.default_rng(7).standard_normal((4096, 1))
    rms = {
        p: float(np.sqrt((filters.apply_filter(x, "lp", p, 800.0, 0.707, 44100) ** 2).mean()))
        for p in (1, 2, 4, 8)
    }
    assert rms[1] > rms[2] > rms[4], rms          # more poles, more attenuation
    assert rms[8] != rms[4], "8-pole must not equal 4-pole"


# --------------------------------------------------------------------------
# filter coefficients must be the device's own forms (FINDINGS 81)
# --------------------------------------------------------------------------
def test_lowpass_coefficients_match_the_decoded_firmware_formula():
    """0xfcc950 builds makeLowPass(sr, f): b = {c1, 2c1, c1}, a1/a2 from n.

    With Q = 1/sqrt2 the firmware's n/Q term is sqrt(2)*n, and the closed form is
    exactly what the disassembly computes.  If anyone swaps the coefficient maths
    (RBJ, a shelf, a different normalisation) this fails.
    """
    from mpc2stems import filters

    fc, sr = 1000.0, 44100
    b0, b1, b2, a1, a2 = filters.juce_coefficients("lp", fc, 1 / np.sqrt(2), sr)
    n = 1 / np.tan(np.pi * fc / sr)
    c1 = 1 / (1 + np.sqrt(2) * n + n * n)
    assert np.isclose(b0, c1, rtol=1e-6)
    assert np.isclose(b1, 2 * c1, rtol=1e-6)
    assert np.isclose(b2, c1, rtol=1e-6)
    assert np.isclose(a1, 2 * c1 * (1 - n * n), rtol=1e-6)
    assert np.isclose(a2, c1 * (1 - np.sqrt(2) * n + n * n), rtol=1e-6)


def _response_db(kind, fc=1000.0, q=1.0, sr=44100):
    from mpc2stems import filters

    b0, b1, b2, a1, a2 = filters.juce_coefficients(kind, fc, q, sr)
    f = np.geomspace(20, 20000, 6000)
    z = np.exp(-1j * 2 * np.pi * f / sr)
    h = (b0 + b1 * z + b2 * z * z) / (1 + a1 * z + a2 * z * z)
    return f, 20 * np.log10(np.abs(h) + 1e-15)


def test_every_filter_kind_has_the_response_its_name_promises():
    """A high-pass must not lose its passband, a notch must null, a peak must lift.

    An earlier version negated b1 without rescaling, which left the high-pass
    passband 46 dB down, and used a low-pass normalisation for the notch.
    """
    for kind, q, centre, away in (
        ("lp", 1 / np.sqrt(2), -3.0, 0.0),
        ("hp", 1 / np.sqrt(2), -3.0, 0.0),
        ("bp", 1 / np.sqrt(2), 0.0, None),
        ("bs", 1.0, -60.0, 0.0),
    ):
        f, db = _response_db(kind, 1000.0, q)
        assert abs(db[np.argmin(abs(f - 1000.0))] - centre) < 3.0, (kind, db[np.argmin(abs(f - 1000.0))])
        if away is not None:
            edge = db[0] if abs(db[0]) < abs(db[-1]) else db[-1]
            assert abs(edge - away) < 1.0, (kind, edge)


def test_all_29_filter_types_are_either_modelled_or_reported():
    """No filter type may be silently mis-rendered.

    The enum has 29 entries (FINDINGS 80).  Each must either have a mapping or be
    reported by the audit when a pad actually engages it.
    """
    from mpc2stems import filters

    assert set(filters.FILTER_ENUM) == set(range(1, 30))
    for value, (name, kind, poles) in filters.FILTER_ENUM.items():
        if kind is None:
            assert name in {"Model1", "Model2", "Model3", "Vocal1", "Vocal2", "Vocal3"}, name
        else:
            assert kind in {"lp", "hp", "bp", "bs", "bb"}, (value, kind)
            assert poles in {1, 2, 4, 6, 8}, (value, poles)


def test_svf_matches_the_device_topology():
    """The pad filter is a Chamberlin SVF, not a biquad (FINDINGS 85).

    The firmware's FilterBlock processor (`0x274e720`) runs, per channel:

        low += f*band;  high = x - low - qd*band;  band += f*high

    and the filter type only selects which state is emitted.  A biquad cannot
    reproduce the state-dependent behaviour, and the taps must differ from each
    other the way a state-variable filter's do.
    """
    from mpc2stems import filters

    rng = np.random.default_rng(11)
    x = rng.standard_normal((8192, 1)).astype(np.float32)
    taps = {k: filters.svf_states(x, k, 1000.0, 0.707, 44100) for k in ("lp", "hp", "bp", "bs")}
    for k, y in taps.items():
        assert np.isfinite(y).all(), k
    # low-pass rejects highs, high-pass rejects lows, and the taps are distinct
    def band_energy(y, lo, hi):
        f = np.fft.rfftfreq(len(y), 1 / 44100)
        sp = np.abs(np.fft.rfft(y.mean(axis=1))) ** 2
        return float(sp[(f >= lo) & (f < hi)].sum())

    assert band_energy(taps["lp"], 100, 300) > band_energy(taps["lp"], 8000, 16000)
    assert band_energy(taps["hp"], 8000, 16000) > band_energy(taps["hp"], 100, 300)
    assert not np.allclose(taps["lp"], taps["bp"])
    # default renderer uses the device topology
    assert render.RenderParams().filter_topology == "svf"
