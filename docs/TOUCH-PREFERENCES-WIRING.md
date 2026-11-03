# TouchPreferences wiring investigation

All addresses in this note are Ghidra addresses for `knob.bin`, firmware
1.3.0.12. Subtract 0x100000 for the mod's file-VA constants. Findings here are
static decompilation evidence, not a successful runtime activation.

## Constructor and dependencies

`03337be0(self, dependency2, dependency3)` constructs the component, sets the
name Preferences, and calls the base constructor `02f90170`. It stores:

| Object offset | Meaning supported by code |
|---|---|
| +0x130 | dependency3; used for key/action lookup and observer registration |
| +0x138 | dependency2; passed to the settings flush routine on Apply |
| +0x140 / +0x148 | begin/end of tab component pointer vector |
| +0x158 | sidebar component allocated as 0x148 bytes |
| +0x160 | content container allocated as 0xb0 bytes |
| +0x168 | footer button allocated as 0x1d8 bytes |

The root has fields through +0x168; its exact allocation size is not established.
The constructor ends with `0125f7b0(self, 720, 740)`.
It registers an observer through `01493b40(dependency3+8, self+0x118, 1)`.
Passing arbitrary/null context pointers is therefore unsafe.

The constructor's analyzed references are metadata/data references, not direct
code callers. This does not prove the screen is unreachable: indirect factories
or unanalyzed code may create it. Neither the live dependency objects nor the
population/factory routine has yet been identified. Constructing the root alone
does not show where tab components get added to its vector.

## Layout and navigation

Root vtable: `070787d8`. Its resize handler is `03336b90` (slot 34).
For width W and height H, it sets:

- sidebar: x=0, y=0, width=114, height=H-30;
- content: x=114, y=0, width=W-114, height=H-30;
- footer button: x=W-70, y=H-24, width=64, height=18.

At 320x240 this leaves 206x210 for content. The parent adapts its rectangles,
but that does not establish that any tab's controls fit. Scaling the original
720x740 uniformly to fit would use approximately 0.324 scale and a width of 234
pixels; that is unsuitable for readable settings text.

`03336d10(self, index)` switches tabs: clears the content container, validates
index against the vector, invokes the tab's activation method at vtable+0x168
when overridden, then adds the chosen component to the content container using
`01279110`. Use this routine rather than editing the selected child manually.

## Apply and close

- `03336c70(self, clickedButton)` handles the footer button. It invokes each
  tab's overridden vtable+0x160 method, calls `010bea70(self->dependency2)`,
  then closes through `02f8ee00(self, 1)`.
- `010bea70` takes a mutex at dependency2+0x38 and conditionally flushes dirty
  settings (flag at +0x109). The precise dependency type is not yet named.
- `03336c10(self)` closes through `02f8ee00(self, 1)`.
- `03336c20(self)` queries action 0x3104 through dependency3 and can close with
  result 0. Its precise input-event semantics need runtime confirmation.
- `02f8ee00` walks parent components, detects a native screen/window host using
  RTTI, and removes/closes the screen through that host. This supports attaching
  to the existing host rather than painting the component independently.

## Candidate adaptation, not yet implemented

Use a compact parent host with a tab selector operated by DATA, give the selected
tab the full screen width below a small header, and provide a viewport for content
that exceeds the available area. Retain native tab activation and Apply/close.
Native child focus and editing must be mapped to hardware controls before this
can be considered usable; simply showing a touchscreen panel is insufficient.

First runtime experiment should capture the live dependencies and tab factory,
then open/close the native component without settings edits. After that, measure
the child layouts at 320px width to choose between reflow and horizontal panning.
SHIFT+DATA should consume both press and release, avoid opening duplicates, and
provide a reliable exit. No button hook has been added yet.

## Corrections to earlier notes

`PREFERENCES-TABS.md` inferred a physical touchscreen entry point from the class
name and unused panel combinations. That inference is not proof of a reachable
entry point on this hardware. Likewise, merely calling the constructor does not
establish valid ownership, tab population, or input handling.
