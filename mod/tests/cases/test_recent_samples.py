# /// script
# requires-python = ">=3.10"
# ///
"""Preview-only files stay out; pad loads enter and reload from Recent Samples."""
from emulator import Emulator
import time

PROJECT = 'populated'

CARD_ROOT = '/media/MPC SAMPLE'
STORAGE_ROOT = f'{CARD_ROOT}/MPC-Sample'
SAMPLE_ROOT = f'{STORAGE_ROOT}/Samples'
PROJECT_DATA = '/data/TempProjects/Temp-Project_[ProjectData]'


def sample_path(mpc, press_pad=True):
    if press_pad:
        mpc.tap('PAD 1')
    layout = mpc.firmware_layout()
    screen = mpc.u64(mpc.symbol_address('g_screen'))
    binding = mpc.u64(screen + layout.page_binding)
    state = mpc.u64(binding + 0x60)
    layer = mpc.u64(state + layout.state_layers)
    sample = mpc.u64(layer + 0x4a8)
    assert sample, 'Pad A01 has no sample after browser load'
    holder = mpc.u64(sample + 0x30)
    path_ptr = mpc.u64(holder + 0x120)
    return mpc.read(path_ptr, 512).split(b'\0', 1)[0].decode()


def project_sample_count(mpc):
    result = mpc.ssh(
        f'find "{PROJECT_DATA}" -type f -name "*.wav" | wc -l'
    )
    return int(result)


def open_samples_root(mpc):
    mpc.tap('SAMPLE SELECT')
    mpc.tap('SCREEN 2')  # Select External storage in the browser's top tab.
    mpc.control('DATA', 'turn', 1)  # External opens at MPC-Sample; Samples follows Projects.
    mpc.tap('DATA')


def turn_rows(mpc, delta):
    direction = 1 if delta > 0 else -1
    for _ in range(abs(delta)):
        mpc.control('DATA', 'turn', direction)


def main():
    mpc = Emulator()
    source_files = mpc.ssh(
        f'find "{SAMPLE_ROOT}" -maxdepth 1 -type f -name "*.wav" | sort'
    ).decode().splitlines()
    assert len(source_files) >= 2, 'Need two SD samples to test the browser loader'
    virtual_path = f'{CARD_ROOT}/Recent Samples'
    assert mpc.ssh(f'test ! -e "{virtual_path}" && echo absent').decode().strip() == 'absent'

    open_samples_root(mpc)
    # Preview whichever row the native browser currently selects. Its selection
    # is retained across app restarts, so deriving it from host file order makes
    # this test depend on the previous emulator case.
    mpc.tap('SCREEN 3')
    mpc.tap('SAMPLE')
    mpc.tap('SAMPLE SELECT')
    before_load = project_sample_count(mpc)
    assert mpc.ssh(f'test ! -e "{virtual_path}" && echo absent').decode().strip() == 'absent', (
        'Preview-only browser open created Recent Samples'
    )
    mpc.control('DATA', 'turn', 1)
    mpc.tap('DATA')
    mpc.wait_for(
        lambda: project_sample_count(mpc) > before_load,
        timeout=8,
        message='Native browser did not load the first SD sample',
    )
    mpc.tap('SAMPLE')
    first_path = sample_path(mpc)
    first_name = first_path.rsplit('/', 1)[-1]
    sample_stem = first_name.removesuffix('.wav')

    mpc.tap('SAMPLE SELECT')  # Return to the browser's last directory.
    mpc.tap('SCREEN 1')  # Back to the MPC-Sample directory.
    mpc.tap('SCREEN 1')  # Back again to the actual SD-card root.
    for _ in range(4):  # Root rows: one file and three folders before Recent Samples.
        mpc.control('DATA', 'turn', 1)
    mpc.tap('DATA')  # Open Recent Samples in the native browser.
    time.sleep(.2)
    recent_path = ''
    for attempt in range(128):
        before_recent = project_sample_count(mpc)
        mpc.tap('DATA')
        mpc.wait_for(
            lambda: project_sample_count(mpc) > before_recent,
            timeout=8,
            message='Recent Samples did not expose a loadable native file row',
        )
        mpc.tap('SAMPLE')
        recent_path = sample_path(mpc)
        if recent_path.rsplit('/', 1)[-1].startswith(sample_stem):
            break
        if attempt == 127:
            raise AssertionError(
                f'Recent Samples did not expose {first_path}; last loaded {recent_path}'
            )
        mpc.tap('SAMPLE SELECT')
        mpc.control('DATA', 'turn', 1)
    assert mpc.ssh(f'test ! -e "{virtual_path}" && echo absent').decode().strip() == 'absent'
    print(f'PASS: native Recent Samples reloaded {recent_path}; minimod created no copies')


if __name__ == '__main__':
    main()
