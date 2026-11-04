# HPSS Ghidra research scripts

These Java scripts run through GhidraMCP, using the setup in
[RE-GHIDRA.md](../../../docs/RE-GHIDRA.md). The runner preserves a single MCP
session for initialization, instance connection and script execution.

Copy the Java files to Ghidra's script directory (`~/ghidra_scripts` in this
session). Start Ghidra with `GHIDRA_MCP_ALLOW_SCRIPTS=1`, open project
`mcpsamplebin`, then run from this directory:

```sh
python3 run_bridge.py ResearchDecompile.java 0x17580d0 0x153f0b0
python3 run_bridge.py ResearchDecompile.java 0x1aaaae0 0x219c5b0
python3 run_bridge.py HpssRefs.java 0x153ed40
python3 run_bridge.py FindConst.java 100 3000
python3 run_bridge.py DumpVtable.java 0x6f212a0 70
```

The runner currently contains this workstation's absolute bridge path and
selects `knob.bin`. Addresses are Ghidra VAs, not mod offsets.
ResearchDecompile creates a function at an explicit address if none exists;
this modifies Ghidra analysis, not the firmware binary. HpssEntry searches
backward for PACIASP and produces candidate entries requiring inspection.
HpssCommandTrace lists vtable slots and decompiles slots 4–7 when defined;
use ResearchDecompile explicitly for execute slot 8. Decompiled argument types
and calling conventions require validation before use in native code.
