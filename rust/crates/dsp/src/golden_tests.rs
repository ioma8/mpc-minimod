//! Golden-fixture tests against the original C implementation.
//!
//! `tests/golden/generate.sh` links the shipped C DSP and records what it
//! produces for a fixed input; these tests replay the same input through the
//! Rust port and compare.  A behavioural difference fails here, on the host,
//! instead of on the device.

use core::mem::size_of;

use crate::hpss;
use crate::looper::{Engine, View};
use crate::spectral;

const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/golden/");

/// Read a little-endian f32 fixture.
fn floats(name: &str) -> Vec<f32> {
    let bytes = std::fs::read(format!("{GOLDEN}{name}")).expect("fixture exists");
    bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

/// Read a little-endian i32 fixture.
fn integers(name: &str) -> Vec<i32> {
    let bytes = std::fs::read(format!("{GOLDEN}{name}")).expect("fixture exists");
    bytes
        .chunks_exact(4)
        .map(|chunk| i32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

/// Read one little-endian u32 from a fixture.
fn u32_at(name: &str, index: usize) -> u32 {
    integers(name)[index].cast_unsigned()
}

/// Largest absolute difference and its RMS, for a readable failure message.
fn difference(got: &[f32], want: &[f32]) -> (f32, f32) {
    assert_eq!(got.len(), want.len(), "length");
    let mut peak = 0.0_f32;
    let mut energy = 0.0_f64;
    for (a, b) in got.iter().zip(want.iter()) {
        let delta = (a - b).abs();
        peak = peak.max(delta);
        energy += f64::from(delta) * f64::from(delta);
    }
    let rms = (energy / got.len() as f64).sqrt() as f32;
    (peak, rms)
}

#[test]
fn hpss_matches_the_reference() {
    let input = floats("hpss_input.bin");
    let (harmonic, percussive) = hpss::split(&input, 2, 44_100).expect("valid input");
    for (got, name) in [
        (&harmonic, "hpss_harmonic.bin"),
        (&percussive, "hpss_percussive.bin"),
    ] {
        let want = floats(name);
        let (peak, rms) = difference(got, &want);
        assert!(peak < 1e-4, "{name}: peak difference {peak} (rms {rms})");
        assert!(rms < 1e-6, "{name}: rms difference {rms}");
    }
    // The split must actually separate energy, not return two copies of the
    // input: the percussive part has to keep the noise bursts and the harmonic
    // part the sustained tone.
    let input_energy: f32 = input.iter().map(|value| value * value).sum();
    let harmonic_energy: f32 = harmonic.iter().map(|value| value * value).sum();
    let percussive_energy: f32 = percussive.iter().map(|value| value * value).sum();
    let split_energy = harmonic_energy + percussive_energy;
    assert!(
        (split_energy - input_energy).abs() / input_energy < 0.05,
        "split conserves energy: {split_energy} vs {input_energy}"
    );
    assert!(
        harmonic_energy > 0.2 * input_energy,
        "harmonic part carries the tone"
    );
    assert!(
        percussive_energy > 0.02 * input_energy,
        "percussive part carries the bursts"
    );
}

#[test]
fn spectral_onsets_match_the_reference() {
    let input = floats("spectral_input.bin");
    let detections = spectral::detect(&input, 2, 44_100, 16).expect("valid input");
    let want_detected = u32_at("spectral_onsets.bin", 1);
    let want_count = u32_at("spectral_onsets.bin", 0);
    let want_positions: Vec<u32> = integers("spectral_positions.bin")
        .into_iter()
        .take(want_count as usize)
        .map(i32::cast_unsigned)
        .collect();
    assert_eq!(detections.detected, want_detected, "candidate onsets");
    assert_eq!(detections.positions, want_positions, "slice positions");
    assert!(!want_positions.is_empty(), "the fixture has slices");
    assert_eq!(want_positions[0], 0, "the first slice starts at zero");
    assert!(
        want_positions.windows(2).all(|pair| pair[0] < pair[1]),
        "ascending"
    );
}

/// Replay the exact performance the golden generator recorded.
fn replay_looper_script(input: &[i32]) -> (Vec<i32>, View) {
    let mut output: Vec<i32> = Vec::new();
    let mut position = 0_usize;
    let render = |count: usize, position: &mut usize, output: &mut Vec<i32>| {
        let mut done = 0;
        while done < count {
            let block = (count - done).min(512);
            let start = *position * 2;
            Engine::input_s32_interleaved(&input[start..start + block * 2], 2);
            let mut buffer = vec![0_i32; block * 2];
            Engine::render_s32(&mut buffer, 2);
            output.extend_from_slice(&buffer);
            *position += block;
            done += block;
        }
    };

    Engine::set_latency(2048, crate::looper::LatencySource::Geometry);
    assert!(Engine::open(), "the pool maps");
    render(4410, &mut position, &mut output);

    /* Record one second on pad 1, close it, drain, then play. */
    assert!(Engine::pad(0, false, false));
    render(44_100, &mut position, &mut output);
    assert!(Engine::pad(0, false, false));
    render(8192, &mut position, &mut output);
    assert!(Engine::pad(0, false, false));
    render(1024, &mut position, &mut output);
    assert!(Engine::pad(0, false, false));
    render(4096, &mut position, &mut output);

    /* Overdub, then close back into playback. */
    assert!(Engine::pad(0, true, false));
    render(4096, &mut position, &mut output);
    assert!(Engine::pad(0, true, false));
    render(8192, &mut position, &mut output);

    /* Freeze the sync pulse on the recorded length and record a second loop,
     * which must be quantised to whole pulses. */
    assert!(Engine::button(0x4a, true, false, false));
    render(1024, &mut position, &mut output);
    assert!(Engine::pad(1, false, false));
    render(30_000, &mut position, &mut output);
    assert!(Engine::pad(1, false, false));
    render(8192, &mut position, &mut output);

    /* Stop everything, then play both loops at half master volume. */
    assert!(Engine::button(0x51, true, false, false));
    render(1024, &mut position, &mut output);
    Engine::set_master(64);
    assert!(Engine::pad(0, false, false));
    render(4096, &mut position, &mut output);
    assert!(Engine::pad(1, false, false));
    render(4096, &mut position, &mut output);

    /* Clear the unused pad, snapshot while the view is still open, then close. */
    assert!(Engine::pad(2, false, true));
    let mut view = View::default();
    assert!(Engine::snapshot(&mut view));
    Engine::close();
    render(2048, &mut position, &mut output);
    assert_eq!(position * 2, output.len());
    (output, view)
}

#[test]
fn looper_trace_matches_the_reference() {
    crate::looper::reset_for_tests();
    let input = integers("looper_input.bin");
    let want = integers("looper_output.bin");
    let frames = usize::try_from(integers("looper_frames.bin")[0]).unwrap_or(0);
    assert_eq!(input.len(), frames * 2);
    assert_eq!(want.len(), frames * 2);

    let (output, view) = replay_looper_script(&input);
    assert_eq!(output.len(), want.len());

    for (index, name) in [(0, "looper_pad0.bin"), (1, "looper_pad1.bin")] {
        let got = crate::looper::pad_samples_for_tests(index, 16384);
        let want = floats(name);
        let (peak, rms) = difference(&got, &want);
        assert!(
            peak < 1e-6,
            "pad {index} buffer differs: peak {peak} rms {rms}"
        );
    }

    let mut mismatches = 0_usize;
    let mut first = None;
    for (index, (got, expected)) in output.iter().zip(want.iter()).enumerate() {
        if got != expected {
            mismatches += 1;
            first.get_or_insert((index, *got, *expected));
        }
    }
    assert_eq!(
        mismatches, 0,
        "looper output differs in {mismatches} samples, first at {first:?}"
    );

    let fixture = std::fs::read(format!("{GOLDEN}looper_view.bin")).expect("view fixture");
    assert_eq!(fixture.len(), size_of::<View>(), "view size");
    assert_eq!(view.open, 1);
    assert_eq!(view.sync, 1);
    assert_eq!(view.click, 0);
    assert_eq!(view.sync_frames, 44_100);
    assert_eq!(view.lat_frames, 2048);
    assert_eq!(view.lat_src, b'B');
    assert_eq!(view.recording_pad, -1);
    assert_eq!(view.pads[0].state, 2);
    assert_eq!(view.pads[0].len_frames, 44_100);
    assert_eq!(view.pads[0].rec_frames, 44_100);
    assert_eq!(view.pads[1].state, 2);
    assert_eq!(view.pads[1].len_frames, 44_100);
    assert_eq!(view.pads[1].rec_frames, 30_000);
    assert_eq!(view.pads[2].state, 0);
    assert_eq!(view.pads[2].len_frames, 0);
}

#[test]
fn bass_matches_the_reference() {
    // The reference was compiled with its `rand()` replaced by the same
    // deterministic generator `SequenceNoise` implements, so the whole voice
    // chain is comparable sample for sample.
    let want = floats("bass_audio.bin");
    let frames = usize::try_from(integers("bass_frames.bin")[0]).unwrap_or(0);
    let got = crate::bass::render_with(
        44_100,
        0.55,
        0.35,
        0.62,
        &mut crate::dsp::SequenceNoise::new(1),
    )
    .expect("renders");
    assert_eq!(got.frames, frames);
    assert_eq!(got.audio.len(), want.len());
    let mut mismatches = 0_usize;
    let mut first = None;
    for (index, (value, expected)) in got.audio.iter().zip(want.iter()).enumerate() {
        if value.to_bits() != expected.to_bits() {
            mismatches += 1;
            first.get_or_insert((index / 2, index % 2, *value, *expected));
        }
    }
    let max = got
        .audio
        .iter()
        .zip(want.iter())
        .fold(0.0_f32, |peak, (a, b)| peak.max((a - b).abs()));
    assert_eq!(
        mismatches, 0,
        "bass output differs in {mismatches} samples, first at {first:?}, max {max:e}"
    );
}

#[test]
fn looper_view_matches_the_c_layout() {
    use core::mem::{offset_of, size_of};
    // The paint hook in the firmware glue reads this struct through the C ABI,
    // and the emulator test reads the pad array through `g_pad_stride`.
    assert_eq!(size_of::<crate::looper::Pad>(), 40);
    assert_eq!(size_of::<View>(), 332);
    assert_eq!(offset_of!(View, sync_frames), 12);
    assert_eq!(offset_of!(View, pulse_phase), 16);
    assert_eq!(offset_of!(View, recording_pad), 24);
    assert_eq!(offset_of!(View, lat_src), 32);
    assert_eq!(offset_of!(View, message), 33);
    assert_eq!(offset_of!(View, pads), 76);
    assert_eq!(size_of::<crate::looper::PadView>(), 16);
}

#[test]
fn convert_matches_the_reference() {
    let pairs = integers("convert_pairs.bin");
    let want = floats("convert_out.bin");
    for (i, &expected) in want.iter().enumerate() {
        let got = crate::sample::s32_pair_to_f32(pairs[i * 2], pairs[i * 2 + 1]);
        assert_eq!(
            got.to_bits(),
            expected.to_bits(),
            "pair {i}: {:?} vs {:?}",
            pairs[i * 2],
            pairs[i * 2 + 1]
        );
    }
}
#[test]
fn probe_synth_chain() {
    use crate::dsp::SequenceNoise;
    use crate::dsp::filters::Svf;
    use crate::dsp::string_voice::StringVoice;
    let mut noise = SequenceNoise::new(1);
    let harmonics = 0.55_f32;
    let drive = 0.35_f32;
    let cutoff = 0.62_f32;
    let mut voice = StringVoice::new(44_100.0);
    voice.set_sustain(false);
    voice.set_frequency(65.406_39);
    voice.set_accent(0.65 + drive * 0.35);
    voice.set_structure(0.25 + drive * 0.6);
    voice.set_brightness(0.05 + harmonics * 0.9);
    voice.set_damping(0.35 + (1.0 - harmonics) * 0.55);
    voice.trigger();
    let mut filter = Svf::new(44_100.0);
    let ch = 700.0 * 24.0_f32.powf(cutoff);
    filter.set_frequency(if ch < 44_100.0 * 0.45 {
        ch
    } else {
        44_100.0 * 0.45
    });
    filter.set_resonance(0.05);
    filter.set_drive(drive * 0.35);
    for i in 0..5 {
        let s = voice.process(i == 0, &mut noise);
        filter.process(s);
        println!(
            "i={i} voice={:08x} low={:08x} aux={:08x}",
            s.to_bits(),
            filter.low_pass().to_bits(),
            voice.aux().to_bits()
        );
    }
}
