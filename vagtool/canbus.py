"""CAN layer.

Two implementations with the same tiny interface (send/recv/close):

* SocketCanBus - stdlib AF_CAN raw socket, no third-party dependency. Works
  with any SocketCAN interface, including the 8devices Korlan (usb_8dev).
* PairedBus - in-memory pair used by the simulated ECU and the tests, so the
  whole TP2.0/KWP2000 stack can run without hardware.
"""

import collections
import socket
import struct
import threading
import time

# struct for a classic CAN frame as the kernel exposes it on AF_CAN/CAN_RAW
_FRAME_FMT = "=IB3x8s"
_FRAME_SIZE = struct.calcsize(_FRAME_FMT)

CAN_EFF_FLAG = 0x80000000
CAN_RTR_FLAG = 0x40000000
CAN_ERR_FLAG = 0x20000000
CAN_SFF_MASK = 0x000007FF


class CanError(Exception):
    pass


class SocketCanBus:
    def __init__(self, channel="can0"):
        self.channel = channel
        try:
            self.sock = socket.socket(socket.AF_CAN, socket.SOCK_RAW, socket.CAN_RAW)
            self.sock.bind((channel,))
        except OSError as e:
            raise CanError(f"cannot open SocketCAN interface {channel!r}: {e}") from e
        self.sock.settimeout(0.0)

    def send(self, can_id, data):
        if len(data) > 8:
            raise ValueError("classic CAN frame limited to 8 bytes")
        frame = struct.pack(_FRAME_FMT, can_id & CAN_SFF_MASK, len(data), bytes(data))
        try:
            self.sock.send(frame)
        except OSError as e:
            raise CanError(f"send failed on {self.channel}: {e}") from e

    def recv(self, timeout=0.1):
        """Return (can_id, data) or None on timeout."""
        deadline = time.monotonic() + timeout
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return None
            self.sock.settimeout(remaining)
            try:
                raw = self.sock.recv(_FRAME_SIZE)
            except (socket.timeout, TimeoutError):
                return None
            except OSError as e:
                raise CanError(f"recv failed on {self.channel}: {e}") from e
            can_id, dlc, payload = struct.unpack(_FRAME_FMT, raw)
            if can_id & (CAN_ERR_FLAG | CAN_RTR_FLAG):
                continue
            return can_id & CAN_SFF_MASK, payload[:dlc]

    def close(self):
        self.sock.close()


class PairedBus:
    """One end of an in-memory two-node bus. Frames sent here arrive there."""

    def __init__(self, name="a"):
        self.name = name
        self._inbox = collections.deque()
        self._cv = threading.Condition()
        self._peer = None
        self.closed = False

    @classmethod
    def pair(cls):
        a, b = cls("a"), cls("b")
        a._peer, b._peer = b, a
        return a, b

    def _deliver(self, can_id, data):
        with self._cv:
            self._inbox.append((can_id, bytes(data)))
            self._cv.notify_all()

    def send(self, can_id, data):
        if len(data) > 8:
            raise ValueError("classic CAN frame limited to 8 bytes")
        if self._peer is None:
            raise CanError("bus has no peer")
        self._peer._deliver(can_id, data)

    def recv(self, timeout=0.1):
        deadline = time.monotonic() + timeout
        with self._cv:
            while not self._inbox:
                remaining = deadline - time.monotonic()
                if remaining <= 0 or self.closed:
                    return None
                self._cv.wait(remaining)
            return self._inbox.popleft()

    def close(self):
        with self._cv:
            self.closed = True
            self._cv.notify_all()


def open_bus(channel):
    """`channel` is a SocketCAN interface name."""
    return SocketCanBus(channel)
