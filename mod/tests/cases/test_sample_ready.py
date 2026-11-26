# /// script
# requires-python = ">=3.10"
# ///
"""A populated startup project exposes a sample on pad A01."""
from emulator import Emulator

PROJECT = 'populated'


def main():
    mpc = Emulator()
    mpc.tap('PAD 1')
    layout = mpc.firmware_layout()
    screen = mpc.u64(mpc.symbol_address('g_screen'))
    binding = mpc.u64(screen + layout.page_binding)
    state = mpc.u64(binding + 0x60)
    layers = mpc.u64(state + layout.state_layers)
    sample = mpc.u64(layers + 0x4a8)
    assert sample, 'Pad A01 has no sample'
    metadata = mpc.u64(sample + 0x130)
    frames = mpc.u32(metadata + 4)
    assert frames > 1, f'Pad A01 has invalid sample length: {frames}'
    print(f'PASS: startup project has {frames} frames on pad A01', flush=True)


if __name__ == '__main__':
    main()
