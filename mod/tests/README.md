# Emulator tests

The suite builds minimod, starts each case with its own startup project, and
restores the emulator's original temporary project, preload and service state.
Run it from the repository root with the lab running:

```sh
uv run --script mod/tests/emulator_suite.py
```

List cases without starting the lab or building anything, or run selected cases:

```sh
uv run --script mod/tests/emulator_suite.py --list
uv run --script mod/tests/emulator_suite.py spectral midi_chromatic
uv run --script mod/tests/emulator_suite.py --no-build spectral
```

The suite builds by default so it tests the current source. `--no-build` reuses
`mod/build/minimod.so` and its map; use it only when they are already current.
The runner prints a result and time per case, a total, and a screenshot path
when a test process fails. It exits nonzero if any case fails. It uses uv for
its Python processes, including tests with their own inline dependencies.

## First-time setup

1. Install [uv](https://docs.astral.sh/uv/), Rust and Cargo, Zig, LLVM tools,
   zip, QEMU 9+, Docker, and OpenSSH. Add the Rust target:

   ```sh
   rustup target add aarch64-unknown-linux-gnu
   ```

2. Clone the lab next to this repository and follow its firmware setup. The lab
   requires your own MPC Sample 1.3.0.12 firmware partitions; they are not in
   either repository. Its first build uses Docker.

   ```sh
   git clone https://github.com/orrpunk-art/virtual-mpc-sample.git ../virtual-mpc-sample
   # Supply firmware/extracted/part1, part2 and parr as described by the lab README.
   uv run --script ../virtual-mpc-sample/lab.py build
   ```

3. Start the lab in another terminal and leave it running:

   ```sh
   uv run --script ../virtual-mpc-sample/lab.py run --headless
   ```

   Wait for the panel and SSH to start, then run the suite above. If the lab is
   elsewhere, pass `--lab /path/to/virtual-mpc-sample`. If it was started with
   `MPCLAB_PORT_OFFSET=100`, use the same environment variable for the suite.
   The lab's [README](https://github.com/orrpunk-art/virtual-mpc-sample) has
   platform requirements and firmware instructions.

The suite uses the lab's shared directory and SSH connection. It stops and
restarts `ac50` for isolation; do not interact with the panel or run two suites
against the same lab at once. A restoration failure leaves the backup under the
reported `/lab/.minimod-emulator-.../backup` path for recovery.

## Write a test

Add `cases/test_<name>.py`: the suite discovers it automatically as `<name>`.
Use `cases/test_sample_ready.py` as a complete example. Declare the startup
fixture with a literal `PROJECT = 'populated'` or `PROJECT = 'empty'` at the
top level; populated is the default. For external MIDI, also declare
`MIDI_INPUT = True` so the runner enables Pad MIDI In before the test. Each case
starts with the Sample page visible and minimod preloaded.

```python
# /// script
# requires-python = ">=3.10"
# ///
"""Describe the behavior this test checks."""
from emulator import Emulator

PROJECT = 'populated'

mpc = Emulator()
mpc.tap('PAD 1')
assert mpc.u64(mpc.symbol_address('g_screen')) != 0
print('PASS: Sample screen is available')
```

## Cases that listen

`microtonal_pitch` measures what the firmware actually plays, so it records the
lab's PCM stream (S16LE stereo at 44.1 kHz) and compares the pitch of two pads.
QEMU's audio chardev serves **one** client and the panel page at
<http://127.0.0.1:8765> is the other one: close that page while this case runs,
or it skips with a message saying exactly that. Every other case is unaffected.

Run it by name with `uv run --script mod/tests/emulator_suite.py <name>`. The
helper also provides `control(name, action, value)`, `state()`, `wait_for(...)`,
`midi(hex_bytes)`, `screenshot()`, `ssh(command)`, `process()`,
`read(address, size)`, `u32(address)` and
`u64(address)`. The script receives `--lab` and `--panel` automatically. For
additional packages, add `dependencies = ["package"]` to its inline uv script
metadata. Assert observable behavior, use `wait_for` for asynchronous changes,
and include an informative assertion message. Cases run in separate processes;
do not depend on another case's edits or the previously selected pad.
