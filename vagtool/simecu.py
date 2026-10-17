"""A small fake PPD1-style engine ECU speaking TP2.0 + KWP2000.

It exists so the rest of this package can be developed and tested without the
car: run it on a vcan interface, or hand it one end of a PairedBus in a test.

It is a stand-in, not a model of the real ECU. In particular, because the real
Coding-II byte sequence is not known (see README), the simulator starts a forced
regeneration when it sees the value 21295 (0x532F) inside any writing service.
"""

import threading
import time

from . import kwp2000
from .tp20 import (BROADCAST_ADDR, OP_CHANNEL_TEST, OP_DISCONNECT, OP_PARAM_REQ,
                   OP_PARAM_RESP, OP_SETUP_OK, OP_SETUP_REQ)

REGEN_CODE = 21295
PART_NUMBER = b"03G906016AB"
SW_VERSION = b"0003"
SYSTEM_DESC = b"R4 2.0L EDC  G000SG"
SEED = 0x1A2B3C4D


def _factor(raw, bmax=255):
    """Split a value into A*B with both bytes in range, as close as possible."""
    if raw <= 0:
        return 0, 1
    best = None
    for a in range(1, 256):
        b = min(max(round(raw / a), 0), bmax)
        error = abs(a * b - raw)
        if best is None or error < best[0]:
            best = (error, a, b)
            if error == 0:
                break
    return best[1], best[2]


def _cell(formula, value):
    """Encode a physical value back into (formula, A, B) the way the ECU would.
    Inverse of vagtool.blocks.FORMULAS for the formulas the simulator uses."""
    if formula == 0x01:                      # rpm = A*B/5
        a, b = _factor(round(value * 5))
    elif formula == 0x05:                    # C = A*(B-100)*0.1
        a, x = _factor(round(value * 10), bmax=155)
        a, b = a, x + 100
    elif formula == 0x7E:                     # g = A*B*0.1
        a, b = _factor(round(value * 10))
    elif formula == 0x36:                     # count = A*256+B
        raw = int(value)
        a, b = raw >> 8 & 0xFF, raw & 0xFF
    elif formula == 0x08:                     # bit field
        raw = int(value)
        a, b = raw >> 8 & 0xFF, raw & 0xFF
    else:
        raise ValueError(f"simulator cannot encode formula {formula:#04x}")
    return bytes([formula, a, b])


class SimEcu:
    def __init__(self, bus, module=0x01, debug=False):
        self.bus = bus
        self.module = module
        self.debug = debug

        self.tx_addr = 0x300        # ECU -> tester
        self.rx_addr = 0x740        # tester -> ECU
        self.open = False
        self.tx_seq = 0

        # simulated vehicle state
        self.coolant = 85.0
        self.rpm = 2100.0
        self.load_g = 28.0
        self.egt_pre_turbo = 320.0
        self.egt_pre_dpf = 290.0
        self.egt_post_dpf = 270.0
        self.forced_regen = False
        self.regen_counter = 0
        # 0x41D7 = VAG 16855 = P0471 exhaust pressure sensor range/performance
        # (intermittent), 0x4802 = VAG 18434 = P2002 particulate trap
        # efficiency below threshold (static).
        self.dtcs = [(0x41D7, 0x88), (0x4802, 0x02)]
        self.session = None
        self.logged_in = False
        self.basic_setting_group = None
        self.coding = 11
        self.fault_mileage = 184233

    # -- physics-ish ------------------------------------------------------
    def tick(self):
        if self.forced_regen:
            self.regen_counter += 1
            self.egt_pre_turbo = min(self.egt_pre_turbo + 25, 740.0)
            self.egt_pre_dpf = min(self.egt_pre_dpf + 22, 660.0)
            self.egt_post_dpf = min(self.egt_post_dpf + 18, 600.0)
            self.load_g = max(self.load_g - 0.3, 0.0)
            if self.load_g <= 0.5:
                self.forced_regen = False
        else:
            self.egt_pre_turbo = max(self.egt_pre_turbo - 5, 300.0)
            self.egt_pre_dpf = max(self.egt_pre_dpf - 5, 280.0)
            self.egt_post_dpf = max(self.egt_post_dpf - 5, 260.0)

    def regen_status_bits(self):
        return 0x02 if self.forced_regen else 0x00

    # -- KWP2000 ----------------------------------------------------------
    def handle_kwp(self, req):
        sid = req[0]

        if sid == kwp2000.SID_START_DIAGNOSTIC_SESSION:
            self.session = req[1]
            return bytes([0x50, req[1]])

        if sid == kwp2000.SID_READ_ECU_IDENTIFICATION:
            if req[1] == kwp2000.IDENT_PART_NUMBER:
                return bytes([0x5A, req[1]]) + self.identification()
            return bytes([0x5A, req[1]]) + PART_NUMBER

        if sid == kwp2000.SID_READ_DATA_BY_LOCAL_ID and req[1] == kwp2000.LID_FREEZE_FRAME:
            return bytes([0x61, kwp2000.LID_FREEZE_FRAME]) + self.freeze_frame()

        if sid == kwp2000.SID_READ_DATA_BY_LOCAL_ID:
            group = req[1]
            body = self.group_data(group)
            if body is None:
                return bytes([0x7F, sid, 0x31])
            return bytes([0x61, group]) + body

        if sid == kwp2000.SID_READ_DTC_BY_STATUS:
            out = bytes([0x58, len(self.dtcs)])
            for code, status in self.dtcs:
                out += code.to_bytes(2, "big") + bytes([status])
            return out

        if sid == kwp2000.SID_CLEAR_DIAGNOSTIC_INFORMATION:
            self.dtcs = []
            return bytes([0x54])

        if sid == kwp2000.SID_TESTER_PRESENT:
            return bytes([0x7E, 0x01])

        if sid == kwp2000.SID_SECURITY_ACCESS:
            return self.handle_security_access(req)

        if sid in (kwp2000.SID_START_ROUTINE_BY_LOCAL_ID,
                   kwp2000.SID_STOP_ROUTINE_BY_LOCAL_ID):
            return self.handle_routine(req)

        if (sid == kwp2000.SID_WRITE_DATA_BY_LOCAL_ID and len(req) > 2
                and req[1] == kwp2000.LID_CODING and req[-4] == 0x03):
            self.coding = int.from_bytes(req[-3:], "big")
            if self.coding == REGEN_CODE:
                self.forced_regen = True
                self.regen_counter = 0
            if self.debug:
                print(f"sim: coding written: {self.coding}")
            return bytes([0x7B, req[1]])

        # Stand-in for Coding-II: any service carrying the code starts a regen.
        if REGEN_CODE.to_bytes(2, "big") in req[1:]:
            self.forced_regen = True
            self.regen_counter = 0
            if self.debug:
                print("sim: forced regeneration enabled")
            return bytes([sid + 0x40]) + req[1:2]

        return bytes([0x7F, sid, 0x11])

    def identification(self):
        """The 1A 9B layout: part number, software version, coding, system."""
        part = PART_NUMBER.ljust(12, b" ")[:12]
        version = SW_VERSION.ljust(4, b" ")[:4]
        coding = bytes([0x03]) + self.coding.to_bytes(3, "big")
        workshop = bytes.fromhex("059738DE") + (12345).to_bytes(2, "big")
        system = SYSTEM_DESC.ljust(20, b" ")[:20]
        return part + version + coding + workshop + system

    def handle_security_access(self, req):
        """VAG seed and key: the key is just code + seed."""
        sid = req[0]
        if len(req) < 2:
            return bytes([0x7F, sid, 0x13])
        if req[1] == kwp2000.ACCESS_REQUEST_SEED:
            return bytes([0x67, kwp2000.ACCESS_REQUEST_SEED]) + SEED.to_bytes(4, "big")
        if req[1] == kwp2000.ACCESS_SEND_KEY:
            if len(req) != 6:
                return bytes([0x7F, sid, 0x13])
            key = int.from_bytes(req[2:6], "big")
            if key == (REGEN_CODE + SEED) & 0xFFFFFFFF:
                self.logged_in = True
                if self.debug:
                    print("sim: login accepted")
                return bytes([0x67, kwp2000.ACCESS_SEND_KEY])
            # 0x35: access authorization failed, incorrect Login code
            return bytes([0x7F, sid, 0x35])
        return bytes([0x7F, sid, 0x12])

    def handle_routine(self, req):
        """Basic settings: 31 BA / 31 B9 to run, 32 B8 to stop."""
        sid = req[0]
        if len(req) != 4:
            return bytes([0x7F, sid, 0x13])
        routine, group = req[1], req[3]

        if sid == kwp2000.SID_STOP_ROUTINE_BY_LOCAL_ID:
            if routine != kwp2000.ROUTINE_BASIC_SETTING_STOP:
                return bytes([0x7F, sid, 0x12])
            self.basic_setting_group = None
            self.forced_regen = False
            return bytes([0x72, routine])

        if routine not in (kwp2000.ROUTINE_BASIC_SETTING_PREPARE,
                           kwp2000.ROUTINE_BASIC_SETTING_START):
            return bytes([0x7F, sid, 0x12])
        if not self.logged_in:
            # 0x33: access authorization required, perform Login
            return bytes([0x7F, sid, 0x33])
        if self.coolant < 50.0:
            # 0x87: temperature too low
            return bytes([0x7F, sid, 0x87])

        if routine == kwp2000.ROUTINE_BASIC_SETTING_PREPARE:
            self.basic_setting_group = group
        else:
            if self.basic_setting_group != group:
                # 0x24: incorrect sequence
                return bytes([0x7F, sid, 0x24])
            self.forced_regen = True
            self.regen_counter = 0
            if self.debug:
                print(f"sim: basic setting {group} running, forced regeneration on")
        return bytes([0x71, routine])

    def freeze_frame(self):
        """Fault environment data for the most recent fault."""
        packed = (25 << 26) | (10 << 22) | (14 << 17) | (9 << 12) | (35 << 6) | 12
        return (bytes([0x44, 0x72])                 # the fault it belongs to
                + bytes([0x02, 0x05, 0x28])         # priority, frequency, cycle
                + self.fault_mileage.to_bytes(3, "big")
                + bytes([0x00])
                + packed.to_bytes(5, "big"))

    def group_data(self, group):
        self.tick()
        if group == 2:
            return (_cell(0x01, self.rpm) + _cell(0x08, 0)
                    + _cell(0x08, 0) + _cell(0x05, self.coolant))
        if group == 70:
            bits = self.regen_status_bits()
            return (bytes([0x08, 0x00, bits]) + bytes([0x08, 0x00, 0x01])
                    + _cell(0x36, self.regen_counter) + bytes([0x08, 0x00, 0x00]))
        if group == 75:
            return (_cell(0x05, self.egt_pre_turbo) + _cell(0x05, self.egt_pre_dpf)
                    + _cell(0x7E, self.load_g) + _cell(0x05, self.egt_post_dpf))
        return None

    # -- TP2.0 ------------------------------------------------------------
    def _send(self, data, addr=None):
        addr = self.tx_addr if addr is None else addr
        if self.debug:
            print(f"sim TX {addr:03x}  {bytes(data).hex(' ')}")
        self.bus.send(addr, bytes(data))

    def send_message(self, payload):
        body = len(payload).to_bytes(2, "big") + bytes(payload)
        while body:
            last = len(body) <= 7
            self._send(bytes([(0x10 if last else 0x20) | self.tx_seq]) + body[:7])
            self.tx_seq = (self.tx_seq + 1) & 0x0F
            body = body[7:]

    def run(self, stop=None, idle_timeout=None):
        """Serve until `stop` (threading.Event) is set."""
        buf = b""
        deadline = None if idle_timeout is None else time.monotonic() + idle_timeout
        while not (stop is not None and stop.is_set()):
            if deadline is not None and time.monotonic() > deadline:
                return
            frame = self.bus.recv(timeout=0.1)
            if frame is None:
                continue
            can_id, data = frame
            if not data:
                continue
            if deadline is not None:
                deadline = time.monotonic() + idle_timeout
            if self.debug:
                print(f"sim RX {can_id:03x}  {data.hex(' ')}")

            if can_id == BROADCAST_ADDR:
                if len(data) >= 7 and data[0] == self.module and data[1] == OP_SETUP_REQ:
                    self._send([0x00, OP_SETUP_OK,
                                self.tx_addr & 0xFF, self.tx_addr >> 8,
                                self.rx_addr & 0xFF, self.rx_addr >> 8, 0x01],
                               addr=BROADCAST_ADDR + self.module)
                    self.open = True
                    self.tx_seq = 0
                    buf = b""
                    # A new channel is a new session: the login does not carry over.
                    self.logged_in = False
                    self.basic_setting_group = None
                continue

            if can_id != self.rx_addr or not self.open:
                continue

            op = data[0]
            if op == OP_PARAM_REQ or op == OP_CHANNEL_TEST:
                self._send([OP_PARAM_RESP, 0x0F, 0x8A, 0xFF, 0x4A, 0xFF])
                continue
            if op == OP_DISCONNECT:
                self.open = False
                continue
            if op & 0xF0 == 0xB0 or op == OP_PARAM_RESP:
                continue

            kind, seq = op >> 4, op & 0x0F
            if kind > 0x3:
                continue
            buf += data[1:]
            if kind in (0x0, 0x1):
                self._send([0xB0 | ((seq + 1) & 0x0F)])
            if kind in (0x1, 0x3):
                if len(buf) >= 2:
                    length = int.from_bytes(buf[:2], "big")
                    req = buf[2:2 + length]
                    if req:
                        self.send_message(self.handle_kwp(req))
                buf = b""


def serve(channel="vcan0", debug=False):
    from .canbus import SocketCanBus
    bus = SocketCanBus(channel)
    ecu = SimEcu(bus, debug=debug)
    print(f"simulated PPD1 ECU on {channel}: listening for TP2.0 channel setup on 0x200")
    try:
        ecu.run()
    except KeyboardInterrupt:
        pass
    finally:
        bus.close()


def spawn_on_pair(debug=False):
    """Returns (tester_bus, ecu, stop_event, thread) for in-process tests."""
    from .canbus import PairedBus
    tester, ecu_bus = PairedBus.pair()
    ecu = SimEcu(ecu_bus, debug=debug)
    stop = threading.Event()
    thread = threading.Thread(target=ecu.run, kwargs={"stop": stop}, daemon=True)
    thread.start()
    return tester, ecu, stop, thread
