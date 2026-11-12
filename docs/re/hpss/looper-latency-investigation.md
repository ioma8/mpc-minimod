# Looper latency investigation

Firmware 1.3.0.12. Addresses below are Ghidra VAs with image base `0x100000`.

The firmware contains a JUCE `ALSAAudioIODevice` implementation. Its vtable at
`0x06e5c5b8` has candidate output/input latency accessors at slots 19/20,
`0x00ef73d0` and `0x00ef73e0`, which return fields `+0x1d4` and `+0x1d8`.
This mapping is consistent with the JUCE 6 `AudioIODevice` virtual method order;
the exact JUCE version and runtime call sites still need verification.

The device open method `0x00eff1c0` calls `0x00efe700` with its embedded
implementation at object `+0x30`. The latter stores playback and capture
values at embedded `+0x1a4` and `+0x1a8`, respectively, after opening ALSA
streams with `0x00efe000`. That ALSA setup reads the hardware period size and
number of periods and sets the stream field at `+0x18` to
`(period_count - 1) * period_size`; stream direction 0 is output, 1 is input.
These fields are configured values, not a direct measurement of codec delay.

The mod intercepts `snd_pcm_readi/readn` and `snd_pcm_writei`, not the JUCE
callback. Consequently these callback-level reported values cannot simply be
added without mapping the hook positions, outstanding blocks, software ring
backlog and timestamps to the same frame timeline. `snd_pcm_avail` is currently
sampled before a potentially blocking capture read, while output delay is a
smoothed earlier value. They are not one synchronized measurement.

The emulator showed 1,024-frame ALSA buffer capacities with 128-frame periods;
one of those figures establishes the instantaneous amount of queued audio. A
single playback status sample reported 281 queued frames. Three emulator-only
output-to-input loopback probes with 2,048-frame compensation recorded the
simulated pulse 388, 393, and 437 frames early. These values include the
emulator's loopback path and do not establish the physical device's offset.
The optional `--measure-loopback-latency` mode of `mod/tests/emulator_looper.py`
repeats the digital emulator probe; it is not a physical-device calibration.
