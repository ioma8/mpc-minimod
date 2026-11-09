"""Read the emulated screen and recognise the app's modal dialog.

The device suites read the mod's own status string, which says "created" even
when the app itself reported "Command failed" on screen. This looks at the screen.
"""
import struct
import urllib.request
import zlib


def png_pixels(data):
    """Decode an 8-bit, non-interlaced RGB or RGBA PNG into (width, height, rows)."""
    assert data[:8] == b'\x89PNG\r\n\x1a\n', 'not a PNG'
    pos, idat, width, height, kind = 8, b'', 0, 0, 2
    while pos < len(data):
        size, tag = struct.unpack('>I4s', data[pos:pos + 8])
        body = data[pos + 8:pos + 8 + size]
        if tag == b'IHDR':
            width, height, depth, kind, _, _, interlace = struct.unpack('>IIBBBBB', body)
            assert depth == 8 and kind in (2, 6) and interlace == 0, 'unsupported PNG'
        elif tag == b'IDAT':
            idat += body
        pos += 12 + size
    step = 4 if kind == 6 else 3
    raw = zlib.decompress(idat)
    stride = width * step
    rows, prior = [], bytearray(stride)
    for y in range(height):
        base = y * (stride + 1)
        filt, line = raw[base], bytearray(raw[base + 1:base + 1 + stride])
        for i in range(stride):
            left = line[i - step] if i >= step else 0
            up = prior[i]
            corner = prior[i - step] if i >= step else 0
            if filt == 1:
                line[i] = (line[i] + left) & 255
            elif filt == 2:
                line[i] = (line[i] + up) & 255
            elif filt == 3:
                line[i] = (line[i] + ((left + up) >> 1)) & 255
            elif filt == 4:
                p = left + up - corner
                pa, pb, pc = abs(p - left), abs(p - up), abs(p - corner)
                pred = left if pa <= pb and pa <= pc else up if pb <= pc else corner
                line[i] = (line[i] + pred) & 255
        rows.append(line)
        prior = line
    return width, height, rows, step


def modal_showing(png):
    """Whether the app's dialog box (a white frame on black) is on screen."""
    width, height, rows, step = png_pixels(png)
    def bright(x, y):
        pixel = rows[y][x * step:x * step + 3]
        return min(pixel) > 180
    # The dialog draws a white rectangle inset from the edges of the screen.
    for y in range(40, 70):
        if sum(bright(x, y) for x in range(20, width - 20)) > (width - 40) * 0.9:
            return True
    return False


def screen_png(panel):
    with urllib.request.urlopen(panel + '/screen.png', timeout=5) as response:
        return response.read()


def assert_no_modal(panel, what):
    assert not modal_showing(screen_png(panel)), \
        f'{what}: the app is showing a dialog (e.g. "Command failed") on the panel'
