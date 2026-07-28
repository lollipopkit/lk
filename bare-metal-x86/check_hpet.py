#!/usr/bin/env python3
"""The high precision timer, read by a driver written in LK.

Every other clock here is counted or coarse. The PIT raises an interrupt and the
kernel counts them, so what it knows is "about a thousand of something per
second"; the RTC knows the date and nothing finer. The HPET states its own tick
period, in femtoseconds, in a register — so a reading becomes real time by
arithmetic instead of by a constant somebody measured once.

What that makes this a test of, besides the timer:

* **64-bit registers.** The capability word's period is its *high* 32 bits, so
  `capabilities >> 32` has to be a logical shift; on an `i64` carrier the same
  expression sign-extends as soon as the chip sets its own bit 63, and the
  period comes back with no relation to time.
* **Unsigned division.** Ticks per second is `10^15 / period`, and ticks per
  microsecond `10^9 / period`. Both operands are `u64`.
* **Subtraction across readings.** `later - earlier` at 64 bits is a distance
  whatever the origin, including across the counter's own wrap.

So the assertions below are not only "there is a timer". They are that the rate
the driver computed from the chip's own statement agrees with the ticks it
counted — two numbers that come apart if either is derived wrongly.

One thing this cannot claim: that it would catch the *signed* forms. QEMU's
capability word has bit 63 clear, so an arithmetic `>> 32` answers what a
logical one does, and swapping them here changes nothing. What makes the driver
unsigned is the register, not this test; what this test catches is a rate that
disagrees with the chip, a counter that does not advance, and the two numbers
disagreeing with each other.
"""

import os
import re
import socket
import subprocess
import sys
import tempfile
import time

# What QEMU's HPET runs at: a 10 ns period, stated as 10,000,000 femtoseconds,
# so 10^15 / 10^7. Checked exactly rather than as a range — it is a fixed
# property of the emulated chip, so any other number means the driver read the
# wrong register or divided by the wrong thing. Verified to fire: pointing
# `hpet_period_femtoseconds` at the counter instead of the capability word
# reports 0 Hz here.
EXPECTED_HZ = 100_000_000
# How far the driver's microsecond figure may be from the one recomputed here
# from its own tick count. Integer division truncates at both ends, so one is
# the floor of the disagreement rather than a tolerance for being wrong.
MAX_MICROSECOND_SLACK = 2


def send_line(connection, text):
    """Types `text` and Enter through the monitor, a key at a time."""
    names = {" ": "spc", "-": "minus", ".": "dot", "/": "slash"}
    for character in text:
        connection.sendall(f"sendkey {names.get(character, character)}\n".encode())
        time.sleep(0.25)
    connection.sendall(b"sendkey ret\n")
    time.sleep(3)


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
            time.sleep(2)
            connection = socket.socket(socket.AF_UNIX)
            connection.connect(monitor)
            time.sleep(0.3)
            connection.recv(65536)
            # Twice: one reading proves the registers were decoded, two prove
            # the counter *runs*. A stopped counter — an enable bit never set,
            # or an address nothing decodes — gives the same tick delta both
            # times, and usually zero.
            for _ in range(2):
                send_line(connection, "hpet")
            connection.sendall(b"quit\n")
            connection.close()
        finally:
            qemu.terminate()
            qemu.wait(timeout=10)

        with open(serial, errors="replace") as handle:
            transcript = handle.read()

        failures = []
        # Unanchored: the spinner writes to the same serial line from a timer
        # interrupt, so a shell line can arrive with another task's byte stuck
        # to either end of it.
        readings = re.findall(r"hpet (\d+)Hz ticks (\d+) us (\d+)", transcript)
        if len(readings) < 2:
            note = ""
            if "no timer" in transcript:
                note = " (the driver reported no timer at the standard address)"
            elif "would not start" in transcript:
                note = " (the driver could not start the counter)"
            failures.append(f"expected 2 readings, got {len(readings)}{note}")

        for hertz, ticks, micros in readings:
            hertz, ticks, micros = int(hertz), int(ticks), int(micros)
            if hertz != EXPECTED_HZ:
                failures.append(
                    f"the driver computed {hertz} Hz from the chip's period, not {EXPECTED_HZ}"
                )
            if ticks == 0:
                failures.append("the counter did not advance between two reads of it")
                continue
            # The driver's own two numbers, checked against each other: the
            # rate came from the chip's stated period, the microseconds from
            # the ticks and that same period, so a mistake in either shows up
            # as the two disagreeing while each still looks like a number.
            expected_micros = ticks // (hertz // 1_000_000) if hertz >= 1_000_000 else 0
            if abs(micros - expected_micros) > MAX_MICROSECOND_SLACK:
                failures.append(
                    f"{ticks} ticks at {hertz} Hz is {expected_micros} us, "
                    f"but the driver said {micros}"
                )

        if len(readings) >= 2 and readings[0][1] == readings[1][1]:
            failures.append(
                f"both readings counted exactly {readings[0][1]} ticks, which is a "
                f"counter that is not running"
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
            f"OK: found the HPET at its standard address, computed {EXPECTED_HZ} Hz from the "
            f"period it states, and watched its 64-bit counter advance"
        )
        return 0


if __name__ == "__main__":
    sys.exit(main())
