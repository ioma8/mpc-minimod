# Extract dispatch and lifetime investigation

Firmware 1.3.0.12; Ghidra image base `0x100000`. Evidence is limited to the
examined Sample-page Extract path and the emulator traces below.

- `FUN_017580d0` constructs the reference-counted command with
  `FUN_0153ed40`, then calls `FUN_01ad0a00`. See `extract-ui.txt`.
- `FUN_01ad0a00` calls `FUN_01acc1f0`, which calls `FUN_01ac9a50`.
  `FUN_01ac9a50` calls the command's virtual slot at `+0x40` directly.
- The command vtable at `0x06f07640`, slot 8 (`+0x40`), points to
  `FUN_0153f0b0`, the hooked Extract execute method.
- The constructor stores its page and state arguments as raw fields; this is
  not evidence that those objects remain alive after the UI callback returns.
  Inside execute, the resolved sample is copied with a reference-counted holder.
  That protects the sample, not demonstrably the page, layer, or program.
- Temporary SDK instrumentation logged thread IDs and call boundaries for a
  synth render and a reverb render. Both produced `handler enter`, `execute
  enter`, `execute exit`, `handler exit`, all on Linux TID 9343. Both emulator
  tests passed. Instrumentation was removed afterward.

This supports synchronous command dispatch on the tested path, not a general
claim about all firmware command paths or devices. It does not establish that a
native command can be cancelled, or that native file writing never delegates
work. The SDK writer currently accepts substitution only on the armed thread and
requires the native writer to consume each descriptor synchronously.

The trace contradicts the older blanket statement in `docs/RE-METHOD.md` that
Extract finishes on an executor thread. Review that statement against any
previous runtime evidence before broadening the conclusion. A responsive
asynchronous design still needs a firmware-supported way to run or stage the
native commit without violating UI-thread ownership, or a proven way to retain
all of its objects and cancel/serialize project changes.
