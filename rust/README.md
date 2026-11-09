# minimod in Rust

The firmware mod, rewritten in Rust:

* [`mpc-sample-sdk`](https://github.com/ioma8/mpc-sample-sdk) — the universal mod SDK: firmware image and verification,
  the hook engine and all 16 sites, the `Mod` trait and event decoding, the ALSA
  taps, drawing, the generic `Task<T>` worker and `RenderJob`, and typed access
  to the app's page, sample, pad, slice, WAV, control-surface, effect and
  Extract objects.
* `crates/minimod` — the features built only on the SDK, in safe Rust (the gate
  rejects any `unsafe` token here): the SAMPLE OPTIONS menu,
  AutoTrim, HPSS, Spectral Chop, the effects and bass synth, 16 Levels, MIDI
  chromatic playback and the Live Looper, with the DSP adapters grouped under
  `backend/`.  It implements `Mod` and emits `export_mod!(Minimod)`;
  `mod/build.sh` builds it as a `cdylib`.
* `crates/dsp` — audio DSP (looper, HPSS, onset detection, filters, synths). Pure
  computation over slices, no firmware addresses, host-testable.

The hands-on SDK guide is
[`mpc-sample-sdk/docs/README.md`](https://github.com/ioma8/mpc-sample-sdk/blob/main/docs/README.md).

```sh
./check.sh                                  # fmt, clippy, tests, cross build
cargo test --workspace                      # host tests and golden fixtures
```

Status, the lint policy and the proof method are in
[`../docs/RUST-PORT.md`](../docs/RUST-PORT.md).
