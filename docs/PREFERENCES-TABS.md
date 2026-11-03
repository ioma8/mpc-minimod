> See [TouchPreferences wiring investigation](TOUCH-PREFERENCES-WIRING.md) for
> verified constructor, layout, tab-switch and close routines. The touchscreen
> entry-point claims below are historical inferences, not runtime evidence.

# How the Preferences tabs are (not) reachable

The ten tabs observed in the binary — `WiFiTab`, `EthernetTab`, `BluetoothTab`,
`AudioTab`, `InfoTab`, `LegalTab`, `BackupRestoreTab`, `SoftwareUnlockTab`,
`CrashReportsTab`, `NotImplementedTab` — all belong to one screen,
`TouchPreferences`. This document records how that screen can be opened, and why
it is undocumented anywhere.

## 1. There is no panel button for it

The lab ships a complete map of the front panel (`stock-map/build/notes_en.json`),
built by pressing every button and every SHIFT combination on stock firmware. It
has **56 entries** and none of them opens a settings/preferences screen:

```
SAMPLE  SEQ  PAD FX  KNOB FX  ERASE  NOTE REPEAT  PAD BANK  CHOP  MUTE  LOOP
16 LEVELS  SAMPLE SELECT  TAP TEMPO  MINUS  PLUS  SAMPLE RECORD  SEQ RECORD
PLAY  STOP  DATA
SHIFT+SAMPLE (INPUT CONFIG)   SHIFT+SEQ (STEP EDIT)   SHIFT+PAD FX (FLEX BEAT)
SHIFT+KNOB FX (FX SELECT)     SHIFT+ERASE (COPY)      SHIFT+NOTE REPEAT
SHIFT+PAD BANK                SHIFT+CHOP              SHIFT+MUTE (UNMUTE ALL)
SHIFT+LOOP (REVERSE)          SHIFT+16 LEVELS (TYPE)  SHIFT+SAMPLE SELECT (SAVE)
SHIFT+TAP TEMPO (METRO)       SHIFT+MINUS (UNDO)      SHIFT+PLUS (REDO)
SHIFT+SAMPLE RECORD (RECALL)  SHIFT+SEQ RECORD (RECALL)  SHIFT+PLAY (CONTINUE)
SHIFT+STOP                    SHIFT+PAD 1…16 (FULL LEVEL … PROJECT)  SHIFT+DATA
```

The panel has 45 physical controls in `panel/controls.json` — no menu/settings key
among them. `SCREEN 1–3` are the *sample-page* tabs (Trim / Tune / Filter,
confirmed in the screenshot), not Preferences.

## 2. It is a touchscreen screen

`TouchPreferences` is a Component with its own tab strip; nothing reachable from
the front panel raises it. The only input the device has beyond the panel is the
digitizer — so the entry point is a touch target on the LCD.

That is also why the tabs are documented nowhere: the panel-only documentation
paths (the lab map, and the vendor's own button guide) never see them.

**And it cannot be reached in the emulator**: the app ignores pointer input. The
lab states it in `panel/server.py:414` (*"the app ignores tablet/touch input"*),
and it was verified by injecting an absolute pointer tap over QMP
`input-send-event` onto the `virtio-tablet` — 0 px change on screen and no state
change. The app opens `/dev/input/event0..2` (tablet, keyboard, gpio-keys) but
acts on none of the tablet events.

## 3. The settings table is shared MPC code

The Preferences screen pulls its rows from a large settings table that includes
entries which cannot apply to this hardware:

```
WALL POWER DIMMING / BRIGHTNESS     FOOTSWITCH 1 / 2
TOUCH STRIP BEHAVIOUR               CLIP WORKFLOW / MAIN MODE
STEP SEQUENCER BUTTON MODES         TRACK MUTE STATE PER SEQUENCER
```

together with ones that clearly do (MIDI INPUT/OUTPUT, PADS, DISPLAY RESOLUTION,
DATE & TIME). So `TouchPreferences` is the common MPC preferences implementation;
which tabs a given model builds is decided in code, not by a string.

## 4. Options to actually open it

1. **On real hardware** — tap the settings target on the touchscreen. That is the
   only user-facing path, and it exists only there.
2. **In the emulator** — impossible as shipped: no touch delivery reaches the app.
   Would need the lab to emulate the device's touch controller (the virtio-tablet
   is ignored).
3. **Programmatically (mod)** — call the `TouchPreferences` root component and
   make it visible, then select a tab index. This is the only way to see the tabs
   *in the lab*. What is still missing: the address of the code that creates /
   shows that component, and its "add/select tab" virtual. Finding it needs the
   Ghidra session restarted (it was stopped) or a deeper local xref pass on the
   tab constructors (`TouchPreferences::*Tab`).

## 5. What this means for the earlier findings

* `docs/BLUETOOTH.md` §2.4–2.5 and `docs/WIFI.md` §3.2 remain correct: the tabs
  exist as normal production UI, and no string-level flag gates Bluetooth or
  WiFi. The reason they are invisible is this document's subject — the screen is
  touch-only — not a gate.
* The developer diagnostics tab (`BLUETOOTH.md` §2.5) is separate again: it is
  compiled out of the call graph entirely, so even a working touchscreen would
  not show it.

## 6. Reproduce

```sh
# panel map: every button and SHIFT combination, no settings entry
python3 -c "import json;d=json.load(open('stock-map/build/notes_en.json'));print(len(d))"
grep -c . panel/controls.json     # 45 physical controls, none is menu/settings

# the app ignores injected touch (expect 0 px)
#   QMP input-send-event abs x/y + btn on the virtio-tablet, then screendump

# tabs exist as classes
strings -a firmware/extracted/part2 | grep -aoE "TouchPreferences[0-9]+[A-Za-z]+Tab"
```
