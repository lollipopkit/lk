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

# The spinner's cell: the top-left of its window, which starts at
# `text_columns() - 11` of 53 columns.
CELL_X, CELL_Y = 42 * 6, 0
GLYPH_W, GLYPH_H = 5, 7
SAMPLES = 6

# Where the spinner deliberately draws *outside* its window, one glyph plus a
# gap to the left. Those pixels belong to the shell, and the window rectangle
# is the only thing stopping them: without it the same code would light them.
OUTSIDE_X = CELL_X - GLYPH_W - 1
# From y=1, not y=0: the shell window's own frame runs along the top row of the
# screen, so row 0 there is legitimately drawn. The spinner's overreach is at
# window y=1, which is where it would land if it were not clipped.
OUTSIDE_Y = 1
BACKGROUND = 0x00


def region(path, x0, y0, w, h):
    with open(path, "rb") as handle:
        data = handle.read()
    _magic, dimensions, _maxval, pixels = data.split(b"\n", 3)
    width, _height = (int(value) for value in dimensions.split())
    return [
        tuple(pixels[((y0 + dy) * width + (x0 + dx)) * 3 : ((y0 + dy) * width + (x0 + dx)) * 3 + 3])
        for dy in range(h)
        for dx in range(w)
    ]


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
            outside = []
            for index in range(SAMPLES):
                shot = os.path.join(workdir, f"screen{index}.ppm")
                connection.sendall(f"screendump {shot}\n".encode())
                time.sleep(0.8)
                seen.append(glyph_at(shot))
                outside.append(region(shot, OUTSIDE_X, OUTSIDE_Y, GLYPH_W, GLYPH_H))
            connection.sendall(b"quit\n")
            connection.close()
        finally:
            qemu.terminate()
            qemu.wait(timeout=10)

    # Deterministic first: if the task never ran, nothing was ever drawn in the
    # corner and every sample is background.
    # `glyph_at` samples the *red* byte of each pixel, so the background is
    # its red channel — comparing against 0x28 (the blue one) made this guard
    # unable to fire at all.
    if all(all(byte == BACKGROUND for byte in frame) for frame in seen):
        raise SystemExit("the corner is untouched: the second task never ran at all")
    distinct = len(set(seen))
    if distinct < 2:
        raise SystemExit(
            f"the spinner drew the same glyph in all {SAMPLES} screenshots: "
            "the second task never ran, or never got the CPU back"
        )
    lit = [pixel for frame in outside for pixel in frame if pixel != (0x00, 0x14, 0x28)]
    if lit:
        raise SystemExit(
            f"{len(lit)} pixel(s) outside the spinner's window were written: "
            "the rectangle did not contain it"
        )
    print(
        f"OK: the spinner advanced ({distinct} distinct frames) while the shell waited, "
        "and its out-of-window writes were dropped"
    )


if __name__ == "__main__":
    main()
