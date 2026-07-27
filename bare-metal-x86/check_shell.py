#!/usr/bin/env python3
"""Boot the image, type at its shell through QEMU's monitor, and check the answers.

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
# A session: a mistyped command corrected with backspace, then `echo`, then
# enough blank lines to push the banner off the top — which is what tells
# scrolling apart from wrapping — and finally `exit`.
KEYS = (
    ["h", "e", "l", "x", "backspace", "p", "ret"]
    + ["e", "c", "h", "o", "spc", "l", "k", "ret"]
    + ["p", "a", "g", "e", "ret"]
    + ["p", "a", "g", "e", "ret"]
    + ["m", "e", "m", "ret"]
    + ["s", "y", "n", "c", "ret"]
    + ["y", "i", "e", "l", "d", "ret"]
    + ["w", "i", "n", "ret"]
    # Focus moves to the other pane, `help` is typed there, focus comes back.
    # The shell must not answer the second one: that is the whole claim.
    + ["tab"]
    + ["h", "e", "l", "p", "ret"]
    + ["tab"]
    + ["ret"] * 22
    + ["e", "x", "i", "t", "ret"]
)
# Lines the shell must answer with. `help` lists the commands it knows, `echo`
# repeats its argument, `exit` says goodbye — each proving a different part:
# the byte-wise command match, the argument tail, and the loop ending.
EXPECTED_LINES = [
    "help clear echo keys mem page sync yield win time disk cat run exit",
    "lk",
    # Two pages handed out in order, from the range the loader reported. The
    # addresses are what proves the allocator rather than a counter.
    "02000000",
    "02001000",
    # `yield` leaves LK, enters the kernel through a software interrupt, is
    # rescheduled, and comes back. Printing at all is the proof.
    "back",
    "bye",
]

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
    # `sync` reports two counters that the timer handler and the spinner task
    # both bump, under one critical section. They can only differ if an
    # increment read a stale value — which is exactly what happens without the
    # section, and what nothing else in the system can cause.
    pair = next((line for line in output.splitlines() if "/" in line and line.strip("./0123456789") == ""), None)
    if pair is None:
        raise SystemExit("shell: `sync` printed no counter pair")
    # Every dot, not just the ends: the timer prints one on whatever line is
    # current, and `31.710/31712` would otherwise split into two counters that
    # differ — a passing run reported as a lost update.
    left, right = pair.replace(".", "").split("/")
    if left != right:
        raise SystemExit(f"shared counters diverged ({left} != {right}): an update was lost")
    if int(left) == 0:
        raise SystemExit("shared counters never moved: the tasks are not contending")
    # The shell's answers. Checked as substrings because the timer handler
    # prints a '.' every half-second on the same line.
    for expected in EXPECTED_LINES:
        if expected not in output:
            raise SystemExit(f"shell: expected {expected!r} in the output")
    if EXPECTED_REPORT not in output:
        raise SystemExit(f"shell: expected {EXPECTED_REPORT!r} in the output")
    answered = output.count("help clear echo")
    if answered != 1:
        raise SystemExit(
            f"the shell answered `help` {answered} times, expected 1: "
            "the second one was typed with the focus on another window"
        )

    _magic, dimensions, _maxval, pixels = data.split(b"\n", 3)
    width, _height = (int(value) for value in dimensions.split())
    # The title was amber at cell (1,1). After scrolling, that row is either
    # background or whatever moved up into it — never the title colour.
    offset = ((1 * 8) * width + (1 * 6)) * 3
    if tuple(pixels[offset:offset + 3]) == (0xFF, 0xC0, 0x40):
        raise SystemExit("the title is still at the top: the screen wrapped instead of scrolling")
    print(f"OK: {len(KEYS)} keystrokes; commands answered and the screen scrolled")


if __name__ == "__main__":
    main()
