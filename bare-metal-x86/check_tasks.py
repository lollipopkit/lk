#!/usr/bin/env python3
"""Boot the image and check that a second task runs without being asked to.

The shell task spends its time in `cpu_wait_for_interrupt`. The spinner task
never yields — it draws a glyph and burns the rest of its slice. So if the
glyph in the corner changes between screenshots, something took the CPU away
from one task and gave it to the other, which is the whole claim.

Screenshots rather than the serial line: both tasks share the UART, and
interleaved output would prove they both *ran*, not that either was
interrupted mid-work.
"""

import os
import socket
import subprocess
import sys
import tempfile
import time

# The spinner's cell: `text_columns() - 2` of 53 columns, on the top row.
CELL_X, CELL_Y = 51 * 6, 0
GLYPH_W, GLYPH_H = 5, 7
SAMPLES = 4


def glyph_at(path):
    with open(path, "rb") as handle:
        data = handle.read()
    _magic, dimensions, _maxval, pixels = data.split(b"\n", 3)
    width, _height = (int(value) for value in dimensions.split())
    return bytes(
        pixels[((CELL_Y + dy) * width + (CELL_X + dx)) * 3]
        for dy in range(GLYPH_H)
        for dx in range(GLYPH_W)
    )


def main():
    image = sys.argv[1] if len(sys.argv) > 1 else (
        "target/x86_64-unknown-none/release/lk-bare-metal-x86.multiboot"
    )
    with tempfile.TemporaryDirectory() as workdir:
        monitor = os.path.join(workdir, "monitor")
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
            for _ in range(100):
                if os.path.exists(monitor):
                    break
                time.sleep(0.1)
            time.sleep(2)
            connection = socket.socket(socket.AF_UNIX)
            connection.connect(monitor)
            time.sleep(0.3)
            connection.recv(65536)
            seen = []
            for index in range(SAMPLES):
                shot = os.path.join(workdir, f"screen{index}.ppm")
                connection.sendall(f"screendump {shot}\n".encode())
                time.sleep(1.2)
                seen.append(glyph_at(shot))
            connection.sendall(b"quit\n")
            connection.close()
        finally:
            qemu.terminate()
            qemu.wait(timeout=10)

    distinct = len(set(seen))
    if distinct < 2:
        raise SystemExit(
            f"the spinner drew the same glyph in all {SAMPLES} screenshots: "
            "the second task never ran, or never got the CPU back"
        )
    print(f"OK: the spinner advanced ({distinct} distinct frames) while the shell waited")


if __name__ == "__main__":
    main()
