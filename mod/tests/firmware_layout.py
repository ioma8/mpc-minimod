"""Runtime object layouts used by the device-side emulator checks."""
from dataclasses import dataclass


@dataclass(frozen=True)
class FirmwareLayout:
    version: str
    sample_page_vtable: int
    page_binding: int
    state_layers: int
    state_program: int
    surface_pads: int
    surface_pads_end: int
    chop_binding: int


LAYOUTS = (
    FirmwareLayout('1.3.0.12', 0x6E212A0, 0x178, 0x22E8, 0x2310,
                   0x2808, 0x2810, 0x168),
    FirmwareLayout('1.4.0.8', 0x76F6170, 0x1B8, 0x23A8, 0x23D0,
                   0x28B0, 0x28B8, 0x1A8),
)


def detect(app_base, screen_vtable):
    """Return the supported layout proved by a live Sample-page vtable."""
    for layout in LAYOUTS:
        if screen_vtable == app_base + layout.sample_page_vtable:
            return layout
    raise AssertionError(f'Unknown Sample-page vtable: {screen_vtable:#x}')
