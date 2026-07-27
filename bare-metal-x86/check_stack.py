#!/usr/bin/env python3
"""Boot, overlap two windows, and check that clicking decides which is on top.

Overlap is where a window *manager* starts. Until now the two windows did not
overlap, and every drawing question had an obvious answer; once they do, three
things have to be true at once and none of them is automatic:

1. **A dragged window covers what it lands on.** That much comes free.
2. **Clicking the one underneath brings it to the front** — and it stays there.
   This is the half that is easy to get wrong in a way that looks right for a
   second: a window drawing its own contents has no idea what is above it, so a
   clock that redraws itself once a second will bury whatever was just raised.
   The check waits long enough for that to have happened.
3. **The one now underneath is still there**, not erased by having been covered.

The clock window is the instrument. Its contents are a pure function of the tick
counter, so it can be redrawn at any moment from state that was being kept
anyway — which is what makes it safe to uncover, and also why it is the window
that re-draws most often.
"""

import os
import socket
import subprocess
import sys
import tempfile
import time

FRAME_FOCUSED = (0xFF, 0xC0, 0x40)
FRAME_IDLE = (0x20, 0x30, 0x40)
# The clock window's title bar, as `program.lk` places it, and where it is
# dragged to: over the spinner window at the top right.
CLOCK_X, CLOCK_Y = 42 * 6, 3 * 8
SPINNER_X, SPINNER_Y = 42 * 6, 0
# The row the clock's top edge lands on after the drag, inside the spinner.
OVERLAP_Y = 4
SAMPLE_LEFT, SAMPLE_RIGHT = 260, 300


def read_ppm(path):
    with open(path, "rb") as handle:
        data = handle.read()
    _magic, dimensions, _maxval, pixels = data.split(b"\n", 3)
    width, height = (int(value) for value in dimensions.split())
    return width, height, pixels


def frame_pixels_on_row(path, y):
    """How many frame-coloured pixels lie on one scan line of the sample area."""
    width, _height, pixels = read_ppm(path)
    total = 0
    for x in range(SAMPLE_LEFT, SAMPLE_RIGHT):
        offset = (y * width + x) * 3
        if tuple(pixels[offset:offset + 3]) in (FRAME_FOCUSED, FRAME_IDLE):
            total += 1
    return total


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
            time.sleep(3)
            connection = socket.socket(socket.AF_UNIX)
            connection.connect(monitor)
            time.sleep(0.3)
            connection.recv(65536)

            def screenshot(name):
                path = os.path.join(workdir, name)
                connection.sendall(f"screendump {path}\n".encode())
                time.sleep(1.2)
                return path

            pointer = [160, 100]

            def move_to(x, y):
                connection.sendall(f"mouse_move {x - pointer[0]} {y - pointer[1]}\n".encode())
                pointer[0], pointer[1] = x, y
                time.sleep(0.4)

            # Drag the clock up onto the spinner by its title bar.
            move_to(CLOCK_X + 4, CLOCK_Y + 2)
            connection.sendall(b"mouse_button 1\n")
            time.sleep(0.4)
            for step in range(1, 6):
                move_to(CLOCK_X + 4, CLOCK_Y + 2 - step * 4)
            time.sleep(0.5)
            connection.sendall(b"mouse_button 0\n")
            time.sleep(1.5)
            dragged = screenshot("dragged.ppm")

            # Click the spinner where the clock does not cover it.
            move_to(SPINNER_X + 58, SPINNER_Y + 2)
            connection.sendall(b"mouse_button 1\n")
            time.sleep(0.5)
            connection.sendall(b"mouse_button 0\n")
            # Long enough for at least one clock tick to have redrawn the clock:
            # a raise that does not survive that is not a raise.
            time.sleep(3)
            raised = screenshot("raised.ppm")
            connection.sendall(b"quit\n")
            connection.close()
        finally:
            qemu.terminate()
            qemu.wait(timeout=10)

        on_top = frame_pixels_on_row(dragged, OVERLAP_Y)
        after_click = frame_pixels_on_row(raised, OVERLAP_Y)
        # The spinner's own frame must still be there afterwards: a window that
        # comes to the front by erasing everything is not a window manager.
        spinner_frame = frame_pixels_on_row(raised, SPINNER_Y)

        failures = []
        if on_top < 20:
            failures.append(f"the dragged window is not on top: {on_top} frame pixels on its edge")
        if after_click != 0:
            failures.append(f"clicking the window underneath did not raise it: {after_click} frame pixels remain")
        if spinner_frame < 20:
            failures.append(f"the raised window has no frame of its own: {spinner_frame} pixels")
        if failures:
            raise SystemExit("stacking wrong:\n  " + "\n  ".join(failures))
    print("OK: the dragged window covered the other, and clicking the other brought it back to the front")


if __name__ == "__main__":
    main()
