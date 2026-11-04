# HPSS firmware investigation

> **Status 2026-11-04:** the SAMPLE OPTIONS flow is implemented in the mod and
> validated in the emulator. See [HPSS-HANDOFF.md](../HPSS-HANDOFF.md) and
> [runtime validation](re/hpss/runtime-validation.md). The sections below are
> the chronological investigation that led to the shipped hooks; statements
> such as "no UI" or "unresolved" describe the state at the time of that
> capture, not the current implementation.

Work in progress for firmware 1.3.0.12. Addresses below are Ghidra virtual
addresses; subtract `0x100000` for the mod's file offsets.

The existing prototype is in the original worktree's `tools/hpss/`. It splits
WAV audio using a short-time Fourier transform, median filters and soft masks.
It is a host Python prototype and is not part of the device mod.

The Java script `ResearchDecompile.java`, run through the Ghidra MCP bridge,
shows that `FUN_0296bba0` retrieves a pad state from the surface vector at
`+0x2808..+0x2810`. `FUN_02903b90` retrieves a shared sample reference for a
layer: the layer array is at pad state `+0x22e8`, with stride `0xb30`, and the
sample and shared ownership control block are at layer `+0x4a8` and `+0x4b0`.
The stock pad occupancy predicate walks the layer array. Reuse the native
empty-pad finder described below instead of duplicating that predicate.

`FUN_02a2ed50` searches the project sample pool by sample name, locking its
mutex and returning a shared reference. It does not import a file. The stock
load workflow `FUN_01a5bd00` constructs a command via `FUN_01a95ab0` and submits
it through `FUN_01ad0a00`; it then looks up the loaded sample in the pool and
constructs another command via `FUN_01a95f90` for the program. The command
inputs and the safe way to construct and execute them still need to be traced.
Writing a pointer directly into a layer would skip ownership and project state.

The native HPSS DSP module exists and is included in the build, but no menu
or HPSS pad assignment is connected yet. The existing source sample must remain untouched, and both destination
pads must be confirmed empty before the operation commits.

## Confirmed native assignment path

Java decompilation of the command vtables identified:

- Load command vtable `0x6f40fb0`, execute slot 8: `FUN_01a97bd0`.
  Its first execution calls `FUN_01ee0620(outSharedSample, request)`.
- Assignment command vtable `0x6f414e0`, execute slot 8: `FUN_01a9a3b0`.
  It snapshots all eight layers, then delegates each change to `FUN_01a9a290`.
- `FUN_01a9a290` resolves the shared sample's pool index through
  `FUN_02a2eb00`, adds one, maps the layer control with
  `FUN_0288cd80(0x40d, layer)`, and calls `FUN_029c6790` with that float value,
  program/surface object, pad index and control ID. Zero selects no sample.
  It also sets the layer mode using mapped control `0x45e`.
- `FUN_029c6790` handles synchronization and routes sample controls through
  `FUN_029c65c0`; ordinary controls use its queued path.

The import request constructor is `FUN_01ee0580`. Its request has five service
pointers at offsets `0..0x20`, a shared file wrapper at `0x28/0x30`, a JUCE
sample-name string at `0x38`, pad/layer integers at `0x40/0x44` (initially -1),
and options at `0x48`. `FUN_01ee0620` invokes the file loader `FUN_01ef6140`,
then registers the result using `FUN_02a31210`. Its service pointers still need
identification and capture; a fabricated request with null pointers is unsafe.

Earlier candidate route (superseded by the Chop Extract route below): use the
stock load request and command manager,
then the stock assignment command, with two empty pads selected and rechecked
on the UI thread. Run only audio separation on a worker. Command grouping and
rollback must be confirmed before claiming atomic two-pad insertion or undo.

Raw Java-script output is retained in `docs/re/hpss/`. No runtime execution of
these candidate native calls has been attempted.

## Preferred route: reuse Chop Extract (user-directed)

The Sample/Chop Extract handler is `FUN_017580d0` (file `0x16580d0`).
It gets the source pad state through `screen+0x178 -> +0x60`, and its program
through `padState+0x2310`. It calls `FUN_0296dd00(program)` for the destination.
That native finder scans pad indices 0..127 and returns the first unused pad,
or -1; this is not a circular scan starting after the source pad.

The handler invokes `FUN_0296bbe0(program, destination)` and `Page_LoadPad`
to copy the source settings, then constructs `ExtractAndAssignSampleCommand`
with `FUN_0153ed40`. Arguments are name, destination index, extraction settings
at screen+0x9ba0, source pad state, project context, and three native services.
The handler resolves services from its registry at screen+0x100, and submits
the command through the command manager at screen+0x138.

The command's vtable is `0x6f07640`; execute slot 8 is `FUN_0153f0b0`.
Execution creates an ExtractSliceCommand, obtains its resulting shared sample,
then calls the previously identified `FUN_01a95f90` assignment command with
layer index 0. It also manages the extracted file and sample path bookkeeping.

This is now the preferred integration path. Reuse its native empty-pad finder,
creation and assignment mechanisms. Do not implement a separate pad allocator.
Still unresolved: how to supply HPSS-processed audio at the extraction stage
without changing the source sample, and how to make two outputs succeed or
roll back together. The native command has not been executed by the mod yet.

## Continuation: extraction audio stage

`PTR_DAT_072657e0` resolves to `0x6f42748`; the ExtractSliceCommand vtable
address point is `0x6f42758`. Its execute slot 8 is `FUN_01aaaae0`.
That function calls `FUN_0219c5b0`, passing the source shared sample, range,
bit depth, output name and a range-processing flag. Its output shared sample
lives at command offsets `+0xb8/+0xc0`.

`FUN_0219c5b0` obtains an audio provider from the source using
`FUN_02a43250`. It optionally wraps it with `FUN_021d9fd0` for the requested
range, then calls `FUN_0219b1f0` with that provider. It queues follow-up work
through a thread-local executor and calls the executor's slot at `+0x20`.
This identifies an audio-provider boundary worth tracing for HPSS. The provider
ABI and executor completion semantics are not yet established; do not treat
this as a callable C signature merely from the decompiler's inferred types.

`FUN_029047b0`, used by the empty-pad finder, iterates the layer array from
`+0x22e8` to `+0x22f0` and calls `FUN_02914930` on each layer, returning occupied
on the first true result. This confirms why the native finder should be reused.

## Requested behavior and remaining work

The requested menu is SAMPLE OPTIONS, opened by SHIFT + DATA in Sample mode.
DATA confirms HPSS. The source stays unchanged; harmonic then percussive outputs
should use the same available-pad behavior as stock Extract. Panel definitions
identify DATA press as button `0x6f` and DATA rotation as CC `100`.

The DSP source and build changes exist; no end-to-end HPSS runtime trial has
been performed. Remaining work:
identify the provider ABI and native services; integrate the host algorithm as
on-device processing; connect the menu; handle failure and two-pad availability;
then verify output audio, source preservation, project save/reload and undo.
Two consecutive calls to the empty-pad finder before assignment return the same
pad; the two-output operation must account for that using the native state flow.

## Evidence and reproduction

The research branch is `feature/sample-options-hpss` in the sibling worktree
`sample-converter-hpss`. The original `sample-converter` checkout retains its
uncommitted documentation updates and host HPSS prototype. No merge or commit
has been performed for this feature.

See [raw decompilation index](re/hpss/README.md) and
[Java scripts and bridge runner](../tools/ghidra/hpss/README.md).
Ghidra outputs are static evidence from `knob.bin` in project `mcpsamplebin`.
No calls in these outputs have been verified as an HPSS runtime integration.

## Provider ABI: assembly correction

The range provider's vtable is `0x6f7de10`. Slot 3 (`+0x18`) reports position;
slot 4 (`+0x20`) checks end-of-stream; slot 5 (`+0x28`) reads a block; slot 6
(`+0x30`) seeks. The range provider stores its wrapped provider at `+0x28` and
start offset at `+0x30`. Its position subtracts that offset; seeking adds it.

Crucially, the decompiled read signatures are misleading. Assembly for
`FUN_021c5f60` and `FUN_021d9ab0` confirms the provider is passed in `x0`, the
read argument remains in `x1`, and the result storage is passed in AArch64 `x8`.
This is an indirect aggregate return, not an ordinary output pointer in `x0`.
Calling the pseudo-C prototype literally would corrupt the call. The complete
result layout still needs inspection before declaring an adapter.

`FUN_02a43250` obtains a provider using the source sample's shared reader at
`+0x170/+0x178`, under a shared-pointer lock. `FUN_0219b1f0` calls
`FUN_02199800` to write the provider to temporary storage before subsequent
sample work. `FUN_01e8ad40` moves the provider owner into a writer context,
clearing the original owner's pointer. Provider ownership therefore must be
respected, including destruction after the writer takes it.

Next trace: the result storage layout written by a concrete reader and consumed
by the writer. These findings identify an integration boundary but do not yet
establish a working replacement provider.

## Writer boundary resolved: interleaved float32

`FUN_021c6e40` reads provider blocks and invokes a callback with the block and
an integer frame offset. The block has frame count at `+0` and audio pointer
at `+8`. It also owns cleanup callbacks at `+0x10/+0x18` and trailing storage;
those fields must not be synthesized or overwritten.

The WAV-writing callback `FUN_01e8aff0` passes the pointer at block `+8` and
frame count to writer vtable slot `+0x20`. Concrete writer `FUN_01e93570`
multiplies frames by channel count (`writer+0x58`), then consumes sequential
float values. For 32-bit output it copies `frames * channels * 4` bytes directly;
for narrower output it converts those floats. Thus the input is contiguous
interleaved float32, not planar channel pointers.

This supports a narrower proposed integration than a new provider: intercept
only the writer callback for an explicitly tracked HPSS extraction, temporarily
substitute the block's audio pointer with the appropriate processed output
slice, invoke the original callback, and restore the pointer. Keep the block's
frame count, cleanup callbacks and original source audio untouched. All other
extracts and writes must pass through unchanged. The buffer must remain alive
until the native writer completes. Thread/executor scoping and callback ABI
still require runtime validation before enabling the hook.

The intended flow is now: prepare equal-length interleaved HPSS buffers, use
native Extract to create each destination, substitute audio only at its writer
callback, and let native Extract register and assign each result. The native
free-pad finder and command remain responsible for destination selection and
sample lifecycle. No replacement reader implementation is needed if this
scoped interception validates successfully.

## Native DSP port (2026-11-04)

`mod/src/hpss.c` now implements the offline float32 transform; `hpss.h` declares
its interleaved-buffer interface. The mod build includes this module, but no UI
or Extract hook calls it yet. The module is not yet reachable from the device UI.

Constants follow the user's final specification: 2048 FFT, 512 hop, symmetric
Hann, reflect padding, 41/17 median widths at 44100 Hz, sixth-power mask with
margin 0.55, complementary masks, two inverse transforms, window-energy
normalization, no gate or peak normalization. Other rates use the supplied
width formulas, rounding to nearest integer then raising even widths to odd.
Section 3.4's mask equation is used; the overview's numerator omits the margin
and conflicts with the detailed equation. Digital silence gives percussive
mask zero under the specified denominator guard.

Medians maintain sorted windows with one removal/insertion per step. Channels
are processed sequentially to bound spectrum memory. Caller owns both output
buffers and must free them. The DSP consumes already decoded PCM; RIFF reading,
write bit depth, UI, progress, native assignment and rollback are not integrated.

Validation performed: host compilation with `-Wall -Wextra -Werror`, ARM64 Linux
object compilation with Zig, and numerical comparison to `hpss.py` explicitly
using power 6 and margin .55. Cases were 1-frame mono, 127-frame stereo,
44100-frame stereo at 44.1 kHz, and 48000-frame mono at 48 kHz. Maximum sample
error against Python was 1.35e-7; maximum reconstruction error was 1.35e-7.
These synthetic tone/impulse checks establish DSP parity for these cases only;
there has been no emulator or hardware HPSS run.

## Resumed Ghidra session: constructor ABI and range input

The reopened `knob.bin` program is accessible through the MCP Java-script runner.
`PTR_PTR_072657a0` resolves to `0x6fd21a8`, the registry key used for the final
ExtractAndAssign constructor service argument.

Assembly confirms `FUN_0153ed40` is an ordinary nine-argument constructor:
`x0` destination command, `x1` name, `w2` destination pad, `x3` extraction
settings, `x4` source pad state, `x5` project, `x6/x7` two services, and the
third service on the stack. It has no hidden aggregate-return argument.

`FUN_02a49a80` reads the extraction settings object's values at `+0x78`,
`+0xd0`, `+0x128`, `+0x180`, `+0x1d8`, `+0x230`, `+0x2e0`, `+0x338`, and
`+0x288`, copying them into a compact 0x38-byte record. A full-sample HPSS call
must supply appropriate independent settings rather than altering the live
Chop selection. Which of the non-boundary fields need preservation remains
unverified.

Writer callback assembly confirms block pointer in `x1`; it reads the frame
count as 32 bits at `+0` and the audio pointer as 64 bits at `+8`. The frame
offset callback argument is unused by the stock callback. A replacement can
use that argument only after checking the producer's assembly.

## Completion sequence after implementation audit

The DSP is the only implemented feature component. Compile success does not
establish that the menu, writer hook, pad commands, or project persistence work.
The next step is a minimal native Extract invocation, not more broad decompilation.

1. **Establish an emulator baseline.** Confirm panel inputs and audio work, load
   a known project, make a stock Extract, and record the selected source, new
   pad, file and process identity. Current ac50 is running with zero restarts;
   CS/audio bridge ports were closed during the audit and need checking.
2. **Build the native bridge and prove unchanged-audio extraction.** Capture or
   resolve the live screen, selected pad state, program, command manager and
   service registry. Validate their identities and lifetimes at runtime.
   Construct independent full-length extraction settings; preserve the live
   Chop/Trim state. Invoke the stock command with native ownership management.
   Pass criterion: a playable independent sample on the native first-free pad,
   with source audio and settings unchanged. Confirm save/reload and undo.
3. **Prove scoped writer substitution.** Associate a writer with one specific
   extraction job, including executor thread handoffs; a global busy flag alone
   is insufficient. Validate frame-offset ABI, frame counts, channels, result
   ownership and completion timing. First write a known half-amplitude buffer
   and compare the new WAV numerically. Restore the original block pointer on
   all paths and leave ordinary Extract/record/export operations unaffected.
4. **Complete input and worker handling.** Obtain the selected sample exactly,
   without frame-count-only file matching. Decode PCM16/24/32 and float32/64,
   including extensible WAVs; existing AutoTrim's pcm_at loses precision and is
   unsuitable for HPSS. Handle unsaved recordings via native sample data or an
   exact native-backed file. Hold source/project references while processing;
   reject stale completion after source/project changes. Do DSP on a worker,
   with memory admission checks and no firmware model calls from that worker.
5. **Commit both outputs using Extract.** Preflight capacity for two pads with
   native occupancy semantics. Use the native free-pad finder at each commit;
   two calls before assignment return the same pad. Use harmonic/percussive
   names and full-length independent extraction settings. Preserve sample rate,
   channels and specified output depth (16-bit source stays 16; wider input
   follows the reference's 24-bit output). Verify completion before freeing
   buffers. Group native commands if supported and validate rollback when the
   second output fails, without undoing unrelated user work or deleting files
   still referenced by native history.
6. **Connect SAMPLE OPTIONS.** SHIFT+DATA only on the visible Sample screen;
   edge detection prevents repeated opens/confirmations. DATA confirms HPSS.
   Draw title, selected item, explanation and processing/error feedback within
   320x240. Suppress underlying DATA actions while open; add an explicit close
   action. Keep SHIFT+B1 Chop Extract and SHIFT+B3 AutoTrim working normally.
7. **Run the end-to-end acceptance checks.** Known source produces two playable
   new pads; occupied pads remain unchanged; source file hash, layer references
   and trim values remain unchanged. Export/read the new audio and compare with
   the settled Python parameters, allowing output quantization. Cover mono,
   stereo, silence, very short input, 44.1/48 kHz, exactly two/one/zero free pads,
   repeated confirmation, project changes, processing/write failure, undo and
   save/reload. Check responsiveness, memory, audio and ordinary Extract again.

Do not expose a working-looking HPSS action before the native bridge passes its
unchanged-audio and substitution milestones. No new user information is needed
for these next steps. All work remains uncommitted in the feature worktree.

## Emulator milestone: native handler invocation

A compile-time-only `MINIMOD_HPSS_PROBE=1` hook now records stock Extract's
screen, pad state and program. After one stock Extract, SHIFT+DATA in Chop mode
invokes the same handler through its trampoline, with press-edge detection.
The release build excludes this probe; this is not the SAMPLE OPTIONS menu.

Emulator evidence: normal SHIFT+B1 advanced the native free-pad index 1 -> 2.
Then the mod's SHIFT+DATA invocation advanced it 2 -> 3. Both used screen
`0xffffe52cca90`, matching the screen-paint hook; source state remained
`0xffffe45fc290` and program `0xffffe45efc20`. ac50 PID remained 459 with
NRestarts=0. This establishes successful native command invocation and pad
allocation in this emulator session.

## Shipped integration (2026-11-04)

The mod uses stock Extract and intercepts only the pieces HPSS needs:

| hook | offset | role |
|---|---|---|
| Extract constructor | `0x143ed40` | map the armed command to its audio buffer and replace the name with `HPSS Harmonic` / `HPSS Percussive` |
| Extract execute | `0x143f0b0` | install the substituted buffer and patch the command's settings range to `0 .. frames` for the copy |
| WAV writer callback | `0x1d8aff0` | swap the block's audio pointer to the next slice of the processed float32 buffer, then restore it |
| Extract handler | `0x16580d0` | called directly to reuse native free-pad selection, `Page_LoadPad`, command creation and assignment |

The constructor stores the settings pointer; the execute method is the place
that reads it. Patching the live screen settings before calling the handler is
not sufficient because the handler re-syncs them from the UI. The execute hook
patches the settings object only for the duration of that read; the UI's live
trim/chop range is restored immediately afterwards.

`mod/src/minimod.c` also adds:

- exact WAV decoding for PCM16/24/32 and float32/64, including
  `WAVE_FORMAT_EXTENSIBLE`; frame count, channels and rate must match the native
  sample metadata;
- a unique `TempProjects` fallback for unsaved recordings, never a
  frame-count-only match;
- an HPSS worker thread, project/source identity checks on completion, two-pad
  preflight through the native occupancy predicate, and failure feedback;
- SAMPLE OPTIONS by `SHIFT+DATA`, DATA-wheel selection, DATA confirmation,
  `SHIFT+DATA` close, DATA-repeat suppression and page visibility gating;
- restoration of the Sample page's Chop/Full mode after the native handler.

The DATA encoder is not visible through the app's JUCE/button hooks. It arrives
on `/dev/snd/midiC1D0` as a rawmidi stream; this firmware's control-surface
reader returns 32-byte events with one MIDI byte at offset 16. A `read`
interposer tracks `B0 64 value` while the menu is open, uses the value for menu
selection, and replaces the whole `B0` message with active-sensing bytes before
the app sees it so the underlying page does not also move.

Deliberate limits: the app emits float32 WAVs on this firmware; the working
project copy may be re-saved with 24-bit dither (±2 LSB). If the second native
write fails, the harmonic output is kept and the UI reports `HPSS: 1 of 2`; a
destructive pad-clear rollback was tested and removed because it corrupted the
project and caused a missing-sample dialog on reload.
