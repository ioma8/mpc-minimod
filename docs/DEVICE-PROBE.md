# Reading numbers off the device

Two ways to get ground truth out of a real MPC Sample. Both are needed: they
answer different questions.

| route | gives | needs | stock firmware? |
| --- | --- | --- | --- |
| **A. solo projects** (`mpc2stems solo-projects`, 12 exports) | isolated audio per pad, through the master chain | one export per pad | **yes** |
| **B. in-process probe** (`tools/device/`, `LD_PRELOAD`) | the app's own DSP arguments, e.g. the filter cutoff in Hz | boot-thingy or a plain `autorun` wrapper | yes (nothing is flashed) |

Neither can reproduce élastique's phase; that needs the vendor code itself.

---

## A. Per-pad audio from stock firmware

Stock 1.3.0 exports a **mixdown only** — there is no stem export, no per-track
split, and nothing in the UI that solos a pad into a file. The way in is to hand
the device a project that contains exactly one playing pad, export, and repeat.
No mod is involved; the silence is achieved in the `.xpj` (every other layer loses
its `sampleFile`/`sampleName` and goes `active: false`), and each variant keeps
the master chain, so its export is that pad through the same bus as the full mix.

```sh
uv run mpc2stems solo-projects project1/bapap1.xpj --out solo
```

That writes twelve folders (~7 MB total), one per pad that plays, each with only
the one sample it needs:

```
solo-00-rnb-kik-grove1-91v.xpj + …_[ProjectData]/
solo-09-rnb-bas-grove2-g-91v.xpj + …_[ProjectData]/
…
```

On the device, for each folder: copy it to the card the way you would any project
(folder and `.xpj` together, `_[ProjectData]` next to it), open the project, and
export the song. Twelve exports in one sitting is the whole cost.

Then send back:

| what | why |
| --- | --- |
| the 12 exported WAVs | per-pad ground truth, including the master chain |
| the stock mixdown of the original project (already have `Song1.wav`) | the sum every stem must add up to |
| *optional:* one export with the **Color Compressor insert removed** | gives a pre-master reference, which separates pad error from bus error in one shot |

`tests/test_project.py::test_solo_projects_isolate_one_pad_each` proves the
variants are faithful: each one renders identically to that pad inside the full
project, so a difference against the device export is model error, not a variant
artefact.

Attribution is the point. In the full mixdown **no pad ever sounds alone** — the
sequence has zero intervals where only the bass plays — so per-pad model error
cannot be separated from the sum. One pad per file removes that limitation, and
89 % of the current residual is exactly this unattributable per-pad error.

### If you do run SampQOL OS

Then there is a no-code route as well: SampQOL's *Export Stems* writes one WAV per
**track** (24-bit 44.1 kHz, with submix effects, never with master FX), and
*Explode* (Step Edit → Track View → `SHIFT`+`B3`) moves each pad with notes onto
its own track, except pads sharing a mute group — here pads 13 and 14, so twelve
pads give eleven files. Export stems from an exploded **copy** of the project.

## B. In-process probe: `probe-tanf.so`

`tools/device/probe-tanf.c` interposes `tanf`. The app builds every JUCE IIR
coefficient as `tanf(pi * f / fs)` (FINDINGS §73) and `tanf` is an ordinary
`JUMP_SLOT` import, so the interposer sees the **cutoff in Hz that each pad
actually gets** — the mapping that a mixdown cannot resolve (FORMAT §10).

```sh
cd tools/device && make test && make device      # host test, then aarch64 .so
```

Deploy inside the app process, the same way the community patches work: the SD
card is mounted `noexec`, so the library has to live in `/run` (tmpfs) and be
preloaded before the app starts.

```sh
# /run/probe.sh, run from autorun before the stock app starts
cp /path/to/card/tools/probe-tanf.so /run/probe-tanf.so
export MPC_PROBE_LOG=/data/mpc-probe.log
export MPC_PROBE_FS=44100
export LD_PRELOAD=/run/probe-tanf.so
exec /usr/bin/launch-ac50
```

Open a project with several distinct `filterCutoff` values (the reference
project has 0.3307, 0.4567, 0.5354 and 0.6457), let it play, then read:

```
# mpc filter probe pid=1342 fs=44100 base=0x55c3a12000
hz=196.412 lr=0x55c3a21d988
hz=254.918 lr=0x55c3a21d988
```

`lr - base` identifies the call site: `0xfcc988`, `0xfcca4c`, `0xfccb0c` and
`0xfccbe0` are the four JUCE coefficient builders, so their lines are the
voice filter and any other `tanf` user can be ignored. Four distinct cutoffs from
four distinct normalised values pin the curve, and the same run tells you whether
the 2-argument (fixed Q) or 3-argument (Q from resonance) builder is the one the
voice uses — which is the difference between `0xfcc950` and `0xfcca10`.

The probe is measurement-only: it writes at most one line per distinct cutoff and
never changes a return value.

## C. The virtual lab: what it can and cannot do

The community's emulator (`garrettjwilke/virtual-mpc-sample`, `python3 lab.py run`)
boots the stock OS and is drivable from this repo. Measured facts, so nobody
re-derives them:

| capability | how |
| --- | --- |
| shell in the guest | `ssh -p 2122 -i out/keys/id_ed25519 root@127.0.0.1` (the lab's own `lab.py ssh` assumes port 2022; this instance runs on 2122) |
| swap the project the app opens | write `/data/TempProjects/Temp-Project.xpj` + `Temp-Project_[ProjectData]/`, then `systemctl restart ac50` |
| send controls | `POST /control {"name": "PLAY", "action": "press"}` on the panel port (this instance: **8865**) |
| read the screen | `GET /screen.png` (320×240) |
| tap the audio | WebSocket `ws://127.0.0.1:5657`, raw S16LE stereo 44.1 kHz |
| build device code | `zig cc -target aarch64-linux-gnu` (same as the community apps) |

Three limits, all measured:

* **`PLAY` plays the current sequence, not the song.** A take is therefore
  controlled from the file: pin `data.currentSequence` and reduce
  `songs[0].items` to that one sequence. Sequence 3 covers 11 of the 12 pads,
  sequence 4 covers pad 27.
* **The audio tap is not sample-accurate.** The bridge reports `drop` (it was
  735 k after idling without a listener); *while* a reader is attached it drops
  ~1 frame/s, which accumulates into phase drift. Spectrum and envelope survive,
  waveform correlation does not. The rear routing also applies its own gain
  (`gain=50`) and a tilt, so this tap cannot validate levels either — it is a
  monitor, not a capture.
* **Controls can wedge.** After several app restarts the screen stops responding
  while the panel still reports `connected: true`; `systemctl restart ac50` fixes
  it. Any automation must restart the app per take and verify with a screenshot.

`tools/emulator/capture-pads.py` implements that loop (swap → restart → wait for a
*new* PID → play → record → screenshot). It works mechanically, but the two limits
above mean it cannot replace the device's own export: only the app writing a WAV
is authoritative.

**What is still missing:** the button sequence that reaches `SongScreen` →
`SongExportScreen` on stock 1.3.0. Everything tried, so a later attempt does not
repeat it: `SEQ` (1–4 presses), `SHIFT`+`SEQ`, `SAMPLE`, `SHIFT`+`SAMPLE` (Input
Config), `PAD FX`, `KNOB FX`, `DATA` press, `DATA` turns ±, `SHIFT`+`PAD 11`,
`SHIFT`+`PAD BANK`, `SHIFT`+`CHOP`, `SHIFT`+`MUTE`, `SHIFT`+`16 LEVELS`,
`SHIFT`+`NOTE REPEAT`, `SHIFT`+`ERASE`, `SHIFT`+`SAMPLE SELECT`, `SEQ RECORD`,
`SAMPLE RECORD`, `TAP TEMPO`, `LOOP`, `MINUS`/`PLUS`, plus each soft key
(`SCREEN 1/2/3`) plain and with `SHIFT`. Observed effects: soft keys toggle
`BPM: SEQ/GBL`, select `Rec Quant` and step through sequences; `SAMPLE SELECT`
opens the sample browser; `SHIFT`+`SAMPLE` opens Input Config; `DATA` turns move
the grid position. No Song screen appeared.

The classes are in the firmware (`10SongScreen`, `16SongExportScreen`, plus
`Export to...` and `Song Export to audio mixdown or new sequence?`), and the
song-related action names sit together in one registry cluster near `0x4610998`
(`SONG`, `SONG REC`, `<<BAR`, `BAR>>`, `LATCH`, `PLAY STR`, `TRACK MUTE`,
`SEQUENCE LIST`, `COPY TO SONG`, `ERASE SONG`), but the screen names appear only
in that registry, not as code references, so the entry point is not reachable by
an adrp/add xref alone.

### What an automated run in the lab actually produced

`tools/emulator/capture-pads.py` did the full loop for all twelve solo projects
(swap → restart → wait for a new PID → play → record → screenshot): **11 of 12
takes contain the pad**, one (pad 16) came back silent because the app wedged.
Files: `out-capture/audio/*.wav`, screenshots in `out-capture/screens/`.

Two measurements decide what those takes are worth:

* **the tap's common-mode response is close to flat** — comparing the ten usable
  takes against the model, the *shared* deviation per octave is only −3.3 … +2.5 dB.
  So the tap is not wildly EQ'd, and per-pad *shape* comparisons are meaningful;
* **but per-pad spread is 4–11 dB**, largest at 11 kHz and 5.7 kHz. Those bands
  carry almost no energy for most pads, so the spread there is noise, not model
  error. At 707 Hz the spread is 7.1 dB with real energy behind it — that band is
  worth a second look.

What the takes cannot support: absolute level (the rear routing fixes `gain=50`),
and anything waveform-aligned (the tap drops ~1 frame/s, which accumulates into
phase drift; envelope correlation is only ~0.56). A sample-accurate in-guest tap
was written (`tools/emulator/tap.c`, raw ALSA ioctls, zig-built, no libasound)
and it reads the loopback cleanly at the app's own parameters (4 ch S32_LE,
44100, period 128, buffer 1024), but it captured silence: with the lab's bridge
stopped the app's input side loses its partner, and the silence feeder on the
sibling device did not restore it. aloop's exact pairing for this app therefore
remains unresolved — the bridge source uses `hw:Loopback,1,0` for *both*
directions, which is not the pairing a naive reading of snd-aloop suggests.

**Conclusion:** the lab is good for automation, project fixtures and screens, not
for sample-accurate audio. Per-pad ground truth still needs the app writing a WAV
(its own export) — on hardware via `USER-TASK.md`, or in the lab by calling the
export path from an injected module.
