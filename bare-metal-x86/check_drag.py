#!/usr/bin/env python3
"""Boot, drag a window over the terminal's text, and drag it away again.

The claim is the one a window manager lives or dies on: **what a window covers
comes back**. Everything else here is arrangement — a title bar, a grab offset,
a clamp at the screen edge — but a screen that cannot repair itself has no
window manager, only a way of destroying text.

There is no backing store for pixels. What makes the repair possible is that
the terminal keeps its characters in a grid (`SHARED_GRID` in `program.lk`), so
the shell can be asked what belongs at a cell rather than remembering what the
screen looked like. This drags the pane window onto a line of text, checks the
text is gone, drags it off, and checks the text is back.

Counting lit pixels rather than testing one: which pixels of a glyph are lit is
a property of the font, and a check that encodes it breaks when a letter is
redrawn for reasons that have nothing to do with dragging.
"""

import os
import socket
import subprocess
import tempfile
import time

from kernel import kernel_image

FOREGROUND = (0x40, 0xFF, 0x90)
FRAME_FOCUSED = (0xFF, 0xC0, 0x40)
# The pane window, as `program.lk` defines it: 62x18 at the top right.
PANE_W, PANE_H = 62, 18
PANE_START_X = 42 * 6
# Where the text under test is. `help` answers on the row below the one it is
# typed on, which after boot is row 6; the sample area is its left end, which is
# where the pane will be dragged.
# From x=1, not x=0: the shell window's own frame is the outermost pixel
# column, and the terminal's first character cell starts at x=0 — they share
# that column, and whichever is drawn last wins. The overlap is a property of a
# full-screen window with a one-pixel frame, not of dragging, so it is measured
# out rather than being allowed to fail this check for the wrong reason.
TEXT_LEFT, TEXT_TOP, TEXT_W, TEXT_H = 1, 48, PANE_W - 1, 8
# Where the pane ends up. Not a wish: the window follows the pointer at the
# offset it was grabbed with, so grabbing at (+4,+2) and walking the pointer to
# (4,50) puts the window's corner at (0,48). Getting this wrong the first time
# put the second grab two pixels above the title bar, where it grabbed nothing
# — and the check reported "the text did not come back", which was true and
# said nothing about why.
DROP_X, DROP_Y = 0, 48


def read_ppm(path):
    with open(path, "rb") as handle:
        data = handle.read()
    _magic, dimensions, _maxval, pixels = data.split(b"\n", 3)
    width, height = (int(value) for value in dimensions.split())
    return width, height, pixels


def count_colour(path, colour, left, top, width, height):
    image_width, _height, pixels = read_ppm(path)
    total = 0
    for y in range(top, top + height):
        for x in range(left, left + width):
            offset = (y * image_width + x) * 3
            if tuple(pixels[offset:offset + 3]) == colour:
                total += 1
    return total


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

            def move_to(x, y, steps=24):
                """Walks the pointer there, since the device reports relative motion."""
                connection.sendall(f"mouse_move {x - move_to.x} {y - move_to.y}\n".encode())
                move_to.x, move_to.y = x, y
                time.sleep(0.5)

            # The shell puts the pointer in the middle of the screen at startup.
            move_to.x, move_to.y = 160, 100

            # Something to cover: `help` prints a line that reaches across the
            # sample area.
            for key in ["h", "e", "l", "p", "ret"]:
                connection.sendall(f"sendkey {key}\n".encode())
                time.sleep(0.25)
            time.sleep(1.5)
            before = screenshot("before.ppm")

            # Grab the pane by its title bar and drag it onto the text. The
            # pointer is walked rather than teleported: the device reports
            # relative motion, and a drag is defined by where the pointer is
            # while the button is down.
            move_to(PANE_START_X + 4, 2)
            connection.sendall(b"mouse_button 1\n")
            time.sleep(0.4)
            for step in range(1, 13):
                move_to(PANE_START_X + 4 - step * 21, 2 + step * 4, steps=1)
            time.sleep(0.5)
            connection.sendall(b"mouse_button 0\n")
            time.sleep(1.5)
            covered = screenshot("covered.ppm")

            # And away again, back to where it started.
            move_to(DROP_X + 4, DROP_Y + 2)
            connection.sendall(b"mouse_button 1\n")
            time.sleep(0.4)
            for step in range(1, 13):
                move_to(DROP_X + 4 + step * 21, DROP_Y + 2 - step * 4, steps=1)
            time.sleep(0.5)
            connection.sendall(b"mouse_button 0\n")
            time.sleep(1.5)
            restored = screenshot("restored.ppm")
            connection.sendall(b"quit\n")
            connection.close()
        finally:
            qemu.terminate()
            qemu.wait(timeout=10)

        text_before = count_colour(before, FOREGROUND, TEXT_LEFT, TEXT_TOP, TEXT_W, TEXT_H)
        text_covered = count_colour(covered, FOREGROUND, TEXT_LEFT, TEXT_TOP, TEXT_W, TEXT_H)
        text_after = count_colour(restored, FOREGROUND, TEXT_LEFT, TEXT_TOP, TEXT_W, TEXT_H)
        moved_frame = count_colour(covered, FRAME_FOCUSED, DROP_X, DROP_Y, PANE_W, PANE_H)

        failures = []
        if text_before == 0:
            failures.append("there was no text under the drop point to begin with")
        if moved_frame == 0:
            failures.append(f"the window did not arrive at ({DROP_X},{DROP_Y})")
        if text_covered >= text_before:
            failures.append(f"the window did not cover the text ({text_covered} vs {text_before} lit pixels)")
        if text_after != text_before:
            failures.append(
                f"the text did not come back: {text_after} lit pixels, expected {text_before}"
            )
        if failures:
            raise SystemExit("dragging wrong:\n  " + "\n  ".join(failures))
    print(f"OK: a dragged window covered {text_before} lit pixels of text and gave every one of them back")


if __name__ == "__main__":
    main()
