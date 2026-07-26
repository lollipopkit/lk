#!/usr/bin/env python3
"""Boot the image, screenshot it through QEMU's monitor, and check the pixels.

Reading the framebuffer back from LK proves the writes reached the device's
memory. It does *not* prove the mode was set: an unconfigured card still accepts
writes to its BAR. Only what QEMU scans out can show that, which is what this
checks — and it is the closest thing to "the screen showed the right thing"
available on a machine with no screen.
"""

import os
import socket
import subprocess
import sys
import tempfile
import time

WIDTH, HEIGHT = 320, 200
# What `program.lk`'s gradient puts at the corners and the middle: red rising
# left to right, green rising top to bottom, blue constant.
EXPECTED = [
    ((0, 0), (0, 0, 64)),
    ((WIDTH // 2, HEIGHT // 2), (127, 127, 64)),
    ((WIDTH - 1, HEIGHT - 1), (254, 253, 64)),
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
    image = sys.argv[1] if len(sys.argv) > 1 else (
        "target/x86_64-unknown-none/release/lk-bare-metal-x86.multiboot"
    )
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
    print(f"OK: {WIDTH}x{HEIGHT}, gradient scanned out as drawn")


if __name__ == "__main__":
    main()
