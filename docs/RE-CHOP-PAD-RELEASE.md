# Repeated Chop pad playback failure

## Reproduction on minimod v0.10.2

1. Load a sample several seconds long onto pad A01 and open the Sample page.
2. Press CHOP and set K3 (Chop Type) to **Regions 4**.
3. Play pads 1, 2, 3 and 4 in order, releasing each before pressing the next.
4. Repeat the sequence at least nine times. Pads 2–4 cease starting their
   regions; their selected region still changes, but its playback cursor does
   not move and the newly selected chop does not sound. Pad 1 continues playing.

In the emulator, the unmodified v0.10.2 build reproduced the failure after eight
presses on each affected pad using `Alvin Cash - The Get Away.wav`. The 21st
round showed zero changed waveform pixels inside regions 2–4 between two
screenshots 350 ms apart, versus 336 changed pixels while they worked.

## Cause and correction

The firmware's pad press callback identifies the physical chop pad, while its
release callback identifies the sample pad (A01). The enclosing control-surface
event identifies the physical pad for both edges. Minimod's ownership ledger
matched releases using the callback's sample-pad number, so the slots for pads
2–4 filled after eight presses. Later presses were consumed by minimod.

The SDK now exposes the enclosing surface event only while the firmware is
handling it on the same thread. Minimod uses its physical pad number to match
ownership on release; it forwards the original firmware callback and its
arguments unchanged. Events outside a surface callback keep their existing
routing.

The emulator regression creates a temporary 16-second sample, selects Regions 4,
plays each region 21 times, and checks cursor movement and audio in each region.
Run it with:

```sh
uv run --script mod/tests/emulator_suite.py chop_repeated_pads
```
