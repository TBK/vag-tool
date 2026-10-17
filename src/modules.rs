//! VAG logical module addresses, as a TP2.0 channel setup names them.
//!
//! The table is `kwpbridge/didb/modules.json` from
//! <https://github.com/dspl1236/KWPBridge> (MIT, Copyright (c) 2026
//! dspl1236), reformatted. It is the same numbering VCDS shows, where the
//! engine is 01 and the gateway 19 - written in hex, which is how every VAG
//! tool writes these.

/// One addressable control unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Module {
    pub address: u8,
    /// VAG's own symbol, e.g. `MOT_01` for the engine.
    pub symbol: &'static str,
    pub name: &'static str,
}

/// Every module in the table, sorted by address.
pub static MODULES: &[Module] = &[
    Module {
        address: 0x01,
        symbol: "MOT_01",
        name: "Engine Control Module 1",
    },
    Module {
        address: 0x02,
        symbol: "GET_02",
        name: "Transmission Control Module",
    },
    Module {
        address: 0x03,
        symbol: "BRE_03",
        name: "Brakes 1",
    },
    Module {
        address: 0x04,
        symbol: "LKP_04",
        name: "Steering Angle Sender",
    },
    Module {
        address: 0x06,
        symbol: "SVB_06",
        name: "Seat Adjustment Passenger Side",
    },
    Module {
        address: 0x07,
        symbol: "ABE_07",
        name: "Display Control Unit",
    },
    Module {
        address: 0x08,
        symbol: "KLH_08",
        name: "Air Conditioning",
    },
    Module {
        address: 0x09,
        symbol: "EZE_09",
        name: "Central Electrics",
    },
    Module {
        address: 0x10,
        symbol: "PLA_10",
        name: "Parking Assistance 2",
    },
    Module {
        address: 0x11,
        symbol: "M2B_11",
        name: "Engine Control Module 2",
    },
    Module {
        address: 0x12,
        symbol: "KUP_12",
        name: "Clutch Control Unit",
    },
    Module {
        address: 0x13,
        symbol: "DIS_13",
        name: "Adaptive Cruise Control",
    },
    Module {
        address: 0x14,
        symbol: "RAD_14",
        name: "Wheel Dampening Electronics",
    },
    Module {
        address: 0x15,
        symbol: "AIR_15",
        name: "Airbag",
    },
    Module {
        address: 0x16,
        symbol: "LRE_16",
        name: "Steering Column Electronics",
    },
    Module {
        address: 0x17,
        symbol: "SCH_17",
        name: "Dash Board",
    },
    Module {
        address: 0x18,
        symbol: "ZST_18",
        name: "Auxiliary Parking Heater",
    },
    Module {
        address: 0x19,
        symbol: "DID_19",
        name: "Gateway",
    },
    Module {
        address: 0x20,
        symbol: "FLA_20",
        name: "High Beam Assistance",
    },
    Module {
        address: 0x21,
        symbol: "BMM_21",
        name: "Battery Energy Control Module 2",
    },
    Module {
        address: 0x22,
        symbol: "ALR_22",
        name: "All Wheel Control",
    },
    Module {
        address: 0x23,
        symbol: "BKV_23",
        name: "Brake Boost",
    },
    Module {
        address: 0x25,
        symbol: "WEG_25",
        name: "Immobilizer",
    },
    Module {
        address: 0x26,
        symbol: "ELD_26",
        name: "Electronic Roof Control",
    },
    Module {
        address: 0x27,
        symbol: "ABH_27",
        name: "Display Control Unit Rear",
    },
    Module {
        address: 0x28,
        symbol: "KBH_28",
        name: "Climate Control Unit Rear",
    },
    Module {
        address: 0x29,
        symbol: "LSL_29",
        name: "Light Control Left",
    },
    Module {
        address: 0x30,
        symbol: "SFZ_30",
        name: "Special Function 2",
    },
    Module {
        address: 0x31,
        symbol: "MOV_31",
        name: "Engine Electronics Group",
    },
    Module {
        address: 0x32,
        symbol: "SPR_32",
        name: "Lock Electronics",
    },
    Module {
        address: 0x33,
        symbol: "OBDII",
        name: "All OBD Systems",
    },
    Module {
        address: 0x34,
        symbol: "NIV_34",
        name: "Ride Control System",
    },
    Module {
        address: 0x35,
        symbol: "ZEN_35",
        name: "Central Locking Control Module",
    },
    Module {
        address: 0x36,
        symbol: "SVF_36",
        name: "Seat Adjustment Driver Side",
    },
    Module {
        address: 0x37,
        symbol: "NAV_37",
        name: "Navigation",
    },
    Module {
        address: 0x38,
        symbol: "DEL_38",
        name: "Roof Electronic Control Module",
    },
    Module {
        address: 0x39,
        symbol: "LSR_39",
        name: "Light Control Right",
    },
    Module {
        address: 0x40,
        symbol: "KKP_40",
        name: "Air Conditioning Compressor",
    },
    Module {
        address: 0x41,
        symbol: "DIE_41",
        name: "Diesel Pump Control Unit",
    },
    Module {
        address: 0x42,
        symbol: "TFA_42",
        name: "Door Electronics Driver Side",
    },
    Module {
        address: 0x43,
        symbol: "BKU_43",
        name: "Brakes 2",
    },
    Module {
        address: 0x44,
        symbol: "LKH_44",
        name: "Steering Assistance",
    },
    Module {
        address: 0x45,
        symbol: "IRA_45",
        name: "Interior Monitoring",
    },
    Module {
        address: 0x46,
        symbol: "ZKS_46",
        name: "Central Module Comfort System",
    },
    Module {
        address: 0x47,
        symbol: "SOU_47",
        name: "Sound System",
    },
    Module {
        address: 0x48,
        symbol: "SHF_48",
        name: "Seat Adjustment Rear Driver Side",
    },
    Module {
        address: 0x50,
        symbol: "SDR_50",
        name: "Seat Adjustment 3rd Row",
    },
    Module {
        address: 0x51,
        symbol: "ELA_51",
        name: "Drive Motor Control Module",
    },
    Module {
        address: 0x52,
        symbol: "TBF_52",
        name: "Door Electronics Passenger Side",
    },
    Module {
        address: 0x53,
        symbol: "BFS_53",
        name: "Parking Brake",
    },
    Module {
        address: 0x54,
        symbol: "HKS_54",
        name: "Rear Spoiler",
    },
    Module {
        address: 0x55,
        symbol: "LWR_55",
        name: "Headlight Regulation",
    },
    Module {
        address: 0x57,
        symbol: "TVT_57",
        name: "TV Tuner",
    },
    Module {
        address: 0x58,
        symbol: "KSZ_58",
        name: "Fuel Supply",
    },
    Module {
        address: 0x60,
        symbol: "FSR_60",
        name: "Tachograph",
    },
    Module {
        address: 0x61,
        symbol: "BAR_61",
        name: "Battery Regulation",
    },
    Module {
        address: 0x62,
        symbol: "THL_62",
        name: "Door Electronics Rear Left",
    },
    Module {
        address: 0x63,
        symbol: "ESF_63",
        name: "Entry Assistance Driver Side",
    },
    Module {
        address: 0x65,
        symbol: "REI_65",
        name: "Tire Pressure Monitoring 1",
    },
    Module {
        address: 0x66,
        symbol: "SSV_66",
        name: "Seat Adjustment Rear",
    },
    Module {
        address: 0x67,
        symbol: "SPS_67",
        name: "Voice Control",
    },
    Module {
        address: 0x68,
        symbol: "WIE_68",
        name: "Wiper Control Unit",
    },
    Module {
        address: 0x69,
        symbol: "AHF_69",
        name: "Trailer Function",
    },
    Module {
        address: 0x71,
        symbol: "BAL_71",
        name: "Battery Charger",
    },
    Module {
        address: 0x72,
        symbol: "THR_72",
        name: "Door Electronics Rear Right",
    },
    Module {
        address: 0x73,
        symbol: "ESB_73",
        name: "Entry Assistance Passenger Side",
    },
    Module {
        address: 0x74,
        symbol: "STF_74",
        name: "Chassis Control",
    },
    Module {
        address: 0x75,
        symbol: "NOT_75",
        name: "Telematics Communication Unit",
    },
    Module {
        address: 0x76,
        symbol: "EPH_76",
        name: "Parking Assistance",
    },
    Module {
        address: 0x77,
        symbol: "TEL_77",
        name: "Telephone",
    },
    Module {
        address: 0x78,
        symbol: "SBT_78",
        name: "Sliding Door Right",
    },
    Module {
        address: 0x80,
        symbol: "BR1_80",
        name: "Battery Regulation Auxiliary Accumulator 1",
    },
    Module {
        address: 0x81,
        symbol: "WHB_81",
        name: "Gear Shift Control Module",
    },
    Module {
        address: 0x82,
        symbol: "HUD_82",
        name: "Head Up Display",
    },
    Module {
        address: 0x84,
        symbol: "WBK_84",
        name: "Night Vision",
    },
    Module {
        address: 0x85,
        symbol: "KST_85",
        name: "On-Board Camera",
    },
    Module {
        address: 0x87,
        symbol: "MMPC906",
        name: "Lane Departure Warning System LT3",
    },
    Module {
        address: 0x88,
        symbol: "MKF_88",
        name: "Multi Contour Seat Driver Side",
    },
    Module {
        address: 0x89,
        symbol: "MKB_89",
        name: "Multi Contour Seat Passenger Side",
    },
    Module {
        address: 0x90,
        symbol: "GSR_90",
        name: "Pretensioner Front Right",
    },
    Module {
        address: 0x91,
        symbol: "YMO_01",
        name: "Engine Control Module C1",
    },
    Module {
        address: 0x92,
        symbol: "ASG906",
        name: "Transmission Electronics LT3",
    },
    Module {
        address: 0x93,
        symbol: "YWE_25",
        name: "Immobilizer C1",
    },
    Module {
        address: 0x94,
        symbol: "ARCADE906",
        name: "Airbag LT3",
    },
    Module {
        address: 0x95,
        symbol: "ESP906",
        name: "Electronic Stability Program LT3",
    },
    Module {
        address: 0x96,
        symbol: "KI906",
        name: "Dash Board LT3",
    },
    Module {
        address: 0x97,
        symbol: "MTCO906",
        name: "Tachograph LT3",
    },
    Module {
        address: 0x98,
        symbol: "TPM906",
        name: "Tire Pressure Monitoring LT3",
    },
    Module {
        address: 0x99,
        symbol: "EZS_LT3",
        name: "Electronic Ignition Starter Switch LT3",
    },
];

/// The engine ECU, which is what this tool talks to by default.
pub const ENGINE: u8 = 0x01;

/// The gateway, worth a look when the engine will not answer.
pub const GATEWAY: u8 = 0x19;

pub fn by_address(address: u8) -> Option<&'static Module> {
    MODULES.iter().find(|m| m.address == address)
}

/// `01`, or `01 Engine Control Module 1` when the address is in the table.
pub fn describe(address: u8) -> String {
    match by_address(address) {
        Some(module) => format!("{address:02X} {}", module.name),
        None => format!("{address:02X}"),
    }
}

/// Resolve what the user typed: a name or symbol, `0x19`, or a decimal
/// number. Bare numbers stay decimal so `-m 1` keeps meaning the engine;
/// every VAG tool writes addresses in hex, so prefer a name or `0x19`.
pub fn parse(text: &str) -> Result<u8, String> {
    let trimmed = text.trim();
    if let Some(hex) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        return u8::from_str_radix(hex, 16).map_err(|e| format!("{trimmed}: {e}"));
    }
    if let Ok(number) = trimmed.parse::<u8>() {
        return Ok(number);
    }

    let wanted = trimmed.to_ascii_lowercase();
    // "engine" and "gateway" are what people actually type; the full names
    // and VAG's symbols work too.
    let found = MODULES.iter().find(|m| {
        m.symbol.to_ascii_lowercase() == wanted
            || m.name.to_ascii_lowercase() == wanted
            || (wanted == "engine" && m.address == ENGINE)
            || (wanted == "gateway" && m.address == GATEWAY)
    });
    match found {
        Some(module) => Ok(module.address),
        None => Err(format!(
            "no module called {trimmed:?}; `vag-tool modules` lists them, \
             or give an address like 0x19"
        )),
    }
}
