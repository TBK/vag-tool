//! DPF reading and monitoring for the PD/PPD 2.0 TDI (BMN).
//!
//! The procedure this supports, from the Ross-Tech wiki page "Diesel Particle
//! Filter Emergency Regeneration", PD/PPD section:
//!
//! * prerequisites: coolant above 50 C (group 002 field 4), filter load below
//!   the spec VCDS shows (group 075 field 3), electrical consumers on
//! * trigger: Coding-II (11), code 21295, or security access plus a basic
//!   setting - see the README
//! * drive: 30-60 km/h, 1500-2500 rpm in 4th/5th, 15-20 minutes, EGT before
//!   the turbo above 700 C
//! * watch: group 070 fields 1 and 3, group 075 fields 1 to 4

use std::io::Write;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::time::Instant;

use crate::blocks::{self, Cell, RegenStatus};
use crate::kwp2000::Kwp2000Client;
use crate::link::CanLink;
use crate::{Result, blocks::decode_group};

pub const GROUP_ENGINE: u8 = 2;
pub const GROUP_REGEN: u8 = 70;
pub const GROUP_FILTER: u8 = 75;

pub const COOLANT_MIN_C: f64 = 50.0;
pub const EGT_TARGET_C: f64 = 700.0;
/// Ross-Tech quotes 30 g / 40 g for the common-rail engines; the PD page only
/// says "below the spec VCDS shows". Treated as a caution, not as truth.
pub const LOAD_CAUTION_G: f64 = 30.0;
pub const LOAD_STOP_G: f64 = 40.0;

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub at: Duration,
    pub rpm: Option<Cell>,
    pub coolant: Option<Cell>,
    pub regen_status: Option<Cell>,
    pub regen_counter: Option<Cell>,
    pub egt_pre_turbo: Option<Cell>,
    pub egt_pre_dpf: Option<Cell>,
    pub egt_post_dpf: Option<Cell>,
    pub load: Option<Cell>,
}

impl Snapshot {
    pub fn regen(&self) -> RegenStatus {
        self.regen_status
            .map(|cell| blocks::decode_regen_status(cell.value))
            .unwrap_or_default()
    }
}

fn now() -> Duration {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
}

pub async fn read_snapshot<L: CanLink>(
    kwp: &mut Kwp2000Client<L>,
    with_engine: bool,
) -> Result<Snapshot> {
    let mut snap = Snapshot {
        at: now(),
        ..Default::default()
    };

    if with_engine {
        let cells = decode_group(&kwp.read_group(GROUP_ENGINE).await?);
        if cells.len() >= 4 {
            snap.rpm = Some(cells[0]);
            snap.coolant = Some(cells[3]);
        }
    }

    let cells = decode_group(&kwp.read_group(GROUP_REGEN).await?);
    if cells.len() >= 3 {
        snap.regen_status = Some(cells[0]);
        snap.regen_counter = Some(cells[2]);
    }

    let cells = decode_group(&kwp.read_group(GROUP_FILTER).await?);
    if cells.len() >= 4 {
        snap.egt_pre_turbo = Some(cells[0]);
        snap.egt_pre_dpf = Some(cells[1]);
        snap.load = Some(cells[2]);
        snap.egt_post_dpf = Some(cells[3]);
    }
    Ok(snap)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Ok,
    Warn,
    Stop,
}

impl Level {
    pub fn prefix(&self) -> &'static str {
        match self {
            Level::Ok => "  ok ",
            Level::Warn => "warn ",
            Level::Stop => "STOP ",
        }
    }
}

pub fn check_prerequisites(snap: &Snapshot) -> Vec<(Level, String)> {
    let mut out = Vec::new();

    match snap.coolant {
        None => out.push((Level::Warn, "coolant temperature not read".into())),
        Some(cell) if cell.unit != "C" => out.push((
            Level::Warn,
            format!(
                "coolant cell decoded as {}, check group 002 field 4 by hand",
                cell.unit
            ),
        )),
        Some(cell) if cell.value < COOLANT_MIN_C => out.push((
            Level::Stop,
            format!(
                "coolant {:.0} C is below the {COOLANT_MIN_C:.0} C minimum - warm the engine first",
                cell.value
            ),
        )),
        Some(cell) => out.push((Level::Ok, format!("coolant {:.0} C", cell.value))),
    }

    match snap.load {
        None => out.push((Level::Warn, "filter load not read".into())),
        Some(cell) if cell.unit == "g" => {
            if cell.value >= LOAD_STOP_G {
                out.push((
                    Level::Stop,
                    format!(
                        "filter load {:.1} g is at or above {LOAD_STOP_G:.0} g - do not force a \
                         regeneration, the filter needs replacing",
                        cell.value
                    ),
                ));
            } else if cell.value >= LOAD_CAUTION_G {
                out.push((
                    Level::Warn,
                    format!(
                        "filter load {:.1} g is above {LOAD_CAUTION_G:.0} g - compare against the \
                         spec VCDS shows before forcing anything",
                        cell.value
                    ),
                ));
            } else {
                out.push((Level::Ok, format!("filter load {:.1} g", cell.value)));
            }
        }
        Some(cell) => out.push((
            Level::Warn,
            format!(
                "filter load reads {} - compare against the spec VCDS shows",
                blocks::format_cell(&cell)
            ),
        )),
    }

    let regen = snap.regen();
    if regen.forced {
        out.push((Level::Ok, "forced regeneration is already active".into()));
    } else if regen.normal {
        out.push((Level::Ok, "a normal regeneration is running".into()));
    } else {
        out.push((
            Level::Ok,
            format!("no regeneration active (status {})", regen.bits_text()),
        ));
    }
    out
}

pub const CSV_COLUMNS: &str = "time,rpm,coolant_c,regen_bits,forced_regen,normal_regen,\
regen_counter,egt_pre_turbo_c,egt_pre_dpf_c,egt_post_dpf_c,filter_load,filter_load_unit";

fn csv_value(cell: &Option<Cell>) -> String {
    cell.map(|c| format!("{:.2}", c.value)).unwrap_or_default()
}

pub fn csv_row(snap: &Snapshot) -> String {
    let regen = snap.regen();
    format!(
        "{:.2},{},{},{},{},{},{},{},{},{},{},{}",
        snap.at.as_secs_f64(),
        csv_value(&snap.rpm),
        csv_value(&snap.coolant),
        regen.bits_text(),
        u8::from(regen.forced),
        u8::from(regen.normal),
        csv_value(&snap.regen_counter),
        csv_value(&snap.egt_pre_turbo),
        csv_value(&snap.egt_pre_dpf),
        csv_value(&snap.egt_post_dpf),
        csv_value(&snap.load),
        snap.load.map(|c| c.unit).unwrap_or("")
    )
}

fn cell_or_dashes(cell: &Option<Cell>, decimals: usize) -> String {
    match cell {
        Some(cell) => format!("{:.*}", decimals, cell.value),
        None => "----".to_string(),
    }
}

pub fn format_line(snap: &Snapshot, started: Option<Duration>) -> String {
    let regen = snap.regen();
    let flag = if regen.forced {
        "FORCED"
    } else if regen.normal {
        "regen"
    } else {
        "-"
    };
    let elapsed = match started {
        Some(started) => {
            let secs = snap.at.saturating_sub(started).as_secs();
            format!(" {:02}:{:02}", secs / 60, secs % 60)
        }
        None => String::new(),
    };
    format!(
        "{}{}  {flag:>6}  bits {}  rpm {:>5}  cool {:>4}C  EGT {:>4}/{:>4}/{:>4}C  load {:>5}{}  ctr {}",
        clock(snap.at),
        elapsed,
        regen.bits_text(),
        cell_or_dashes(&snap.rpm, 0),
        cell_or_dashes(&snap.coolant, 0),
        cell_or_dashes(&snap.egt_pre_turbo, 0),
        cell_or_dashes(&snap.egt_pre_dpf, 0),
        cell_or_dashes(&snap.egt_post_dpf, 0),
        cell_or_dashes(&snap.load, 1),
        snap.load.map(|c| c.unit).unwrap_or(""),
        cell_or_dashes(&snap.regen_counter, 0),
    )
}

/// Wall clock as HH:MM:SS in UTC, without pulling in a date library.
fn clock(at: Duration) -> String {
    let secs = at.as_secs() % 86_400;
    format!(
        "{:02}:{:02}:{:02}",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

/// Polls the DPF groups and writes a CSV log.
pub struct Monitor {
    pub interval: Duration,
    pub engine_every: u64,
    csv: Option<std::fs::File>,
    pub started: Option<Duration>,
    pub samples: u64,
    pub forced_seen: bool,
    last_engine: (Option<Cell>, Option<Cell>),
}

impl Monitor {
    pub fn new(interval: Duration) -> Self {
        Monitor {
            interval,
            engine_every: 5,
            csv: None,
            started: None,
            samples: 0,
            forced_seen: false,
            last_engine: (None, None),
        }
    }

    pub fn with_csv(mut self, path: impl AsRef<Path>) -> Result<Self> {
        let mut file = std::fs::File::create(path)?;
        writeln!(file, "{CSV_COLUMNS}")?;
        self.csv = Some(file);
        Ok(self)
    }

    pub async fn poll<L: CanLink>(&mut self, kwp: &mut Kwp2000Client<L>) -> Result<Snapshot> {
        let with_engine = self.samples.is_multiple_of(self.engine_every);
        let mut snap = read_snapshot(kwp, with_engine).await?;
        // Group 002 is read less often; keep showing the last values in between.
        if with_engine {
            self.last_engine = (snap.rpm, snap.coolant);
        } else {
            snap.rpm = self.last_engine.0;
            snap.coolant = self.last_engine.1;
        }
        if self.started.is_none() {
            self.started = Some(snap.at);
        }
        self.samples += 1;
        if snap.regen().forced {
            self.forced_seen = true;
        }
        if let Some(file) = self.csv.as_mut() {
            writeln!(file, "{}", csv_row(&snap))?;
            file.flush()?;
        }
        Ok(snap)
    }

    /// Run until `duration` elapses, or until Ctrl-C when it is `None`.
    pub async fn run<L: CanLink>(
        &mut self,
        kwp: &mut Kwp2000Client<L>,
        duration: Option<Duration>,
        mut on_sample: impl FnMut(&Snapshot, Option<Duration>),
    ) -> Result<()> {
        let end = duration.map(|d| Instant::now() + d);
        loop {
            if let Some(end) = end
                && Instant::now() >= end
            {
                return Ok(());
            }
            let tick = Instant::now();
            let snap = self.poll(kwp).await?;
            on_sample(&snap, self.started);

            let elapsed = tick.elapsed();
            let remaining = self.interval.saturating_sub(elapsed);
            if self.wait(kwp, remaining).await? {
                return Ok(());
            }
        }
    }

    /// Sleep between samples, keeping the TP2.0 channel alive. The ECU drops an
    /// idle channel after about a second, so a long interval still needs
    /// channel tests in between. Returns true if Ctrl-C arrived.
    async fn wait<L: CanLink>(
        &mut self,
        kwp: &mut Kwp2000Client<L>,
        total: Duration,
    ) -> Result<bool> {
        let deadline = Instant::now() + total;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(false);
            }
            let slice = remaining.min(Duration::from_millis(300));
            tokio::select! {
                _ = tokio::time::sleep(slice) => {}
                _ = tokio::signal::ctrl_c() => return Ok(true),
            }
            if remaining > Duration::from_millis(300) {
                kwp.channel
                    .keepalive_if_idle(Duration::from_millis(500))
                    .await?;
            }
        }
    }
}
