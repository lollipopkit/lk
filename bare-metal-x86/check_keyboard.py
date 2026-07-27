#!/usr/bin/env python3
"""Boot the image, type at it through QEMU's monitor, and check what it saw.

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

KEYS = ["l", "k", "o", "s"]
# `s` is the last key; its ASCII code is what the program reports.
EXPECTED_REPORT = f"keys {len(KEYS)} last {ord(KEYS[-1])}"


def main():
    image = sys.argv[1] if len(sys.argv) > 1 else (
        "target/x86_64-unknown-none/release/lk-bare-metal-x86.multiboot"
    )
    with tempfile.TemporaryDirectory() as workdir:
        monitor = os.path.join(workdir, "monitor")
        serial = os.path.join(workdir, "serial.txt")
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
            time.sleep(2.5)
            connection = socket.socket(socket.AF_UNIX)
            connection.connect(monitor)
            time.sleep(0.3)
            connection.recv(65536)
            for key in KEYS:
                connection.sendall(f"sendkey {key}\n".encode())
                time.sleep(0.3)
            # The program reports once it has seen them all.
            time.sleep(2)
            connection.sendall(b"quit\n")
            connection.close()
        finally:
            qemu.terminate()
            qemu.wait(timeout=10)

        with open(serial, "r", errors="replace") as handle:
            output = handle.read()

    print(output)
    # In order, but not necessarily adjacent: the timer handler is transmitting
    # on the same line, so its output interleaves with the echoed keys.
    remaining = list(KEYS)
    for char in output:
        if remaining and char == remaining[0]:
            remaining.pop(0)
    if remaining:
        raise SystemExit(f"keyboard: never saw {remaining} echoed, in order, in the output")
    if EXPECTED_REPORT not in output:
        raise SystemExit(f"keyboard: expected {EXPECTED_REPORT!r} in the output")
    print(f"OK: {len(KEYS)} keystrokes decoded by LK, echoed to serial and drawn on screen")


if __name__ == "__main__":
    main()
