#!/usr/bin/env python3
"""A task's whole life: it waits without spending anything, it ends, and
everything it held comes back.

`check_spawn.py` next door proves tasks *run*: a spinner and a clock, both
`while (true)`, both preempted by the timer. That is half a scheduler. Every task
in this kernel was a loop that was started, and a table of them was a watermark —
a slot could be born and never die, so nothing showed whether a slot or a stack
could come back.

The awkward part is not the marking, it is the timing: **a task cannot release
the stack it is standing on**. The release would hand back the pages holding the
frame that is about to return, and the next interrupt would land on memory the
allocator had given away. So an exiting task marks itself dead and stops, and the
scheduler gives the pages back a tick later — when that task is neither the one
running nor the one about to run.

Four numbers, and each one fails differently:

* **20 of 20 started.** The table holds sixteen slots. Twenty tasks fit only if
  slots come back; a kernel that leaked them stops at whatever was spare.
* **The watermark barely moves.** It is one past the highest slot ever taken, so
  running twenty tasks one at a time should raise it by one and leave it there —
  the same slot, reused. A watermark that climbed with each task would mean each
  one took a fresh slot and nothing was reclaimed.
* **The free-page count is the number it started at.** Not close to it. Each task
  takes a stack; a reclaim that marked the slot free without releasing the pages
  looks exactly like success from every other angle.
* **A sleep takes as long as it says.** Ten sleeps of fifty ticks have to take
  about five hundred ticks — not none, which is what a `task_sleep` that
  returned immediately would take, and which is exactly what the first version
  did: the scheduler's slice shortcut handed back the current task on seven
  ticks out of eight without asking whether it was still runnable, so a task
  that had just marked itself blocked was resumed anyway. Every wake was
  counted, every number looked right, and only the elapsed time gave it away.
* **The work actually happened.** Each task bumps a counter 200,000 times, so
  `ran` has to be twenty times that. Without it, "every task ended" is also what
  a kernel that never scheduled them would report — which is what the first
  version of this did, because the shell runs with interrupts masked and nothing
  schedules without the timer.
"""

import os
import re
import socket
import subprocess
import sys
import tempfile
import time

# Must match `TASK_BRIEF_ROUNDS` and the loop inside `lk_task_brief`.
ROUNDS = 20
STEPS_PER_TASK = 200000
# `TASK_CAPACITY` in the program. Fewer than the rounds, on purpose.
CAPACITY = 16
# The scheduler's slice, in ticks: a woken task runs at the next decision, so
# each wake may be up to this late.
SLICE_TICKS = 8
SLEEP_SLACK = 60


def send_line(connection, text):
    """Types `text` and Enter through the monitor, a key at a time."""
    names = {" ": "spc", "-": "minus", ".": "dot", "/": "slash"}
    for character in text:
        connection.sendall(f"sendkey {names.get(character, character)}\n".encode())
        time.sleep(0.25)
    connection.sendall(b"sendkey ret\n")
    time.sleep(6)


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
            send_line(connection, "task")
            # Twice: the second cycle runs on slots and pages the first one gave
            # back, so anything the reclaim got subtly wrong shows here rather
            # than in a first run that had untouched memory to draw on.
            send_line(connection, "task")
            send_line(connection, "sleep")
            connection.sendall(b"quit\n")
            connection.close()
        finally:
            qemu.terminate()
            qemu.wait(timeout=10)

        with open(serial, errors="replace") as handle:
            transcript = handle.read()

        failures = []
        # Unanchored: the spinner writes to the same serial line from a timer
        # interrupt, so a shell line can arrive with another task's byte stuck to
        # either end of it.
        cycles = re.findall(
            r"task (\d+)/(\d+) slots (\d+)->(\d+) pages (\d+)->(\d+) ran (\d+)", transcript
        )
        if len(cycles) < 2:
            failures.append(f"expected two cycles, got {len(cycles)}: {cycles}")

        for index, (started, wanted, slots_before, slots_after, pages_before, pages_after, ran) in enumerate(cycles):
            if int(wanted) != ROUNDS:
                failures.append(f"cycle {index}: the program ran {wanted} rounds, not {ROUNDS}")
            if int(wanted) <= CAPACITY:
                failures.append(
                    f"cycle {index}: {wanted} rounds fits in {CAPACITY} slots, so it would pass "
                    f"without any slot coming back"
                )
            if started != wanted:
                failures.append(f"cycle {index}: only {started} of {wanted} tasks started")
            if int(slots_after) - int(slots_before) > 1:
                failures.append(
                    f"cycle {index}: the watermark went {slots_before}->{slots_after}; "
                    f"slots are not being reused"
                )
            if pages_before != pages_after:
                failures.append(
                    f"cycle {index}: the pages did not come back: {pages_before}->{pages_after}"
                )
            if int(ran) != ROUNDS * STEPS_PER_TASK:
                failures.append(
                    f"cycle {index}: the tasks did {ran} steps, not {ROUNDS * STEPS_PER_TASK} — "
                    f"they were not all scheduled"
                )

        # The waiting half. The lower bound is what separates sleeping from
        # returning immediately; the upper bound is what catches a wake that was
        # missed and had to wait out another whole period. The slack is the
        # scheduling granularity — a woken task runs at the next decision, which
        # is up to one slice away, once per wake.
        slept = re.search(r"sleep wakes (\d+)/(\d+) ticks (\d+) want (\d+)", transcript)
        if not slept:
            step = re.search(r"sleep: .*", transcript)
            failures.append(f"`sleep` did not report: {step.group(0) if step else 'nothing'}")
        else:
            woke, rounds, ticks, want = (int(g) for g in slept.groups())
            if woke != rounds:
                failures.append(f"the sleeper woke {woke} times, not {rounds}")
            if ticks < want:
                failures.append(f"{rounds} sleeps took {ticks} ticks, less than the {want} asked for")
            if ticks > want + rounds * SLICE_TICKS + SLEEP_SLACK:
                failures.append(f"{rounds} sleeps took {ticks} ticks, far more than the {want} asked for")

        if "exception #" in transcript:
            fault = re.search(r"!! exception .*", transcript)
            failures.append(f"the machine faulted: {fault.group(0)!r}")

        if failures:
            print("\n".join(failures))
            print("--- transcript ---")
            print(transcript)
            return 1
        print(f"OK: a task slept for the time it asked for, and {ROUNDS} tasks ran to "
              f"completion and returned through {CAPACITY} slots, giving back every slot "
              f"and every stack page")
        return 0


if __name__ == "__main__":
    sys.exit(main())
