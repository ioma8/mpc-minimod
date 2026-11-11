# Synth pad creation

## Emulator result

The empty-project synth regression passed with the SDK 0.4.0 development
revision after correcting the native seed holder address. The test confirmed the
created sample's active layer, audio geometry and pitch, with no app restart.

## Behavior

Minimod renders the selected synth onto the currently selected pad, including
when the project has no populated pads. It uses the stock Extract workflow to
register and assign the generated sample. Extract stores the rendered audio as
a WAV in the project data, so this workflow creates a normal, persistent MPC
sample.

After Extract assigns the sample, minimod enables the destination
`InstrumentLayer` through `mpc_sample_sdk::platform::pads::set_layer_active`.
The SDK helper is provided by the SDK revision referenced in
[`rust/Cargo.toml`](../rust/Cargo.toml). The feature flow is in
[`jobflow.rs`](../rust/crates/minimod/src/jobflow.rs).

The SDK temporarily borrows a real native seed immediately around Extract.
It restores that layer afterward and removes only newly created provider copies,
preserving pads that were populated before the render. Extract receives the rendered buffer, selected destination and
rendered frame count.

## Why the pad was silent

Extract created a valid sample and waveform but left the live destination layer
inactive. The WAV was stereo, 33,075 frames at 44.1 kHz, with a 69 Hz peak; pad
PCM stayed zero until the layer was enabled.

Ghidra showed two distinct representations. The project serializer
`FUN_0157f350` handles the serialized model's `active` field at layer element
`+0x28`. The live pad object is an `InstrumentLayer`; its vtable serializer
`FUN_029159a0` emits `Active` from object offset `+0xb18`. The SDK uses the live
object path and verifies the byte after writing it. Details and every tested
hypothesis are in [`HYPOTHESES.csv`](../HYPOTHESES.csv).

## Verification

On MPC Sample firmware 1.3.0.12, the remote SDK build passed a zero-sample
project render on A01 and A05:

- Both renders produced stereo 33,075-frame output at 44.1 kHz with a 69 Hz
  peak and no app restart.
- A01's live active byte was `1`. Pressing it produced PCM RMS 495.95, peak
  3868 and 42,596 nonzero samples. After saving and reloading, it remained
  active and produced PCM RMS 495.99, peak 3868 and 43,090 nonzero samples.
- A05's live active byte was `1`; pressing it produced PCM RMS 1634.62, peak
  12717 and 43,202 nonzero samples. Only A05 was populated in that test.
- The emulator is currently left with the saved A01 project loaded and audible.

The empty-project fixture and sample-content regression can be run with:

```sh
python3 mod/tests/make_empty_project.py SOURCE.xpj /tmp/empty-debug.xpj
# Load the fixture into the running emulator, then:
python3 mod/tests/emulator_synth_empty.py --lab /path/to/virtual-mpc-sample --pad 1
```

The regression checks that the destination layer is active and checks the generated
sample's geometry, waveform and pitch.
Live pad PCM and save/reload behavior were additionally measured through the
emulator's PCM stream and the process memory reader.

## Address notes

Firmware: MPC Sample 1.3.0.12. Ghidra VA equals the firmware mod offset plus
`0x100000`. Extract's destination is patched at `command + 0x58` before execute;
the SDK owns that patch and restores it afterward. The active-layer API owns
`InstrumentLayer::active` at `+0xb18`.

The recording constructor `FUN_01ef4ac0` and its sample-pool registration chain
were explored as an alternative. They are not used by the synth pad-creation
path. The historical call traces remain in [`RE-RECORDING.md`](RE-RECORDING.md).
