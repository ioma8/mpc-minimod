# mpc-mini

> **Firmware 1.3.0.12 only.** This mod is built against one exact build of
> `/usr/bin/MPC Sample` (`b6584cba76e416e432ad202509099d34373dc2c3d6a8fb625f5678bf0193b508`);
> every hook is an absolute file offset into that binary, so it does not work on
> any other firmware. The autorun package refuses to load on a different build
> (it compares a hash of the app's first 4 KiB), but `lab.py preload` / manual
> `LD_PRELOAD` has no such guard and would crash the app.

A minimal mod for the Akai MPC Sample (firmware 1.3.0.12), delivered as an SD
`autorun` package. Nothing on disk is modified; a reboot returns to stock.

Status, evidence and the open work are in [`docs/MOD-MINI.md`](../docs/MOD-MINI.md);
the Sample/Trim page is documented in [`docs/TRIM-PAGE.md`](../docs/TRIM-PAGE.md);
the ecosystem study is [`docs/ECOSYSTEM.md`](../docs/ECOSYSTEM.md).

## Features

| # | feature | state |
| --- | --- | --- |
| 1 | octave shift in 16 Levels → Tune with MINUS / PLUS | working, verified in the emulator |
| 2 | autotrim of the current sample from its silent edges | **broken**: the trigger runs but writes an unrelated property — see [`docs/TRIM-PAGE.md`](../docs/TRIM-PAGE.md) §6 |
| 3 | chromatic control from an external MIDI controller | note path working and verified with simulated MIDI |

## Controls

* **Autotrim** — `SHIFT` + **B3** (third soft key, next to Normalize). Holding
  SHIFT relabels that slot to **AutoTrim** (`Loop Lock / Normalize / AutoTrim`);
  releasing restores `Trim / Tune / Filter`. It computes the first/last non-silent
  frame correctly, but the write currently lands in the wrong property, so the
  Start/End markers do not move (root cause and fix plan in
  [`docs/TRIM-PAGE.md`](../docs/TRIM-PAGE.md)).
* **Octave shift** — `PLUS` / `MINUS` while 16 LEVELS is lit.
* **Chromatic MIDI** — always on while 16 LEVELS is lit (no toggle).

## Build

```sh
./build.sh     # build/mpcmini.so  (aarch64 glibc, LD_PRELOAD)
./pack.sh      # build/mpc-mini.zip (autorun package with a valid autorun.check)
```

## Try it

Emulator (fastest loop):

```sh
cd ~/projects/customs/virtual-mpc-sample
MPCLAB_PORT_OFFSET=100 python3 lab.py run
MPCLAB_PORT_OFFSET=100 python3 lab.py preload /path/to/mod/build/mpcmini.so
MPCLAB_PORT_OFFSET=100 python3 lab.py ssh 'cat /run/mpcmini.log'
```

Device: unzip `build/mpc-mini.zip`, copy `autorun/` to the card root (or install
the zip as a boot-thingy app), power on.

## Development switches

`MPCMINI_HOOKS` (hook mask), `MPCMINI_SAFE=1` (pass-through), `MPCMINI_DEBUG=1`,
`MPCMINI_PROBE=1` with `MPCMINI_PROBES=<bits>`. All are off by default; the log
is `/run/mpcmini.log`.
