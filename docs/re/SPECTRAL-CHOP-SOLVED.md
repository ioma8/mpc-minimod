# Spectral Chop — solved: mechanisms, implementation, and the path there

Firmware **1.3.0.12**. Branch `feature/spectral-chop`. Every address is a
**mod offset**; the matching Ghidra VA is `offset + 0x100000` (see §2.6).
Everything marked *verified* was measured on the emulator in this lab.

## 1. The result

**SAMPLE OPTIONS → Spectral Chop** detects the onsets of the sample on the
selected pad and rewrites that pad's chop so every slice boundary is one of the
detected frames. It creates no samples, uses no free pads and writes no file.
It reports `n slices` only after reading every boundary back.

Verified by `mod/tests/emulator_spectral.py` with the release build, one boot,
release library digest checked on the device:

```
PASS boot 1: first from Trim; exact 10 boundaries, Manual, pid=13416
PASS boot 1: repeat;         exact 10 boundaries, Manual, pid=13416
PASS boot 1: repeat again;   exact 10 boundaries, Manual, pid=13416
boot 1: regions initial type=0     PASS boot 1: regions
boot 1: threshold initial type=1   PASS boot 1: threshold
PASS boot 1: after pad playback
PASS: menu, model, repeat, modes, playback, re-entry and restart checks
```

The test pins the fixture's exact boundaries:
`[0, 1536, 12288, 24064, 36352, 48128, 60416, 67584, 72192, 84480]` with the
last slice ending at `88200`. It also asserts the chop type is Manual (`0`),
that the pid does not change, that `NRestarts` does not change, and that
re-entering the Chop view keeps the markers.

## 2. The model

The firmware-wide reference (loader, screen/paint, layers, properties, commands,
input, sample pool, persistence, threading) is
[FIRMWARE-MECHANISMS.md](FIRMWARE-MECHANISMS.md). This section is only what the
Chop action needs.

### 2.1 Objects

```
screen                 SampleModeScreen; vtable 0x6e212a0 (checked at runtime)
screen + 0x168         chop parameter binding          (§2.4)
screen + 0x178         pad controls
screen + 0x180         selection object; +0x618 = "select slice" input
screen + 0x1ee0        Chop group visibility; visible <=> chop mode
                       (FN 0x112fd50 on the sub-object answers "is visible")

controls + 0x60        pad state
state  + 0x22e8        std::vector<Layer>, stride 0xb30   (0x2904ae0 walks it)
layer  + 0x418         selected marker: 0x80 start, 0x81 end, else slice index
layer  + 0x4a8         the Sample
layer  + 0x6e0 bit 0   chop flag
state  + 0x60          command manager for the slice commands
```

### 2.2 The sample and its slice container

```
sample + 0x130     -> meta
meta   + 0     u16 channels
meta   + 4     u32 frames
meta   + 8     u32 sample rate
meta   + 0x70 / 0x78 / 0x80   vector<Slice*> begin / end / capacity
meta   + 0x10  the slice container ("cont") the native API below operates on
```

### 2.3 A Slice

```
Slice + 0x78    start frame        (mirror at +0x128)
Slice + 0xd0    end frame
```

~0x340 bytes each, 16 slices maximum. Adjacent slices share the boundary:
`slice[i].+0xd0 == slice[i+1].+0x78`. The stock K1/K2 knobs edit exactly these
words for the slice a pad press selects — measured, not inferred: K1 moved
slice 0's `+0xd0` 2465 → 4906 and K2 moved slice 1's `+0xd0` 4907 → 5143 →
5295 → 6051, carrying the neighbour's mirrors each time.

### 2.4 The chop parameter binding

`screen + 0x168` points at the binding that owns the chop type:

```
binding + 0x80   cached parameter object   (fallback setter lives here)
binding + 0x9c   chop type, int            Manual == 0
binding + 0xc0   parameter / target block passed to the callback
binding + 0xd0   manager
binding + 0xd8   model callback (invoke)
binding + 0xe0   owner
binding + 0xe8   flags; bit 0 selects the fallback path
```

Setting the type the way the stock UI does:

* flags bit 0 clear and an owner present →
  `((void(*)(uintptr_t, uintptr_t, const int *))*(binding+0xd8))`
  `(binding+0xc0, owner, &manual)` with `manual = 0`;
* otherwise `FN(0x15d2840)(binding + 0x80, 0)`.

This is the path the stock Split handler takes (Ghidra `0x1756728`). Assigning
the view's cached enum directly does **not** survive: the model callback is what
preserves the pad's chop type. The type is then read back from `binding + 0x9c`
and must be `0`.

### 2.5 The native slice API

| purpose | mod | Ghidra | signature |
|---|---|---|---|
| reset the list | `0x2950aa0` | `0x2a50aa0` | `void (cont)` |
| insert a slice at a frame | `0x2950d20` | `0x2a50d20` | `int (cont, frame, flags=1, frames)` → new index |
| reconcile ends incl. tail | `0x2950bb0` | `0x2a50bb0` | `void (cont, frames)` |
| select a slice | `0x16680a0` | `0x26680a0` | `void (selection + 0x618, const int *index)` |
| Manual, fallback path | `0x15d2840` | `0x25d2840` | `void (binding + 0x80, 0)` |

`FUN_02a50d20` (decompiled) constructs the slice, registers its listeners
(`FUN_02a4b170`, `FUN_02a4a080`), notifies observers (`FUN_0102d0d0`), links it
into the container (`FUN_0246b140`) and calls `FUN_02a50bb0` to reconcile the
neighbouring ends. **The firmware owns allocation, destruction, listener wiring,
selection and the broadcast**, which is why the view updates by itself and why
no rebuild trick is needed.

### 2.6 Address spaces

Two spaces are in play and mixing them is the single most expensive mistake in
this investigation:

| space | example | used by |
|---|---|---|
| Ghidra VA | `0x2a50d20` | the decompiler, all RE docs |
| mod offset | `0x2950d20` | `FN()`, `install_hook()`, `mk` |

`Ghidra VA = mod offset + 0x100000`. Runtime address = `base + mod offset`,
where `base` is the module's load address (`0xaaaaaaaa0000` in the lab, but it
is a PIE — always read it from `/proc/<pid>/maps`).

## 3. How the action runs

### 3.1 Threads

| thread | who | may touch the model? |
|---|---|---|
| input hook | `buttons()`, `cc_capture()` | **no** |
| UI thread | `screen_paint()`, after the stock paint | **yes** |
| probe poll | the `/run/minimod.chop` reader | no — it only publishes |

The input hook runs on the control-surface thread. A CHOP press only
*schedules* a page switch, so it cannot be followed by a sleep, a poll or a
second toggle in the same callback. Hence a phase machine driven from the paint
hook.

### 3.2 Phases

`spectral_start()` (input hook) does one thing: CAS `g_spectral_active`
`0 → -1`, copy the button event, store `1`, request a repaint. Everything else
happens in `spectral_tick()` (UI thread).

| phase | work |
|---|---|
| 0 | close our menu, mark busy, resolve the sample, locate its WAV, run the detector, validate the positions, press CHOP if needed, select Manual, `deadline = now + 3000 ms` |
| 1 | wait until chop mode **and** `type == 0`, then reset + insert + reconcile + select slice 0 + verify, `deadline = now + 150 ms` |
| 2 | after the settle, verify once more and report |

Each pass re-resolves the sample and aborts with `sample changed; cancelled` if
screen, state, sample, frames, channels or rate differ. Failures are explicit:
`no sample on this pad`, `cannot read sample WAV`, `no onsets`, `invalid onsets`,
`cannot select Manual chop`, `Manual chop did not open`, `native marker write
failed`, `marker verification failed`.

### 3.3 Input suppression and state

While `g_spectral_active` is set the `read()` filter drops control-surface
bytes, the same way the SAMPLE OPTIONS menu does while `g_hpss_menu` is set, so
a knob turn cannot race the operation. `g_hpss_busy` keeps the menu closed for
the duration. The flag is atomic (release/acquire) because it crosses threads.

### 3.4 Writing and verification

```
FN(0x2950aa0)(cont);                                       /* reset            */
for i in 0..n-1:
    index = FN(0x2950d20)(cont, pos[i], 1, frames);         /* insert, in order */
    if index != i: fail                                     /* order is proof   */
FN(0x2950bb0)(cont, frames);                               /* ends + tail      */
FN(0x16680a0)(selection + 0x618, &first);                  /* select slice 0   */
verify: count == n, slice[i].+0x78 == pos[i],
        slice[i].+0xd0 == (i+1 < n ? pos[i+1] : frames)
```

The implementation deliberately does **not** shorten the container's vector and
does **not** write observable Slice fields directly.

## 4. Mechanisms catalogue

Everything below was used to get here or is used by the shipped code.

### 4.1 Chop-model mechanisms

| mechanism | how |
|---|---|
| find the selected pad's sample | `screen+0x178` → `+0x60` → `+0x22e8` → layer `+0x4a8` |
| identify the sample's format | `sample+0x130` → `{u16 ch, u32 frames, u32 rate}` |
| enumerate slices | container vector at `meta+0x70`, stride 8, count `(end-begin)/8` |
| read/write a boundary | `Slice+0x78` start (mirror `+0x128`), `+0xd0` end |
| insert a slice properly | native `0x2950d20(cont, frame, 1, frames)` |
| clear the list | native `0x2950aa0(cont)` |
| fix adjacent ends and the tail | native `0x2950bb0(cont, frames)` |
| move the selection | native `0x16680a0(selection+0x618, &index)` |
| force Manual chop type | binding model callback via `binding+0xd8` |
| read the chop type | `binding+0x9c` (Manual `0`) |
| detect chop mode | `FN(0x112fd50)(screen+0x1ee0) != 0` |
| detect the Sample page | `FN(0x112fd50)(screen) != 0` plus the vtable check |

### 4.2 Runtime and instrumentation mechanisms

| mechanism | how |
|---|---|
| run code inside the firmware | `LD_PRELOAD` an aarch64 `.so` into `/usr/bin/MPC Sample` |
| resolve the module base | parse `/proc/self/maps`; base is not fixed (PIE) |
| hook a function | patch a `B <relay>` at the entry; a trampoline copies up to four prologue instructions, stopping before any PC-relative one, and returns the trampoline as `orig_*` |
| call a function by offset | `FN(offset, signature)(...)` |
| read/write foreign memory | `pread`/`pwrite` on `/proc/self/mem` (`rd()` / `wr()`) |
| logging | `mlog()` → `/run/minimod.log` (debug builds), read by line offset from the host |
| drive the mod at runtime | `/run/minimod.chop` command file, consumed by a poll thread and executed on the UI thread |
| watch who writes a field | `mprotect`-based page watch: SIGSEGV handler logs address, pc, lr, sp |
| find who mutates a list | hook the list accessor and log the caller's return address |
| find a field the UI edits | snapshot **every word** of every object on every button press and knob turn, then diff |
| catch a rebuild | hook the allocator (`alloc` probe) — which also proved the app does **not** use libc `malloc` for slices |
| search memory for a value | `findval` (u32) and `findq` (u64/double) over all writable mappings |
| read runtime vtables | read the object's first qword and print entries as offsets — several `.data.rel.ro` classes read as garbage statically |
| suppress input safely | drop bytes in the `read()` filter while a flag is set |
| cross-thread hand-off | atomic CAS + release/acquire flag, one phase per UI pass |

### 4.3 Verification mechanisms

| mechanism | how |
|---|---|
| drive the UI without a human | lab panel HTTP: `/control`, `/screen.png`, `/state?since=0` (LED 75 = CHOP, 82 = PLAY) |
| measure drawn markers | scan row `y = 92` of the 320×240 screenshot for grey pixels (`|r-g|<8`, `|g-b|<8`, `90<r<220`), collapse runs, map x → frame through the waveform box |
| read the model from the host | `dd` from `/proc/<pid>/mem` over ssh |
| locate a static in a stripped build | `nm` on an unstripped link → `build/minimod.map` (`g_screen`); the shipped object is the same link with `-s` |
| prove the base is right | assert `*(screen) == stock_load_address + 0x6e212a0` |
| prove the deployed library is the build | compare `sha256sum` on the device with the local file |
| prove nothing restarted | `pidof` and `systemctl show ac50 -p NRestarts` before/after |
| prove the pads still work | press pads after the write and re-read the model |

### 4.4 Traps that cost time

* **Address space.** A hook installed at `base + GhidraVA` lands 0x100000 past
  the target; the probe "never fired" for an entire pass because of it.
* **The input hook is not the UI thread.** Blocking it or touching the model
  from it is unsafe.
* **A page switch is asynchronous.** Pressing CHOP and polling in the same
  callback cannot work.
* **The Chop view caches its layout.** A repaint or a pad press does not redraw
  markers after a raw field write; only a rebuild does. The native API avoids
  the problem entirely by broadcasting.
* **`meta+0xb0…` of the layer are not `SlicePoints`.** Those words are inline
  `std::string` data; the deserializer's layout belongs to a different class.
* **The app has its own allocator.** An `LD_PRELOAD` `malloc` hook never sees
  slice allocation, so the mod must never allocate a Slice.

## 5. The path here

### 5.1 Passes

1. **Threshold chop** — selected the stock Threshold mode, so markers appeared
   at the firmware's positions and our detector was ignored. Replaced with a
   fail-safe (`Spectral: not applied`).
2. **Extract** — writing slices through the Extract command restarted the app.
   Removed.
3. **Command API hunt** — found `Insert`/`Split`/`Remove`/`Remove All`
   (`0x1a60af0`, `0x1a60d50`, `0x1a5d1c0`, `0x1a5cf40`) and the UI wrapper
   `0x21a3b10`; all need an "editor" object that three heap scans failed to
   find. No write path.
4. **Address-space fix** — `HPSS-HANDOFF.md` and `docs/re/README.md` both state
   the `+0x100000` rule. With hooks finally on the right functions,
   `isSampleValid` on `layer+0x4a8` returned 1, and Split/Merge were seen to
   fire **no** command and no dispatcher: the UI edits the list directly. That
   redirected the work to direct model edits.
5. **Direct writes** — the boundary layout was pinned by a human-in-the-loop
   snapshot (§5.2); writing `+0x78`/`+0xd0`, shrinking the vector and toggling
   CHOP produced the first correct chop. This is what the release build shipped
   at `9b299b9`.
6. **Native API** — the container's own `Reset`/`Insert`/`Reconcile` were
   located, together with the binding callback for the chop type. The direct
   writes, the synthetic Split presses, the vector shortening and the CHOP
   toggle were all removed. The action also moved onto the UI thread as a phase
   machine, input suppression was added, and the emulator acceptance test was
   written. This is the shipped implementation.

### 5.2 The human in the loop

Two remarks changed the investigation more than any probe:

> "the knob 1 or 2 move boundaries of selected chop … you select given chop by
> pressing a pad"

> "split button works only when chop type is set to manual"

They became one experiment: snapshot every word of every slice on every button
press and every knob turn, then have the user perform the gesture in the panel.
The diff was unambiguous — `+0xd0` with its `+0x78`/`+0x128` mirrors.

### 5.3 Disproved along the way

* the rebuild lives in the function containing `0x1a601d0` — it is an unwind
  cleanup;
* the editor is findable by scanning for `*(p+0x4c8) == sample` — never
  observed at all;
* a pad press inserts a slice through `0x21a3b10` — it only moves the selection;
* slices are allocated per edit through libc `malloc` — the app has its own
  allocator;
* `layer+0xb0…+0xf8` hold `SlicePoints` — inline string data;
* `+0xd0` is a start frame — it is the end;
* K1/K2 are zoom controls — a false conclusion from a test that selected the
  start/end marker instead of a slice;
* writing the fields and rebuilding is the way to apply a chop — it works, but
  the native API is correct and needs no rebuild.

`HYPOTHESES.csv` holds the numbered record of all of them.

## 6. Reproducing it

### 6.1 Build

```sh
./mod/build.sh      # build/minimod.so + build/minimod.map (g_screen offset)
./mod/pack.sh       # build/minimod.zip (SD autorun package)
```

`build.sh` links twice: an unstripped `build/minimod.elf` from which `nm` takes
`g_screen` into `build/minimod.map`, then the shipped `build/minimod.so` with
`-s`. The layout of the two links is identical. `zig cc` rejects `-Map` in every
spelling, which is why the map is produced this way.

Debug build with logging and the probe:

```sh
zig cc -target aarch64-linux-gnu -shared -fPIC -O2 -fno-omit-frame-pointer \
  -mbranch-protection=standard -Wall -Wextra -DMINIMOD_LOG=1 -DMINIMOD_DEBUG=1 \
  -o mod/build/minimod-dbg.so mod/src/minimod.c mod/src/hpss.c mod/src/spectral.c
```

### 6.2 Run

```sh
cd ~/projects/customs/virtual-mpc-sample
python3 lab.py run --headless
python3 lab.py preload /path/to/mod/build/minimod.so
```

Put the **Drums 2s** fixture on `A03` (88200 frames) — the test asserts it.

### 6.3 Test

```sh
python3 mod/tests/emulator_spectral.py --lab ~/projects/customs/virtual-mpc-sample --boots 1
```

It restarts `ac50`, checks the deployed digest against the local file, derives
the module base from `/proc/<pid>/maps`, validates `g_screen` through the screen
vtable, walks to the sample, runs the real menu action, and compares exact
boundaries across first-run, repeats, Regions, Threshold, pad playback and CHOP
re-entry.

### 6.4 Probe commands (debug builds)

`/run/minimod.chop`, one command, executed on the UI thread:

| command | purpose |
|---|---|
| `dump` | view state, slice count, layer fields |
| `count` | slice count |
| `snap` / `snaptrack` / `snapoff` | state snapshot; snapshot on every button and knob event |
| `sl <i>` | 0x140 bytes of slice `i` |
| `lay` | 0xb30 bytes of the layer |
| `rw <addr> <len>` / `ww <addr> <hex>` | read / write memory |
| `rp` | repaint the screen |
| `vt [n]` / `vl [n]` | runtime vtables (sample, slice container) |
| `mk <off> [n]` | words at `base + off` |
| `scan` / `scan2` / `find` | heap searches for editor-shaped objects |
| `samp` | sample validity (`0x2943640`) |
| `alloc` / `allocoff` | malloc-size logging |
| `findval <u32>` / `findq <u64>` | value search in writable memory |
| `wvec` | page watch on the slice vector header and element buffer |

## 7. Limits

- At most **16** slices — the stock chop limit.
- The pad's previous chop is replaced; stock slice edits are not undoable
  either.
- A user edit during the operation is detected only through the sample identity
  check, not per boundary.
- Manual type selection depends on the binding's layout
  (`+0xd8`/`+0xc0`/`+0xe0`/`+0xe8`); a firmware change there turns into an
  explicit `cannot select Manual chop` rather than a wrong chop.
- Real hardware has never been tested.
- Not pursued once the native API was found: where Threshold mode takes its
  positions from, and whether the pad's chop is persisted through the
  deserializer's `SlicePoints` (a different class from the runtime layer).
