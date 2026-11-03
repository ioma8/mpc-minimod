# Chromatic MIDI in 16 Levels — investigation log

Goal: an external MIDI keyboard should play the **selected pad's sample
chromatically** in 16 Levels, instead of triggering one sample per key.

Status: **not working.** The injection point is identified and proven, but the
app's 16 Levels pitch path never runs in the emulator, so there is nothing to
drive. This file records every finding, including the approaches that failed, so
the next attempt does not repeat them.

All addresses are **file VAs** for firmware 1.3.0.12. Ghidra addresses are
file VA + `0x100000` — mixing them up produced two wrong conclusions earlier in
this investigation (see §7).

## 1. The audio/MIDI topology

ALSA cards inside the device:

| card | id | what it is |
|---|---|---|
| 0 | `Loopback` | the app's audio tap |
| 1 | `Surface` | control surface **and pads** (SerialMIDI) |
| 2 | `MIDI` | rear MIDI jack (SerialMIDI) |
| 3 | `fmidi` | USB MIDI gadget |

The app opens **card 1 only** (two descriptors). It never opens card 2, so an
external keyboard on the rear jack reaches nothing. The panel's MIDI log confirms
the bytes arrive (`in 90 26 64` …) and are then dropped.

Writing to a rawmidi node does **not** feed the app's input: bytes written to
`/dev/snd/midiC1D0` appear in the panel log as `out`, i.e. they go out to the
wire. This kills the "echo the note onto the Surface card" idea (§4).

## 2. The pad path

A panel pad press fires exactly two of the mod's hooks:

```
minimod: cs code=0x04
minimod: note_on pad=4 vel=0.79        ← PAD 5
```

So pads call `note_on` (`A_NOTE_ON`, file `0x1e27360`; Ghidra `0x1f27360`):

```c
void FUN_01f27360(undefined4 a0, long a1, int pad, undefined4 a3, long a4)
```

* `a1` and `a4` are pointers (`a4` is read at `+0x158`); `a4 == 0` is handled.
* The mod declares `(void *input, int src, int pad, void *props, float vel)`.
  forwarding works because the app ignores the spare `s0` float, but `src` is
  truncated to `int` — harmless only because the value is passed straight back.
* `cs code=0x04` is the control-surface side of the same press.

Pads arrive from the panel as **channel-10** notes (`99 25 vv` for PAD 1, ids
37…52). Channel-1 notes (`90 …`) take the plain MIDI map — that is why an
injected `90` note ignores 16 Levels and why the bridge must speak `99`.

## 3. Replaying a pad press (works, but is not the pitch path)

The mod reads card 2 itself, maps `k = (note - 36) mod 16`, and calls the app's
own `note_on` with the arguments captured from a real pad press:

```
note_on pad=4 vel=0.79                 ← template captured from PAD 5
midi note=48 -> pad 12
midi note=52 -> pad 0
midi note=55 -> pad 3
```

The call executes cleanly (app stable, panel responsive, 0.07 s redraw) **but the
app plays that pad's own sample**. Replaying `note_on(pad)` reproduces a normal
pad press, not a 16-Levels step — so the pitch decision is not inside `note_on`.

## 4. Approaches that do not work

| approach | why it fails |
|---|---|
| read card 2 in the mod and play pads | the app never opens card 2, so no notes arrive at all |
| read card 2 **and** echo onto card 1 | writes go out to the wire (`out` in the panel log), not into the app |
| that echo, continued | it floods the control-surface stream; the app stops accepting buttons |
| `read()` interposer + suppress/rewrite | only fires when the app reads a MIDI node; it does not open card 2 |
| replay `note_on` with a captured template | plays the pad's own sample (see §3) |
| `A_PITCH_OFF` (`0x27e4400`) | not a pitch setter: it builds a MIDI-style byte plus a bitmask; it never runs during playback |
| `sixteen_proc` (`0x21472f0`) | never called in the emulator, even with 16 Levels toggled |

## 5. 16 Levels does not engage in the emulator

Measured, not inferred:

* 16 LEVELS is button code `0x28` (panel: id 40, `90 28 7F`, channel 1).
* Pressing it and then PAD 5 gives `note_on pad=4` and the header shows
  **A05 HipHop-Kik-BeYou2** — the pad's *own* sample, not a pitch of A01.
* `sixteen_proc` count over the whole session: **0**.
* `pitch offset` (the `A_PITCH_OFF` hook) count: **0**.

So the app is not in the pitch mode the feature needs. Either the button only
arms the mode and something else enables it, or the mode applies to a screen the
emulator never reaches (the device has no digitizer, so screens are reached
through hardware buttons only).

**This is the open question.** Until a pad press produces a pitched note,
chromatic MIDI cannot be built on the 16-Levels path.

## 6. Operational facts (cost hours, worth writing down)

* `/run` is **tmpfs**: `lab.py preload` installs the library and the systemd
  drop-in there, so **a reboot always returns to the stock service**. Re-preload
  after every boot.
* The root filesystem is **read-only** (dm-verity): `mkdir /usr/local` fails with
  "Read-only file system", so the mod cannot be installed persistently.
* `lab.py preload` **rewrites the drop-in**, dropping any `Environment=` lines
  you added. Re-add `MINIMOD_MIDI` / `MINIMOD_DEBUG` afterwards.
* Restarting the app (`systemctl restart ac50`) does **not** break the panel:
  measured 0.07 s redraw afterwards. What broke it was the mod flooding the
  control-surface wire (§4).
* Reboot with `lab.py stop` then `lab.py run --headless`; the panel is a separate
  host process and must be restarted (`lab.py panel`) after a VM restart.

## 7. Corrections to earlier notes

* `docs/MOD-REFERENCE.md` claimed "Chromatic control — external MIDI — works".
  It does not; the claim is corrected there.
* Two "wrong anchor" conclusions during this investigation were my own address
  error (using file VAs as Ghidra addresses). The anchors are correct:
  `note_on` at `0x1e27360`, `sixteen_proc` at `0x21472f0`, `pitch_off` at
  `0x27e4400`.
* The earlier "octave shift works" confirmation was based on the key handler's
  log, not on audio: `A_PITCH_OFF` never fires during playback, so the `+`/`-`
  keys change the mod's state and nothing else.

## 8. Next step

Find the function that a **genuinely active** 16 Levelpad press calls. With the
mode actually engaged, a pad press must produce a different call than
`note_on(pad)`; the mod's existing hooks (buttons, cs_event, note_on) will show
which one fires. That call is the injection point the keyboard should replay.
