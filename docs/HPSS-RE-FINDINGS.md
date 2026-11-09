# HPSS reverse-engineering findings and method (2026-11-04)

> **Historical.** Written while the mod was C. `mod/src/minimod.c` and the other C and C++ sources are
> gone (see git history); the equivalent code is in `rust/crates/`, mapped in the "Final state" section of
> [`RUST-PORT.md`](RUST-PORT.md). File, line and function references below point at the removed C, and build
> commands that compile it no longer apply (`mod/build.sh` builds the Rust crate).

> Historical input experiments: the raw-read suppression described below was
> replaced on 2026-11-05 by a single native signed DATA-event hook.
> See [current input mechanism and validation](re/SAMPLE-OPTIONS-INPUT.md).


This is the "what we learned and how we learned it" companion to
[HPSS-RE.md](HPSS-RE.md) and
[the runtime validation](re/hpss/runtime-validation.md). It records the
findings from the final implementation pass, the tooling that actually paid
off, and the paths that looked plausible but were blind.

## 1. New findings

### 1.1 The DATA encoder is not a JUCE event

The encoder never reached any of the app callbacks the mod already knew:

| path | result |
|---|---|
| `buttons` (`0x159f2f0`) | button/note events only; no direction for the wheel |
| `cs_event` (`0x23aff60`) | never called for the wheel |
| `RotaryFixedConnection` (`0x15f5690`) | never called for the wheel |
| `cc_capture` (`0x159f230`) | hook site never installed; the address is not a working CC entry |
| JUCE `mouseWheelMove` slot on `SampleModeScreen` | the screen does not override it |

It arrives on `/dev/snd/midiC1D0` as a rawmidi stream. The app reads it with
`read()`, not `snd_rawmidi_read()`: an interposer on `snd_rawmidi_read` saw
nothing.

On this firmware each `read()` returns **32 bytes** and carries **one MIDI
byte at offset 16**. The header bytes observed were:

```
00 01 00 00 <4-byte timestamp> 21 16 00 00 00 00 00 00 <midi byte> ...
```

The header is not fully decoded; only the payload at `+16` matters for input.
A wheel turn is the ordinary MIDI sequence:

```
B0 64 value          CC 0x64 on channel 0
```

`value` is relative: `01` is one direction, `7F` the other. Note events for the
front-panel keys use `90/80 <note> <velocity>`; SHIFT is note `0x31`, DATA push
is note `0x6F`.

### 1.2 Rewriting the CC is not enough — it must be dropped

The first attempt rewrote the CC payload to a harmless value. That failed in
two ways:

- active-sensing bytes (`0xFE`) still allowed the app to move the page;
- a valid but "unused" CC (`B0 7F 00`) was also acted on.

The only reliable suppression was to **return 0 from the read** for the three
events that make up the CC message. The menu still sees the original value
before dropping the bytes. Note events are left alone so DATA can still
confirm/close.

There is also a race between `SHIFT+DATA` and the menu becoming visible on the
UI thread. A 1-second guard is armed as soon as the rawmidi reader sees
`SHIFT` + `DATA`; while dropping a CC the guard is extended by 200 ms so a
message cannot leak in part if the menu closes mid-sequence.

### 1.3 Extract settings are a live object, not a snapshot

The Sample page's extraction settings live at `screen+0x9ba0`. The range is:

- `+0x78` — start frame
- `+0xd0` — end frame (exclusive)

Patching these around the call to the Extract handler does **not** work: the
handler re-syncs them from the UI before constructing the command. The
constructor (`0x143ed40`) stores the settings pointer; the execute method
(`0x143f0b0`) is what copies the range into the slice command through
`FUN_02a49a80`.

So the range has to be patched in the **execute hook**, immediately before
calling the original execute, and restored immediately after. That is what
makes a full-sample HPSS extraction possible while the live Chop/Trim range
stays untouched.

### 1.4 Writer callback and output depth

The writer callback (`0x1d8aff0`) receives a block with:

- frame count at `+0`
- interleaved float32 pointer at `+8`

The concrete writer (`0x1e93570`) consumes that pointer sequentially. The
substitution is therefore a pointer swap plus a running frame offset, and the
original pointer must be restored after the callback returns.

Despite the older assumption of 16/24-bit output, the stock Extract writer on
1.3.0.12 emits **float32 WAVs** even for a 24-bit source. The native outputs are
float32; the DSP is float32 end to end.

### 1.5 Native Extract changes the Sample page mode

After the two extractions the Sample page can be left in **Chop** mode even
when it started in Full mode. The fix is to remember the mode before the
operation and, if it changed, replay the CHOP button through the app's own
button dispatcher. The event template is captured from the last real button
event; no model field is written directly.

### 1.6 Project persistence rewrites the working copy

The original user-content WAV is untouched, but the app's project working copy
can be rewritten by the native project save. On a 24-bit sample the observed
change was 24-bit dither: every sample moved by at most 2 LSB (`+2`, `-2` or
`0`, roughly half and half). That is native save behaviour, not HPSS. The
original file's SHA-256 stayed identical.

### 1.7 Destructive rollback is unsafe

Clearing the first output pad by writing the assignment controls (`0x40d`,
`0x45e`) outside the native command/history path made the app delete the file
and then report a missing sample on reload. The shipped code keeps the
harmonic output and reports `HPSS: 1 of 2` when the second write fails. It does
not clear pads or delete files.

### 1.8 Names and duplicate runs

The stock command names outputs from the constructor argument. The mod replaces
that argument with `HPSS Harmonic`, `HPSS Percussive`, or `Reverb`, depending on
the transform. When the same name already exists the app appends `1`, `2`, ...
to the file and sample name, just like stock Extract.

### 1.9 Extending output with the final writer block

Add Reverb needs space after the source for its decay. Increasing Extract's
requested end frame does not extend the source reader: Ghidra's
`FUN_021c6e40` stops at the reader's own frame count (`reader+0xc`). The source
frame getter is `FUN_02a41f80`, which reads `sample+0x134`.

The writer boundary can extend the result safely. `FUN_01e8aff0` passes a block
frame count (`+0`) and interleaved float32 pointer (`+8`) to the concrete writer
`FUN_01e93570`; that writer consumes the supplied count and emits the matching
number of frames. For the armed Reverb extraction only, `hpss_writer_hook()`
points the final source block at the remainder of the processed buffer and
temporarily changes its frame count to include the tail. It restores both block
fields after the synchronous writer call and lets normal reader cleanup run.
The native reader never reads past the source. The WAV file ends up longer,
while native Extract still registers and assigns it to a free pad.

The emulator produced 21,526 frames from the 10,501-frame A01 sample: 11,025
tail frames (250 ms) with RMS `0.002719`. The native command reported assignment
success and the firmware had no restart. The regression path is
`mod/tests/emulator_reverb.py`; full implementation and validation notes are in
[REVERB-HANDOFF.md](REVERB-HANDOFF.md).
Ghidra output for the getter and reader stop is
[`reverb-reader-boundary.txt`](re/hpss/reverb-reader-boundary.txt); the existing
writer captures are [read-loop.txt](re/hpss/read-loop.txt) and
[wav-write-block.txt](re/hpss/wav-write-block.txt).

## 2. Tooling that proved useful

### 2.1 Ghidra MCP bridge

The bridge is `/Users/jakubkolcar/.local/bin/bridge-mcp-ghidra`, used through
[`tools/ghidra/hpss/run_bridge.py`](../tools/ghidra/hpss/run_bridge.py). Ghidra
must be running with `GHIDRA_MCP_ALLOW_SCRIPTS=1` and project `mcpsamplebin`
open. The runner keeps one MCP session so the connection and tool registration
survive between script calls.

What paid off:

- `ResearchDecompile.java` on the handler, constructor, execute, writer callback
  and `FUN_02a49a80` — this is where the settings lifecycle was found.
- `HpssInstructions.java` on the writer and constructor — validated the block
  layout and the nine-argument constructor ABI.
- `DumpVtable.java` — quickly showed what `SampleModeScreen` actually overrides.
  It ruled out the screen-level `mouseWheelMove` theory.
- `HpssRefs.java` — good for data/string xrefs, but some hook sites are called
  through indirect tables and show no xrefs.
- `FindConst.java` — good for scanning the whole image for a literal; it ruled
  out a literal `cmp #0x64` in the app's control-surface code, which is why the
  encoder had to be found dynamically.

What did not pay off:

- guessing vtable slots for `mouseWheelMove`;
- assuming a CC handler would compare against `100` as an immediate;
- trusting the decompiler's parameter list for virtual/provider methods without
  checking the assembly (the older provider work already warned about this).

### 2.2 Emulator probes

The decisive encoder discovery came from an LD_PRELOAD `read()` interposer, not
from Ghidra. The useful loop was:

1. build the mod with `-DMINIMOD_LOG=1`;
2. `python3 lab.py preload mod/build/minimod-hpss.so`;
3. open the menu with the panel API;
4. turn the encoder and read `/run/minimod.log`;
5. print every read's `fd`, `n` and nonzero payload bytes.

That immediately exposed `fd=36` (`/dev/snd/midiC1D0`), `n=32`, and the payload
at `+16`.

Other probes that were worth keeping:

- **half-amplitude writer test** before HPSS: `source * 0.5` through stock
  Extract, compared sample by sample — it proved the writer substitution path
  before adding the DSP;
- **screen hashing** with `sha256sum` on `/screen.png` — the only reliable way
  to prove that encoder movement did not move the page underneath;
- **`/proc/<pid>/mem` + `dd`/`od`** inside the guest — no Python is installed,
  so this was the quickest way to read the live settings object and trim range;
- **panel API** (`POST /control`) and `/state` — driving SHIFT/DATA/encoder and
  checking LEDs without clicking the page;
- `systemctl show ac50 -p MainPID -p NRestarts` — fast regression check that a
  probe did not crash the app;
- `scp` + the sample browser — loading known WAVs and testing the real UI path.

The guest has no `python3`, `strace` or `gdb`; `dd | od`, `sha256sum`, `ss` and
the lab's SSH/panel APIs were the practical observation tools.

### 2.3 Host-side checks

- `mod/tests/hpss_dsp.py` compiles the production DSP and checks reconstruction,
  silence, stereo and invalid arguments.
- `tools/hpss/hpss.py` is the Python reference used for sample-by-sample parity;
  it is not the device code.
- `tools/hpss/wavdiff.py` compares two WAVs (max/RMS error).
- `tools/hpss/excerpt.py` and `tools/hpss/mix.py` built the manual demo pads;
  `tools/hpss/wavio.py` is their stdlib WAV layer.
- WAV comparison and demo building were done on the host because the guest has
  no Python.

### 2.4 What is in the repository vs external

In this repository:

- `mod/src/hpss.c` / `hpss.h` — shipped DSP.
- `mod/src/minimod.c` — bindings, SAMPLE OPTIONS UI, read interposer.
- `mod/tests/hpss_dsp.py` — DSP regression.
- `tools/hpss/` — Python reference, algorithms, WAV helper scripts.
- `tools/ghidra/hpss/` — the Java scripts and the MCP runner used here.
- `tools/emulator/` — the older in-repo lab; the final manual runs used the
  external `virtual-mpc-sample` checkout.
- `docs/re/hpss/` — captured decompilation and runtime evidence.

External, intentionally not copied into the repository:

- the Ghidra project `mcpsamplebin` and the firmware binary (large, local
  analysis artefacts; `.gitignore` excludes `analysis/`);
- `bridge-mcp-ghidra` and Ghidra itself;
- the `virtual-mpc-sample` emulator/lab and its `lab.py`;
- host tools used for observation: `ssh`, `scp`, `sha256sum`, `dd`/`od`,
  `tesseract` and a temporary numpy venv.

The reproduction commands in §4 assume those external pieces are installed.

## 3. Blind paths and rejected hypotheses

| hypothesis | why it was wrong |
|---|---|
| encoder goes through `buttons` | no rotation event; only notes/presses |
| encoder goes through `cs_event` | hook installed, never called for the wheel |
| encoder goes through `RotaryFixedConnection` | hook installed, never called |
| `0x159f230` is a CC dispatcher | site is not a valid CC entry; hook never installed. The old comment was wrong |
| screen `mouseWheelMove` vtable slot | `SampleModeScreen` does not override it; children handle/focus the event |
| literal `100` compare in app code | no such compare in the control-surface area; the encoder is not dispatched that way |
| `snd_rawmidi_read` interposer | the app reads the device with `read()` directly |
| 32-byte event is raw MIDI at offset 0 | only one MIDI byte per event, at offset 16 |
| rewrite CC to `0xFE`/CC 127/0 | the app still acted on the modified CC; only dropping the reads worked |
| patch live settings around the handler call | handler re-syncs settings; patch belongs in execute, around the copy |
| output depth follows the source (16/24) | stock writer emits float32 on this firmware |
| pad clear is a safe rollback | it broke project persistence and caused a missing-sample dialog |
| frame count alone identifies a WAV | rejected; HPSS requires frame count + channels + rate, and a unique fallback |

## 4. Reproduction checklist

```sh
# Ghidra scripts (from this repository)
python3 tools/ghidra/hpss/run_bridge.py ResearchDecompile.java 0x17580d0 0x153f0b0
python3 tools/ghidra/hpss/run_bridge.py ResearchDecompile.java 0x2a41f80 0x21c6e40 0x1e8aff0 0x1e93570
python3 tools/ghidra/hpss/run_bridge.py DumpVtable.java 0x6f212a0 70
python3 tools/ghidra/hpss/run_bridge.py FindConst.java 100 3000

# Device/emulator probe build
cd mod
zig cc -target aarch64-linux-gnu -shared -fPIC -O2 \
  -fno-omit-frame-pointer -mbranch-protection=standard -Wall -Wextra \
  -DMINIMOD_LOG=1 -o build/minimod-hpss.so src/minimod.c src/hpss.c

# Emulator
cd ../virtual-mpc-sample
python3 lab.py preload /path/to/mod/build/minimod-hpss.so
python3 lab.py panel
python3 lab.py ssh 'tail -f /run/minimod.log'

# Add Reverb tail: use a project with A01 loaded and an empty destination pad
python3 mod/tests/emulator_reverb.py --lab /path/to/virtual-mpc-sample
```

For a clean UI test, use `POST /screen.png` hashes before and after a sequence;
a visual diff alone is too easy to mistake for a layout change.
