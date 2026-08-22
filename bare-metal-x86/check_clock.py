#!/usr/bin/env python3
"""The battery-backed clock, read by a driver written in LK.

Every other clock in this kernel counts. The PIT counts down and raises an
interrupt, the kernel counts those, and what that gives is elapsed time: a
machine that has been up for four seconds cannot say whether it is Tuesday. The
RTC is the one thing on the board that kept running while the power was off.

QEMU is told a *fixed* base time, so this is not "close to the host clock" but an
exact answer with a known value. The moment is chosen to make the two format
questions fail loudly rather than plausibly:

* **BCD.** The registers hold binary-coded decimal unless status B says
  otherwise, so 59 arrives as `0x59`. A driver that skips the conversion reads
  it as 89 — and reads month 11 as 17, and hour 19 as 25. The base below is all
  such values, so a missing conversion is not a near miss.
* **The update window.** The chip copies its counters into the registers once a
  second, and a read that straddles that gives a mixture of before and after:
  01:59:59 becomes 01:00:59, which is a clock that is perfect except for one
  second in every hour. Waiting for the in-progress flag is not enough on its
  own — an update can begin between the check and the reads — so the driver
  reads twice and requires the two to agree. This check runs `clock` several
  times across several seconds, which is the only way to give that window a
  chance to be hit.

The whole driver is `drivers/rtc.lk`; the board contributes nothing.
"""

import os
import re
import socket
import subprocess
import sys
import tempfile
import time

from kernel import kernel_image

# Every field is one a missing BCD conversion would mangle: 11 → 17, 19 → 25,
# 59 → 89, 58 → 88.
BASE = "2019-11-19T19:59:58"
BASE_YEAR, BASE_MONTH, BASE_DAY = 2019, 11, 19
BASE_HOUR, BASE_MINUTE, BASE_SECOND = 19, 59, 58
# How far the guest's clock may have advanced by the time a reading is taken.
# Generous: the run types several commands through the monitor, each with a
# deliberate pause.
MAX_DRIFT_SECONDS = 120


def send_line(connection, text):
    """Types `text` and Enter through the monitor, a key at a time."""
    names = {" ": "spc", "-": "minus", ".": "dot", "/": "slash"}
    for character in text:
        connection.sendall(f"sendkey {names.get(character, character)}\n".encode())
        time.sleep(0.25)
    connection.sendall(b"sendkey ret\n")
    time.sleep(2.5)


def as_seconds(year, month, day, hour, minute, second):
    """Seconds since the base date, for comparing two readings.

    Not a real calendar: the run spans seconds, so days and months only have to
    be *equal* to the base, which is checked separately.
    """
    return ((day * 24 + hour) * 60 + minute) * 60 + second


def main():
    image = kernel_image()
    with tempfile.TemporaryDirectory() as workdir:
        monitor = os.path.join(workdir, "monitor")
        serial = os.path.join(workdir, "serial.txt")
        qemu = subprocess.Popen(
            [
                "qemu-system-x86_64",
                "-kernel", image,
                "-display", "none",
                # The whole point: a known answer, not the host's clock.
                "-rtc", f"base={BASE}",
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
            # Several readings across several seconds. One reading proves the
            # registers were decoded; a sequence proves the clock *runs*, and
            # gives the update window a chance to be straddled.
            for _ in range(4):
                send_line(connection, "clock")
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
        readings = re.findall(
            r"clock (\d{4})-(\d{2})-(\d{2}) (\d{2}):(\d{2}):(\d{2})", transcript
        )
        if len(readings) < 4:
            unsettled = transcript.count("clock: unsettled")
            failures.append(
                f"expected 4 readings, got {len(readings)}"
                + (f" ({unsettled} unsettled)" if unsettled else "")
            )

        stamps = []
        for year, month, day, hour, minute, second in readings:
            year, month, day = int(year), int(month), int(day)
            hour, minute, second = int(hour), int(minute), int(second)
            # The date is the base's, exactly. A missing BCD conversion turns
            # November into month 17, which is not a date at all.
            if (year, month) != (BASE_YEAR, BASE_MONTH):
                failures.append(f"read {year}-{month:02d}, not {BASE_YEAR}-{BASE_MONTH:02d}")
            if day not in (BASE_DAY, BASE_DAY + 1):
                failures.append(f"read day {day}, not {BASE_DAY}")
            if not (0 <= hour < 24 and 0 <= minute < 60 and 0 <= second < 60):
                failures.append(f"read {hour}:{minute}:{second}, which is not a time")
            stamps.append(as_seconds(year, month, day, hour, minute, second))

        if stamps:
            start = as_seconds(BASE_YEAR, BASE_MONTH, BASE_DAY, BASE_HOUR, BASE_MINUTE, BASE_SECOND)
            drift = stamps[0] - start
            if not 0 <= drift <= MAX_DRIFT_SECONDS:
                failures.append(
                    f"the first reading is {drift}s from the base time, which is not the "
                    f"clock QEMU was told to keep"
                )
            # And it *runs*: never backwards, and it moved at all across four
            # readings taken seconds apart. A driver that read a constant would
            # pass every other check here.
            for earlier, later in zip(stamps, stamps[1:]):
                if later < earlier:
                    failures.append(f"the clock went backwards: {earlier} then {later}")
            if len(stamps) > 1 and stamps[-1] == stamps[0]:
                failures.append("the clock did not advance across four readings seconds apart")

        if "exception #" in transcript:
            fault = re.search(r"!! exception .*", transcript)
            failures.append(f"the machine faulted: {fault.group(0)!r}")

        if failures:
            print("\n".join(failures))
            print("--- transcript ---")
            print(transcript)
            return 1
        print(f"OK: read {BASE} out of the CMOS registers, decoded from BCD, and watched it run")
        return 0


if __name__ == "__main__":
    sys.exit(main())
