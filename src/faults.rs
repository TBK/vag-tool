//! VAG fault decoding: codes, status bytes and KWP2000 freeze frames.
//!
//! The packing below is how VAG encodes a fault in the two bytes a KWP2000 or
//! KWP1281 ECU returns, and how the status byte splits. Verified against the published
//! mapping: raw `0x4034` is both VAG 16436 and P0052.
//!
//! Fault descriptions themselves are text tables that live in
//! `assets/labels/<lang>.tsv`, [`LabelTable`]
//! looks keys up in one of those, and everything works without it.

use std::path::Path;

use crate::{Error, Result};

/// Where the tables live once `extract-labels` has put them there: one
/// `<lang>.tsv` per language.
pub const DEFAULT_LABELS: &str = "assets/labels";

/// The language to ask for. A database holding only another one is used
/// anyway: a Danish table beats no descriptions at all.
pub const DEFAULT_LANG: &str = "en";

/// First line of the file `extract-labels` writes. Checking it turns a
/// confusing empty result into a sentence that says what to do.
const MAGIC: &str = "#vag-labels 1";

/// The comment line that names a language and how many records it has, which
/// is what saves counting them. `tools/extract-labels` writes it.
const LANG_PREFIX: &str = "#vag-lang ";

const SAE_LETTERS: [char; 4] = ['P', 'C', 'B', 'U'];

/// VAG's 16 KWP2000 fault types (status & 0x7F), in their wording.
pub const FAULT_STATUS_TEXTS: [&str; 16] = [
    "",
    "upper limit exceeded",
    "lower limit not reached",
    "mechanical malfunction",
    "no signal/communication",
    "no or incorrect basic setting/adaptation",
    "short circuit to B+",
    "short circuit to ground",
    "implausible signal",
    "open circuit/short circuit to ground",
    "open circuit/short circuit to B+",
    "open circuit",
    "electrical error in circuit",
    "please read DTC",
    "faulty",
    "cannot be checked currently",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fault {
    /// The two bytes as the ECU sent them.
    pub raw: u16,
    pub status: u8,
}

impl Fault {
    pub fn new(raw: u16, status: u8) -> Self {
        Fault { raw, status }
    }

    /// Bit 14 set means the code is an SAE code packed into the 16 bits.
    pub fn is_sae(&self) -> bool {
        (self.raw >> 14) == 1
    }

    pub fn code_text(&self) -> String {
        if self.is_sae() {
            let letter = SAE_LETTERS[usize::from((self.raw >> 12) & 3)];
            format!("{letter}{}{:03}", (self.raw >> 10) & 3, self.raw & 0x3FF)
        } else {
            format!("{:05}", self.raw)
        }
    }

    /// The 5-digit number VCDS shows, which is just the raw value.
    pub fn vag_code(&self) -> u16 {
        self.raw
    }

    pub fn status_code(&self) -> u8 {
        self.status & 0x7F
    }

    pub fn intermittent(&self) -> bool {
        self.status >> 7 != 0
    }

    /// Key into the VAG text table.
    pub fn text_key(&self) -> String {
        if self.is_sae() {
            format!("{}00", self.code_text())
        } else {
            format!("VAG{}", self.code_text())
        }
    }

    pub fn status_text(&self) -> Option<&'static str> {
        FAULT_STATUS_TEXTS
            .get(usize::from(self.status_code()))
            .copied()
            .filter(|text| !text.is_empty())
    }

    /// `description` is what [`LabelTable::get`] answered for
    /// [`Self::text_key`], or `None` if there is no table.
    pub fn describe(&self, description: Option<&str>) -> String {
        let mut parts = vec![self.code_text()];
        if self.is_sae() {
            parts.push(format!("(VAG {})", self.vag_code()));
        }
        if let Some(description) = description {
            parts.push(format!("- {}", description.replace('\n', " / ")));
        }
        match self.status_text() {
            Some(detail) => parts.push(format!("[{detail}]")),
            None if self.status_code() != 0 => {
                parts.push(format!("[status type {}]", self.status_code()))
            }
            None => {}
        }
        parts.push(if self.intermittent() {
            "intermittent".to_string()
        } else {
            "static".to_string()
        });
        parts.join(" ")
    }
}

pub fn decode_faults(entries: &[(u16, u8)]) -> Vec<Fault> {
    entries
        .iter()
        .map(|&(code, status)| Fault::new(code, status))
        .collect()
}

/// VAG text table lookups against the sorted file `extract-labels` writes.
///
/// One record per line, `<lang>\t<key>\t<text>`, sorted by the whole line,
/// with `#` comments carrying the provenance at the top. Opening it reads the
/// file and parses those few comment lines; the 72,000 records are never
/// walked, because a lookup is a binary search that snaps each probe back to
/// the start of the line it lands in.
pub struct LabelTable {
    data: Vec<u8>,
    /// Where the records begin, past the comments.
    body: usize,
    lang: String,
    records: usize,
}

impl std::fmt::Debug for LabelTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LabelTable")
            .field("lang", &self.lang)
            .field("records", &self.records)
            .finish_non_exhaustive()
    }
}

impl LabelTable {
    /// Read the table and use [`DEFAULT_LANG`] if it is in there.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_lang(path, DEFAULT_LANG)
    }

    pub fn open_lang(path: impl AsRef<Path>, preferred: &str) -> Result<Self> {
        let path = path.as_ref();
        let owned;
        let path = if path.is_dir() {
            owned = pick_file(path, preferred)?;
            owned.as_path()
        } else {
            path
        };
        let data = std::fs::read(path)?;
        if !data.starts_with(MAGIC.as_bytes()) {
            return Err(Error::Value(format!(
                "{} is not a label table; run `extract-labels <apk>` to write one",
                path.display()
            )));
        }

        // The comments at the top say which languages are in the file and how
        // many records each has, so neither has to be counted.
        let mut langs: Vec<(String, usize)> = Vec::new();
        let mut body = 0;
        for line in data.split(|&b| b == b'\n') {
            if !line.is_empty() && line[0] != b'#' {
                break;
            }
            body += line.len() + 1;
            if let Some(rest) = line.strip_prefix(LANG_PREFIX.as_bytes()) {
                let rest = String::from_utf8_lossy(rest);
                if let Some((lang, count)) = rest.split_once(' ') {
                    langs.push((lang.to_string(), count.trim().parse().unwrap_or(0)));
                }
            }
        }

        let (lang, records) = pick_language(&langs, preferred)
            .ok_or_else(|| Error::Value(format!("{} names no language", path.display())))?;
        let body = body.min(data.len());
        Ok(LabelTable {
            data,
            body,
            lang,
            records,
        })
    }

    /// [`DEFAULT_LABELS`] relative to the working directory, if
    /// `extract-labels` has put a table there. Returns `None` rather than
    /// failing: fault codes still print without descriptions.
    pub fn default_table() -> Option<Self> {
        let path = Path::new(DEFAULT_LABELS);
        if !path.is_dir() {
            return None;
        }
        Self::open(path).ok()
    }

    /// The language actually being read.
    pub fn lang(&self) -> &str {
        &self.lang
    }

    /// Records the file says it holds for that language.
    pub fn len(&self) -> usize {
        self.records
    }

    pub fn is_empty(&self) -> bool {
        self.records == 0
    }

    /// The text for a key such as `P047100` or `VAG00281`.
    pub fn get(&self, key: &str) -> Option<String> {
        let needle = format!("{}\t{}\t", self.lang, key);
        let line = self.find(needle.as_bytes())?;
        let text = line.strip_prefix(needle.as_bytes())?;
        let text = unescape(&String::from_utf8_lossy(text));
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_string())
    }

    /// The first record line that is not less than `needle`, or `None` past
    /// the end. Binary search over a file of lines: probe a byte, snap back
    /// to the start of its line, compare whole lines.
    fn find(&self, needle: &[u8]) -> Option<&[u8]> {
        let body = self.data.get(self.body..)?;
        let mut lo = 0; // start of a line, and never past the answer
        let mut hi = body.len();
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            // Snap back to the line `mid` is inside. Never before `lo`, so
            // the search cannot stall.
            let start = match body[lo..mid].iter().rposition(|&b| b == b'\n') {
                Some(i) => lo + i + 1,
                None => lo,
            };
            let end = line_end(body, start);
            if &body[start..end] < needle {
                // Everything up to and including this line is too small.
                lo = (end + 1).min(hi);
            } else {
                // `start <= mid < hi`, so this always narrows.
                hi = start;
            }
        }
        (lo < body.len()).then(|| &body[lo..line_end(body, lo)])
    }
}

fn line_end(body: &[u8], start: usize) -> usize {
    match body[start..].iter().position(|&b| b == b'\n') {
        Some(i) => start + i,
        None => body.len(),
    }
}

/// Which file in a directory of them: `<preferred>.tsv` if it is there, else
/// whichever comes first, so a directory holding only `da.tsv` still works.
fn pick_file(dir: &Path, preferred: &str) -> Result<std::path::PathBuf> {
    let wanted = dir.join(format!("{preferred}.tsv"));
    if wanted.is_file() {
        return Ok(wanted);
    }
    let mut tables: Vec<std::path::PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "tsv"))
        .collect();
    tables.sort();
    tables.into_iter().next().ok_or_else(|| {
        Error::Value(format!(
            "{} holds no <lang>.tsv; run `extract-labels <apk> --lang all`",
            dir.display()
        ))
    })
}

/// Which language to read: the one asked for, else the one with the most
/// records, so a file holding only `da` is still useful.
fn pick_language(langs: &[(String, usize)], preferred: &str) -> Option<(String, usize)> {
    langs
        .iter()
        .find(|(lang, _)| lang == preferred)
        .or_else(|| langs.iter().max_by_key(|(_, count)| *count))
        .cloned()
}

/// The inverse of the escaping `extract-labels` applies. A trailing lone
/// backslash and an unknown escape are both kept as written.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Fault environment data, as a KWP2000 ECU returns it for local id 0x6C.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FreezeFrame {
    pub priority: Option<u8>,
    pub frequency: Option<u8>,
    pub driving_cycle: Option<u8>,
    pub mileage_km: Option<u32>,
    pub timestamp: Option<String>,
    pub raw: Vec<u8>,
}

impl FreezeFrame {
    pub fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(value) = self.priority {
            out.push(format!("priority            {value}"));
        }
        if let Some(value) = self.frequency {
            out.push(format!("frequency counter   {value}"));
        }
        if let Some(value) = self.driving_cycle {
            out.push(format!("driving cycle       {value}"));
        }
        if let Some(value) = self.mileage_km {
            out.push(format!("mileage             {value} km"));
        }
        if let Some(value) = &self.timestamp {
            out.push(format!("time stamp          {value}"));
        }
        out
    }
}

/// Decode the body of a `21 6C` response (local identifier still first):
///
/// ```text
/// 6C <2 bytes> <priority> <frequency> <cycle> <mileage:3> <1 byte>
///    <packed date/time:5> ...
/// ```
///
/// The two leading bytes and the one before the time stamp are not understood.
pub fn decode_freeze_frame(body: &[u8]) -> Result<FreezeFrame> {
    if body.first() != Some(&crate::kwp2000::LID_FREEZE_FRAME) {
        return Err(Error::Unexpected(format!(
            "not a 0x6C freeze frame: {}",
            crate::tp20::hex_bytes(&body[..body.len().min(4)])
        )));
    }
    let data = &body[1..];
    let mut frame = FreezeFrame {
        raw: body.to_vec(),
        ..Default::default()
    };
    if data.len() < 8 {
        return Ok(frame);
    }

    frame.priority = Some(data[2]);
    frame.frequency = Some(data[3]);
    frame.driving_cycle = Some(data[4]);
    let mileage = u32::from_be_bytes([0, data[5], data[6], data[7]]);
    if mileage > 0 && mileage < 1_000_000 {
        frame.mileage_km = Some(mileage);
    }

    if data.len() >= 14 {
        let packed = data[9..14]
            .iter()
            .fold(0u64, |acc, &b| (acc << 8) | u64::from(b));
        let second = packed & 0x3F;
        let minute = (packed >> 6) & 0x3F;
        let hour = (packed >> 12) & 0x1F;
        let day = (packed >> 17) & 0x1F;
        let month = (packed >> 22) & 0x0F;
        let year = (packed >> 26) & 0x7F;
        if (1..127).contains(&year)
            && (1..13).contains(&month)
            && day > 0
            && hour < 24
            && minute < 60
            && second < 60
        {
            frame.timestamp = Some(format!(
                "{:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}",
                2000 + year
            ));
        }
    }
    Ok(frame)
}
