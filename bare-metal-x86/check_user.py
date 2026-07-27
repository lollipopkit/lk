#!/usr/bin/env python3
"""Boot, drop to ring 3, and check that the boundary is the machine's, not the
program's good manners.

Three claims, and the second is the one that makes the first mean anything:

1. **A ring-3 program can talk to the kernel.** It prints `USER` a byte at a
   time through `int 0x80` — the only vector whose gate has DPL 3, so it is the
   only instruction that takes a user task into the kernel on purpose.
2. **It cannot touch the kernel's memory.** Immediately after, it writes to
   `0x300000` — the shared page every interrupt handler uses — and the CPU
   faults with `cr2=0x300000` and a user-mode error code. Until this existed,
   "the program does not touch the framebuffer" was a fact about the program;
   now it is a fact about the page tables.

3. **A ring-3 task can be preempted.** A separate task, spawned into ring 3 at
   boot, prints `3` for ever and never yields — and the shell answers a command
   while it runs. The timer took the CPU from ring 3 and gave it back, which the
   one-shot program above cannot show because it has no way back at all.

The error code is checked, not just the address: bit 2 is what says the access
came from ring 3. A fault at that address from ring 0 would be a kernel bug
with the same `cr2` and a different meaning.
"""

import os
import socket
import subprocess
import sys
import tempfile
import time

# What the ring-3 program prints through the syscall, and where it then tries to
# write. Both are in `src/user.rs`.
GREETING = "USER"
FORBIDDEN = "cr2=0000000000300000"
# present | write | user: the access was a write, from ring 3, to a page that is
# there but not user-accessible.
USER_WRITE_FAULT = "error=0000000000000007"


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
            time.sleep(2.5)
            connection = socket.socket(socket.AF_UNIX)
            connection.connect(monitor)
            time.sleep(0.3)
            connection.recv(65536)
            # First: does the shell still answer while a ring-3 task runs?
            for key in ["k", "e", "y", "s", "ret"]:
                connection.sendall(f"sendkey {key}\n".encode())
                time.sleep(0.3)
            time.sleep(2)
            with open(serial, errors="replace") as handle:
                while_running = handle.read()

            for key in ["u", "s", "e", "r", "ret"]:
                connection.sendall(f"sendkey {key}\n".encode())
                time.sleep(0.3)
            # The fault ends the excursion; the reporter prints and halts.
            time.sleep(4)
            connection.sendall(b"quit\n")
            connection.close()
        finally:
            qemu.terminate()
            qemu.wait(timeout=10)

        with open(serial, errors="replace") as handle:
            transcript = handle.read()

        failures = []
        # The ring-3 task's own output, and the shell answering in the middle
        # of it: either alone proves nothing.
        if while_running.count("3") < 5:
            failures.append("the ring-3 task never ran")
        if "\n5\n" not in while_running and "\n6\n" not in while_running:
            failures.append("the shell did not answer while the ring-3 task was running")
        if GREETING not in transcript:
            failures.append(f"ring 3 did not print {GREETING!r} through the syscall")
        if "#PF page fault" not in transcript:
            failures.append("the forbidden write did not fault: the ring boundary is not enforced")
        if FORBIDDEN not in transcript:
            failures.append(f"the fault was not at the shared page ({FORBIDDEN})")
        if USER_WRITE_FAULT not in transcript:
            failures.append(f"the fault was not a ring-3 write ({USER_WRITE_FAULT})")
        if failures:
            raise SystemExit(
                "ring 3 wrong:\n  " + "\n  ".join(failures) + "\n--- serial ---\n" + transcript[-400:]
            )
    print("OK: ring 3 spoke through a syscall, was preempted while never yielding, and was refused kernel memory")


if __name__ == "__main__":
    main()
