#!/usr/bin/env python3
"""Boot and check that three tasks are running, two of them spawned by name.

The board no longer knows what tasks exist. `src/tasks.rs` supplies stacks and
the switch; *which* code runs on them is decided by the program, which calls
`spawn_task(symbol_address("lk_task_..."))` — a code address, from a table, the
way a kernel starts anything.

What that is checked by is the screen. Two windows are owned by two different
tasks — a spinner and a clock — and the shell is a third. If either window stops
changing while the shell sits idle at its prompt, either the task was never
started or the scheduler stopped reaching it; both are the same failure from
here, and both are what this catches.

The clock is the sharper of the two: it is driven by the tick count, so it
changing means a task ran *and* the timer interrupt is still being delivered
while other tasks hold the CPU.
"""

import os
import socket
import subprocess
import sys
import tempfile
import time

# The two windows, as `program.lk` places them: the spinner at the top right and
# the clock three rows below it. Sampled inside their frames.
SPINNER = (254, 2, 30, 8)
CLOCK = (254, 26, 44, 8)
# Between samples: long enough for the clock to have moved a whole second on.
SETTLE = 1.4


def read_ppm(path):
    with open(path, "rb") as handle:
        data = handle.read()
    _magic, dimensions, _maxval, pixels = data.split(b"\n", 3)
    width, height = (int(value) for value in dimensions.split())
    return width, height, pixels


def region(path, rect):
    left, top, width, height = rect
    image_width, _height, pixels = read_ppm(path)
    return b"".join(
        pixels[((y * image_width + x) * 3):((y * image_width + x) * 3 + 3)]
        for y in range(top, top + height)
        for x in range(left, left + width)
    )


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
            time.sleep(3)
            connection = socket.socket(socket.AF_UNIX)
            connection.connect(monitor)
            time.sleep(0.3)
            connection.recv(65536)

            def screenshot(name):
                path = os.path.join(workdir, name)
                connection.sendall(f"screendump {path}\n".encode())
                time.sleep(SETTLE)
                return path

            # Several samples rather than two. Both windows change *periodically*
            # — the clock once a second, the spinner once a slice — so two shots
            # can land on the same phase and show no difference on a machine
            # where everything is working. Asking "did it ever change" over a
            # series is the same claim without the coin flip.
            #
            # It is still wall-clock, though, and that makes it the one check
            # here sensitive to what else the *host* is doing: with a compile
            # running alongside, four samples 1.4s apart have all landed inside
            # one spinner phase and reported both windows frozen. Three such
            # failures in a row once looked exactly like a real regression and
            # were not — the same build passed 4/4 on an idle machine. Re-run a
            # failure here on a quiet host before believing it.
            shots = [screenshot(f"sample{i}.ppm") for i in range(4)]
            connection.sendall(b"quit\n")
            connection.close()
        finally:
            qemu.terminate()
            qemu.wait(timeout=10)

        with open(serial, errors="replace") as handle:
            transcript = handle.read()

        failures = []
        if "no tasks" in transcript:
            failures.append("the program reported that a task could not be spawned")
        for name, rect in [("spinner", SPINNER), ("clock", CLOCK)]:
            seen = {region(shot, rect) for shot in shots}
            if len(seen) < 2:
                failures.append(f"the {name} task's window never changed across {len(shots)} samples")
        if failures:
            raise SystemExit(
                "spawning wrong:\n  " + "\n  ".join(failures) + "\n--- serial ---\n" + transcript[-400:]
            )
    print("OK: three tasks, two of them spawned from an address the program looked up by name")


if __name__ == "__main__":
    main()
