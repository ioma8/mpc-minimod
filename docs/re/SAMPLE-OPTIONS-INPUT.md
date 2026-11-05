# SAMPLE OPTIONS input — native DATA events

Firmware **1.3.0.12**, corrected **2026-11-05**. This is the current input
mechanism for HPSS / Spectral Chop / Close. It supersedes the raw-read,
evdev, rotary and float-property experiments in earlier notes.

## Why turns were ignored

The previous implementation inferred a detent from a changed float property
at mod offset `0x13bb930`. That function is a generic value setter, not the
DATA event source. Its first observation of a property was discarded, only
eight properties were tracked, and any subsequent change could set
`g_wheel_native`. Once set, that flag disabled the MIDI selection path for the
rest of the menu session, although the read wrapper continued dropping the
CC messages before native handling. Background changes could therefore
select a supposed input source that received no actual DATA events.

A host reproduction using the original production functions established the
failure: observe an unrelated property at 0.2 then 0.3, then deliver DATA +1.
The flag is 1 and selection stays unchanged on DATA. The old read wrapper also
assumed one payload byte per 32-byte record and mixed parser state across
file descriptors. Those transport assumptions are no longer part of menu input.

The earlier claim that DATA is a float-property change, or that hardware DATA
cannot use the firmware's decoded CC path, was not supported by the trace.

## Ghidra trace and ABI

Addresses below show **Ghidra VA / mod offset**. The difference is `0x100000`.

| Function | Ghidra | Mod | Role |
|---|---|---|---|
| decoded control dispatch | `0x169f230` | `0x159f230` | `(event, unsigned raw, float lo, float range)` |
| integer event emitter | `0x150dbc0` | `0x140dbc0` | `(signal, int delta)` — the menu hook |
| float value broadcaster | `0x14bb930` | `0x13bb930` | `(property, float value)` — not menu input |
| button dispatch | `0x169f2f0` | `0x159f2f0` | `(event)` — learns the control surface |

The dispatcher reads the controller number at `event+8` and follows
`context = *(event)`, `surface = *(context+0x10)`. K1/K2/K3 (`0x10/11/12`)
normalize their raw values and call the float broadcaster. DATA (`0x64`)
takes a different branch:

```c
int delta = raw > 0x40 ? (int)raw - 0x80 : (int)raw;
integer_event(surface + 0x2da8, delta);
```

The decisive instructions at Ghidra `0x169f280` are:

```text
cmp  w3, #0x64
b.ne return
ldr  x0, [x0, #0x10]
cmp  w1, #0x40
sub  w2, w1, #0x80
csel w1, w2, w1, hi
mov  x2, #0x2da8
add  x0, x0, x2
b    0x150dbc0
```

`FUN_0150dbc0` locks the signal's mutex at `+0x20` and walks its listener
list, invoking vtable slot `+0x28` with the integer delta. It has **no cached
value comparison**: +1 followed by +1 is two events. The float broadcaster,
in contrast, compares/stores `property+0x50` before notifying. Ghidra's
inferred mixed float/integer prototypes are misleading; the instruction
registers establish the ABI (`x0`, `w1`, `s0`, `s1`).

The integer emitter is shared by other signals. Hooking it unconditionally
would hijack unrelated input, so matching the DATA signal object is required.

## Current implementation

In `mod/src/minimod.c`:

1. `buttons()` learns `surface+0x2da8` from the native button event before
   opening SAMPLE OPTIONS and publishes it atomically as `g_data_signal`.
2. `data_signal()` intercepts only that exact object. With the menu open, a
   nonzero delta moves one item according to its sign and requests repaint.
   Identical consecutive deltas are never deduplicated. Zero does nothing.
3. The event is consumed while the menu or Spectral Chop owns input. During
   processing it does not move the selection. With the menu closed, or for
   any other signal object, the original emitter runs normally.
4. `cc_capture()` is only a native modal filter: it blocks knob/fader controls
   behind the menu and passes DATA to the firmware's decoder and integer
   signal. It never moves the selection itself.
5. The DATA button gesture is still consumed independently, including its
   release, so confirming/closing the menu cannot also trigger a stock action.

There is **one navigation source**. The custom `read()` interposer, evdev
fallback, float-property wheel inference, source-selection latch, guessed
relative encodings, raw-message guards and rotary stub were removed. ALSA
framing, batching and message decoding remain the firmware's responsibility.
The existing float-broadcast hook remains for AutoTrim diagnostics only.

## Reproduce and validate

Decompile using the project's Java scripts through the Ghidra MCP bridge:

```sh
GHIDRA_MCP_ALLOW_SCRIPTS=1 ~/Applications/ghidra_12.2_PUBLIC/ghidraRun ~/mcpsamplebin.gpr
python3 tools/ghidra/hpss/run_bridge.py ResearchDecompile.java 0x169f230 0x150dbc0 0x14bb930
python3 tools/ghidra/hpss/run_bridge.py HpssInstructions.java 0x169f230
```

Build and checks:

```sh
./mod/build.sh
./mod/pack.sh
python3 mod/tests/menu_input.py
python3 /path/to/virtual-mpc-sample/lab.py preload mod/build/minimod.so
python3 mod/tests/emulator_menu.py   # Python with Pillow; visible Sample page
python3 mod/tests/emulator_spectral.py --lab /path/to/virtual-mpc-sample --boots 1
```

The host test executes the production hooks with stub native callbacks. It
checks 200 repeated signed events, zero, unrelated signals, busy suppression,
closed-menu forwarding and separation from the knob filter. The release
emulator test checks 96 turns across three openings, both directions, ignored
knobs and closing via DATA. The Spectral regression passed six applications (including a repeat,
Threshold mode and pad playback), verified exact slice boundaries after
re-entry, and observed no firmware restart. Its test labelled “regions”
reported initial type 0 in this run, so that label is not evidence of a
different starting mode.

These checks exercise the actual native hook in the emulator. The corrected
package still needs confirmation on the physical device; do not describe
emulator results as a hardware test. The SD package is `mod/build/minimod.zip`.
