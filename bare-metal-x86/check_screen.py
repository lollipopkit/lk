#!/usr/bin/env python3
"""Boot the image, screenshot it through QEMU's monitor, and check the pixels.

Reading the framebuffer back from LK proves the writes reached the device's
memory. It does *not* prove the mode was set: an unconfigured card still accepts
writes to its BAR. Only what QEMU scans out can show that, which is what this
checks — and it is the closest thing to "the screen showed the right thing"
available on a machine with no screen.

It also checks a *glyph*: the title is drawn by LK's own text renderer from a
font this repository wrote, so a lit pixel where a letter's stroke belongs is
the difference between "text rendering works" and "something was written to
memory".
"""

import os
import socket
import subprocess
import tempfile
import time

from kernel import kernel_image

WIDTH, HEIGHT = 320, 200
# What `program.lk` draws: a dark background, a title in amber at cell (1,1),
# and a prompt in green at cell (1,3). The checked points are the background
# well away from any text, and the top-left pixel of the title's first glyph
# (`L`), which that glyph lights.
BACKGROUND = (0x00, 0x14, 0x28)
TITLE = (0xff, 0xc0, 0x40)
EXPECTED = [
    # Well inside the shell window: its frame occupies the outermost pixels
    # now, so a corner is no longer background.
    ((WIDTH // 2, HEIGHT - 20), BACKGROUND),
    ((1 * 6, 1 * 8), TITLE),
]

def read_ppm(path):
    with open(path, "rb") as handle:
        data = handle.read()
    magic, dimensions, _maxval, pixels = data.split(b"\n", 3)
    if magic != b"P6":
        raise SystemExit(f"unexpected screenshot format {magic!r}")
    width, height = (int(value) for value in dimensions.split())
    return width, height, pixels


def main():
    image = kernel_image()
    with tempfile.TemporaryDirectory() as workdir:
        monitor = os.path.join(workdir, "monitor")
        shot = os.path.join(workdir, "screen.ppm")
        qemu = subprocess.Popen(
            [
                "qemu-system-x86_64",
                "-kernel", image,
                "-display", "none",
                "-serial", "file:" + os.path.join(workdir, "serial.txt"),
                "-monitor", f"unix:{monitor},server,nowait",
            ]
        )
        try:
            # The monitor socket appears a moment after the process does.
            for _ in range(100):
                if os.path.exists(monitor):
                    break
                time.sleep(0.1)
            # Let the program finish drawing before asking for the screen.
            time.sleep(3)
            connection = socket.socket(socket.AF_UNIX)
            connection.connect(monitor)
            time.sleep(0.3)
            connection.recv(65536)
            connection.sendall(f"screendump {shot}\n".encode())
            time.sleep(1.5)
            connection.sendall(b"quit\n")
            connection.close()
        finally:
            qemu.terminate()
            qemu.wait(timeout=10)

        width, height, pixels = read_ppm(shot)
        if (width, height) != (WIDTH, HEIGHT):
            raise SystemExit(f"screen is {width}x{height}, expected {WIDTH}x{HEIGHT} — the mode was not set")
        failures = []
        for (x, y), expected in EXPECTED:
            offset = (y * width + x) * 3
            actual = tuple(pixels[offset:offset + 3])
            if actual != expected:
                failures.append(f"({x},{y}) is {actual}, expected {expected}")
        if failures:
            raise SystemExit("screen contents wrong:\n  " + "\n  ".join(failures))
    print(f"OK: {WIDTH}x{HEIGHT}, text scanned out as drawn")


if __name__ == "__main__":
    main()
