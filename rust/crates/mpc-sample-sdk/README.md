# mpc-sample-sdk

The universal mod SDK for the Akai MPC Sample 1.3.0.12.

It owns everything that is not a feature:

* **Firmware** — find the loaded app, verify the 36 fingerprint words, and map
  the image (`image`, `loader`).
* **Hooks** — the in-memory `b`/relay/trampoline engine and all 16 sites.  The
  displaced originals stay inside `sites`; a mod never stores or types one.
* **API** — the `Mod` trait, the event types, and `export_mod!`, which emits
  the `.init_array` entry point that installs the hooks and calls the mod.
* **Audio** — the four ALSA taps the app resolves by name, the stream geometry,
  `OutputInfo` for a mod that mixes into the output or listens to the input, and
  the latency the device geometry implies.
* **Drawing** — a `Graphics` handle over the app's own `setColour`/`fillRect`/
  `Font` calls, the toast, the installed page-paint slot and the repaint pump.
* **Platform objects** — typed, bounds-checked access to the page, the selected
  sample, pads, slices, trim, WAV files, paths, the status line, the clock, the
  writer, the control surface, the effect renderer and the Extract command.
* **Safe state** — `state::Global` and `export_static!` keep process-wide UI
  state and the exported statics out of the feature crate, so a mod can be
  written without `unsafe`.

## A minimal mod

```rust
use mpc_sample_sdk::{Mod, PadEvent, PadFlow};

#[derive(Default)]
struct MyMod;

impl Mod for MyMod {
    fn on_pad(&mut self, event: PadEvent) -> PadFlow {
        if event.action == mpc_sample_sdk::PadAction::Press {
            // ...
        }
        PadFlow::Forward
    }
}

mpc_sample_sdk::export_mod!(MyMod);
```

Build it as a `cdylib` and `LD_PRELOAD` it on the device.  `install` refuses a
missing app, a different firmware and a build where no site could be patched;
the mod's `on_start` runs only after the hooks are in.

## Layout

```
src/lib.rs          the public surface and the crate docs
src/event.rs        Mod, the event types and the flows
src/loader.rs       install, export_mod!, the firmware words
src/sites.rs        the 16 site adapters
src/hook.rs         the patch engine
src/image.rs        the image, memory window and firmware call helpers
src/audio.rs        the ALSA taps, OutputInfo, latency and S32 mixing
src/draw.rs         Graphics, the toast, the screen and the repaint pump
src/midi.rs         NoteEvent and replay
src/task.rs         the generic one-shot worker
src/render.rs       RenderJob, RenderOutput and source metadata
src/platform/       page, sample, pads, slices, trim, WAV, paths, status,
                    clock, writer, control, effect, extract
```

## Vocabulary

| term | meaning |
|---|---|
| **Selection** | The exact Sample-page source: screen, page state, sample, frames, channels and rate. `Selection::is_current()` answers “is this still the visible selection?”; `read_audio()` loads the app buffer first, then the WAV. |
| **Task** | One generic one-shot worker: it owns a value, moves it to a named thread, catches a panic when the runtime unwinds, and hands the value back once through `take()`. |
| **RenderJob** | One audio request: a `Selection`, a feature-defined `kind`, interleaved input and the `RenderOutput` buffers the backend fills. |
| **RenderOutput** | One interleaved f32 buffer plus its frame and channel geometry. |
| **backend** | Feature-side code that knows a DSP algorithm (`minimod/src/backend/`). The SDK never depends on it. |

The SDK does not know any feature.  See `crates/minimod` for a full mod built on
it.
