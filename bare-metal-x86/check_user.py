#!/usr/bin/env python3
"""Boot, drop to ring 3, and check that the boundary is the machine's, not the
program's good manners.

Three claims, and the second is the one that makes the first mean anything:

1. **A ring-3 program can talk to the kernel**, and the kernel does not believe
   what it is told. `USER` comes out a byte at a time through `int 0x80` — the
   only vector whose gate has DPL 3. `str` comes out through a call that takes a
   *pointer*, which the kernel checks against the bounds of the user section
   before following. `N` is the same call handed the kernel's own address and
   refusing it; a `Y` would mean the kernel printed its own memory on request.
2. **It cannot touch the kernel's memory** — including the kernel *next to it*.
   Immediately after, it reads `0x100010`: the kernel's first instruction, in
   the same 2 MiB as the program itself. The CPU faults with a user-mode error
   code. That address is the point: while the first 2 MiB was one
   user-accessible page, this read succeeded and told nobody, and only a
   4 KiB-granular table makes it the fault it should be.

3. **Two ring-3 tasks, preempted, in address spaces of their own.** Both are
   spawned at boot, both never yield, and both keep a stack at the *same*
   virtual address — `0x4000_0000`, which the kernel's own space maps to memory
   this machine does not have. Each writes one letter into its stack and prints
   what it reads back for ever: `A` and `B`. Sharing a space would mean the
   second write landed on the first's page and both printed the same letter.
   That the shell answers in the middle of it is the preemption claim; the
   one-shot program above cannot show it, having no way back at all.

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
# `USER` is the byte-at-a-time call; `str` is the same text through the call
# that takes a *pointer*, which the kernel checked before following; `N` is that
# call refusing a kernel pointer. A `Y` there would mean the kernel read its own
# memory because a user task asked it to.
GREETING = "USERstrN"
# The kernel's first instruction — in the same 2 MiB as the user program, which
# is what makes it the interesting address to be refused.
FORBIDDEN = "cr2=0000000000100010"
# present | user: a read, from ring 3, of a page that is there but not marked
# user-accessible. Bit 2 is the whole claim — the same fault from ring 0 would
# be a kernel bug with the same `cr2`.
USER_WRITE_FAULT = "error=0000000000000005"


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
        if while_running.count("A") < 3 or while_running.count("B") < 3:
            failures.append("both ring-3 tasks did not run")
        # Interleaved, not one after the other: a task that ran to completion
        # before the other started would say nothing about preemption.
        if "AB" not in while_running and "BA" not in while_running:
            failures.append("the two ring-3 tasks never interleaved")
        if "\n5\n" not in while_running and "\n6\n" not in while_running:
            failures.append("the shell did not answer while the ring-3 task was running")
        if GREETING not in transcript:
            failures.append(
                f"expected {GREETING!r}: the syscalls, the checked pointer, and the refusal of a kernel one"
            )
        if "#PF page fault" not in transcript:
            failures.append("the forbidden write did not fault: the ring boundary is not enforced")
        if FORBIDDEN not in transcript:
            failures.append(f"the fault was not at the kernel's own code ({FORBIDDEN})")
        if USER_WRITE_FAULT not in transcript:
            failures.append(f"the fault was not a ring-3 access ({USER_WRITE_FAULT})")
        if failures:
            raise SystemExit(
                "ring 3 wrong:\n  " + "\n  ".join(failures) + "\n--- serial ---\n" + transcript[-400:]
            )
    print("OK: ring 3 spoke through a syscall, was preempted while never yielding, "
        "and was refused the kernel page next to its own")


if __name__ == "__main__":
    main()
