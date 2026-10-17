//! `vag-tool` - KWP2000-over-TP2.0 diagnostics over SocketCAN.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};

use vag_tool::kwp2000::{
    self, Kwp2000Client, SESSION_DIAGNOSTIC, build_coding_ii_request, changes_ecu,
    parse_identification,
};
use vag_tool::link::AdapterLink;
use vag_tool::modules;
use vag_tool::tp20::{Tp20Channel, hex_bytes};
use vag_tool::{Error, Result, blocks, dpf, faults, logdecode, simecu};

#[derive(Parser)]
#[command(
    name = "vag-tool",
    about = "KWP2000-over-TP2.0 diagnostics for VAG PQ35 (A3 8P 2.0 TDI PD/BMN) over SocketCAN",
    version
)]
struct Cli {
    /// SocketCAN interface
    #[arg(short, long, global = true, default_value = "can0")]
    channel: String,

    /// Module to talk to: a name such as `engine` or `gateway`, or an
    /// address such as 0x19 (`vag-tool modules` lists them)
    #[arg(short, long, global = true, default_value_t = modules::ENGINE,
          value_parser = modules::parse)]
    module: u8,

    /// Print every CAN frame
    #[arg(long, global = true)]
    debug: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List the VAG module addresses this tool knows by name
    Modules,

    /// Open a session and read ECU identification
    Info {
        /// Also print the raw bytes
        #[arg(long)]
        raw: bool,
    },
    /// Read the DPF groups once and check the regeneration prerequisites
    Status,
    /// Poll the DPF groups continuously during the drive
    Monitor {
        /// Write samples to this file
        #[arg(long)]
        csv: Option<PathBuf>,
        /// Seconds between samples
        #[arg(long, default_value_t = 1.0)]
        interval: f64,
        /// Stop after this many seconds
        #[arg(long)]
        duration: Option<f64>,
    },
    /// Read arbitrary measuring block groups
    Blocks {
        #[arg(required = true, value_parser = parse_u8)]
        group: Vec<u8>,
        /// Keep reading until Ctrl-C
        #[arg(long)]
        watch: bool,
    },
    /// Read stored fault codes
    Dtc {
        /// A label table, i.e. what tools/extract-labels writes
        #[arg(long)]
        labels: Option<PathBuf>,
    },
    /// Read fault environment data (mileage, date, counters)
    FreezeFrame {
        #[arg(long)]
        raw: bool,
    },
    /// Clear the fault memory
    ClearDtc,
    /// Send one raw KWP2000 request, e.g. `raw '21 46'`
    Raw {
        hex: String,
        /// Allow a service that is not a known read-only one
        #[arg(long)]
        write: bool,
    },
    /// Log in with a numeric code (VCDS function 16)
    SecurityAccess { code: u32 },
    /// Run a basic setting group (VCDS function 04)
    BasicSettings {
        #[arg(value_parser = parse_u8)]
        group: u8,
        /// Log in with this code first
        #[arg(long)]
        code: Option<u32>,
        /// Stop the group instead of starting it
        #[arg(long)]
        stop: bool,
        /// Poll the DPF groups once it is running
        #[arg(long)]
        monitor: bool,
        /// Write monitor samples to this file
        #[arg(long)]
        csv: Option<PathBuf>,
        #[arg(long, default_value_t = 1.0)]
        interval: f64,
    },
    /// Assemble a Coding-II write (VCDS function 11)
    Coding2 {
        /// The code to write, e.g. 21295
        #[arg(long)]
        code: u32,
        /// The ECU's own software version string, from `info`
        #[arg(long)]
        sw_version: String,
        /// The 4 bytes after 9A, taken from a capture of this ECU
        #[arg(long)]
        prefix: String,
        /// Workshop code
        #[arg(long, default_value_t = 12345)]
        wsc: u16,
        /// Actually send it
        #[arg(long)]
        write: bool,
    },
    /// Reassemble a sniffed candump log into KWP2000 messages
    DecodeLog {
        path: PathBuf,
        /// Show only messages that are not a known read-only service
        #[arg(long)]
        only_writes: bool,
    },
    /// Run a fake ECU on a vcan interface
    Simecu,
}

fn parse_u8(value: &str) -> std::result::Result<u8, String> {
    let parsed = if let Some(hex) = value.strip_prefix("0x") {
        u8::from_str_radix(hex, 16)
    } else {
        value.parse()
    };
    parsed.map_err(|e| format!("{value}: {e}"))
}

fn parse_hex(value: &str) -> Result<Vec<u8>> {
    let cleaned: String = value
        .replace("0x", "")
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .collect();
    hex::decode(&cleaned).map_err(|e| Error::Value(format!("{value:?} is not hex: {e}")))
}

/// Open the interface, a TP2.0 channel and a diagnostic session.
async fn connect(cli: &Cli) -> Result<Kwp2000Client<AdapterLink>> {
    let link = AdapterLink::open(&cli.channel).map_err(|e| {
        Error::Can(format!(
            "{e}\nhint: bring the interface up first, e.g.\n  \
             sudo ip link set {} type can bitrate 500000\n  \
             sudo ip link set {} up",
            cli.channel, cli.channel
        ))
    })?;
    let mut channel = Tp20Channel::new(link, cli.module).with_debug(cli.debug);
    channel.open().await.map_err(|e| {
        Error::Tp20(format!(
            "{e}\nhint: ignition on, the engine ECU is module 0x01, and the bus \
             must be 500 kbit/s"
        ))
    })?;
    let mut kwp = Kwp2000Client::new(channel);
    kwp.start_diagnostic_session(SESSION_DIAGNOSTIC).await?;
    Ok(kwp)
}

#[tokio::main]
async fn main() -> ExitCode {
    // Rust masks SIGPIPE, which turns `vag-tool monitor | head` into a panic
    // on the first write after head exits. Put the default handler back.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };

    let cli = Cli::parse();
    match run(&cli).await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: &Cli) -> Result<ExitCode> {
    match &cli.command {
        Command::Modules => {
            println!("addr  symbol      name");
            for module in modules::MODULES {
                println!(
                    "  {:02X}  {:<10}  {}",
                    module.address, module.symbol, module.name
                );
            }
            println!(
                "\n{} modules. Give -m a name (`engine`), a symbol (`DID_19`) or an \
                 address (`0x19`).",
                modules::MODULES.len()
            );
            Ok(ExitCode::SUCCESS)
        }

        Command::Info { raw } => {
            let mut kwp = connect(cli).await?;
            let body = kwp
                .read_ecu_identification(kwp2000::IDENT_PART_NUMBER)
                .await?;
            println!(
                "module {} on {}",
                modules::describe(cli.module),
                cli.channel
            );
            match parse_identification(&body) {
                Ok(ident) => {
                    for line in ident.lines() {
                        println!("  {line}");
                    }
                    if *raw {
                        println!("  raw                 {}", hex_bytes(&ident.raw));
                    }
                    if !ident.sw_version.is_empty() {
                        println!("\nfor coding2: --sw-version {}", ident.sw_version);
                    }
                }
                Err(_) => println!("  unparsed            {}", hex_bytes(&body)),
            }
            kwp.channel.close().await;
            Ok(ExitCode::SUCCESS)
        }

        Command::Status => {
            let mut kwp = connect(cli).await?;
            let snap = dpf::read_snapshot(&mut kwp, true).await?;
            for group in [dpf::GROUP_ENGINE, dpf::GROUP_REGEN, dpf::GROUP_FILTER] {
                let cells = blocks::decode_group(&kwp.read_group(group).await?);
                println!("{}", blocks::format_group(group, &cells));
            }
            println!();
            let checks = dpf::check_prerequisites(&snap);
            for (level, message) in &checks {
                println!("{}{message}", level.prefix());
            }
            println!();
            let blocked = checks.iter().any(|(level, _)| *level == dpf::Level::Stop);
            if blocked {
                println!("Do not force a regeneration until the above is resolved.");
            } else {
                println!(
                    "Regeneration prerequisites look acceptable. To start one, try \
                     `security-access 21295` then `basic-settings 70`, or the Coding-II \
                     route - see the README."
                );
            }
            kwp.channel.close().await;
            Ok(if blocked {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            })
        }

        Command::Monitor {
            csv,
            interval,
            duration,
        } => {
            let mut kwp = connect(cli).await?;
            print!("polling groups 070 and 075 (Ctrl-C to stop)");
            match csv {
                Some(path) => println!(", logging to {}", path.display()),
                None => println!(),
            }
            let mut monitor = dpf::Monitor::new(Duration::from_secs_f64(*interval));
            if let Some(path) = csv {
                monitor = monitor.with_csv(path)?;
            }
            monitor
                .run(
                    &mut kwp,
                    duration.map(Duration::from_secs_f64),
                    |snap, started| println!("{}", dpf::format_line(snap, started)),
                )
                .await?;
            println!(
                "{} samples, forced regeneration {}",
                monitor.samples,
                if monitor.forced_seen {
                    "seen"
                } else {
                    "never seen"
                }
            );
            kwp.channel.close().await;
            Ok(ExitCode::SUCCESS)
        }

        Command::Blocks { group, watch } => {
            let mut kwp = connect(cli).await?;
            loop {
                for &g in group {
                    match kwp.read_group(g).await {
                        Ok(body) => {
                            println!("{}", blocks::format_group(g, &blocks::decode_group(&body)))
                        }
                        Err(e) => println!("{g:03}  {e}"),
                    }
                }
                if !watch {
                    kwp.channel.close().await;
                    return Ok(ExitCode::SUCCESS);
                }
                println!();
            }
        }

        Command::Dtc { labels } => {
            let table = match labels {
                Some(path) => match faults::LabelTable::open(path) {
                    Ok(table) => Some(table),
                    Err(e) => {
                        eprintln!(
                            "warning: could not read labels from {}: {e}",
                            path.display()
                        );
                        None
                    }
                },
                // `extract-labels` puts a table in assets/; use it if it is
                // there.
                None => faults::LabelTable::default_table(),
            };
            if let Some(table) = table.as_ref().filter(|t| t.lang() != faults::DEFAULT_LANG) {
                eprintln!(
                    "note: the label table holds {}, not {}",
                    table.lang(),
                    faults::DEFAULT_LANG
                );
            }
            let mut kwp = connect(cli).await?;
            let entries = kwp.read_dtcs().await?;
            if entries.is_empty() {
                println!("no stored faults");
            } else {
                println!("{} stored fault(s):", entries.len());
                for fault in faults::decode_faults(&entries) {
                    let description = table.as_ref().and_then(|t| t.get(&fault.text_key()));
                    println!("  {}", fault.describe(description.as_deref()));
                }
                if table.is_none() {
                    println!(
                        "\n(for fault descriptions run `extract-labels <apk>` once, or pass \
                         --labels <dir-or-file>)"
                    );
                }
            }
            kwp.channel.close().await;
            Ok(ExitCode::SUCCESS)
        }

        Command::FreezeFrame { raw } => {
            let mut kwp = connect(cli).await?;
            let body = kwp.read_freeze_frame().await?;
            kwp.channel.close().await;
            let frame = faults::decode_freeze_frame(&body)?;
            let lines = frame.lines();
            if lines.is_empty() {
                println!("empty freeze frame: {}", hex_bytes(&frame.raw));
                return Ok(ExitCode::FAILURE);
            }
            println!("fault environment data (local identifier 0x6C):");
            for line in lines {
                println!("  {line}");
            }
            if *raw {
                println!("  raw                 {}", hex_bytes(&frame.raw));
            }
            Ok(ExitCode::SUCCESS)
        }

        Command::ClearDtc => {
            let mut kwp = connect(cli).await?;
            kwp.clear_dtcs().await?;
            println!("fault memory cleared");
            kwp.channel.close().await;
            Ok(ExitCode::SUCCESS)
        }

        Command::Raw { hex, write } => {
            let payload = parse_hex(hex)?;
            let sid = *payload
                .first()
                .ok_or_else(|| Error::Value("no bytes given".into()))?;
            if changes_ecu(sid) && !write {
                return Err(Error::Value(format!(
                    "service {sid:#04x} is not a known read-only service.\n\
                     Re-run with --write if that is what you mean. Read the README \
                     section on replaying a captured sequence first."
                )));
            }
            let mut kwp = connect(cli).await?;
            let response = kwp.request(&payload).await;
            kwp.channel.close().await;
            println!("-> {}", hex_bytes(&payload));
            println!("<- {}", hex_bytes(&response?));
            Ok(ExitCode::SUCCESS)
        }

        Command::SecurityAccess { code } => {
            let mut kwp = connect(cli).await?;
            let result = kwp.security_access(*code).await;
            kwp.channel.close().await;
            let (seed, key) = result?;
            println!("login accepted with code {code}");
            println!("  seed {seed:#010x}  key {key:#010x}  (key = code + seed)");
            Ok(ExitCode::SUCCESS)
        }

        Command::BasicSettings {
            group,
            code,
            stop,
            monitor,
            csv,
            interval,
        } => {
            let mut kwp = connect(cli).await?;
            if let Some(code) = code {
                let (seed, _) = kwp.security_access(*code).await?;
                println!("login accepted (seed {seed:#010x})");
            }
            if *stop {
                kwp.stop_basic_setting(*group).await?;
                println!("basic setting {group:03} stopped");
                kwp.channel.close().await;
                return Ok(ExitCode::SUCCESS);
            }
            kwp.prepare_basic_setting(*group).await?;
            println!("basic setting {group:03} prepared (31 BA)");
            kwp.start_basic_setting(*group).await?;
            println!("basic setting {group:03} started (31 B9)");

            if *monitor {
                println!("monitoring; Ctrl-C to stop (this does NOT stop the setting)");
                let mut mon = dpf::Monitor::new(Duration::from_secs_f64(*interval));
                if let Some(path) = csv {
                    mon = mon.with_csv(path)?;
                }
                mon.run(&mut kwp, None, |snap, started| {
                    println!("{}", dpf::format_line(snap, started))
                })
                .await?;
            }
            kwp.channel.close().await;
            Ok(ExitCode::SUCCESS)
        }

        Command::Coding2 {
            code,
            sw_version,
            prefix,
            wsc,
            write,
        } => {
            let request = build_coding_ii_request(*code, sw_version, &parse_hex(prefix)?, *wsc)?;
            println!("Coding-II request for code {code}:");
            println!("  {}", hex_bytes(&request));
            if !write {
                println!(
                    "\nNot sent. This writes the ECU's coding; re-run with --write once you \
                     have checked the bytes against a capture from this ECU."
                );
                return Ok(ExitCode::SUCCESS);
            }
            let mut kwp = connect(cli).await?;
            let response = kwp.request(&request).await;
            kwp.channel.close().await;
            println!("accepted: {}", hex_bytes(&response?));
            Ok(ExitCode::SUCCESS)
        }

        Command::DecodeLog { path, only_writes } => {
            let text = std::fs::read_to_string(path)?;
            let mut decoder = logdecode::SessionDecoder::new(cli.module);
            decoder.feed_lines(text.lines());
            if decoder.tester_tx.is_none() {
                println!(
                    "note: no TP2.0 channel setup seen; the capture must include the \
                     0x200 handshake for directions to be known"
                );
            }
            for message in &decoder.messages {
                if *only_writes && !message.is_write() {
                    continue;
                }
                println!("{}", message.format());
            }
            Ok(ExitCode::SUCCESS)
        }

        Command::Simecu => {
            let link = AdapterLink::open(&cli.channel)?;
            let mut ecu = simecu::SimEcu::new(link).with_debug(cli.debug);
            println!(
                "simulated PPD1 ECU on {}: listening for TP2.0 channel setup on 0x200",
                cli.channel
            );
            tokio::select! {
                result = ecu.run(None) => result?,
                _ = tokio::signal::ctrl_c() => println!("\nstopped"),
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}
