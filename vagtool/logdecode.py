"""Offline reassembly of a sniffed TP2.0 session.

Point this at a candump log of someone else's VCDS (or OBDeleven) session and it
prints the KWP2000 messages, flagging the ones that write something. That is how
the Coding-II / 21295 sequence this tool cannot guess gets recovered: capture the
session, find the flagged message, replay it with `vagtool raw`.

Accepts both candump formats:
    (1612345678.123456) can0 740#1000022101
    can0  740   [5]  10 00 02 21 01
"""

import dataclasses
import re

from .kwp2000 import SERVICE_NAMES, changes_ecu
from .tp20 import BROADCAST_ADDR

LOG_RE = re.compile(r"^\((?P<ts>[\d.]+)\)\s+(?P<iface>\S+)\s+(?P<id>[0-9A-Fa-f]+)#(?P<data>[0-9A-Fa-f ]*)")
PLAIN_RE = re.compile(r"^\s*(?P<iface>\S+)\s+(?P<id>[0-9A-Fa-f]+)\s+\[(?P<dlc>\d+)\]\s+(?P<data>[0-9A-Fa-f ]*)")


@dataclasses.dataclass
class Message:
    ts: float
    direction: str          # "tester" or "ecu"
    payload: bytes

    @property
    def service(self):
        return self.payload[0] if self.payload else None

    @property
    def is_write(self):
        sid = self.service
        if sid is None:
            return False
        if sid == 0x7F:
            return False
        if self.direction == "ecu":
            return changes_ecu(sid - 0x40)
        return changes_ecu(sid)

    def describe(self):
        if not self.payload:
            return "empty"
        sid = self.payload[0]
        if sid == 0x7F:
            return (f"negative response to "
                    f"{SERVICE_NAMES.get(self.payload[1], hex(self.payload[1]))}")
        if self.direction == "ecu" and (sid - 0x40) in SERVICE_NAMES:
            return f"{SERVICE_NAMES[sid - 0x40]} response"
        return SERVICE_NAMES.get(sid, f"service {sid:#04x}")


def parse_frames(lines):
    for line in lines:
        m = LOG_RE.match(line) or PLAIN_RE.match(line)
        if not m:
            continue
        data = bytes.fromhex(m.group("data").replace(" ", ""))
        yield float(m.groupdict().get("ts") or 0.0), int(m.group("id"), 16), data


class SessionDecoder:
    """Reassembles TP2.0 frames into KWP2000 messages."""

    def __init__(self, module=0x01):
        self.module = module
        self.tester_tx = None
        self.ecu_tx = None
        self.buffers = {"tester": b"", "ecu": b""}
        self.messages = []

    def feed(self, ts, can_id, data):
        if not data:
            return
        if can_id == BROADCAST_ADDR or can_id == BROADCAST_ADDR + self.module:
            if len(data) >= 6 and data[1] == 0xD0:
                self.ecu_tx = data[2] | (data[3] << 8)
                self.tester_tx = data[4] | (data[5] << 8)
            return

        if can_id == self.tester_tx:
            direction = "tester"
        elif can_id == self.ecu_tx:
            direction = "ecu"
        else:
            return

        op = data[0]
        if op & 0xF0 in (0xA0, 0xB0, 0xC0, 0xD0):
            return                                  # control frame or ack

        kind = op >> 4
        if kind > 0x3:
            return
        self.buffers[direction] += data[1:]
        if kind in (0x1, 0x3):
            buf = self.buffers[direction]
            self.buffers[direction] = b""
            if len(buf) < 2:
                return
            length = int.from_bytes(buf[:2], "big")
            payload = buf[2:2 + length]
            if payload:
                self.messages.append(Message(ts, direction, payload))

    def feed_log(self, lines):
        for ts, can_id, data in parse_frames(lines):
            self.feed(ts, can_id, data)
        return self.messages


def decode_file(path, only_writes=False):
    decoder = SessionDecoder()
    with open(path) as fh:
        messages = decoder.feed_log(fh)

    if decoder.tester_tx is None:
        yield ("note", "no TP2.0 channel setup seen; capture must include the "
                       "0x200 handshake for directions to be known")
    for msg in messages:
        if only_writes and not msg.is_write:
            continue
        arrow = "->" if msg.direction == "tester" else "<-"
        mark = "  *** not a known read-only service" if msg.is_write else ""
        yield ("msg", f"{msg.ts:.3f} {arrow} {msg.payload.hex(' ')}"
                      f"    {msg.describe()}{mark}")
