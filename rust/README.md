# minimod in Rust

The firmware mod, rewritten in Rust:

* `crates/dsp` — audio DSP (looper, HPSS, onset detection, filters, synths). Pure
  computation over slices, no firmware addresses, host-testable.
* `crates/minimod` — the `LD_PRELOAD` library itself: hook engine, entry points the app
  calls, feature glue. `mod/build.sh` builds it as a `cdylib`; no C is involved.

```sh
./check.sh                                  # fmt, clippy, tests, cross build
cargo test --workspace                      # host tests and golden fixtures
```

Status, the lint policy and the proof method are in
[`../docs/RUST-PORT.md`](../docs/RUST-PORT.md).
