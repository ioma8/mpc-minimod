---
name: MINIMOD
description: Familiar MPC Sample interaction, made clearer across device and promotional media.
colors:
  device-background: "#0c0c10"
  device-panel: "#101014"
  device-bar: "#15151a"
  amber: "#ffb43c"
  selection: "#3a2a10"
  white: "#ffffff"
  muted: "#b8b8be"
  track: "#2a2a32"
  playing: "#54d06a"
  recording: "#ff4d4d"
  promo-background: "#090b10"
  promo-amber: "#ffb941"
typography:
  promo-display:
    fontFamily: "Arial Black, Arial, sans-serif"
    fontWeight: 900
  promo-body:
    fontFamily: "Arial, sans-serif"
    fontWeight: 700
spacing:
  device-inset: "14px"
  device-row: "20px"
  video-inset: "75px"
---

## Overview

**Creative North Star: Familiar instrument, clearer flow.** MINIMOD should look at home on the MPC Sample: dark, compact, direct, and focused on the sound. Promos use the same amber and near-black foundation, with a larger type scale and more space for a phone screen. The hardware and the work remain the heroes.

These tokens document existing device colors and promo assets; use them as the starting point for new work. The device and a 1080 × 1920 video are different surfaces: do not transfer pixel sizes between them.

## Colors

- **Amber** identifies the active control, useful emphasis, and the short accent rule. The slightly different promo amber is the observed value in the current video assets; it is not a second feature color.
- Use **device background**, **panel**, and **bar** as flat dark layers; use white for primary content and muted gray for secondary information. Selection uses the dark amber-brown row behind white text.
- In the looper, green means playing and red means recording; amber identifies overdubbing. Keep the text labels (`PLAY`, `REC`, `DUB`, `STOP`) as well as color. Screen colors and physical pad LEDs have distinct values: follow the implementation when editing either.
- Promo feature colors seen in older overview cards are optional, not a replacement for the common amber identity. Limit a piece to one accent plus any meaningful device-state colors.

## Typography

- **Device:** use the existing firmware/SDK text renderer and its established compact character. Do not assume the firmware's font is Arial or introduce a new typeface without a device-specific reason. Existing menu titles use an 18 px renderer size; rows use 12 px and help text 10 px. These are observed values, not minimums for every new design.
- **Promos:** use Arial Black for large, short headlines and Arial Bold for supporting copy, with the fallbacks in the tokens. These are the fonts used in the existing generated promo cards.
- Prefer one clear idea per caption. Use uppercase for short headings and control names; use normal sentence case where it improves reading. Spell physical controls as printed: `SHIFT`, `DATA`, `K1`, `K2`, `K3`, `+`, `−`.
- Make promotional copy legible at phone size. Existing 1080 × 1920 videos use headlines around 80–110 px and supporting lines around 48–70 px; fit the words to the safe area rather than forcing a fixed size.

## Layout

- **On device:** design for the actual 320 × 240 screen. Preserve its hierarchy, physical-control mapping, and readable feedback. The existing menu uses a 14 px panel inset, 20 px rows, and a compact control hint at the bottom. Keep layout changes compatible with stock screens rather than importing a phone or desktop card layout.
- **Vertical video:** start from 1080 × 1920 at 30 fps. Existing promos use a 75 px left inset and reserve the top, right, and bottom for platform overlays. Treat that inset as a starting point, not a guarantee: check the actual crop and platform UI for each upload. Put essential copy toward the center-left, away from the right-side controls and bottom captions.
- Lead with MINIMOD and MPC Sample when context is missing. Show one feature, its actual action, and the result. Use a brief closing repository callout when useful; avoid making it the only way to identify the project.
- Make device imagery large enough to understand. Preserve the full screen when its content matters; crop camera footage around the device and hands without hiding the action. Distinguish emulator captures from physical-hardware footage.
- **Motion and sound:** use restrained motion that helps a viewer follow the action. Let real device or emulator audio be heard; avoid music or effects that obscure it. Cuts should follow the demonstration and leave enough reading time.

## Elevation & Depth

Use flat, solid surfaces and small tonal changes. Separate content with spacing, thin rules, or an active-row fill; shadows and decorative gradients are not needed to establish hierarchy. Hardware photography may retain its natural light and texture.

## Shapes

Use simple rectangles, thin amber rules, and the existing 4 × 4 pad grid where it communicates the device or looper state. On the device, retain the firmware's compact rectangular geometry. In promos, small rounded ends on an accent rule are fine; avoid adding heavy frames around footage or doubling a screenshot's existing border.

## Components

- **Device menu:** centered title, vertically spaced options, one filled selected row, and a concise DATA control hint. Preserve the source geometry and colors in [menu.rs](rust/crates/minimod/src/menu.rs).
- **Looper pad cell:** a 4 × 4 grid with a state label, state color, and relevant meter or timing information. Never make color the sole indication of state.
- **Promo card:** one short headline, one dominant device image or actual screen capture, and at most a brief supporting line. The existing videos use a small amber rule as a recurring cue.
- **Repository callout:** show `github.com/ioma8/mpc-minimod` legibly, preferably broken across two lines at the slash when needed. Shorts descriptions do not make external URLs clickable; use the channel profile link or a related video where appropriate.

## Do's and Don'ts

### Do

- Keep the original MPC Sample look and control language recognizable.
- Verify feature behavior, shortcut names, and hardware/emulator claims against the current code and documentation.
- Check exported frames at phone size and listen to the actual mix; test the device UI on its target screen or an accurate emulator.
- Give labels and state changes enough contrast and distinguish status with words as well as color.

### Don't

- Do not crowd a small screen or a Short with multiple competing messages.
- Do not imply a control action occurred if the footage only describes it.
- Do not replace the actual interface or sound with a fabricated demonstration.
- Do not use an octave claim for an eight-semitone shift; state the real increment.
