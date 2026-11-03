# Code review — `mod/src/minimod.c`

Scope: the whole shipped module (~3 900 lines), reviewed against the binary it
targets (firmware 1.3.0.12, sha256 `b6584cba…3b508`).

**Verdict:** the feature works, but the file is *not* in a shippable state. It
still carries every debugging facility that was used while reverse-engineering —
an arbitrary-memory-write channel, page-protection tampering, process-wide fatal
signal handlers, five dead experimental hooks and four always-on polling
threads. There is also one genuine memory-safety bug in the logger.

Severity order below; each item gives location, impact and the fix.

---

## Critical

### C1. `mlog` over-reads its buffer — memory disclosure / crash
`mod/src/minimod.c:181-192`

```c
char buf[512];
int n = vsnprintf(buf, sizeof buf, fmt, ap);
if (n > 0)
    (void)!write(log_fd, buf, (size_t)n);
```

`vsnprintf` returns the length the output *would* have had. When a message is
longer than 511 bytes, `n > sizeof buf` and `write` sends `n` bytes starting at
`buf` — reading past the array into adjacent stack memory. That is a stack
over-read on every long log line (several messages embed 512-byte paths, e.g. the
`sample obj=… path='…'` and `autotrim: sample=… path='…'` lines, so this is
reachable, not theoretical).

Fix:
```c
if (n > 0) {
    if (n > (int)sizeof buf - 1) n = (int)sizeof buf - 1;
    (void)!write(log_fd, buf, (size_t)n);
}
```

### C2. Debug command channel ships enabled — arbitrary memory write
`mod/src/minimod.c:3885-3990` (`heap_probe_thread`), commands include:

| command | effect |
|---|---|
| `set <addr> <float>` | writes 4 bytes at **any** address |
| `model <v>` | writes `c + 0x7ea0` |
| `findval <f>` | scans the whole heap |
| `snap` / `diff` / `snapb` / `diffb` / `find` / `who` | heap probing |

Triggered by writing `/run/minimod.heap`. In a shipped mod this is an
unauthenticated arbitrary-write primitive reachable by any process that can write
that path, and a malformed line can crash the app (the thread runs with the app's
privileges and no validation beyond `strtoull`).

Fix: wrap the whole thread and file path in `#ifdef MINIMOD_DEBUG` and compile it
out of release builds. Do not ship a `set` command.

### C3. `watch_arm` removes write permission from live app pages
`mod/src/minimod.c:115-132`, triggered from `/run/minimod.watch` at `:4219-4228`

```c
int rc = mprotect((void *)page, 0x1000, PROT_READ);
```

This makes an app page (e.g. `screen + 0x1f4c`) read-only. The next write by the
app to that page faults; the module's `SIGSEGV` handler (`:131`) catches it,
logs, restores `PROT_READ|PROT_WRITE` and **returns** — resuming execution after
a fault. If the handler does not recognise the page, execution continues from a
faulting instruction.

Fix: delete for release. Page-protection tampering plus a returning fault handler
is not something to leave in a binary that runs on hardware.

### C4. Process-wide handlers for fatal signals, which return
`mod/src/minimod.c:131` (handler) installed at `:4280-4295` for **SIGSEGV,
SIGBUS, SIGILL and SIGABRT**

Catching `SIGILL`/`SIGABRT` and returning is never correct, and swallowing
`SIGSEGV`/`SIGBUS` hides real app crashes and can loop forever. This changes the
behaviour of the host application for every fault, not just the mod's.

Fix: install only `SIGSEGV`/`SIGBUS`, only in debug builds, and only to log the
fault then `sigaction` back to default and re-raise (never resume).

### C5. The mod can kill the app on its own failure
`mod/src/minimod.c:4418` and `:4420`

```c
if (mprotect(relay_page, 4096, PROT_READ | PROT_EXEC) != 0) _exit(121);
if (mprotect(tramp_page, 4096, PROT_READ | PROT_EXEC) != 0) _exit(122);
```

A hardening step failing terminates the host application. A mod must degrade to
"not active", never take the app down. Fix: log and `return` (leave hooks
installed but the pages writable, or unhook and continue).

---

## High

### H1. Five experimental hooks are still installed in release
`mod/src/minimod.c:4353-4365` — `pagector_probe`, `setctl_probe`,
`ctlhandler_probe`, `gettrim_probe`, `copypad_probe`.

They exist only for the investigation documented in `docs/TRIM-PAGE.md`. Each one
costs a trampoline and, worse, does `/proc/self/mem` reads **on every call**:

```c
static void ctlhandler_probe(void *a0, uint64_t a1, uint64_t a2)
{
    uintptr_t vt = 0;
    if (a0 && rd((uintptr_t)a0, &vt, 8) && ...)   /* open+pread+close */
```

`ctlhandler_probe` fires on every control event; `gettrim_probe` on every pad
lookup. That is 3 syscalls per event, on the UI thread, for no functionality.

Fix: delete all five (and `setctl_probe`'s `orig_setctl`, which is unused after
the experiments were reverted).

### H2. Four always-on polling threads
`mod/src/minimod.c:4401-4412` — `watch_thread` (200 ms), `slider_probe_thread`
(300 ms), `bcast_probe_thread` (300 ms), `heap_probe_thread` (400 ms).

On an embedded sampler these run forever, waking the CPU and reading memory
(hundreds of `rd()` calls per second through a freshly opened `/proc/self/mem`).
The heap thread also `open`/`read`/`close`s `/run/minimod.heap` four times a
second forever.

Fix: remove in release (`#ifdef MINIMOD_DEBUG`), or at minimum gate on an env var
checked once at load.

### H3. Log file grows without bound on tmpfs
`mod/src/minimod.c:4280`

```c
log_fd = open("/run/minimod.log", O_WRONLY | O_CREAT | O_APPEND, 0644);
```

`/run` is a tmpfs (RAM). Nothing rotates or caps the file; a long session with
per-event logging (H1 guarantees plenty) fills RAM and can destabilise the whole
system. The log already accumulated thousands of lines in this session.

Fix: cap the file (e.g. stop logging once `st_size > 256 KB`, or reopen with
`O_TRUNC` on load), and make `mlog` a no-op in release unless
`MINIMOD_LOG` is set.

### H4. `rd`/`wr` open `/proc/self/mem` per call
`mod/src/minimod.c:205-222`

```c
int fd = open("/proc/self/mem", O_RDONLY);
pread(fd, dst, n, (off_t)addr);
close(fd);
```

Correct and fault-safe, but three syscalls per access — and it is called in tight
loops: `find_displayed_sample` scans `0x800` bytes in 8-byte steps (`:948`),
`prop_references` and the scans in `model`/`findval` iterate `0x2000` bytes.

Fix: open one fd at load (`O_RDWR`) and keep it; fall back to open-per-call only
if the cached fd fails.

---

## Medium

### M1. `pc_relative` misses branch forms
`mod/src/minimod.c:272-281` recognises `B/BL`, `ADRP`, `ADR`, `LDR literal`.
It does **not** recognise `CBZ/CBNZ`, `TBZ/TBNZ` (all PC-relative) or `RET`.
`install_hook` copies up to four prologue instructions into the trampoline and
stops only at a `pc_relative` match (`:310-317`); a conditional branch in the
first four instructions would be duplicated and re-based incorrectly.

Fix: add `(i & 0x7E000000) == 0x34000000` (CBZ/CBNZ), `(i & 0x7E000000) ==
0x36000000` (TBZ/TBNZ), and treat `RET`/`BR`/`BLR` as "stop here".

### M2. `install_hook` consumes slots before it succeeds
`mod/src/minimod.c:326-328` increments `nslots`/`ntramp` **before** the patch is
confirmed; a failed `wr` or `mprotect` leaves two slots burned and returns 0.

Fix: allocate, patch, and only then bump the counters (or roll back).

### M3. `mapped()` silently disables validation
`mod/src/minimod.c:225-235`

```c
if (!g_rng_loaded) return 1;   /* "assume mapped" */
```

Callers use it to validate pointers before dereferencing; before `ranges_load()`
runs, *everything* is considered mapped. `rd()` still fails safely, but any `wr()`
path guarded only by `mapped()` (e.g. the debug `set`) would write anywhere.

Fix: return 0 (fail closed) when the ranges are unknown, and load them eagerly in
`load()`.

### M4. `do_autotrim` does file I/O on the UI thread
`:3378` is called from the button dispatcher (`:1309`) and reads the whole WAV
(`wav_edges`, `:444`). For a long sample this blocks the UI thread — the very
thread that must repaint.

The off-thread path exists but is dead: `trim_worker` (`:4239`) waits on
`g_trim_req`, and **`g_trim_req` is never set to 1 anywhere** (only cleared at
`:4246`). Either finish the offload (analysis on the worker, commit queued to the
paint hook) or delete the worker.

### M5. Division by zero when the sample has no frames
`mod/src/minimod.c:3748-3749` (main path), also `:3725-3726`, `:3768-3769`

```c
float v0 = (float)a / (float)N;
float v1 = (float)(b + 1) / (float)N;
```

`N` is the frame count taken from the app; nothing guards `N == 0`. The result is
`inf`/`NaN`, which is then written into the trim parameters and into the property
(NaN would also make the `!=` comparison in `Property_Broadcast` always true,
so it propagates into the model).

Fix: `if (!N) return;` as soon as the length is resolved, before any normalisation.

### M6. `MINIMOD_TESTTRIM` is a shipping backdoor
`mod/src/minimod.c:3716-3734` — an environment variable forces arbitrary trim
bounds and drives the write path. It is a test hook; it should be compiled out of
release, or at least require a second opt-in.

### M7. 24-bit PCM is accepted but analysed as 16-bit
`mod/src/minimod.c:452-454`

```c
if (bps < 2 || bps > 4) bps = 2;
```

`bps == 3` passes the check, but the sample loop reads `short` values, so a
24-bit file is scanned with wrong scaling and wrong frame stride. Either handle 3
bytes properly or reject it (`bps != 2 && bps != 4` → refuse and log).

### M8. Shared state between UI, paint and probe threads
`g_mark_v0/v1` (`:1289`), `g_pm[]`, `g_np`, `g_trim_v*` are written on the UI
thread and read in the paint hook; the probe threads read them too. Only
`volatile` is used. 32-bit aligned stores are atomic on aarch64, so this is
practically benign, but *pairs* (e.g. `g_pm[i].param` then `.prop`) can be
observed torn.

Fix: guard with a mutex or an RCU-style pointer swap; at minimum publish compound
state as a single 64-bit store.

---

## Low

### L1. The overlay targets a stub
`install_screen_paint` patches a vtable slot whose original is an empty function
(`FUN_011e2e10` decompiles to `return;`). It works (the stub is called during the
paint pass) but there is no verification that the slot is what it was; if the
app's layout shifts, the overlay silently draws nothing.

### L2. Raw hex addresses instead of named constants
`:4353-4365` use `0x28100c0`, `0x1968020`, `0x2804e90`, `0x2812f10`,
`0x280dca0` inline while the rest of the table uses `A_*` macros. Same for the
many RE'd offsets (`0x1a8`, `0x220`, `0x1588`, `0x22e8`) — no named constants, so
the code is unreadable without the docs.

### L3. `mlog` has no lock
Many threads call it; lines interleave. Cosmetic, but it makes logs harder to
read while debugging.

### L4. Truncated `snprintf` results are not checked
Several `snprintf` calls into fixed buffers (paths, `p[512]`) ignore truncation;
a long path yields a silently wrong filename.

### L5. `#define A_*` comment drift
`A_BROADCAST 0x13bb930` is documented as `(property, float)` and `A_TRIM_CORE
0x15e70c0` as `(float value, param)` — both correct, but the inline comments in
the dispatch table still say "generic observable property setter" for
`A_TRIM_WRITE 0x14d0890`, which is not what the code uses it for.

---

## What is good

* Hook installation is careful: BTI/PAC prologue check, B-range validation,
  `__builtin___clear_cache`, `mprotect` restored to `R|X`, slot bounds checked,
  and a version gate so a wrong binary gets "no mod" rather than a crash.
* Hook arities match their targets — including the 12-argument
  `SamplePage_Ctor` (getting that wrong crashed the app earlier in the project).
* `rd`/`wr` are fault-safe by construction (via `/proc/self/mem`) — the right
  choice for poking an app you do not control.
* The AutoTrim threshold is peak-relative with an absolute floor and is
  documented and overridable (`MINIMOD_TRIM_DB`), which is exactly what the
  requirement asked for.
* The property write goes through the app's own `Property_Broadcast`, so the
  app's model is updated *and* listeners are notified — the hard-won fix.
* The overlay draws inside the app's paint pass with the app's Graphics object,
  which is the only safe way (the Graphics is stack-local).

---

## Priority order to make it shippable

1. **C1** (logger over-read) — one-line fix, real memory-safety bug.
2. **C2** (debug command channel) and **C3** (page protection) and **C4** (fatal
   signal handlers) — delete from release.
3. **H1/H2** (dead hooks and polling threads) — delete from release.
4. **C5** (`_exit`) and **H3** (log cap) — small, avoids killing/destabilising
   the app.
5. **M1/M2** (hook installer hardening) and **M5** (division guard).
6. Everything else as cleanup.

Concretely, one `#ifdef MINIMOD_DEBUG` around the probe hooks, the four threads,
`watch_arm`/`fault_handler`, and the `/run/minimod.*` channels would remove
findings C2, C3, C4, H1, H2 and M6 in a single change and leave a mod that is
roughly 60 % smaller.

---

## Fix status (release-hardening pass applied)

`MINIMOD_DEBUG` (default `0`) now guards every investigation facility; a debug
build is `zig cc … -DMINIMOD_DEBUG=1`.

| id | status | what changed |
|---|---|---|
| C1 | **fixed** | `mlog` clamps the `vsnprintf` result before `write` |
| C2 | **fixed** | `/run/minimod.heap` thread compiled out of release |
| C3 | **fixed** | `watch_arm` / `watch_thread` compiled out of release |
| C4 | **fixed** | handlers debug-only, and now restore `SIG_DFL` and `raise(sig)` instead of resuming |
| C5 | **fixed** | `_exit(121/122)` replaced by a log line and `return` (mod degrades, app survives) |
| H1 | **fixed** | the five probe hooks are `#if MINIMOD_DEBUG` |
| H2 | **fixed** | the four polling threads are `#if MINIMOD_DEBUG` (thread count 24 -> 20) |
| H3 | **fixed** | `mlog` caps at 256 KB and writes one "log cap reached" line |
| H4 | **fixed** | one cached `/proc/self/mem` descriptor, reopened only if it goes stale |
| M1 | **fixed** | `pc_relative` now also covers `CBZ/CBNZ`, `TBZ/TBNZ`, `BR`, `BLR`, `RET` |
| M2 | **fixed** | `install_hook` bumps `nslots`/`ntramp` only after the patch is live |
| M3 | **mitigated** | reverted on purpose: failing closed breaks the mod because the map ranges are loaded lazily and go stale. The only writer that trusted `mapped()` blindly was the debug `set` channel, which is now compiled out |
| M4 | **fixed** | dead `trim_worker` and its `pthread_create` removed (`g_trim_req` was never set) |
| M5 | **fixed** | `if (!N) return;` before any normalisation |
| M6 | **fixed** | `MINIMOD_TESTTRIM` is `#if MINIMOD_DEBUG` |
| M7 | **retracted** | `pcm_at()` handles 16-, 24- and 32-bit correctly (24-bit sign-extends and shifts by 8); the review's claim was wrong |
| M8 | **accepted** | 32-bit aligned flag stores are atomic on aarch64; the compound `g_pm[]` pair is written once at learn time and read afterwards. Documented rather than rewritten, to avoid touching the working path |
| L1-L5 | open | overlay-slot verification, naming, log lock (now also mutexed), `snprintf` truncation checks, comment drift |

Verified after the pass, in the emulator:

```
commit core done p0=0.0000 p1=0.8268 (want 0.0001/0.8254)
notified props 0xffffe576c718/0xffffe576c770
overlay: 5196 px change, pid unchanged
threads: 20 (was 24)
debug channels inert: writing /run/minimod.heap and /run/minimod.watch does nothing
```

Binary size: 61 392 -> 46 384 bytes (-24 %). Package: 34 350 -> 25 832 bytes.

---

## Regression found in the field and fixed (H4)

After the hardening pass the app's **UI froze**: audio (pads) still played, but no
key or knob changed the screen.

**Root cause: H4 as originally implemented.** The cached descriptor was opened
`O_RDWR`:

```c
mem_fd = open("/proc/self/mem", O_RDWR);
... pread(mem_fd, ...)          /* EIO on this kernel */
```

On this kernel a **read through an `O_RDWR` `/proc/self/mem` descriptor fails**
with `EIO`. Every `rd()` therefore returned 0, which silently broke the object
walking the mod depends on (`find_screens`, `warp_path_of`, `prop_references`,
the paint hook's lookups) and left the UI logic wedged while the audio thread
kept running — exactly the reported symptom.

Bisected by rebuilding three variants and measuring the screen delta on a
`SCREEN 2` press:

| build | result |
|---|---|
| pre-hardening | 5317 px (works) |
| hardening, `rd/wr` reverted | 5317 px (works) |
| hardening, `O_RDWR` cached fd | **0 px (frozen)** |
| hardening, separate `O_RDONLY`/`O_WRONLY` fds | **5318 px (works)** |

Fix: two cached descriptors, one per direction, no `O_RDWR`:

```c
static int mem_rd_fd = -1, mem_wr_fd = -1;
rd: if (mem_rd_fd < 0) mem_rd_fd = open("/proc/self/mem", O_RDONLY);
wr: if (mem_wr_fd < 0) mem_wr_fd = open("/proc/self/mem", O_WRONLY);
```

Verified after the fix: `SCREEN 2` 5458 px, knob 11 520 px, AutoTrim 5196 px,
`commit core done p0=0.0000 p1=0.8268`, pid stable.

### Not a bug: pads do not repaint

The report also said "pressing different pads plays sound but the screen does not
change". Measured against the **stock app with no mod loaded** (drop-in removed,
pid 3533): `PAD 2` and `PAD 3` produce **0 px** there too. Pad triggers play the
sample and do not redraw the screen by design; only keys, knobs and page changes
repaint.
