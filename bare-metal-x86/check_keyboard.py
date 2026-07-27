#!/usr/bin/env python3
"""Boot the image, type at it through QEMU's monitor, and check what it saw.

Also checks the *screen* afterwards. Enough newlines to reach the bottom push the title off the
top, so a screen that still shows it wrapped to the first line instead of
scrolling — a difference the serial line cannot show.

The keyboard half of a user interface cannot be checked by looking at memory:
what matters is that a key pressed on the machine reaches an LK function, is
decoded there, and comes back out. `sendkey` puts a real scancode into the
emulated PS/2 controller, which is as close to a keystroke as a headless
machine gets.
"""

import os
import socket
import subprocess
import sys
import tempfile
import time

# `x` then backspace, so the echo shows the correction; then enough newlines
# to push the title off the top, which is what proves scrolling rather than
# wrapping.
# The cursor starts on text row 3 of 25, so it takes 22 newlines to reach the
# bottom and one more to scroll. A few spare.
KEYS = ["l", "k", "x", "backspace", "o", "s"] + ["ret"] * 26
# What reaches the serial line: `backspace` is ASCII 8 and `ret` is 10, neither
# of which prints, so only the letters show.
TYPED = ["l", "k", "x", "o", "s"]
# The last key is Enter, whose code the program reports.
EXPECTED_REPORT = f"keys {len(KEYS)} last 10"


def main():
    image = sys.argv[1] if len(sys.argv) > 1 else (
        "target/x86_64-unknown-none/release/lk-bare-metal-x86.multiboot"
    )
    with tempfile.TemporaryDirectory() as workdir:
        monitor = os.path.join(workdir, "monitor")
        serial = os.path.join(workdir, "serial.txt")
        shot = os.path.join(workdir, "screen.ppm")
        qemu = subprocess.Popen(
            [
                "qemu-system-x86_64",
                "-kernel", image,
                "-display", "none",
                "-serial", "file:" + serial,
                "-monitor", f"unix:{monitor},server,nowait",
            ]
        )
        try:
            for _ in range(100):
                if os.path.exists(monitor):
                    break
                time.sleep(0.1)
            # Let the program get past drawing and into its wait loop.
            time.sleep(1.5)
            connection = socket.socket(socket.AF_UNIX)
            connection.connect(monitor)
            time.sleep(0.3)
            connection.recv(65536)
            for key in KEYS:
                connection.sendall(f"sendkey {key}\n".encode())
                time.sleep(0.12)
            # The program reports once it has seen them all.
            time.sleep(2)
            connection.sendall(f"screendump {shot}\n".encode())
            time.sleep(1.5)
            connection.sendall(b"quit\n")
            connection.close()
        finally:
            qemu.terminate()
            qemu.wait(timeout=10)

        with open(serial, "r", errors="replace") as handle:
            output = handle.read()
        with open(shot, "rb") as handle:
            data = handle.read()

    print(output)
    # In order, but not necessarily adjacent: the timer handler is transmitting
    # on the same line, so its output interleaves with the echoed keys.
    remaining = list(TYPED)
    for char in output:
        if remaining and char == remaining[0]:
            remaining.pop(0)
    if remaining:
        raise SystemExit(f"keyboard: never saw {remaining} echoed, in order, in the output")
    if EXPECTED_REPORT not in output:
        raise SystemExit(f"keyboard: expected {EXPECTED_REPORT!r} in the output")
    _magic, dimensions, _maxval, pixels = data.split(b"\n", 3)
    width, _height = (int(value) for value in dimensions.split())
    # The title was amber at cell (1,1). After scrolling, that row is either
    # background or whatever moved up into it — never the title colour.
    offset = ((1 * 8) * width + (1 * 6)) * 3
    if tuple(pixels[offset:offset + 3]) == (0xFF, 0xC0, 0x40):
        raise SystemExit("the title is still at the top: the screen wrapped instead of scrolling")
    print(f"OK: {len(KEYS)} keystrokes decoded by LK, echoed, drawn, and the screen scrolled")


if __name__ == "__main__":
    main()
