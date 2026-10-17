"""KWP2000 (ISO 14230-3) client as VAG speaks it over TP2.0.

Only the services needed for diagnostics on a PPD1.x/PD engine ECU, plus a raw
escape hatch so a sequence captured from VCDS can be replayed verbatim.
"""

import dataclasses
import time

from .tp20 import Tp20Timeout

# services
SID_START_DIAGNOSTIC_SESSION = 0x10
SID_CLEAR_DIAGNOSTIC_INFORMATION = 0x14
SID_READ_DTC_BY_STATUS = 0x18
SID_READ_ECU_IDENTIFICATION = 0x1A
SID_STOP_DIAGNOSTIC_SESSION = 0x20
SID_READ_DATA_BY_LOCAL_ID = 0x21
SID_SECURITY_ACCESS = 0x27
SID_INPUT_OUTPUT_CONTROL_BY_LOCAL_ID = 0x30
SID_START_ROUTINE_BY_LOCAL_ID = 0x31
SID_STOP_ROUTINE_BY_LOCAL_ID = 0x32
SID_WRITE_DATA_BY_LOCAL_ID = 0x3B
SID_TESTER_PRESENT = 0x3E

# Session ids. 0x89/0x86/0x84/0x83/0x81 are named as OBDeleven names them;
# 0x85 is from pq-flasher. See docs/PROTOCOL.md.
SESSION_DIAGNOSTIC = 0x89          # VAG "standard" diagnostic session
SESSION_ENGINEERING = 0x86
SESSION_PROGRAMMING = 0x85
SESSION_EOL_ECU = 0x84
SESSION_EOL_VW = 0x83
SESSION_OBD = 0x81

# Security access (VCDS function 16): request seed, then send key.
ACCESS_REQUEST_SEED = 0x03
ACCESS_SEND_KEY = 0x04

# Basic settings (VCDS function 04) routine identifiers.
ROUTINE_BASIC_SETTING_PREPARE = 0xBA
ROUTINE_BASIC_SETTING_START = 0xB9
ROUTINE_BASIC_SETTING_STOP = 0xB8

# Coding identifier for writeDataByLocalIdentifier / readEcuIdentification.
LID_CODING = 0x9A
# Local identifier carrying fault environment data (freeze frame).
LID_FREEZE_FRAME = 0x6C

# Second byte of the 31 BA / 31 B9 / 32 B8 routines.
MODE_BASIC_SETTING = 0x00
MODE_ADAPTATION = 0x01
ADAPTATION_SIMPLE = 0x03           # 2-byte channel value
ADAPTATION_LONG = 0x0A

IDENT_PART_NUMBER = 0x9B
IDENT_SHORT = 0x91
IDENT_MODULE_LIST = 0x9F

SERVICE_NAMES = {
    0x10: "startDiagnosticSession", 0x11: "ecuReset",
    0x14: "clearDiagnosticInformation", 0x17: "readStatusOfDTC",
    0x18: "readDTCByStatus", 0x1A: "readEcuIdentification",
    0x20: "stopDiagnosticSession", 0x21: "readDataByLocalIdentifier",
    0x22: "readDataByCommonIdentifier", 0x23: "readMemoryByAddress",
    0x27: "securityAccess", 0x28: "disableNormalMessageTransmission",
    0x29: "enableNormalMessageTransmission", 0x2C: "dynamicallyDefineLocalId",
    0x2E: "writeDataByCommonIdentifier", 0x30: "inputOutputControlByLocalId",
    0x31: "startRoutineByLocalIdentifier", 0x32: "stopRoutineByLocalIdentifier",
    0x33: "requestRoutineResultsByLocalId", 0x34: "requestDownload",
    0x35: "requestUpload", 0x36: "transferData", 0x37: "requestTransferExit",
    0x3B: "writeDataByLocalIdentifier", 0x3D: "writeMemoryByAddress",
    0x3E: "testerPresent", 0x82: "stopCommunication",
}

# VAG's own wording for the codes its KWP2000 ECUs return. Much more specific
# than the ISO 14230 table below, and several of these are exactly what a
# refused regeneration looks like.
VAG_NEGATIVE_RESPONSE_CODES = {
    0x10: "function cancelled, disruption in control module",
    0x11: "function not available",
    0x12: "function not available",
    0x13: "function cannot be performed, message length or format incorrect",
    0x22: "function cancelled, marginal conditions have not been met",
    0x24: "procedure cannot be performed, incorrect sequence",
    0x25: "function cancelled, subsystem does not respond",
    0x26: "function cancelled, entry in DTC memory",
    0x31: "function not available",
    0x33: "access authorization required, perform Login",
    0x35: "access authorization failed, incorrect Login code",
    0x36: "login not possible, number of access attempts exceeded",
    0x37: "login not possible, lock time not yet elapsed",
    0x70: "function cancelled, upload/download not possible",
    0x71: "function cancelled, data transfer error",
    0x72: "function cancelled, delete/programming unsuccessful",
    0x73: "function cancelled, incorrect data block sequence",
    0x78: "function started, control module busy",
    0x7E: "function cannot be performed, incorrect diagnostic mode",
    0x81: "function cancelled, engine RPM too high",
    0x82: "function cancelled, engine RPM too low",
    0x83: "function cancelled, engine running",
    0x84: "function cancelled, engine not running",
    0x85: "function cancelled, engine not running long enough",
    0x86: "function cancelled, temperature too high",
    0x87: "function cancelled, temperature too low",
    0x88: "function cancelled, vehicle speed too high",
    0x89: "function cancelled, vehicle speed too low",
    0x8A: "function cancelled, accelerator pedal position exceeded",
    0x8B: "function cancelled, accelerator pedal position not reached",
    0x8C: "function cancelled, selector lever not in position 'N'",
    0x8D: "function cancelled, driving mode/gear not selected",
    0x8F: "function cancelled, brake pedal not depressed",
    0x90: "function cancelled, selector lever not in position 'P'",
    0x91: "function cancelled, converter clutch locked",
    0x92: "function cancelled, supply voltage too high",
    0x93: "function cancelled, supply voltage too low",
}

NEGATIVE_RESPONSE_CODES = {
    0x10: "generalReject", 0x11: "serviceNotSupported",
    0x12: "subFunctionNotSupported / invalidFormat", 0x21: "busy - repeat request",
    0x22: "conditionsNotCorrect or requestSequenceError", 0x23: "routineNotComplete",
    0x31: "requestOutOfRange", 0x33: "securityAccessDenied", 0x35: "invalidKey",
    0x36: "exceedNumberOfAttempts", 0x37: "requiredTimeDelayNotExpired",
    0x78: "requestCorrectlyReceived - responsePending",
}

# Services known to only read. Anything else either changes the ECU or is a
# VAG-specific service nobody here has identified - both worth flagging, which
# is what makes `decode-log` useful for finding the Coding-II sequence.
READ_ONLY_SERVICES = {0x10, 0x12, 0x13, 0x17, 0x18, 0x1A, 0x20, 0x21, 0x22, 0x23,
                      0x33, 0x3A, 0x3E}


def changes_ecu(service_id):
    """True for a request that is not a known read-only service."""
    return service_id not in READ_ONLY_SERVICES


class NegativeResponse(Exception):
    def __init__(self, service_id, code, raw=b""):
        self.service_id = service_id
        self.code = code
        self.raw = raw
        service = SERVICE_NAMES.get(service_id, f"service {service_id:#04x}")
        reason = VAG_NEGATIVE_RESPONSE_CODES.get(
            code, NEGATIVE_RESPONSE_CODES.get(code, f"code {code:#04x}"))
        super().__init__(f"{service} rejected: {reason}")


class UnexpectedResponse(Exception):
    pass


class Kwp2000Client:
    def __init__(self, channel, debug=False):
        self.channel = channel
        self.debug = debug

    def request(self, payload, expect_response=True, retries=2):
        """Send one KWP2000 request and return the response without its SID."""
        payload = bytes(payload)
        sid = payload[0]
        attempt = 0
        while True:
            if self.debug:
                print(f"KWP TX  {payload.hex(' ')}")
            self.channel.send(payload)
            if not expect_response:
                return b""
            try:
                resp = self._receive_settled(sid)
            except Tp20Timeout:
                if attempt >= retries:
                    raise
                attempt += 1
                continue

            if self.debug:
                print(f"KWP RX  {resp.hex(' ')}")

            if resp[0] == 0x7F:
                code = resp[2] if len(resp) > 2 else 0
                if code == 0x21 and attempt < retries:   # busy, repeat request
                    attempt += 1
                    time.sleep(0.05)
                    continue
                raise NegativeResponse(resp[1] if len(resp) > 1 else sid, code, resp)

            if resp[0] != sid + 0x40:
                raise UnexpectedResponse(
                    f"asked for {SERVICE_NAMES.get(sid, hex(sid))}, got {resp.hex(' ')}")
            return resp[1:]

    def _receive_settled(self, sid, pending_limit=20):
        """Swallow 0x7F .. 0x78 (response pending) replies."""
        for _ in range(pending_limit):
            resp = self.channel.recv()
            if not resp:
                continue
            if resp[0] == 0x7F and len(resp) > 2 and resp[2] == 0x78:
                if self.debug:
                    print("KWP RX  response pending")
                continue
            return resp
        raise UnexpectedResponse("ECU kept answering 'response pending'")

    # -- services ---------------------------------------------------------
    def start_diagnostic_session(self, session=SESSION_DIAGNOSTIC):
        return self.request(bytes([SID_START_DIAGNOSTIC_SESSION, session]))

    def stop_diagnostic_session(self):
        return self.request(bytes([SID_STOP_DIAGNOSTIC_SESSION]))

    def tester_present(self):
        return self.request(bytes([SID_TESTER_PRESENT, 0x01]))

    def read_ecu_identification(self, ident=IDENT_PART_NUMBER):
        return self.request(bytes([SID_READ_ECU_IDENTIFICATION, ident]))

    def read_group(self, group):
        """Measuring block ('Messwerteblock') `group`, 1..255.

        Returns the raw body: the echoed group number followed by four
        (formula, a, b) triples.
        """
        if not 1 <= group <= 0xFF:
            raise ValueError("group must be 1..255")
        body = self.request(bytes([SID_READ_DATA_BY_LOCAL_ID, group]))
        if not body or body[0] != group:
            raise UnexpectedResponse(f"asked for group {group}, got {body.hex(' ')}")
        return body[1:]

    def read_dtcs(self):
        """readDTCByStatus 'all identified faults'. Returns [(code, status)]."""
        body = self.request(bytes([SID_READ_DTC_BY_STATUS, 0x02, 0xFF, 0xFF]))
        if not body:
            return []
        count, rest = body[0], body[1:]
        dtcs = []
        for i in range(0, len(rest) - 2, 3):
            code = int.from_bytes(rest[i:i + 2], "big")
            if code == 0:
                continue
            dtcs.append((code, rest[i + 2]))
        if count and len(dtcs) != count and self.debug:
            print(f"note: ECU announced {count} faults, decoded {len(dtcs)}")
        return dtcs

    def clear_dtcs(self):
        return self.request(bytes([SID_CLEAR_DIAGNOSTIC_INFORMATION, 0xFF, 0x00]))

    # -- security access (VCDS function 16) -------------------------------
    def security_access(self, code):
        """Log in with a numeric code. VAG's key is simply code + seed.

        Returns the seed and key used, so a caller can show its work.
        Raises NegativeResponse if the ECU refuses; 0x35 means wrong code.
        """
        code = int(code)
        body = self.request(bytes([SID_SECURITY_ACCESS, ACCESS_REQUEST_SEED]))
        if len(body) < 5 or body[0] != ACCESS_REQUEST_SEED:
            raise UnexpectedResponse(f"unexpected seed response: {body.hex(' ')}")
        seed = int.from_bytes(body[1:5], "big")
        key = (code + seed) & 0xFFFFFFFF
        self.request(bytes([SID_SECURITY_ACCESS, ACCESS_SEND_KEY])
                     + key.to_bytes(4, "big"))
        return seed, key

    # -- the 31 BA / 31 B9 / 32 B8 family ---------------------------------
    # Three routines with the same shape:  <routine> <mode> <id> [data]
    # mode 0x00 addresses a basic settings group, mode 0x01 an adaptation
    # form (3 = the plain 2-byte channel kind, 10 = long adaptation).
    def open_function(self, mode, ident):
        """31 BA <mode> <id> - select a group or adaptation form."""
        return self.request(bytes([SID_START_ROUTINE_BY_LOCAL_ID,
                                   ROUTINE_BASIC_SETTING_PREPARE, mode, ident]))

    def run_function(self, mode, ident, data=b""):
        """31 B9 <mode> <id> [data] - run it; adaptation appends the new value."""
        return self.request(bytes([SID_START_ROUTINE_BY_LOCAL_ID,
                                   ROUTINE_BASIC_SETTING_START, mode, ident]) + bytes(data))

    def close_function(self, mode, ident):
        """32 B8 <mode> <id> - stop it."""
        return self.request(bytes([SID_STOP_ROUTINE_BY_LOCAL_ID,
                                   ROUTINE_BASIC_SETTING_STOP, mode, ident]))

    def prepare_basic_setting(self, group):
        return self.open_function(MODE_BASIC_SETTING, self._group(group))

    def start_basic_setting(self, group):
        return self.run_function(MODE_BASIC_SETTING, self._group(group))

    def stop_basic_setting(self, group):
        return self.close_function(MODE_BASIC_SETTING, self._group(group))

    @staticmethod
    def _group(group):
        if not 0 <= group <= 0xFF:
            raise ValueError("group must be 0..255")
        return group

    def read_freeze_frame(self):
        """Fault environment data. The response layout is known (`21 6C` ->
        `61 6C ...`); that this is the request is inferred, see docs."""
        return self.request(bytes([SID_READ_DATA_BY_LOCAL_ID, LID_FREEZE_FRAME]))

    def start_routine(self, routine, args=b""):
        """Raw startRoutineByLocalIdentifier, for a captured routine id."""
        return self.request(bytes([SID_START_ROUTINE_BY_LOCAL_ID, routine]) + bytes(args))


@dataclasses.dataclass
class Identification:
    """Decoded `1A 9B` response."""
    part_number: str = ""
    sw_version: str = ""
    coding_length: int = None
    coding: int = None
    system_description: str = ""
    subsystems: int = 0
    raw: bytes = b""

    def lines(self):
        out = [f"part number         {self.part_number}",
               f"software version    {self.sw_version}"]
        if self.system_description:
            out.append(f"system              {self.system_description}")
        if self.coding_length is not None:
            coding = "-" if self.coding is None else f"{self.coding:07d}"
            out.append(f"coding              {coding} ({self.coding_length} bytes)")
        if self.subsystems:
            out.append(f"subsystems          {self.subsystems}")
        return out


def _ascii(data):
    """VAG sets the top bit of the first character as a flag."""
    if not data:
        return ""
    cleaned = bytes([data[0] & 0x7F]) + bytes(data[1:])
    return cleaned.decode("latin-1").strip().strip("\x00").strip()


def parse_identification(body):
    """Decode the body of a `1A 9B` response (identifier byte still first).

        <9B> <part number:12> <software version:4> <coding length:1>
             <coding + workshop identification:9> <system description:20>
             <subsystem blocks of 47 bytes...>
    """
    if not body or body[0] != IDENT_PART_NUMBER:
        raise UnexpectedResponse(f"not a 1A 9B response: {bytes(body[:4]).hex(' ')}")
    data = bytes(body[1:])
    ident = Identification(raw=data)
    ident.part_number = _ascii(data[0:12])
    ident.sw_version = _ascii(data[12:16])
    if len(data) > 16:
        ident.coding_length = data[16]
        if ident.coding_length == 3 and len(data) >= 20:
            ident.coding = int.from_bytes(data[17:20], "big") & 0x7FFFFF
    if len(data) >= 46:
        ident.system_description = _ascii(data[26:46])
    if len(data) > 46:
        ident.subsystems = len(data[46:]) // 47
    return ident


def build_coding_ii_request(code, sw_version, prefix, wsc=12345):
    """Assemble a Coding-II write the way OBDeleven does:

        3B 9A <prefix:4> <wsc:2> <software version as ASCII> 03 <code:3>

    `prefix` has no safe default - the four bytes after 9A differ between
    OBDeleven's own code paths and are not understood here, so the caller must
    supply the ones seen in a capture from this ECU. `sw_version` is the ECU's
    own software version string, which the request embeds; that is why a
    sequence captured from another car cannot be replayed as-is.

    Returns the request bytes. Sending it writes to the ECU - see
    docs/PROTOCOL.md before you do.
    """
    prefix = bytes(prefix)
    if len(prefix) != 4:
        raise ValueError("prefix must be exactly 4 bytes")
    code = int(code)
    if not 0 <= code <= 0x7FFFFF:
        raise ValueError("coding value must fit in 3 bytes")
    if isinstance(sw_version, str):
        sw_version = sw_version.encode("ascii")
    return (bytes([SID_WRITE_DATA_BY_LOCAL_ID, LID_CODING]) + prefix
            + int(wsc).to_bytes(2, "big") + bytes(sw_version)
            + bytes([0x03]) + code.to_bytes(3, "big"))
