#!/usr/bin/env python3
"""A driver written in LK, read off the disk, and run *interpreted*.

`check_run.py` shows the kernel hosting an interpreter: a program it had never
seen computes an answer and prints it. This shows that the same interpreter
reaches the *hardware* — the program below drives the CMOS clock through port
I/O, on a machine whose kernel was compiled hours earlier and never told about
it.

Why that is worth its own check: everything else here proves LK can be
*compiled* into the layer that drives devices. This proves the language can
drive them without being compiled at all — a driver you can edit on the disk and
re-run, on the machine, with no toolchain in sight. The `port_*` and `volatile_*`
intrinsics live in `VmContext`, not in the AOT lowering, so the executor has
them; nothing else asserts that.

The answer is checkable rather than plausible, for the reason `check_clock.py`
gives: QEMU is told a fixed base time, and every field of it is a value a
missing BCD conversion would mangle (59 → 89, 19 → 25). The program does its own
BCD conversion — that is part of what is being tested — so a wrong reading is a
wrong number rather than a crash.
"""

import io
import os
import re
import socket
import subprocess
import sys
import tarfile
import tempfile
import time

SECTOR = 512
SECTORS = 64

# Fixed, so the answer is known. Chosen as `check_clock.py` chooses it: every
# field is one a missing BCD conversion turns into a different number.
BASE = "2019-11-19T19:59:58"
BASE_HOUR, BASE_MINUTE = 19, 59
# How far the guest's clock may have moved by the time the program runs: a boot,
# a command typed one key at a time, and the interpreter's own parse and run.
MAX_DRIFT_SECONDS = 300

# The driver. Deliberately the same job `drivers/rtc.lk` does compiled, so the
# comparison is between *how it runs*, not between two different programs.
DRIVER = b"""fn cmos(reg: Int) -> Int {
    unsafe { port_out_u8(0x70, (reg | 0x80) as u8); };
    return unsafe { port_in_u8(0x71) } as Int;
}

fn from_bcd(v: Int) -> Int {
    return ((v / 16) as Int) * 10 + (v & 0x0f);
}

let guard = 0;
while (guard < 100000 && (cmos(0x0a) & 0x80) != 0) {
    guard = guard + 1;
}
let second = from_bcd(cmos(0x00));
let minute = from_bcd(cmos(0x02));
let hour = from_bcd(cmos(0x04));
println("INTERPRETED RTC " + hour + " " + minute + " " + second);
"""


def send_line(connection, text):
    names = {" ": "spc", "-": "minus", ".": "dot", "/": "slash"}
    for character in text:
        connection.sendall(f"sendkey {names.get(character, character)}\n".encode())
        time.sleep(0.25)
    connection.sendall(b"sendkey ret\n")
    # The whole front end runs — parse, type-check, execute — out of a bump
    # allocator in a kernel built for size.
    time.sleep(12)


def main():
    image = sys.argv[1] if len(sys.argv) > 1 else (
        "target/x86_64-unknown-none/release/lk-bare-metal-x86.multiboot"
    )
    with tempfile.TemporaryDirectory() as workdir:
        disk = os.path.join(workdir, "disk.img")
        monitor = os.path.join(workdir, "monitor")
        serial = os.path.join(workdir, "serial.txt")

        archive = io.BytesIO()
        with tarfile.open(fileobj=archive, mode="w", format=tarfile.USTAR_FORMAT) as tar:
            info = tarfile.TarInfo("rtc.lk")
            info.size = len(DRIVER)
            info.mtime = 0
            tar.addfile(info, io.BytesIO(DRIVER))
        image_bytes = archive.getvalue()
        with open(disk, "wb") as handle:
            handle.write(image_bytes)
            handle.write(b"\0" * (SECTOR * SECTORS - len(image_bytes) % (SECTOR * SECTORS)))

        qemu = subprocess.Popen(
            [
                "qemu-system-x86_64",
                "-kernel", image,
                "-display", "none",
                "-rtc", f"base={BASE}",
                "-drive", f"file={disk},format=raw,if=ide",
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
            send_line(connection, "run rtc.lk")
            connection.sendall(b"quit\n")
            connection.close()
        finally:
            qemu.terminate()
            qemu.wait(timeout=10)

        with open(serial, errors="replace") as handle:
            transcript = handle.read()
        # The spinner writes to the same serial line from a timer interrupt, so
        # a line can arrive with another task's byte stuck into it.
        cleaned = re.sub(r"[AB]", "", transcript)

        failures = []
        reading = re.search(r"INTERPRETED RTC (\d+) (\d+) (\d+)", cleaned)
        if reading is None:
            note = ""
            stage = re.search(r"failed (\d+)", transcript)
            if stage:
                # -5 is "it ran and raised" (see `kernel_run` in src/main.rs).
                note = f" (the interpreter reported stage {stage.group(1)})"
            failures.append(f"the interpreted driver printed nothing{note}")
        else:
            hour, minute, second = (int(part) for part in reading.groups())
            if not (0 <= hour < 24 and 0 <= minute < 60 and 0 <= second < 60):
                failures.append(f"read {hour}:{minute}:{second}, which is not a time")
            else:
                base = BASE_HOUR * 3600 + BASE_MINUTE * 60 + 58
                drift = hour * 3600 + minute * 60 + second - base
                if not 0 <= drift <= MAX_DRIFT_SECONDS:
                    failures.append(
                        f"read {hour}:{minute:02d}:{second:02d}, which is {drift}s from the "
                        f"{BASE} QEMU was told to keep — the registers were decoded wrongly, or "
                        f"not read at all"
                    )

        if "exception #" in transcript:
            fault = re.search(r"!! exception .*", transcript)
            failures.append(f"the machine faulted: {fault.group(0)!r}")

        if failures:
            print("\n".join(failures))
            print("--- transcript ---")
            print(transcript)
            return 1
        print(
            "OK: a driver read off the disk and run interpreted drove the CMOS clock through "
            "port I/O, and read the time QEMU was told to keep"
        )
        return 0


if __name__ == "__main__":
    sys.exit(main())
