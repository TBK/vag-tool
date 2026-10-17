"""Measuring block ("Messwerteblock") decoding.

A readDataByLocalIdentifier response carries four cells, each three bytes:
a formula index and two operands A and B. The formula table below is taken from
the two independent open implementations in RESEARCH:

  * jazdw/vag-blocks, kwp2000.cpp  (GPL-3.0, Jared Wiltshire)
  * seishuku/teensycanbusdisplay, vwtpkwp2k.c  (Matt Williams)

Where both describe a formula they agree. Formulas neither documents are left
out on purpose: `decode_cell` then reports the raw 16-bit value and the unknown
formula index, so a wrong reading is visible rather than silent.
"""

import collections

Cell = collections.namedtuple("Cell", "formula a b value unit known")

# formula index -> (unit, function of A and B)
FORMULAS = {
    0x01: ("rpm", lambda a, b: a * b / 5.0),
    0x04: ("deg", lambda a, b: abs(b - 127) * 0.01 * a),
    0x05: ("C", lambda a, b: a * (b - 100) * 0.1),
    0x07: ("km/h", lambda a, b: a * b * 0.01),
    0x08: ("bin", lambda a, b: (a << 8) | b),
    0x10: ("bin", lambda a, b: (a << 8) | b),
    0x11: ("ascii", lambda a, b: chr(a) + chr(b)),
    0x12: ("mbar", lambda a, b: a * b / 25.0),
    0x14: ("%", lambda a, b: a * b / 128.0 - 1),
    0x15: ("V", lambda a, b: a * b / 1000.0),
    0x16: ("ms", lambda a, b: a * b * 0.001),
    0x17: ("%", lambda a, b: b * a / 256.0),
    0x1A: ("C", lambda a, b: b - a),
    0x21: ("%", lambda a, b: 100.0 * b if a == 0 else 100.0 * b / a),
    0x22: ("kW", lambda a, b: (b - 128) * 0.01 * a),
    0x23: ("l/h", lambda a, b: a * b / 100.0),
    0x25: ("bin", lambda a, b: (a << 8) | b),
    0x27: ("mg/stk", lambda a, b: a * b / 256.0),
    0x31: ("mg/stk", lambda a, b: a * b / 40.0),
    0x33: ("mg/stk", lambda a, b: ((b - 128) / 255.0) * a),
    0x36: ("count", lambda a, b: a * 256 + b),
    0x37: ("s", lambda a, b: a * b / 200.0),
    0x60: ("mbar", lambda a, b: a * b * 0.1),
    0x7E: ("g", lambda a, b: a * b * 0.1),
}


def decode_cell(formula, a, b):
    entry = FORMULAS.get(formula)
    if entry is None:
        return Cell(formula, a, b, (a << 8) | b, "raw", False)
    unit, fn = entry
    return Cell(formula, a, b, fn(a, b), unit, True)


def decode_group(body):
    """Decode the body of a readDataByLocalIdentifier response (group number
    already stripped) into a list of Cells."""
    cells = []
    for i in range(0, len(body) - 2, 3):
        cells.append(decode_cell(body[i], body[i + 1], body[i + 2]))
    return cells


# Field names as VCDS labels them. Source: Ross-Tech wiki, "Diesel Particle
# Filter Emergency Regeneration" (PD/PPD section) and the 2.0 TDI PD label file.
GROUP_FIELDS = {
    2: ("Engine basics", ["Engine speed", "field 2", "field 3",
                          "Coolant temperature"]),
    70: ("DPF regeneration", ["Regeneration status (bit field)", "field 2",
                              "Regeneration counter / timer", "field 4"]),
    75: ("DPF state", ["EGT before turbo", "EGT before particle filter",
                       "Particle filter load", "EGT after particle filter"]),
}


def field_names(group):
    _, names = GROUP_FIELDS.get(group, (None, None))
    if names:
        return names
    return [f"field {i}" for i in range(1, 5)]


def group_title(group):
    title, _ = GROUP_FIELDS.get(group, (None, None))
    return title or f"group {group:03d}"


def decode_regen_status(value):
    """Group 070 field 1 is a bit field. Ross-Tech documents the low two bits:
    xxxxxxx1 = normal regeneration, xxxxxx1x = forced regeneration."""
    return {
        "bits": format(value & 0xFF, "08b"),
        "normal_regen": bool(value & 0x01),
        "forced_regen": bool(value & 0x02),
    }


def format_cell(cell):
    if cell.unit == "bin":
        return f"{format(cell.value & 0xFF, '08b')} (bin)"
    if cell.unit == "ascii":
        return f"{cell.value!r}"
    if not cell.known:
        return f"{cell.value} (raw, formula {cell.formula:#04x})"
    return f"{cell.value:.1f} {cell.unit}"


def format_group(group, cells):
    lines = [f"{group:03d}  {group_title(group)}"]
    for name, cell in zip(field_names(group), cells):
        lines.append(f"     {name:<34} {format_cell(cell):<22}"
                     f" [{cell.formula:02x} {cell.a:02x} {cell.b:02x}]")
    return "\n".join(lines)
