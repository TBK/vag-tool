//! Measuring block ("Messwerteblock") decoding.
//!
//! A readDataByLocalIdentifier response carries four cells of three bytes: a
//! formula index and two operands A and B. The table below comes from the two
//! independent open implementations:
//!
//! * `jazdw/vag-blocks`, `kwp2000.cpp` (GPL-3.0, Jared Wiltshire)
//! * `seishuku/teensycanbusdisplay`, `vwtpkwp2k.c` (Matt Williams)
//!
//! Where both describe a formula they agree. Formulas neither documents are
//! left out on purpose: [`decode_cell`] then reports the raw 16-bit value and
//! the unknown index, so a wrong reading is visible rather than silent.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cell {
    pub formula: u8,
    pub a: u8,
    pub b: u8,
    pub value: f64,
    pub unit: &'static str,
    pub known: bool,
}

pub fn decode_cell(formula: u8, a: u8, b: u8) -> Cell {
    let (fa, fb) = (f64::from(a), f64::from(b));
    let raw = f64::from((u16::from(a) << 8) | u16::from(b));

    let (unit, value): (&'static str, f64) = match formula {
        0x01 => ("rpm", fa * fb / 5.0),
        0x04 => ("deg", (fb - 127.0).abs() * 0.01 * fa),
        0x05 => ("C", fa * (fb - 100.0) * 0.1),
        0x07 => ("km/h", fa * fb * 0.01),
        0x08 | 0x10 | 0x25 => ("bin", raw),
        0x11 => ("ascii", raw),
        0x12 => ("mbar", fa * fb / 25.0),
        0x14 => ("%", fa * fb / 128.0 - 1.0),
        0x15 => ("V", fa * fb / 1000.0),
        0x16 => ("ms", fa * fb * 0.001),
        0x17 => ("%", fb * fa / 256.0),
        0x1A => ("C", fb - fa),
        0x21 => ("%", if a == 0 { 100.0 * fb } else { 100.0 * fb / fa }),
        0x22 => ("kW", (fb - 128.0) * 0.01 * fa),
        0x23 => ("l/h", fa * fb / 100.0),
        0x27 => ("mg/stk", fa * fb / 256.0),
        0x31 => ("mg/stk", fa * fb / 40.0),
        0x33 => ("mg/stk", ((fb - 128.0) / 255.0) * fa),
        0x36 => ("count", fa * 256.0 + fb),
        0x37 => ("s", fa * fb / 200.0),
        0x60 => ("mbar", fa * fb * 0.1),
        0x7E => ("g", fa * fb * 0.1),
        _ => {
            return Cell {
                formula,
                a,
                b,
                value: raw,
                unit: "raw",
                known: false,
            };
        }
    };

    Cell {
        formula,
        a,
        b,
        value,
        unit,
        known: true,
    }
}

/// Decode the body of a readDataByLocalIdentifier response, group number
/// already stripped.
pub fn decode_group(body: &[u8]) -> Vec<Cell> {
    body.as_chunks::<3>()
        .0
        .iter()
        .map(|c| decode_cell(c[0], c[1], c[2]))
        .collect()
}

/// Field names as VCDS labels them. Source: Ross-Tech wiki, "Diesel Particle
/// Filter Emergency Regeneration" (PD/PPD section). Only the fields that page
/// actually names are named here.
pub fn group_title(group: u8) -> String {
    match group {
        2 => "Engine basics".to_string(),
        70 => "DPF regeneration".to_string(),
        75 => "DPF state".to_string(),
        other => format!("group {other:03}"),
    }
}

pub fn field_names(group: u8) -> [&'static str; 4] {
    match group {
        2 => ["Engine speed", "field 2", "field 3", "Coolant temperature"],
        70 => [
            "Regeneration status (bit field)",
            "field 2",
            "Regeneration counter / timer",
            "field 4",
        ],
        75 => [
            "EGT before turbo",
            "EGT before particle filter",
            "Particle filter load",
            "EGT after particle filter",
        ],
        _ => ["field 1", "field 2", "field 3", "field 4"],
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RegenStatus {
    pub bits: u8,
    pub normal: bool,
    pub forced: bool,
}

impl RegenStatus {
    pub fn bits_text(&self) -> String {
        format!("{:08b}", self.bits)
    }
}

/// Group 070 field 1 is a bit field. Ross-Tech documents the low two bits:
/// `xxxxxxx1` = normal regeneration, `xxxxxx1x` = forced regeneration.
pub fn decode_regen_status(value: f64) -> RegenStatus {
    let bits = (value as i64 & 0xFF) as u8;
    RegenStatus {
        bits,
        normal: bits & 0x01 != 0,
        forced: bits & 0x02 != 0,
    }
}

pub fn format_cell(cell: &Cell) -> String {
    match cell.unit {
        "bin" => format!("{:08b} (bin)", (cell.value as i64 & 0xFF) as u8),
        "ascii" => {
            let raw = cell.value as u16;
            let chars: String = [(raw >> 8) as u8, raw as u8]
                .into_iter()
                .map(|b| {
                    if (0x20..0x7F).contains(&b) {
                        b as char
                    } else {
                        '.'
                    }
                })
                .collect();
            format!("{chars:?}")
        }
        _ if !cell.known => format!("{} (raw, formula {:#04x})", cell.value as i64, cell.formula),
        unit => format!("{:.1} {unit}", cell.value),
    }
}

pub fn format_group(group: u8, cells: &[Cell]) -> String {
    let mut out = vec![format!("{group:03}  {}", group_title(group))];
    for (name, cell) in field_names(group).iter().zip(cells) {
        out.push(format!(
            "     {name:<34} {:<22} [{:02x} {:02x} {:02x}]",
            format_cell(cell),
            cell.formula,
            cell.a,
            cell.b
        ));
    }
    out.join("\n")
}
