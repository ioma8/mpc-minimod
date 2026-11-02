# Your task: 12 exports from the MPC Sample

One thing blocks further progress on the converter, and only you can do it: the
device has to hand me **one audio file per pad**. Stock firmware 1.3.0 exports a
mixdown and nothing else — no stems, no per-track split — so the isolation has to
come from the project file. I have generated those project files already; you open
each one and export.

**Estimated effort: 20–30 minutes**, almost all of it exports.

Why it matters, in one number: **~82 % of the remaining error** (residual 0.1811)
is a few-percent mistake spread across the ordinary pads, and it cannot be
attributed from the mixdown because no pad ever sounds alone in the song. One pad
per file turns that into a supervised fit. Expected outcome: ~0.09 if the per-pad
error halves — versus ~0.002 available from the time-stretch, which is why this is
worth twenty minutes of button presses.

Details and background: [`docs/DEVICE-PROBE.md`](docs/DEVICE-PROBE.md) §A.

---

## 0. Prerequisites

- [ ] MPC Sample on firmware **1.3.0** (`SHIFT`+`PAD 8`, top right shows `v1.3.0`)
- [ ] A microSD card the device already reads (exFAT)
- [ ] The 12 folders from `out-solo/` — already generated, 7.1 MB total

Regenerate them at any time (idempotent, ~2 s):

```sh
uv run mpc2stems solo-projects project1/bapap1.xpj --out out-solo
```

Pre-flight check, no device needed — this must print `pads w/ sample : 1` for
every variant, and `(no sample)` for the other pads:

```sh
uv run mpc2stems info out-solo/solo-09-rnb-bas-grove2-g-91v.xpj
```

Each folder is a complete project containing **one** pad; every other layer has
been silenced inside the `.xpj`. Nothing is flashed, no mod is involved, and the
original `project1/` is never touched.

---

## 1. Copy the folders to the card

Twelve project folders, each pairing a `.xpj` with its own `_[ProjectData]`
directory:

```
solo-00-rnb-kik-grove1-91v.xpj          + solo-00-rnb-kik-grove1-91v_[ProjectData]/
solo-01-rnb-prc-grove3-91v.xpj          + …
solo-02-rnb-hat-grove1-91v.xpj
solo-09-rnb-bas-grove2-g-91v.xpj
solo-10-voxph-grove1-g-m85-91v.xpj
solo-13-guitph-grove2-d-m85-91v.xpj
solo-14-guitph-grove1-c-m85-91v.xpj
solo-16-sample-024-trimmed.xpj
solo-17-sample-010-trimmed.xpj
solo-18-chop-16-seq-a04-1-normalized.xpj
solo-20-sample-030-normalized-trimmed-normalized.xpj
solo-27-chop-1-sample-030-normalized-trimmed-nor.xpj
```

- [ ] Copy all 24 items into the folder where you keep projects on the card
      (whatever the device's browser shows as the SD projects location)
- [ ] Keep each `.xpj` **next to** its `_[ProjectData]` folder — the names must
      match exactly, or the device will report missing samples

---

## 2. Export each one

For each of the 12:

1. [ ] Open the project in the device browser
2. [ ] Export the song — **the same export that produced `Song1.wav`**
3. [ ] Change nothing about how you export. There is no master-FX switch on stock
           1.3.0: the export includes the master chain, and that is what I want —
           the solo files must go through the same bus as your full mix (see §4
           for the one extra file that separates the two)
4. [ ] Keep the exported file. If the device names it after the project, the
           twelve files are already self-identifying. If it does not, **rename
           each export to its folder name** — `solo-09.wav`, `solo-18.wav`, … —
           which is what I will match on. The two-digit number is the pad index,
           identical to the pad number in `mpc2stems info`.

Sanity checks as you go:

- [ ] Each export is **not silent**, and is ~29 s long (the song is 3 sequences,
      110 BPM)
- [ ] The pad named in the filename is the sound you hear — e.g. `solo-09` must be
      bass only, no kick

**Minimum viable set** if twelve is too many: `00` (kick), `09` (bass), `18`
(chop 16), `20` (sample 030), `02` (hat). Those carry most of the energy; the
rest can follow later.

---

## 3. Send me back

- [ ] The 12 exported WAVs, **unmodified** (zip is fine; no MP3/flac transcode,
      no normalisation or trimming)
- [ ] The firmware version string from step 0
- [ ] Anything odd you noticed (a silent export, a project that would not open)

---

## 4. Optional, high value: one export with the compressor off

This one file separates pad error from bus error in a single shot, which
otherwise I have to fit statistically.

- [ ] Load the **original** project (`project1/bapap1.xpj`), not a solo variant
- [ ] Open the master strip's inserts (Outputs 1/2) and **unload** the
      `Color Compressor` — if your firmware has no unload, replacing it with an
      empty slot, or bypassing it if that exists, does the same job
- [ ] Export the song once → `bapap1-nocompressor.wav`
- [ ] **Do not save the project**; reload it (or power-cycle) so the insert is
      restored

If no route to a bypassed export is available on the device, skip this section —
the twelve solo exports still work, I just have to fit the bus statistically.

---

## 5. Optional, independent: the filter probe

If you are willing to run one small payload (nothing is flashed; it is an
`LD_PRELOAD` for one session), it pins the filter cutoff mapping exactly — the
last unknown in the model. Build and deployment steps:
[`docs/DEVICE-PROBE.md`](docs/DEVICE-PROBE.md) §B.

- [ ] `cd tools/device && make device`
- [ ] Deploy per §B, open the reference project, let it play
- [ ] Send `mpc-probe.log`

Skip this if it is at all inconvenient — the 12 exports matter more.

---

## 6. What I do when the files arrive

1. Verify sum-of-stems against the pre-master reference and against `Song1.wav`,
   so the per-pad numbers are trustworthy before I fit anything.
2. Compare each export with the model's stem for that pad: level, spectrum,
   attack/decay shape, timing.
3. Fix the stages that show systematic error — my expectation is velocity/gain
   and envelope/filter shape, not anything exotic — and re-measure both per-pad
   and against the mixdown so the headline residual stays comparable.
4. Report which pads improved and by how much, and update `docs/FINDINGS.md`.

Nothing else is needed from you, and nothing on your device is modified by any of
this: the solo projects are new files, and the only thing that touches the
original is the optional §4 export, which you do not save.

---

## Safety

- The variants are new files with **one sample each**; if one fails to open, the
  cause is almost always a missing or misnamed `_[ProjectData]` folder.
- Do not let the device overwrite `project1/bapap1.xpj` with a solo variant.
- Back up the card first if the projects on it matter — this is the usual advice
  for any project writes, and the mod scene reports that a failed load can drop
  samples.
