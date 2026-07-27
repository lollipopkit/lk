#!/usr/bin/env python3
"""A PCI device, driven by LK: found on the bus, reached through its BAR, made
to compute, made to write RAM by itself, and made to interrupt.

Every other device this kernel drives is a fixed-port ISA relic. They are found
by knowing their address, spoken to with `in`/`out`, and they never touch memory
on their own — so none of them shows whether LK can write the kind of driver a
modern machine actually needs. This one does, in five claims:

1. `pci` lists what is on the bus. The list has to *contain* the device this
   script attached and not be a fixed set of lines: the script passes
   `-device edu` and looks for `1234:11e8` in the output, at a slot QEMU chose.
2. The device's registers answer at the address configuration space gave. The
   identification register is a known constant, so a driver that computed the
   BAR wrong reads zeros or all-ones instead — and `edu` reports which step
   failed, so "the BAR is wrong" and "the DMA is wrong" are different lines.
3. It computes. 5! comes back as 120 through the busy-bit protocol, which is
   the shape of every real offload: write the operand, poll the device's own
   status, read the answer back out of the same register.
4. It writes RAM. A pattern goes out to the device's internal memory and comes
   back to a *different* address, which is the only one of the four that no
   amount of port I/O could have done — the bytes at the second address can
   only be there if the device's DMA engine put them there.
5. It interrupts, and the driver installs the gate for it *itself*, at the line
   configuration space named — a kernel that installed every handler at startup
   could not drive a device it had not been told about. `edu` is run twice: a
   handler that acknowledged the device but not the chip, or the chip but not
   the device, passes the first run and hangs or refaults on the second.

The whole driver is `drivers/pci.lk` and `drivers/edu.lk`; the board contributes
nothing to this path.
"""

import os
import re
import socket
import subprocess
import sys
import tempfile
import time

# What QEMU's educational PCI device answers to.
EDU_ID = "1234:11e8"


def send_line(connection, text):
    """Types `text` and Enter through the monitor, a key at a time."""
    names = {" ": "spc", "-": "minus", ".": "dot", "/": "slash"}
    for character in text:
        connection.sendall(f"sendkey {names.get(character, character)}\n".encode())
        time.sleep(0.25)
    connection.sendall(b"sendkey ret\n")
    time.sleep(2.5)


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
                # The device under test. Nothing else in this run needs it, and
                # `pci` has to find it rather than be told where it is.
                "-device", "edu",
                "-serial", "file:" + serial,
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
            send_line(connection, "pci")
            send_line(connection, "edu")
            # Twice. An interrupt path that leaves either end still asserting
            # looks identical to a working one until it is asked again.
            send_line(connection, "edu")
            connection.sendall(b"quit\n")
            connection.close()
        finally:
            qemu.terminate()
            qemu.wait(timeout=10)

        with open(serial, errors="replace") as handle:
            transcript = handle.read()

        failures = []

        # (1) The listing found it, at whatever slot QEMU picked.
        listing = [line for line in transcript.splitlines() if EDU_ID in line]
        if not listing:
            failures.append(f"`pci` did not list {EDU_ID}")
        else:
            # A memory BAR, of the size the device documents: 1 MiB. Checked
            # because the size comes from *probing* the BAR — writing all ones
            # and reading back which bits stuck — and a driver that skipped the
            # restore afterwards would have unmapped the device it just sized.
            if "@" not in listing[0]:
                failures.append(f"`pci` listed {EDU_ID} with no memory BAR: {listing[0]!r}")
            elif not re.search(r"\+00100000\b", listing[0]):
                failures.append(f"`pci` sized the BAR wrong: {listing[0]!r}")

        # A bus with only the device under test on it would mean enumeration
        # found one thing and stopped. QEMU's default machine has a host bridge
        # and an ISA bridge before anything is attached.
        # Unanchored: the spinner task writes to the same serial line from a
        # timer interrupt, so a shell line can arrive with another task's byte
        # stuck to either end of it.
        counted = re.search(r"(\d+) devices", transcript)
        if not counted:
            failures.append("`pci` did not report how many devices it found")
        elif int(counted.group(1)) < 3:
            failures.append(f"`pci` found only {counted.group(1)} devices")

        # (2)(3)(4)(5) The driver reports which step failed, so this can too.
        completed = re.findall(r"edu: ok irq \d+ bar [0-9a-f]{8}", transcript)
        if len(completed) < 2:
            step = re.search(r"edu: (?!ok)\S+", transcript)
            failures.append(
                f"`edu` did not complete: {step.group(0)!r}" if step
                else f"`edu` completed {len(completed)} of 2 runs"
            )

        # An unclaimed vector is a general protection fault raised from inside an
        # interrupt, and the 8259 delivers one on a line nothing is using
        # whenever a request disappears before the CPU acknowledges it. Masking
        # a line while its request is pending — which is what giving up on a
        # wait does — is enough. Checked here because the machine goes on
        # running afterwards and the transcript still looks plausible.
        if "exception #" in transcript:
            fault = re.search(r"!! exception .*", transcript)
            failures.append(f"the machine faulted: {fault.group(0)!r}")

        if failures:
            print("\n".join(failures))
            print("--- transcript ---")
            print(transcript)
            return 1
        print("OK: found a PCI device by enumeration, reached it through its BAR, "
              "made it compute 5!, made it DMA a pattern into RAM, and took its "
              "interrupt on a gate the driver installed for itself")
        return 0


if __name__ == "__main__":
    sys.exit(main())
