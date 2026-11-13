# Product

<!-- impeccable:product-schema 1 -->

## Platform

Embedded interface on the Akai MPC Sample, with supporting documentation and promotional media. The device interface runs at 320 × 240; it is not a web, iOS, or Android app.

## Users

MPC Sample owners who want to sample, make beats, and experiment on the hardware without losing its familiar workflow. Contributors and curious users also encounter MINIMOD through its repository, documentation, and short videos.

## Product Purpose

MINIMOD adds focused creative and workflow improvements to the stock MPC Sample experience. Success means a useful feature is easy to discover, understand, and use without making the instrument feel like a different product.

## Positioning

A free, open-source mod built around small, purposeful enhancements rather than a complete replacement interface. It is designed for MPC Sample firmware 1.3.0.12; see the repository README for current compatibility and installation details.

## Operating Context

The primary interaction is a small hardware screen, pads, buttons, three knobs, and the DATA encoder. People may use it while making music and need immediate feedback about selection, state, and sound. Promos are often viewed vertically on a phone, sometimes with app controls covering the edges.

## Capabilities and Constraints

- Preserve the stock MPC Sample interaction model and use established control names and actual behavior.
- Features include AutoTrim, 16 Levels pitch shifting, sample transforms, synths, and a live looper. Consult [README.md](README.md) and [docs/FEATURES.md](docs/FEATURES.md) for current behavior and per-feature verification.
- Do not present emulator-verified features as hardware-verified. The synth recording in the promo assets was captured from the emulator.
- A 16 Levels shift is eight semitones per press; do not describe an individual press as a 12-semitone octave.

## Brand Commitments

- Name: **MINIMOD**; identify it as a mod for **MPC Sample** when context is not already clear.
- Retain the familiar amber-on-dark MPC Sample screen as the foundation across the device UI and promotions, adapted to each surface.
- Keep interfaces and promotional content simple, approachable, and engaging without clutter.

## Evidence on Hand

- Existing device UI: [menu](docs/images/menu.png), [synth](docs/images/synth.png), [looper](docs/images/looper.png), and [AutoTrim](docs/images/autotrim.png).
- Device rendering: [menu.rs](rust/crates/minimod/src/menu.rs) and [looper_view.rs](rust/crates/minimod/src/looper_view.rs).
- Existing FilmCraft work and capture workflow: `/Users/jakubkolcar/Downloads/minimod-promo/VIDEO-WORKFLOW-AND-YOUTUBE.md`.
- Product photo supplied by the owner: `/Users/jakubkolcar/Downloads/mpcsample.jpg`. Usage rights beyond this supplied project are not established here.

## Product Principles

1. Keep the original instrument recognizable.
2. Explain the actual action and its result in plain language.
3. Show genuine interface behavior and sound; label emulator footage accurately.
4. Make the next action obvious without crowding the screen.
