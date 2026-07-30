#!/usr/bin/env python3
"""Boot, move the PS/2 mouse, click, and check what the screen did.

Three claims:

1. **The cursor is on the screen**, drawn by the shell rather than by the
   interrupt handler — the handler assembles packets and publishes a position,
   and nothing more, because drawing from an interrupt puts the screen in the
   hands of whatever it interrupted.
2. **It follows the mouse, and leaves no trail.** Save-under is what makes the
   second half true: the pixels the cursor covered are put back before it moves
   on. A cursor drawn without restoring looks right in a screenshot taken at
   the right moment and wrong in every other one, so both places are checked.
3. **A click inside a window gives it the keyboard.** The focus is a policy
   decision, so it is made in the shell's own turn; this is the same claim
   `check_focus.py` makes for Tab, reached the other way.

The movement is deliberately large and diagonal. A driver that misframes the
three-byte packet — the failure this device invites, because bit 3 of byte 0 is
the only framing there is — reads the overflow bits out of a movement byte and
reports zero motion, which a small movement would be indistinguishable from.
"""

import os
import socket
import subprocess
import tempfile
import time

from kernel import kernel_image

WIDTH, HEIGHT = 320, 200
POINTER = (0xFF, 0x40, 0x60)
FRAME_IDLE = (0x20, 0x30, 0x40)
FRAME_FOCUSED = (0xFF, 0xC0, 0x40)
# Where the shell puts the pointer at startup, and the pane window's top-left
# cell (see `window_define` in program.lk).
START = (WIDTH // 2, HEIGHT // 2)
PANE_X = 42 * 6


def read_ppm(path):
    with open(path, "rb") as handle:
        data = handle.read()
    _magic, dimensions, _maxval, pixels = data.split(b"\n", 3)
    width, height = (int(value) for value in dimensions.split())
    return width, height, pixels


def pixel(path, x, y):
    width, _height, pixels = read_ppm(path)
    offset = (y * width + x) * 3
    return tuple(pixels[offset:offset + 3])


def pointer_pixels(path):
    width, height, pixels = read_ppm(path)
    found = []
    for y in range(height):
        for x in range(width):
            offset = (y * width + x) * 3
            if tuple(pixels[offset:offset + 3]) == POINTER:
                found.append((x, y))
    return found


def main():
    image = kernel_image()
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
            time.sleep(2.5)
            connection = socket.socket(socket.AF_UNIX)
            connection.connect(monitor)
            time.sleep(0.3)
            connection.recv(65536)

            def screenshot(name):
                path = os.path.join(workdir, name)
                connection.sendall(f"screendump {path}\n".encode())
                time.sleep(1.2)
                return path

            before = screenshot("before.ppm")
            # Twenty small moves rather than one large one: the device reports
            # in packets, and a driver that loses framing recovers (or fails to)
            # across a stream, not on a single event.
            for _ in range(20):
                connection.sendall(b"mouse_move 4 4\n")
                time.sleep(0.05)
            time.sleep(1.5)
            after = screenshot("after.ppm")

            # Into the pane window and click. It is 62x18 at the top right, so
            # the target is a few pixels down — landing below it is a click on
            # the shell, which passes nothing and looks like a broken driver.
            connection.sendall(b"mouse_move 40 -174\n")
            time.sleep(0.6)
            connection.sendall(b"mouse_button 1\n")
            time.sleep(0.4)
            connection.sendall(b"mouse_button 0\n")
            time.sleep(1.5)
            clicked = screenshot("clicked.ppm")
            connection.sendall(b"quit\n")
            connection.close()
        finally:
            qemu.terminate()
            qemu.wait(timeout=10)

        failures = []
        start_pixels = pointer_pixels(before)
        if START not in start_pixels:
            failures.append(f"no cursor at {START} on startup (found {start_pixels[:4]})")
        moved = pointer_pixels(after)
        if not moved:
            failures.append("the cursor disappeared after the mouse moved")
        elif min(moved) == START:
            failures.append("the cursor did not move with the mouse")
        if START in moved:
            failures.append("the cursor left a trail: its old position was not restored")
        # Clicking inside the pane must hand it the keyboard, and take it from
        # the shell — both halves, or a check passes on a program that lights
        # every frame.
        if pixel(clicked, PANE_X, 0) != FRAME_FOCUSED:
            failures.append(f"the clicked window is not focused: frame is {pixel(clicked, PANE_X, 0)}")
        if pixel(clicked, 0, 0) != FRAME_IDLE:
            failures.append(f"the shell kept the focus: its frame is {pixel(clicked, 0, 0)}")
        if failures:
            raise SystemExit("mouse wrong:\n  " + "\n  ".join(failures))
    print("OK: the cursor follows the mouse without a trail, and a click moves the focus")


if __name__ == "__main__":
    main()
