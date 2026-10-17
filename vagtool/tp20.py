"""VW Transport Protocol 2.0 (TP 2.0) over CAN.

Reference: https://jazdw.net/tp20 and https://icanhack.nl/knowledge-base/diagnostics/vw-tp20/
Frame layout derived from the implementations in RESEARCH/pq-flasher (Panda-based)
and RESEARCH/teensycanbusdisplay, ported here to plain SocketCAN.

Channel setup:
    tester -> 0x200: <module> C0 00 10 00 03 01      ask ECU to talk on 0x300
    ECU  -> 0x200+m: 00 D0 <rx lo hi> <tx lo hi> 01   ECU tells us its RX id

Data frames: byte0 = <type nibble><seq nibble>, then up to 7 payload bytes.
    0x0/0x2 more frames follow   0x1/0x3 last frame   0x1/0x0 expect an ACK
    0xB0|seq ACK   0xA0 param request   0xA1 param response
    0xA3 channel test (keepalive)   0xA8 disconnect
The first frame of a message starts with a big-endian 16-bit payload length.
"""

import time

BROADCAST_ADDR = 0x200

# TP2.0 opcodes
OP_SETUP_REQ = 0xC0
OP_SETUP_OK = 0xD0
OP_PARAM_REQ = 0xA0
OP_PARAM_RESP = 0xA1
OP_CHANNEL_TEST = 0xA3
OP_BREAK = 0xA4
OP_DISCONNECT = 0xA8

# Logical module addresses (same numbering VCDS shows)
MODULE_ENGINE = 0x01
MODULE_GATEWAY = 0x1F


class Tp20Error(Exception):
    pass


class Tp20Timeout(Tp20Error, TimeoutError):
    pass


def decode_timing(byte):
    """TP2.0 timing parameter: top 2 bits pick the base, low 6 bits the count."""
    base = {0: 0.1e-3, 1: 1e-3, 2: 10e-3, 3: 100e-3}[byte >> 6]
    return (byte & 0x3F) * base


class Tp20Channel:
    def __init__(self, bus, module=MODULE_ENGINE, timeout=0.5, debug=False):
        self.bus = bus
        self.module = module
        self.timeout = timeout
        self.debug = debug

        self.rx_addr = None
        self.tx_addr = None
        self.tx_seq = 0
        self.rx_seq = 0
        self.gap = 0.001           # minimum time between our frames (T3)
        self.ack_timeout = 0.1     # T1
        self.last_tx = 0.0
        self.closed = True

    # -- plumbing ---------------------------------------------------------
    def _send(self, data, addr=None):
        addr = self.tx_addr if addr is None else addr
        if self.debug:
            print(f"TX {addr:03x}  {data.hex(' ')}")
        self.bus.send(addr, data)
        self.last_tx = time.monotonic()
        if self.gap:
            time.sleep(self.gap)

    def _recv(self, addr=None, timeout=None):
        """Next frame from `addr`, ignoring traffic from every other id."""
        addr = self.rx_addr if addr is None else addr
        deadline = time.monotonic() + (self.timeout if timeout is None else timeout)
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise Tp20Timeout(f"no TP2.0 frame from {addr:#05x} within timeout")
            frame = self.bus.recv(timeout=remaining)
            if frame is None:
                continue
            can_id, data = frame
            if can_id != addr or not data:
                continue
            if self.debug:
                print(f"RX {can_id:03x}  {data.hex(' ')}")
            return data

    # -- channel ----------------------------------------------------------
    def open(self):
        self._send(bytes([self.module, OP_SETUP_REQ, 0x00, 0x10, 0x00, 0x03, 0x01]),
                   addr=BROADCAST_ADDR)
        resp = self._recv(addr=BROADCAST_ADDR + self.module, timeout=max(self.timeout, 1.0))
        if len(resp) < 6:
            raise Tp20Error(f"short channel setup response: {resp.hex()}")
        if resp[1] != OP_SETUP_OK:
            raise Tp20Error(f"ECU refused channel setup (opcode {resp[1]:#04x}): {resp.hex()}")

        rx = resp[2] | (resp[3] << 8)
        tx = resp[4] | (resp[5] << 8)
        if rx & 0xF000 or tx & 0xF000:
            raise Tp20Error(f"ECU marked a CAN id invalid: rx={rx:#06x} tx={tx:#06x}")
        self.rx_addr, self.tx_addr = rx, tx
        self.tx_seq = self.rx_seq = 0
        self.closed = False
        if self.debug:
            print(f"channel open: we send on {self.tx_addr:#05x}, listen on {self.rx_addr:#05x}")

        # Timing parameters: block size 15, T1 100ms, T3 5ms.
        self._send(bytes([OP_PARAM_REQ, 0x0F, 0x8A, 0xFF, 0x32, 0xFF]))
        params = self._recv()
        if params[0] != OP_PARAM_RESP:
            raise Tp20Error(f"unexpected answer to parameter request: {params.hex()}")
        self.ack_timeout = max(decode_timing(params[2]), 0.05)
        self.gap = max(decode_timing(params[4]), 0.001)
        if self.debug:
            print(f"timing: T1={self.ack_timeout * 1000:.0f}ms T3={self.gap * 1000:.0f}ms")
        return self

    def close(self):
        if self.closed:
            return
        try:
            self._send(bytes([OP_DISCONNECT]))
        except Exception:
            pass
        self.closed = True

    def __enter__(self):
        if self.closed:
            self.open()
        return self

    def __exit__(self, *exc):
        self.close()

    # -- acks -------------------------------------------------------------
    def _wait_for_ack(self):
        deadline = time.monotonic() + max(self.ack_timeout * 3, 0.2)
        want = 0xB0 | ((self.tx_seq + 1) & 0x0F)
        while time.monotonic() < deadline:
            data = self._recv(timeout=deadline - time.monotonic())
            if data[0] == want:
                return
            if data[0] & 0xF0 == 0xB0:
                raise Tp20Error(f"out of sequence ACK: got {data[0]:#04x}, wanted {want:#04x}")
            # control frames may overtake the ack; keep looking
        raise Tp20Timeout("ECU did not acknowledge our message")

    def _send_ack(self):
        self._send(bytes([0xB0 | ((self.rx_seq + 1) & 0x0F)]))

    # -- messages ---------------------------------------------------------
    def send(self, payload):
        if self.closed:
            raise Tp20Error("channel is not open")
        if len(payload) > 0xFFFF:
            raise ValueError("payload too long for TP2.0")

        body = len(payload).to_bytes(2, "big") + bytes(payload)
        while body:
            last = len(body) <= 7
            header = (0x10 if last else 0x20) | self.tx_seq
            self._send(bytes([header]) + body[:7])
            if last:
                self._wait_for_ack()
            self.tx_seq = (self.tx_seq + 1) & 0x0F
            body = body[7:]

    def recv(self, timeout=None):
        if self.closed:
            raise Tp20Error("channel is not open")
        deadline = time.monotonic() + (self.timeout if timeout is None else timeout)
        buf = b""
        while True:
            data = self._recv(timeout=max(deadline - time.monotonic(), 0.0))
            op = data[0]

            if op == OP_CHANNEL_TEST:                 # ECU probing us
                self._send(bytes([OP_PARAM_RESP, 0x0F, 0x8A, 0xFF, 0x32, 0xFF]))
                continue
            if op in (OP_PARAM_RESP, OP_BREAK) or op & 0xF0 == 0xB0:
                continue                              # stale keepalive / stray ack

            kind, seq = op >> 4, op & 0x0F
            if kind > 0x3:
                raise Tp20Error(f"unexpected TP2.0 frame: {data.hex()}")

            buf += data[1:]
            self.rx_seq = seq
            if kind in (0x0, 0x1):                    # ECU wants an ack
                self._send_ack()
            if kind in (0x1, 0x3):                    # last frame of the message
                break

        if len(buf) < 2:
            raise Tp20Error("truncated TP2.0 message")
        length = int.from_bytes(buf[:2], "big")
        payload = buf[2:2 + length]
        if len(payload) != length:
            raise Tp20Error(f"message says {length} bytes, got {len(payload)}")
        return payload

    def keepalive(self):
        """Channel test. The channel dies if left idle for about a second."""
        self._send(bytes([OP_CHANNEL_TEST]))
        deadline = time.monotonic() + max(self.timeout, 0.3)
        while time.monotonic() < deadline:
            data = self._recv(timeout=deadline - time.monotonic())
            if data[0] == OP_PARAM_RESP:
                return True
        raise Tp20Timeout("no answer to channel test")

    def keepalive_if_idle(self, interval=0.5):
        if time.monotonic() - self.last_tx >= interval:
            self.keepalive()
