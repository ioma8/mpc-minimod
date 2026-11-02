# Virtual control-surface card

Pads, knobs and buttons are read by the MCU; the app reaches it over UART, which
the vendor kernel exposes as an ALSA **rawmidi** device. Debian has no
`snd-virmidi` and `snd-aloop` has no MIDI support, so this module provides the
card itself: a `Control Surface` rawmidi plus a duplex PCM, so it can occupy
card 0 (the app opens `hw:0` for audio and expects the surface on the same card).

## Built and loading

`snd-mcuvirt.ko` builds **without kbuild** — Zig cross-compiles it against the
Debian 6.12.107+deb13-arm64 headers and the pieces modpost normally injects are
supplied explicitly (`genver.py`):

- `.modinfo` vermagic `6.12.107+deb13-arm64 SMP mod_unload modversions aarch64`
- `__versions` with all 22 symbol CRCs from `Module.symvers` (this kernel has
  `modversions`, so they are mandatory) including `module_layout`
- `.gnu.linkonce.this_module`

Two non-obvious steps were required:

1. `-T arch/arm64/module.lds` at the link stage — arm64 refuses a module without
   the `.plt` section that script placement produces.
2. `module_layout` must be listed explicitly; it never appears as an undefined
   symbol in the object.

The result loads cleanly in the guest:

```
snd_mcuvirt: loading out-of-tree module taints kernel.
snd-mcuvirt: Control Surface rawmidi registered
 0 [MCU]: MCU - MPC Control Surface
by-path controlC0 midiC0D0 pcmC0D0c pcmC0D0p seq timer
```

## What still does not work

- The app initialises audio on this card (`Audio 44100Hz; 6-in; 6-out`) but then
  reports `Unexpected audio capture error: -5` — the null PCM needs proper
  capture `.copy`/ack semantics (compare `snd-dummy`).
- The app does not yet open `midiC0D0`; the predicate it uses to adopt a
  control-surface device is still unidentified.

## Build

```sh
KH=/path/to/extracted/headers ./build.sh
insmod snd-mcuvirt.ko
```
