# Firmware built-in reverb

> **Historical.** Written while the mod was C. `mod/src/minimod.c` and the other C and C++ sources are
> gone (see git history); the equivalent code is in `rust/crates/`, mapped in the "Final state" section of
> [`RUST-PORT.md`](../RUST-PORT.md). File, line and function references below point at the removed C, and build
> commands that compile it no longer apply (`mod/build.sh` builds the Rust crate).

Firmware analyzed: MPC Sample 1.3.0.12 (`knob.bin`) in Ghidra. Addresses below
are Ghidra VAs; subtract `0x100000` for the mod's `base + offset` form.

## Class and factory

The built-in stereo algorithmic effect is registered under the exact type name
`StereoReverb`. Its RTTI string is `N3Air3H3M12StereoReverbE`, which demangles to
`Air::H3M::StereoReverb`. The firmware also registers `StereoReverbSimplified`,
`SpringReverb`, `NonLinearReverb`, `DattorroReverb` and its simplified variant,
`EconomyReverb`, `ModReverb`, `SurroundReverb`, `AnimatedReverb`, `DJReverb` and
`AlgorithmicReverb`, `Vinyl`, and `Decimator` in the same H3 effect factory.

`FUN_03d46910` initializes the type registry and looks up the requested type
string. Its `StereoReverb` entry points to `FUN_03d445b0`, which allocates a
`0x988`-byte instance and invokes the constructor `FUN_03dac6b0`. The constructor
installs the main vtable at `0x07180be0` and a secondary interface vtable at
`0x07180cf0`; it also registers 18 parameters (`0x12`).

## Runtime methods

The main vtable identifies these methods:

| Slot | Function | Observed role |
|---:|---|---|
| 9 | `FUN_03dad0b0` | Resets or resizes the effect's internal buffers when the runtime block size changes, then recalculates its parameters. |
| 11 | `FUN_03dabed0` | Processes a block of stereo float audio using persistent state and the instance's internal delay/filter buffers. |
| 21 | `FUN_03daca50` | Applies one of the 18 parameter changes and updates coefficients/state; parameter indices are handled by a switch from `0` through `0x11`. |

The process callback is stateful and takes input/output channel pointers, a
frame count, processing flags and an offset. Its decompilation shows multiple
delay/filter state buffers, smoothed parameter values, and a stereo output mix.
This is enough to identify the native effect and integration surface; the
internal signal topology and all parameter labels/ranges have not been fully
reconstructed.

## How the firmware creates and runs effects

The normal host path is through an H3 patch, rather than a reverb-specific
global function:

1. `FUN_03cf1a20` handles H3 patch messages and routes patch state into
   `FUN_03cf5180` (`H3Patch::setState`-like parser). It walks the patch's
   `H3Part` elements.
2. For each part, the parser passes the part's effect type string to
   `FUN_03d46910`, along with the owning patch and its processing context. The
   registry finds the matching factory. For `StereoReverb`, that is
   `FUN_03d445b0`.
3. The parser builds a host wrapper around the returned effect and initializes
   its parameter map/defaults (`FUN_03d42960`). It sends generic `H3pc`
   (`0x48337063`, 16-byte context) and `H3ps` (`0x48337073`, 0x74-byte patch
   state) messages, then links the wrapper into the patch's effect chain. Patch
   `H3Assign` entries are processed separately for source/target and parameter
   assignments. `StereoReverb`'s message handler (`FUN_03dacfa0`) does not
   recognize `H3pc` or `H3ps`; it recognizes `H3zz`, `PTCC`, and `0x13`.
4. In the audio processing graph, `FUN_03d27ca0` first runs the pre-effect
   processing, then walks the effect-wrapper list and calls each effect's
   vtable slot 11 (`vtable + 0x58`) with the stereo buffer pointers, frame
   count, processing flags and offset. It then runs post-effect processing.

This confirms that `StereoReverb` is designed as a normal insert in the H3
effect chain. Its base setup (`FUN_03d040d0`) stores the supplied host/context
pointers and reads sample rate plus block-size information from the context.
The host also performs a zero-frame process pass and then calls each effect's
slot 18 (`vtable + 0x90`) when managing the chain (`FUN_03d28420`).

## Standalone offline rendering

The class also runs without constructing an H3 graph. Its factory and virtual
methods need only the small context read by the base class:

```c
struct AudioContext { float sample_rate; uint32_t reserved; int32_t block_size; };
effect = factory(NULL, &context);
vtable[9](effect, &context);       // allocate buffers and apply default params
vtable[11](effect, in, out, n, &flags, offset);
vtable[1](effect);                 // destroy and free
```

The constructor initializes the 18 defaults. Slot 9 takes the context pointer
as its second argument and forwards it to the base sample-rate/block-size
initializer; it is not a boolean. Passing `1` makes the base initializer
dereference address `0x1` and segfault. This was reproduced in the emulator and
fixed by passing `&context`.

The offline renderer in `mod/src/h3m_effect.c` creates one isolated native instance
per job, renders stereo blocks no larger than the configured block size, feeds
four seconds of silence to capture the tail, trims the rendered silence,
destroys the instance, and submits the float output to native Extract. It does
not link the effect into the live H3 chain. `FUN_03d27ca0` explains normal live
graph processing, but SAMPLE OPTIONS does not need to create or modify an H3
patch to use this effect. The normal graph's zero-frame pass and slot 18
lifecycle callback are not required for this isolated render; the direct
factory/setup/process/destroy cycle was verified in the emulator.

`Studio Reverb` and `Studio Reverb Mini` appear as user-facing preset labels in
firmware data. They are not the type name used by this factory; the class type
is `StereoReverb`.

## Relation to SAMPLE OPTIONS

The **Add Reverb** menu action instantiates this class directly in its offline
worker. It retains constructor defaults, uses native Extract for file and pad
ownership, and leaves the selected pad unchanged.

The same offline bridge also powers two more SAMPLE OPTIONS items:

| Menu item | Registry type | Factory VA | Process slot | Controls set before setup |
|---|---|---:|---|---|
| Vinyl | `Vinyl` | `0x3d43cf0` | 11 (`FUN_03dcc7e0`) | Drive `0.18`, Noise Level `0.12` |
| Lo-Fi (Decimator) | `Decimator` | `0x3d457a0` | 11 (`FUN_03d5cd70`) | Bit Depth `0.58`, Sample Rate `0.70` |

Vinyl's constructor is `FUN_03dcce00`, setup is slot 9 (`FUN_03dcd190`),
parameter update is slot 21 (`FUN_03dcce80`), and its 11 descriptors begin at
`0x07183e58`. Decimator's constructor is `FUN_03d5ca80`, setup is slot 9
(`FUN_03d5dc80`), updater is slot 21 (`FUN_03d5df50`), and its 16 descriptors
begin at `0x0729e808`. Decimator's Bit Depth is parameter 0 and Sample Rate is
parameter 4. The values above are normalized H3 parameter values; they give a
moderate lo-fi reduction while leaving its other defaults intact.

`mod/src/h3m_effect.c` selects the factory, applies these normalized values to
the constructor-created parameter array, calls setup with the audio context,
then renders through slot 11. These two effects keep the input frame count;
only Reverb receives an appended silence tail. Native Extract writes each
result with the `Vinyl` or `Lo-Fi` name and assigns it to the next free pad.
Vinyl created `Vinyl1.wav`, 10,501 stereo frames at 44.1 kHz. Output RMS was
`0.315074` versus `0.446192` for the source; its peak was limited to `0.98`.
Lo-Fi created `Lo-Fi1.wav`, 10,501 stereo frames at 44.1 kHz. Output RMS was
`0.423907` and peak `0.895582`. Both reached their `copy created` status after
native Extract assigned a new pad. The firmware PID remained `30277`, with
`NRestarts=0` throughout both actions.

The initial Vinyl tuning (`Drive=0.45`, `Noise Level=0.20`) produced RMS
`0.084433` and hit the global peak scaler. Reducing these controls to `0.18`
and `0.12` raised output RMS to `0.315074`, a more useful level relative to the
source.

## Emulator verification

On 2026-11-05, `mod/tests/emulator_reverb.py` completed through SAMPLE OPTIONS.
It created `Reverb4.wav` at 44.1 kHz stereo with 154,886 frames, including
144,385 appended tail frames. Tail RMS was `0.035679`; the output was float32
and differed from the input. Native Extract assigned the output to a new pad;
the firmware PID stayed `29682` and `NRestarts=0`.

## Reproduction

With the firmware open in the existing Ghidra MCP bridge:

```sh
python3 tools/ghidra/hpss/run_bridge.py FindStr.java StereoReverb 20
python3 tools/ghidra/hpss/run_bridge.py ResearchDecompile.java 0x3d46910 0x3d445b0 0x3dac6b0 0x3dad0b0 0x3dabed0 0x3daca50
python3 tools/ghidra/hpss/run_bridge.py DumpVtable.java 0x07180be0 36
python3 tools/ghidra/hpss/run_bridge.py ResearchDecompile.java 0x3cf1a20 0x3cf5180 0x3d27ca0 0x3d040d0 0x3d28420
python3 tools/ghidra/hpss/run_bridge.py ResearchDecompile.java 0x3d04060 0x3dad0b0 0x3dabed0 0x3dabe60
python3 tools/ghidra/hpss/run_bridge.py DisasmRange.java 0x3dad0b0 60
python3 tools/ghidra/hpss/run_bridge.py ResearchDecompile.java 0x3d5ca80 0x3d5dc80 0x3d5cd70 0x3d5df50 0x3dcce00 0x3dcd190 0x3dcc7e0 0x3dcce80
python3 tools/ghidra/hpss/run_bridge.py DumpVtable.java 0x07178bb8 24
python3 tools/ghidra/hpss/run_bridge.py DumpVtable.java 0x07183d38 24
python3 tools/ghidra/hpss/run_bridge.py DumpParams.java 0x0729e808 16 0x30
python3 tools/ghidra/hpss/run_bridge.py DumpParams.java 0x07183e58 11 0x30
```
