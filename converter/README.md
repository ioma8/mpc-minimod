# mpc2stems

Convert an Akai MPC Sample project (`.xpj` + `[ProjectData]`) into per-pad DAW
stems that sum back to the device's Song export.

A separate tool that lives here; it is independent of the MiniMod.

```bash
uv run mpc2stems info      project1/bapap1.xpj
uv run mpc2stems validate  project1/bapap1.xpj project1-mixdown/Song1.wav
uv run mpc2stems convert   project1/bapap1.xpj --out out \
                           --reference project1-mixdown/Song1.wav --exact
uv run mpc2stems verify    out/stems project1-mixdown/Song1.wav
```

Run everything from this directory.

## Docs

- [`docs/MPC2STEMS.md`](docs/MPC2STEMS.md) — what is modelled, accuracy, refine/exact modes
- [`CONVERTER-HANDOFF.md`](CONVERTER-HANDOFF.md) — current status and limitations
- [`docs/FORMAT.md`](docs/FORMAT.md) — the `.xpj` format, field by field
- [`docs/FINDINGS.md`](docs/FINDINGS.md) — the evidence log
- [`docs/DEVICE-PROBE.md`](docs/DEVICE-PROBE.md) — getting ground truth off a real device
- [`USER-TASK.md`](USER-TASK.md) — the one manual step (per-pad exports) that would lower the residual
