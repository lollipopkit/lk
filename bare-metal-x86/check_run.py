#!/usr/bin/env python3
"""Boot, read an LK program off the disk, and run it.

The kernel is compiled LK. What this checks is that it also *hosts an
interpreter* for LK: a program that was not part of the image, is not known
until it is read, and prints through the kernel's own console.

The program is chosen so that no part of the answer can be guessed from the
image: the sum of the squares of 1..10 is 385, and neither the number nor the
loop that computes it appears anywhere in the kernel. If the interpreter did not
run, nothing prints 385.

The second half is the failure path. Asking for a file that is not there, and
running a program that does not parse, must both come back as a *report* — a
kernel that dies on bad input from a disk is not one you can put a disk in.
"""

import io
import os
import socket
import subprocess
import sys
import tarfile
import tempfile
import time

SECTOR = 512
SECTORS = 64

PROGRAM = b"""let total = 0;
for i in 1..=10 {
    total = total + i * i;
}
println("SUM OF SQUARES");
println(total);
"""
ANSWER = "385"
# `fn` with no body and no closing brace: a parse failure, not a runtime one, so
# the kernel has to survive the stage that runs *before* any of the program does.
BROKEN = b"fn (((\n"


def send_line(connection, text):
    names = {" ": "spc", "-": "minus", ".": "dot", "/": "slash"}
    for character in text:
        connection.sendall(f"sendkey {names.get(character, character)}\n".encode())
        time.sleep(0.25)
    connection.sendall(b"sendkey ret\n")
    # Parsing, type-checking and interpreting a program takes far longer than a
    # shell command: the whole front end runs, out of a bump allocator, in a
    # kernel built for size rather than speed.
    time.sleep(8)


def main():
    image = sys.argv[1] if len(sys.argv) > 1 else (
        "target/x86_64-unknown-none/release/lk-bare-metal-x86.multiboot"
    )
    with tempfile.TemporaryDirectory() as workdir:
        disk = os.path.join(workdir, "disk.img")
        archive = io.BytesIO()
        with tarfile.open(fileobj=archive, mode="w", format=tarfile.USTAR_FORMAT) as tar:
            for name, body in [("sq.lk", PROGRAM), ("bad.lk", BROKEN)]:
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
            send_line(connection, "run sq.lk")
            send_line(connection, "run nope.lk")
            send_line(connection, "run bad.lk")
            # The shell must still be answering afterwards.
            send_line(connection, "keys")
            connection.sendall(b"quit\n")
            connection.close()
        finally:
            qemu.terminate()
            qemu.wait(timeout=10)

        with open(serial, errors="replace") as handle:
            transcript = handle.read()
        failures = []
        if "SUM OF SQUARES" not in transcript:
            failures.append("the program's own output is missing")
        if ANSWER not in transcript:
            failures.append(f"the interpreter did not compute {ANSWER}")
        if "no file" not in transcript:
            failures.append("`run nope.lk` did not report a missing file")
        # -3 is the parse stage (see `kernel_run` in src/main.rs).
        if "failed 3" not in transcript:
            failures.append("`run bad.lk` did not report the parse failure")
        if failures:
            raise SystemExit(
                "hosting the interpreter is wrong:\n  "
                + "\n  ".join(failures)
                + "\n--- serial ---\n"
                + transcript
            )
    print(f"OK: the kernel read an LK program off the disk, ran it ({ANSWER}), and survived two bad ones")


if __name__ == "__main__":
    main()
