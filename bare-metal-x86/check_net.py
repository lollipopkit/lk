#!/usr/bin/env python3
"""Real hardware: an Intel gigabit NIC, driven by LK, asking a question on the
wire and hearing the answer.

The `edu` device in `check_pci.py` proves a driver can find a device, reach its
registers, make it compute, take its interrupt, and make it write RAM. It proves
it against a device invented to make that easy: a handful of registers and a DMA
engine that does one transfer when told to.

A NIC is not commanded, it is *fed*. The driver and the card share two rings of
descriptors in memory, each side owning a moving index, and the whole protocol is
which one may advance and when — nothing is ever started by writing a start bit.
That is the shape of every driver for hardware built in the last thirty years,
and it is the thing the educational device cannot show.

Two independent witnesses, which is the point of this script:

* **The guest says so.** `net` prints the card's own MAC — read out of the
  card's serial EEPROM, not a constant — and the hardware address that answered
  for 10.0.2.2. Every field of the reply is checked inside the kernel: the
  ethertype rules out other traffic, the operation code rules out the card
  hearing its own request, the sender address rules out a reply about another
  host, and the destination rules out a broadcast meant for someone else.

* **The wire says so.** QEMU dumps the segment to a pcap, and this script parses
  it. A driver that convinced itself would still have to put a well-formed frame
  on the wire for the host's stack to answer it, and the capture is where that is
  visible independently of anything the guest believes.

The whole driver is `drivers/e1000.lk` and `drivers/arp.lk`. The board
contributes nothing to this path.
"""

import os
import re
import socket
import struct
import subprocess
import sys
import tempfile
import time

# QEMU's user-mode network: the guest is .15 and the gateway is .2. The
# gateway's hardware address is derived from its IP by slirp, which is why it can
# be written down here.
GATEWAY_IP = bytes([10, 0, 2, 2])
GUEST_IP = bytes([10, 0, 2, 15])
GATEWAY_MAC = "52550a000202"

ETH_TYPE_ARP = 0x0806
ARP_REQUEST = 1
ARP_REPLY = 2


def send_line(connection, text):
    """Types `text` and Enter through the monitor, a key at a time."""
    names = {" ": "spc", "-": "minus", ".": "dot", "/": "slash"}
    for character in text:
        connection.sendall(f"sendkey {names.get(character, character)}\n".encode())
        time.sleep(0.25)
    connection.sendall(b"sendkey ret\n")
    time.sleep(4)


def frames(path):
    """Every frame in a pcap, as bytes."""
    with open(path, "rb") as handle:
        data = handle.read()
    if len(data) < 24:
        return []
    out = []
    offset = 24
    while offset + 16 <= len(data):
        _, _, captured, _ = struct.unpack("<IIII", data[offset:offset + 16])
        out.append(data[offset + 16:offset + 16 + captured])
        offset += 16 + captured
    return out


def arp_of(frame):
    """(operation, sender_mac, sender_ip, target_ip) if this is ARP, else None."""
    if len(frame) < 42 or int.from_bytes(frame[12:14], "big") != ETH_TYPE_ARP:
        return None
    body = frame[14:]
    return (
        int.from_bytes(body[6:8], "big"),
        body[8:14],
        body[14:18],
        body[24:28],
    )


def main():
    image = sys.argv[1] if len(sys.argv) > 1 else (
        "target/x86_64-unknown-none/release/lk-bare-metal-x86.multiboot"
    )
    with tempfile.TemporaryDirectory() as workdir:
        monitor = os.path.join(workdir, "monitor")
        serial = os.path.join(workdir, "serial.txt")
        capture = os.path.join(workdir, "wire.pcap")
        qemu = subprocess.Popen(
            [
                "qemu-system-x86_64",
                "-kernel", image,
                "-display", "none",
                "-netdev", "user,id=n0",
                "-device", "e1000,netdev=n0",
                # The independent witness.
                "-object", f"filter-dump,id=wire,netdev=n0,file={capture}",
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
            send_line(connection, "net")
            # Twice: the second exchange goes round rings the first left
            # part-way through, so a driver that advanced an index wrongly
            # passes once and then never receives again.
            send_line(connection, "net")
            connection.sendall(b"quit\n")
            connection.close()
        finally:
            qemu.terminate()
            qemu.wait(timeout=10)

        with open(serial, errors="replace") as handle:
            transcript = handle.read()

        failures = []

        # (1) What the guest says.
        # Unanchored, deliberately. The spinner task writes to the same serial
        # line from a timer interrupt, so a shell line can arrive with another
        # task's byte stuck to either end of it — which is a property of the
        # machine being preemptive, not a fault to detect. Anchoring to the
        # start of a line made this pass or fail on where a `.` happened to
        # land.
        answers = re.findall(r"net: ok ([0-9a-f]{12}) -> ([0-9a-f]{12})", transcript)
        if len(answers) < 2:
            step = re.search(r"net: (?!ok)\S+", transcript)
            failures.append(
                f"`net` did not complete: {step.group(0)!r}" if step
                else f"`net` completed {len(answers)} of 2 exchanges"
            )
        else:
            for card_mac, gateway_mac in answers:
                if gateway_mac != GATEWAY_MAC:
                    failures.append(
                        f"the reply came from {gateway_mac}, not the gateway's {GATEWAY_MAC}"
                    )
            if len(set(answers)) != 1:
                failures.append(f"the two exchanges disagreed: {answers}")

        if "exception #" in transcript:
            fault = re.search(r"!! exception .*", transcript)
            failures.append(f"the machine faulted: {fault.group(0)!r}")

        # (2) What the wire says. The guest's own MAC comes from the transcript,
        # so this checks the *card* put the address it reported into the frame —
        # a driver that read the EEPROM wrong and framed consistently would pass
        # the guest-side check on its own.
        captured = frames(capture)
        card_mac = bytes.fromhex(answers[0][0]) if answers else None
        requests = [
            frame for frame in captured
            if (parsed := arp_of(frame))
            and parsed[0] == ARP_REQUEST
            and parsed[3] == GATEWAY_IP
            and parsed[2] == GUEST_IP
            and (card_mac is None or parsed[1] == card_mac)
        ]
        replies = [
            frame for frame in captured
            if (parsed := arp_of(frame))
            and parsed[0] == ARP_REPLY
            and parsed[2] == GATEWAY_IP
        ]
        if len(requests) < 2:
            failures.append(
                f"the wire carried {len(requests)} well-formed ARP requests from the card, not 2 "
                f"(of {len(captured)} frames)"
            )
        if len(replies) < 2:
            failures.append(f"the wire carried {len(replies)} ARP replies for the gateway, not 2")
        # Ethernet's minimum is 60 bytes and an ARP request is 42. The card pads,
        # but only because `TCTL_PSP` is set — without it the frame that leaves
        # is a runt, which some paths carry and some discard.
        for frame in requests:
            if len(frame) < 60:
                failures.append(f"the card transmitted a {len(frame)}-byte runt")
                break

        if failures:
            print("\n".join(failures))
            print("--- transcript ---")
            print(transcript)
            return 1
        print("OK: read the card's MAC out of its EEPROM, fed two descriptor rings, "
              "put a well-formed ARP request on the wire, and received the gateway's reply")
        return 0


if __name__ == "__main__":
    sys.exit(main())
