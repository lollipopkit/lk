#!/usr/bin/env python3
"""Boot with a disk attached: read a sector, write one, and read a file by name.

Four claims, and the third is the one that needs a machine to be *stopped* to
make:

1. `disk` reports the drive's own sector count (from IDENTIFY) and the first
   bytes of sector 0 — bytes this script put there before booting, so they
   cannot have come from anywhere else.
2. `disk w` writes a sector (past the archive) and reads it back through the
   controller.
3. After QEMU has exited, the image *file* holds what was written. A drive
   acknowledges a write long before it is on the medium; only looking from
   outside, afterwards, tells the difference between a write that happened and
   a write that was merely accepted.
4. `cat` finds a file *by name* in the tar archive that starts at sector 0 and
   prints its contents — the archive is built here with Python's `tarfile`, so
   what the kernel walks is a real archive written by something else, not a
   layout invented to be easy to parse.
"""

import io
import tarfile

import os
import socket
import subprocess
import tempfile
import time

from kernel import kernel_image

SECTOR = 512
# What this script puts in sector 0, and what the program writes into sector 1.
PLANTED = b"hello.txt"
WRITTEN = b"LK-WROTE-SECTOR1"
# 64 sectors: small enough to build in memory, and the count IDENTIFY should
# report back.
SECTORS = 64
# The file `cat` is asked for, and what it must print. Two lines, so a reader
# that stops at the first newline fails here rather than looking right.
FILE_NAME = "hello.txt"
FILE_BODY = b"HELLO FROM DISK\nSECOND LINE\n"
# Where `disk w` writes. Past the archive, so the write does not damage the
# filesystem the same run is reading — a test that corrupts its own fixture
# passes once and then confuses whoever reads it next.
SCRATCH_SECTOR = 32


def send_line(connection, text):
    """Types `text` and Enter through the monitor, a key at a time."""
    # QEMU's `sendkey` takes key *names*, not characters: punctuation has to be
    # spelled or it is silently dropped, which shows up as a command that was
    # typed with a character missing.
    names = {" ": "spc", "-": "minus", ".": "dot", "/": "slash"}
    for character in text:
        connection.sendall(f"sendkey {names.get(character, character)}\n".encode())
        time.sleep(0.25)
    connection.sendall(b"sendkey ret\n")
    time.sleep(2.5)


def main():
    image = kernel_image()
    with tempfile.TemporaryDirectory() as workdir:
        disk = os.path.join(workdir, "disk.img")
        # A real tar archive, written by Python's `tarfile`. Sector 0 is its
        # first header, whose 100-byte name field starts with the file's name —
        # which is also why the planted marker is a *file name*: one string can
        # serve both the raw-sector read and the archive walk.
        archive = io.BytesIO()
        with tarfile.open(fileobj=archive, mode="w", format=tarfile.USTAR_FORMAT) as tar:
            for name, body in [(PLANTED.decode(), FILE_BODY), ("motd", b"LK OS\n")]:
                info = tarfile.TarInfo(name)
                info.size = len(body)
                info.mtime = 0
                info.uid = 0
                info.gid = 0
                info.uname = ""
                info.gname = ""
                tar.addfile(info, io.BytesIO(body))
        contents = bytearray(SECTOR * SECTORS)
        contents[: archive.tell()] = archive.getvalue()
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
            # `ls` walks the archive: an entry's length is in its own header,
            # so where the next one starts is not known until this one is read.
            # Both entries and both sizes, because a walk that stops after the
            # first would still print something.
            send_line(connection, "ls")
            send_line(connection, "disk")
            send_line(connection, "cat hello.txt")
            send_line(connection, "disk w")
            connection.sendall(b"quit\n")
            connection.close()
        finally:
            qemu.terminate()
            qemu.wait(timeout=10)

        with open(serial, errors="replace") as handle:
            transcript = handle.read()
        failures = []
        for expected in (f"{FILE_NAME} {len(FILE_BODY)}", "motd 6"):
            if expected not in transcript:
                failures.append(f"`ls` did not list {expected!r}")
        expected_read = f"{SECTORS} {PLANTED.decode()}"
        if expected_read not in transcript:
            failures.append(f"`disk` did not report {expected_read!r}")
        for line in FILE_BODY.decode().splitlines():
            if line not in transcript:
                failures.append(f"`cat {FILE_NAME}` did not print {line!r}")
        if WRITTEN.decode() not in transcript:
            failures.append(f"`disk w` did not read back {WRITTEN.decode()!r}")
        # The medium itself, with the machine stopped.
        with open(disk, "rb") as handle:
            handle.seek(SECTOR * SCRATCH_SECTOR)
            landed = handle.read(len(WRITTEN))
        if landed != WRITTEN:
            failures.append(f"sector {SCRATCH_SECTOR} of the image is {landed!r}, expected {WRITTEN!r}")
        if failures:
            raise SystemExit(
                "disk driver wrong:\n  " + "\n  ".join(failures) + "\n--- serial ---\n" + transcript
            )
    print(
        f"OK: read sector 0, printed {FILE_NAME} from the tar archive, "
        f"wrote sector {SCRATCH_SECTOR}, and the image file holds it"
    )


if __name__ == "__main__":
    main()
