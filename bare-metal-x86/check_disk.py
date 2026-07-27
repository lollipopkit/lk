#!/usr/bin/env python3
"""Boot with a disk attached, read a sector, write one, and check the medium.

Three claims, and the third is the one that needs a machine to be *stopped* to
make:

1. `disk` reports the drive's own sector count (from IDENTIFY) and the first
   bytes of sector 0 — bytes this script put there before booting, so they
   cannot have come from anywhere else.
2. `disk w` writes a sector and reads it back through the controller.
3. After QEMU has exited, the image *file* holds what was written. A drive
   acknowledges a write long before it is on the medium; only looking from
   outside, afterwards, tells the difference between a write that happened and
   a write that was merely accepted.
"""

import os
import socket
import subprocess
import sys
import tempfile
import time

SECTOR = 512
# What this script puts in sector 0, and what the program writes into sector 1.
PLANTED = b"LK-DISK-OK-01234"
WRITTEN = b"LK-WROTE-SECTOR1"
# 64 sectors: small enough to build in memory, and the count IDENTIFY should
# report back.
SECTORS = 64


def send_line(connection, text):
    """Types `text` and Enter through the monitor, a key at a time."""
    names = {" ": "spc", "-": "minus"}
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
        disk = os.path.join(workdir, "disk.img")
        contents = bytearray(SECTOR * SECTORS)
        contents[: len(PLANTED)] = PLANTED
        with open(disk, "wb") as handle:
            handle.write(contents)

        monitor = os.path.join(workdir, "monitor")
        serial = os.path.join(workdir, "serial.txt")
        qemu = subprocess.Popen(
            [
                "qemu-system-x86_64",
                "-kernel", image,
                "-display", "none",
                "-drive", f"file={disk},format=raw,if=ide,index=0,media=disk",
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
            send_line(connection, "disk")
            send_line(connection, "disk w")
            connection.sendall(b"quit\n")
            connection.close()
        finally:
            qemu.terminate()
            qemu.wait(timeout=10)

        with open(serial, errors="replace") as handle:
            transcript = handle.read()
        failures = []
        expected_read = f"{SECTORS} {PLANTED.decode()}"
        if expected_read not in transcript:
            failures.append(f"`disk` did not report {expected_read!r}")
        if WRITTEN.decode() not in transcript:
            failures.append(f"`disk w` did not read back {WRITTEN.decode()!r}")
        # The medium itself, with the machine stopped.
        with open(disk, "rb") as handle:
            handle.seek(SECTOR)
            landed = handle.read(len(WRITTEN))
        if landed != WRITTEN:
            failures.append(f"sector 1 of the image is {landed!r}, expected {WRITTEN!r}")
        if failures:
            raise SystemExit(
                "disk driver wrong:\n  " + "\n  ".join(failures) + "\n--- serial ---\n" + transcript
            )
    print(f"OK: read sector 0 ({PLANTED.decode()}), wrote sector 1, and the image file holds it")


if __name__ == "__main__":
    main()
