# minimod in Rust

The firmware mod, rewritten in Rust:

* [`mpc-sample-sdk`](https://github.com/ioma8/mpc-sample-sdk) — the universal mod SDK: firmware image and verification,
  the hook engine and all 17 sites, the `Mod` trait and event decoding, the ALSA
  taps, drawing, the generic `Task<T>` worker and `RenderJob`, and typed access
  to the app's page, sample, pad, slice, WAV, control-surface, effect and
  Extract objects.
* `crates/minimod` — the features built only on the SDK: the SAMPLE OPTIONS menu,
  AutoTrim, HPSS, Spectral Chop, the effects and bass synth, 16 Levels, MIDI
  chromatic playback and the Live Looper, with the DSP adapters grouped under
  `backend/`.  It implements `Mod` and emits `export_mod!(Minimod)`;
  `mod/build.sh` builds it as a `cdylib`.
* `crates/dsp` — audio DSP (looper, HPSS, onset detection, filters, synths). Pure
  computation over slices, no firmware addresses, host-testable.

The feature crate is safe Rust except for small, documented `unsafe` blocks at
firmware boundaries (drawing, raw object access, Extract and the repaint pump),
as required by the SDK's explicit safety contracts. The workspace pins
`mpc-sample-sdk` tag `v0.2.1` for the explicit safety contracts.

The hands-on SDK guide is
[`mpc-sample-sdk/docs/README.md`](https://github.com/ioma8/mpc-sample-sdk/blob/main/docs/README.md).

```sh
./check.sh                                  # fmt, clippy, tests, cross build
cargo test --workspace                      # host tests and golden fixtures
```

Status, the lint policy and the proof method are in
[`../docs/RUST-PORT.md`](../docs/RUST-PORT.md).
