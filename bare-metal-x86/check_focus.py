#!/usr/bin/env python3
"""Boot the image, press Tab, and check that the focus is visible.

Which window has the keyboard was previously something you could only find out
by typing at it. Each window draws its own frame — idle or focused — and
repaints when the focus moves. This checks the pixels, because "the frame
changed colour" is the whole user-facing claim, and nothing on the serial line
shows it.

The repaint is also the point of the design: the key handler that moves the
focus only writes a word. Drawing from an interrupt would put a window's
appearance in the hands of whatever happened to be interrupted, so each window
repaints itself on its own turn — and this check would fail if that never
happened.
"""

import os
import socket
import subprocess
import sys
import tempfile
import time

# The top-left pixel of each window's frame.
SHELL = (0, 0)
PANE = (42 * 6, 0)

IDLE = (0x20, 0x30, 0x40)
FOCUSED = (0xFF, 0xC0, 0x40)


def pixel(path, x, y):
    with open(path, "rb") as handle:
        data = handle.read()
    _magic, dimensions, _maxval, pixels = data.split(b"\n", 3)
    width, _height = (int(value) for value in dimensions.split())
    offset = (y * width + x) * 3
    return tuple(pixels[offset : offset + 3])


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

            def screenshot(name):
                path = os.path.join(workdir, name)
                connection.sendall(f"screendump {path}\n".encode())
                time.sleep(1.2)
                return path

            before = screenshot("before.ppm")
            connection.sendall(b"sendkey tab\n")
            time.sleep(1.5)
            after = screenshot("after.ppm")
            connection.sendall(b"quit\n")
            connection.close()
        finally:
            qemu.terminate()
            qemu.wait(timeout=10)

        checks = [
            ("the shell starts focused", pixel(before, *SHELL), FOCUSED),
            ("the pane starts idle", pixel(before, *PANE), IDLE),
            ("tab moves the focus off the shell", pixel(after, *SHELL), IDLE),
            ("tab moves the focus to the pane", pixel(after, *PANE), FOCUSED),
        ]
    wrong = [f"{what}: frame is {got}, expected {want}" for what, got, want in checks if got != want]
    if wrong:
        raise SystemExit("focus is not visible:\n  " + "\n  ".join(wrong))
    print("OK: the focused window is the highlighted one, before and after Tab")


if __name__ == "__main__":
    main()
