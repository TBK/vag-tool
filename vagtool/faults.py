"""VAG fault decoding: codes, status bytes and KWP2000 freeze frames.

The packing below is how VAG encodes a fault in the two bytes a KWP2000 or
KWP1281 ECU returns, and how the status byte splits. Learned from the OBDeleven
package (see docs/PROTOCOL.md); verified against the published mapping, e.g.
raw 0x4034 -> 16436 -> P0052.

Fault descriptions themselves are a 72,000-entry VAG text table that ships in
OBDeleven's APK. It is not copied here: `LabelTable` reads it from a file or an
APK you already have, and everything works without it, just with bare codes.
"""

import dataclasses
import json
import zipfile

SAE_LETTERS = "PCBU"

# VAG's 16 KWP2000 fault types (status & 0x7F). Their wording.
FAULT_STATUS_TEXTS = {
    0: "",
    1: "upper limit exceeded",
    2: "lower limit not reached",
    3: "mechanical malfunction",
    4: "no signal/communication",
    5: "no or incorrect basic setting/adaptation",
    6: "short circuit to B+",
    7: "short circuit to ground",
    8: "implausible signal",
    9: "open circuit/short circuit to ground",
    10: "open circuit/short circuit to B+",
    11: "open circuit",
    12: "electrical error in circuit",
    13: "please read DTC",
    14: "faulty",
    15: "cannot be checked currently",
}


@dataclasses.dataclass(frozen=True)
class Fault:
    raw: int        # the two bytes as the ECU sent them
    status: int

    @property
    def is_sae(self):
        """Bit 14 set means the code is an SAE code packed into the 16 bits."""
        return (self.raw >> 14) == 1

    @property
    def code_text(self):
        if self.is_sae:
            letter = SAE_LETTERS[(self.raw >> 12) & 3]
            return f"{letter}{(self.raw >> 10) & 3}{self.raw & 0x3FF:03d}"
        return f"{self.raw:05d}"

    @property
    def vag_code(self):
        """The 5-digit number VCDS shows, which is just the raw value."""
        return self.raw

    @property
    def status_code(self):
        return self.status & 0x7F

    @property
    def intermittent(self):
        return bool(self.status >> 7)

    @property
    def text_key(self):
        """Key into the VAG text table."""
        return f"{self.code_text}00" if self.is_sae else f"VAG{self.code_text}"

    def status_text(self):
        return FAULT_STATUS_TEXTS.get(self.status_code)

    def describe(self, labels=None):
        parts = [self.code_text]
        if self.is_sae:
            parts.append(f"(VAG {self.vag_code})")
        description = labels.get(self.text_key) if labels else None
        if description:
            parts.append("- " + description.replace("\n", " / "))
        detail = self.status_text()
        if detail:
            parts.append(f"[{detail}]")
        elif self.status_code:
            parts.append(f"[status type {self.status_code}]")
        parts.append("intermittent" if self.intermittent else "static")
        return " ".join(parts)


def decode_faults(entries):
    """`entries` is the (code, status) list read with readDTCByStatus."""
    return [Fault(code, status) for code, status in entries]


class LabelTable:
    """VAG text table lookups, from a JSON file or straight out of an APK."""

    def __init__(self, mapping):
        self.mapping = mapping

    def __len__(self):
        return len(self.mapping)

    def get(self, key):
        value = self.mapping.get(key)
        return value.strip() if isinstance(value, str) and value.strip() else None

    @classmethod
    def from_path(cls, path, lang="en"):
        if zipfile.is_zipfile(path):
            with zipfile.ZipFile(path) as zf:
                wanted = f"assets/lang/{lang}.json"
                names = zf.namelist()
                if wanted not in names:
                    candidates = [n for n in names
                                  if n.startswith("assets/lang/") and n.endswith(".json")]
                    if not candidates:
                        raise ValueError(f"no assets/lang/*.json inside {path}")
                    wanted = candidates[0]
                with zf.open(wanted) as fh:
                    return cls(json.load(fh))
        with open(path) as fh:
            return cls(json.load(fh))


@dataclasses.dataclass
class FreezeFrame:
    """Fault environment data, as a KWP2000 ECU returns it for local id 0x6C."""
    priority: int = None
    frequency: int = None
    driving_cycle: int = None
    mileage_km: int = None
    timestamp: str = None
    raw: bytes = b""

    def lines(self):
        out = []
        if self.priority is not None:
            out.append(f"priority            {self.priority}")
        if self.frequency is not None:
            out.append(f"frequency counter   {self.frequency}")
        if self.driving_cycle is not None:
            out.append(f"driving cycle       {self.driving_cycle}")
        if self.mileage_km is not None:
            out.append(f"mileage             {self.mileage_km} km")
        if self.timestamp:
            out.append(f"time stamp          {self.timestamp}")
        return out


def decode_freeze_frame(body):
    """Decode the body of a `21 6C` response (local identifier already first).

    Layout, hex offsets as OBDeleven slices them:
        6C <2 bytes> <priority> <frequency> <cycle> <mileage:3> <1 byte>
           <packed date/time:5> ...
    The two leading bytes and the one before the time stamp are not understood.
    """
    if not body or body[0] != 0x6C:
        raise ValueError(f"not a 0x6C freeze frame: {body[:4].hex(' ')}")
    data = body[1:]
    frame = FreezeFrame(raw=bytes(body))
    if len(data) < 8:
        return frame

    frame.priority = data[2]
    frame.frequency = data[3]
    frame.driving_cycle = data[4]
    mileage = int.from_bytes(data[5:8], "big")
    if 0 < mileage < 1_000_000:
        frame.mileage_km = mileage

    if len(data) >= 14:
        packed = int.from_bytes(data[9:14], "big")
        second = packed & 0x3F
        minute = (packed >> 6) & 0x3F
        hour = (packed >> 12) & 0x1F
        day = (packed >> 17) & 0x1F
        month = (packed >> 22) & 0x0F
        year = (packed >> 26) & 0x7F
        if (0 < year < 127 and 0 < month < 13 and day > 0
                and hour < 24 and minute < 60 and second < 60):
            frame.timestamp = (f"{2000 + year:04d}-{month:02d}-{day:02d} "
                               f"{hour:02d}:{minute:02d}:{second:02d}")
    return frame
