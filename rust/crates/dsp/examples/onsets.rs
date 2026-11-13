//! Print spectral onset positions for a raw mono f32 file.
use std::io::Read;
fn main() {
    let path = std::env::args().nth(1).expect("raw f32 path");
    let mut bytes = Vec::new();
    std::fs::File::open(&path)
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    let samples: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    let r = minimod_dsp::spectral::detect(&samples, 1, 44100, 16).unwrap();
    println!(
        "frames={} onsets={:?} detected={}",
        samples.len(),
        r.positions,
        r.detected
    );
}
