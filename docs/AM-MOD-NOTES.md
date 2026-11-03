# am-mod: what we took from it

Upstream: <https://github.com/AlekseyMalykh/am-mod> (firmware r25/r26 era, same
`MPC Sample` binary family; its `FN(0x…)` offsets match ours).

## The one technique that matters: overdraw the screen

am-mod does not try to rename stock widgets. It **replaces the screen's paint
slot in the vtable** and draws on top of the stock painting, using the app's own
`juce::Graphics` entry points.

From `sd-mod-session/src/compressor_ui.c`:

```c
uintptr_t *vt = (uintptr_t *)(base + SCREEN_VT);   /* screen vtable          */
vt[0]  = (uintptr_t)destroyed;        /* destructor, if you need it          */
vt[1]  = (uintptr_t)deleted;          /* deleting destructor                 */
vt[27] = (uintptr_t)paint;            /* Component::paint(Graphics&)         */
```

* Slot **27** (byte offset `0xD8`) is `paint(void* this, void* Graphics&)`.
* The vtable lives in `.data.rel.ro`, so the page must be `mprotect`ed writable
  for the swap and read-only afterwards.
* Keep the original `vt[27]` and call it first if you want the stock UI under
  your drawing (am-mod redraws whole panels; we only add a label).

Drawing helpers (all of these also exist in 1.3.0.12 at the same offsets):

| offset | call |
| --- | --- |
| `0x102d5e0` | `Graphics::setColour(uint32 argb)` |
| `0x102e000` | `Graphics::fillRect(int x, int y, int w, int h)` |
| `0x1082810` | `Font::Font(int typeface, float height)` |
| `0x102dca0` | `Graphics::setFont(const Font&)` |
| `0x10817b0` | `Graphics::drawText(const String&, int x, int y, int w, int h, int justification, bool ellipses, float minScale)` |
| `0xf535c0` | `juce::String::operator=(const char*)` |
| `0x1070d60` / `0xf0a290` | `Font` / `String` destructors |

A full text helper:

```c
uint64_t str = 0, font = 0;
FN(0xf535c0, void (*)(uint64_t *, const char *))(&str, s);
FN(0x1082810, void (*)(uint64_t *, int, float))(&font, 0, size);
FN(0x102dca0, void (*)(void *, const uint64_t *))(g, &font);
FN(0x10817b0, void (*)(void *, const uint64_t *, int, int, int, int, int, int, float))
    (g, &str, x, y, w, h, 0x24 /* centred */, 1, 0.7f);
FN(0x1070d60, void (*)(uint64_t *))(&font);
FN(0xf0a290, void (*)(uint64_t *))(&str);
```

## What we used it for

`minimod` swaps `vt[27]` of `SampleModeScreen` (`base + 0x6e212a0`), calls the
stock paint, and — while SHIFT is held — fills the third soft-key cell
(`x 213..320, y 0..29`, black) and draws **"AutoTrim"** in it. That is the whole
overlay: no widget lookup, no label rename, no layout knowledge beyond the cell.

## Other notes worth keeping

* am-mod identifies screens and components by **vtable pointer comparison**
  (`*(uintptr_t*)p == base + SCREEN_VT`), never by name — cheap and exact. Its
  `values_ui.S` keeps a table of known screen vtables and walks `Component+0x18`
  (parent) to find the owning screen.
* Visibility is `isVisible() && !isShowing()`-style checks via
  `0x112fd50` / `0x112f6b0`.
* It repaints with `0x1149880(screen, 0, screen[0x28])`.
* It keeps a "ticket" (epoch + shift bit) so a queued control movement cannot
  land after the screen changed — a pattern worth copying for the trim commit.
