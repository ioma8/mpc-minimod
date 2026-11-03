# Ghidra setup for the MPC Sample 1.3.0.12 binary

## Why this file exists

The autotrim investigation needs readable pseudo-C, and the Ghidra MCP bridge
was returning

```
Error in /force_decompile: ghidra/program/model/lang/SleighLanguageDescription
```

for every decompile. The cause and the fix are recorded here so it does not
have to be rediscovered.

## The mismatch

| component | version |
|---|---|
| installed Ghidra | **12.2** (`~/Applications/ghidra_12.2_PUBLIC`) |
| released GhidraMCP extension (`GhidraMCP-7.0.0.zip`) | built for **12.1.4** (`extension.properties: version=12.1.4`) |
| Java | 25.0.3 |

The upstream README states it plainly: *"The extension only loads in the exact
Ghidra version it was built for."* The 12.1.4 jar was installed into the 12.2
profile (`~/Library/ghidra/ghidra_12.2_PUBLIC/Extensions/GhidraMCP`), which is
why 205 endpoints loaded but the decompiler path blew up on a class that moved
between the two releases.

## The fix

```bash
git clone https://github.com/bethington/ghidra-mcp
cd ghidra-mcp
git fetch --depth 1 origin chore/ghidra-12.2   # "chore: retarget Ghidra 12.2 and Java 25"
git checkout -f FETCH_HEAD
./gradlew buildExtension -PGHIDRA_INSTALL_DIR=$HOME/Applications/ghidra_12.2_PUBLIC
# -> build/distributions/GhidraMCP-7.0.0.zip  (extension.properties: version=12.2)

EXT=$HOME/Library/ghidra/ghidra_12.2_PUBLIC/Extensions
cp -R "$EXT/GhidraMCP" "$EXT/GhidraMCP.backup-12.1.4"
rm -rf "$EXT/GhidraMCP"
unzip -q build/distributions/GhidraMCP-7.0.0.zip -d "$EXT"
# restart Ghidra
```

Verified afterwards:

```
mcp_health -> ghidra_version 12.2, java 25.0.3, endpoint_count 216
force_decompile 0x2918be0 -> {"status":"success", "decompiled": ...}
get_functions  -> returns fields normally
```

Restarting Ghidra did **not** lose the analysis: both programs reopened with
their full 69 271 functions.

## Driving the bridge without an MCP client

The bridge is an MCP<->HTTP multiplexer; it can be spoken to over stdio:

```
initialize -> tools/list -> tools/call
```

A ~30 line Python helper is enough (see the session notes). `--no-lazy` makes it
advertise all 216 tools immediately.

## Address convention

**Ghidra VA = file VA + 0x100000.** The loader's image base is `0x100000`; every
address in the other docs is a *file* VA (what the mod computes as
`base + offset`). Add `0x100000` to use an address in Ghidra:

| file VA | Ghidra VA | function |
|---|---|---|
| `0x2818be0` | `0x2918be0` | applies one pad-state block |
| `0x2802e60` | `0x2902e60` | loops over the 8 pad blocks |
| `0x280dca0` | `0x290dca0` | per-pad dispatch (`param_2 == 0` resets) |
| `0x2810be0`-ish / `0x28100c0` | `0x29100c0` | page constructor |
| `0x15e70c0` | `0x16e70c0` | trim parameter core |
| `0x13bb930` | `0x14bb930` | property broadcaster |
