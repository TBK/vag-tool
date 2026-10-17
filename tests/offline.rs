//! Offline tests: the whole stack against the simulated ECU, no hardware.

use std::time::Duration;

use vag_tool::kwp2000::{
    Kwp2000Client, SESSION_DIAGNOSTIC, build_coding_ii_request, parse_identification,
};
use vag_tool::link::LoopbackLink;
use vag_tool::simecu::{self, SimEcu};
use vag_tool::tp20::{MODULE_ENGINE, Tp20Channel};
use vag_tool::{Error, blocks, dpf, faults, logdecode, modules};

/// A client wired to a simulated ECU running in a background task.
struct Harness {
    kwp: Kwp2000Client<LoopbackLink>,
}

impl Harness {
    async fn start() -> Harness {
        let (tester, ecu_link) = LoopbackLink::pair();
        tokio::spawn(async move {
            let mut ecu = SimEcu::new(ecu_link);
            // Give up once the test stops talking, so the task cannot outlive it.
            let _ = ecu.run(Some(Duration::from_secs(10))).await;
        });

        let mut channel =
            Tp20Channel::new(tester, MODULE_ENGINE).with_timeout(Duration::from_secs(1));
        channel.open().await.expect("channel setup");
        let mut kwp = Kwp2000Client::new(channel);
        kwp.start_diagnostic_session(SESSION_DIAGNOSTIC)
            .await
            .expect("diagnostic session");
        Harness { kwp }
    }
}

#[tokio::test]
async fn channel_uses_the_addresses_the_ecu_handed_out() {
    let h = Harness::start().await;
    assert_eq!(h.kwp.channel.rx_addr(), 0x300);
    assert_eq!(h.kwp.channel.tx_addr(), 0x740);
    assert!(h.kwp.channel.is_open());
}

#[tokio::test]
async fn identification_is_parsed() {
    let mut h = Harness::start().await;
    let body = h.kwp.read_ecu_identification(0x9B).await.unwrap();
    let ident = parse_identification(&body).unwrap();
    assert_eq!(ident.part_number, "03G906016AB");
    assert_eq!(ident.sw_version, "0003");
    assert_eq!(ident.coding_length, Some(3));
    assert_eq!(ident.coding, Some(11));
    assert!(ident.system_description.contains("EDC"));
}

#[tokio::test]
async fn group_75_decodes_to_sane_values() {
    let mut h = Harness::start().await;
    let cells = blocks::decode_group(&h.kwp.read_group(75).await.unwrap());
    assert_eq!(cells.len(), 4);
    assert_eq!(cells[0].unit, "C");
    assert!(cells[0].value > 250.0, "EGT was {}", cells[0].value);
    assert_eq!(cells[2].unit, "g");
    assert!(
        (cells[2].value - 28.0).abs() < 1.0,
        "load {}",
        cells[2].value
    );
}

#[tokio::test]
async fn unknown_group_is_a_negative_response() {
    let mut h = Harness::start().await;
    let err = h.kwp.read_group(200).await.unwrap_err();
    assert!(
        matches!(err, Error::NegativeResponse { code: 0x31, .. }),
        "{err}"
    );
}

#[tokio::test]
async fn snapshot_and_prerequisites() {
    let mut h = Harness::start().await;
    let snap = dpf::read_snapshot(&mut h.kwp, true).await.unwrap();
    assert!((snap.coolant.unwrap().value - 85.0).abs() < 1.0);
    assert!(!snap.regen().forced);
    let levels = dpf::check_prerequisites(&snap);
    assert!(!levels.iter().any(|(level, _)| *level == dpf::Level::Stop));
}

#[tokio::test]
async fn key_is_code_plus_seed() {
    let mut h = Harness::start().await;
    let (seed, key) = h.kwp.security_access(simecu::REGEN_CODE).await.unwrap();
    assert_eq!(seed, simecu::SEED);
    assert_eq!(key, simecu::REGEN_CODE.wrapping_add(simecu::SEED));
}

#[tokio::test]
async fn wrong_code_is_rejected_with_vag_wording() {
    let mut h = Harness::start().await;
    let err = h.kwp.security_access(12345).await.unwrap_err();
    assert!(
        matches!(err, Error::NegativeResponse { code: 0x35, .. }),
        "{err}"
    );
    assert!(err.to_string().contains("incorrect Login code"), "{err}");
}

#[tokio::test]
async fn basic_setting_needs_login_first() {
    let mut h = Harness::start().await;
    let err = h.kwp.prepare_basic_setting(70).await.unwrap_err();
    assert!(err.needs_login(), "{err}");
    assert!(err.to_string().contains("perform Login"), "{err}");
}

#[tokio::test]
async fn basic_setting_runs_the_regeneration() {
    let mut h = Harness::start().await;
    h.kwp.security_access(simecu::REGEN_CODE).await.unwrap();
    h.kwp.prepare_basic_setting(70).await.unwrap();
    h.kwp.start_basic_setting(70).await.unwrap();

    let snap = dpf::read_snapshot(&mut h.kwp, false).await.unwrap();
    assert!(snap.regen().forced);

    h.kwp.stop_basic_setting(70).await.unwrap();
    let snap = dpf::read_snapshot(&mut h.kwp, false).await.unwrap();
    assert!(!snap.regen().forced);
}

#[tokio::test]
async fn start_without_prepare_is_a_sequence_error() {
    let mut h = Harness::start().await;
    h.kwp.security_access(simecu::REGEN_CODE).await.unwrap();
    let err = h.kwp.start_basic_setting(70).await.unwrap_err();
    assert!(
        matches!(err, Error::NegativeResponse { code: 0x24, .. }),
        "{err}"
    );
}

#[tokio::test]
async fn forced_regeneration_shows_up_in_the_monitor() {
    let mut h = Harness::start().await;
    // The simulator accepts the code inside any writing service; on the real
    // ECU this is the captured Coding-II sequence replayed with `raw`.
    let mut request = vec![0x3B, 0x00];
    request.extend_from_slice(&(simecu::REGEN_CODE as u16).to_be_bytes());
    h.kwp.request(&request).await.unwrap();

    let mut monitor = dpf::Monitor::new(Duration::ZERO);
    monitor
        .run(&mut h.kwp, Some(Duration::from_millis(300)), |_, _| {})
        .await
        .unwrap();
    assert!(monitor.forced_seen);
    assert!(monitor.samples > 1, "only {} samples", monitor.samples);

    let snap = dpf::read_snapshot(&mut h.kwp, false).await.unwrap();
    assert!(snap.regen().forced);
    assert!(snap.egt_pre_turbo.unwrap().value > 400.0);
}

#[tokio::test]
async fn channel_survives_a_slow_interval() {
    // A long poll interval must not let the TP2.0 channel go idle.
    let mut h = Harness::start().await;
    let mut monitor = dpf::Monitor::new(Duration::from_millis(1200));
    monitor
        .run(&mut h.kwp, Some(Duration::from_millis(2500)), |_, _| {})
        .await
        .unwrap();
    assert!(monitor.samples >= 2);
    // still usable after the idle gaps
    assert_eq!(
        blocks::decode_group(&h.kwp.read_group(75).await.unwrap()).len(),
        4
    );
}

#[tokio::test]
async fn csv_log_has_a_header_and_rows() {
    let mut h = Harness::start().await;
    let path = std::env::temp_dir().join(format!("vagtool-test-{}.csv", std::process::id()));
    let mut monitor = dpf::Monitor::new(Duration::ZERO).with_csv(&path).unwrap();
    monitor
        .run(&mut h.kwp, Some(Duration::from_millis(200)), |_, _| {})
        .await
        .unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[0].starts_with("time,rpm,coolant_c"));
    assert!(lines.len() > 2, "only {} lines", lines.len());
    std::fs::remove_file(&path).ok();
}

#[tokio::test]
async fn dtc_read_decode_and_clear() {
    let mut h = Harness::start().await;
    let entries = h.kwp.read_dtcs().await.unwrap();
    assert_eq!(entries, vec![(0x41D7, 0x88), (0x4802, 0x02)]);

    let decoded = faults::decode_faults(&entries);
    assert_eq!(decoded[0].code_text(), "P0471");
    assert_eq!(decoded[1].code_text(), "P2002");
    assert!(decoded[0].intermittent());
    assert!(!decoded[1].intermittent());
    assert_eq!(decoded[1].status_text(), Some("lower limit not reached"));

    h.kwp.clear_dtcs().await.unwrap();
    assert!(h.kwp.read_dtcs().await.unwrap().is_empty());
}

#[tokio::test]
async fn freeze_frame_decodes() {
    let mut h = Harness::start().await;
    let frame = faults::decode_freeze_frame(&h.kwp.read_freeze_frame().await.unwrap()).unwrap();
    assert_eq!(frame.priority, Some(2));
    assert_eq!(frame.frequency, Some(5));
    assert_eq!(frame.driving_cycle, Some(0x28));
    assert_eq!(frame.mileage_km, Some(184_233));
    assert_eq!(frame.timestamp.as_deref(), Some("2025-10-14 09:35:12"));
}

// -- pure decoding, no ECU needed ----------------------------------------

#[test]
fn known_formulas() {
    assert!((blocks::decode_cell(0x01, 100, 35).value - 700.0).abs() < 0.01);
    assert_eq!(blocks::decode_cell(0x1A, 10, 100).unit, "C");
    assert!((blocks::decode_cell(0x05, 70, 200).value - 700.0).abs() < 0.01);
}

#[test]
fn unknown_formula_is_flagged_raw() {
    let cell = blocks::decode_cell(0xAB, 0x12, 0x34);
    assert!(!cell.known);
    assert_eq!(cell.value as u16, 0x1234);
    assert!(blocks::format_cell(&cell).contains("formula 0xab"));
}

#[test]
fn sae_codes_unpack_from_the_raw_bytes() {
    assert_eq!(faults::Fault::new(0x4034, 0).code_text(), "P0052");
    assert_eq!(faults::Fault::new(0x4802, 0).code_text(), "P2002");
    assert_eq!(faults::Fault::new(0x4034, 0).text_key(), "P005200");
    assert_eq!(faults::Fault::new(0x4034, 0).vag_code(), 16436);
}

#[test]
fn vag_codes_are_the_raw_value() {
    let fault = faults::Fault::new(281, 0x0B);
    assert!(!fault.is_sae());
    assert_eq!(fault.code_text(), "00281");
    assert_eq!(fault.text_key(), "VAG00281");
    assert_eq!(fault.status_text(), Some("open circuit"));
}

#[test]
fn status_bit_7_is_intermittent() {
    assert!(faults::Fault::new(0x4034, 0x81).intermittent());
    assert!(!faults::Fault::new(0x4034, 0x01).intermittent());
    assert_eq!(faults::Fault::new(0x4034, 0x81).status_code(), 1);
}

/// Write a label table the way `tools/extract-labels` does: a magic first
/// line, then `<lang>\t<key>\t<text>` sorted by the whole line. That layout
/// is the contract between two programs that share no code, so these tests
/// spell it out, and `reads_the_committed_label_table` below reads the real
/// article.
fn label_file(name: &str, records: &[&str]) -> std::path::PathBuf {
    let lang = records
        .first()
        .and_then(|r| r.split('\t').next())
        .unwrap_or("en");
    label_file_named(name, &format!("{lang}.tsv"), records)
}

fn label_file_named(name: &str, file: &str, records: &[&str]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("vagtool-labels-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(file);
    std::fs::write(&path, table_text(records)).unwrap();
    path
}

/// The magic line, a `#vag-lang` line per language with its count - which is
/// where the reader learns both, instead of counting records - then the
/// records. All of it in sorted order, as `extract-labels` writes it.
fn table_text(records: &[&str]) -> String {
    let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for record in records {
        *counts
            .entry(record.split('\t').next().unwrap_or("en"))
            .or_default() += 1;
    }
    let mut lines = vec!["#vag-labels 1".to_string()];
    lines.extend(counts.iter().map(|(l, n)| format!("#vag-lang {l} {n}")));
    lines.extend(records.iter().map(|r| r.to_string()));
    lines.join("\n") + "\n"
}

#[test]
fn description_comes_from_the_label_table_when_present() {
    let path = label_file(
        "present",
        &["en\tP200200\tParticulate Trap Bank 1\\nEfficiency Below Threshold"],
    );
    let table = faults::LabelTable::open(&path).unwrap();
    let fault = faults::Fault::new(0x4802, 0x02);
    let text = fault.describe(table.get(&fault.text_key()).as_deref());
    assert!(
        text.contains("Particulate Trap Bank 1 / Efficiency"),
        "{text}"
    );
    assert!(text.contains("lower limit not reached"), "{text}");
    std::fs::remove_dir_all(path.parent().unwrap()).ok();
}

/// The gateway address said 0x1F until a sourced table showed it does not
/// exist; VAG's own symbol for 0x19 is `DID_19`, Diagnose-Interface fuer
/// Datenbus. Pinned so it cannot drift back.
#[test]
fn the_gateway_is_module_0x19() {
    assert_eq!(vag_tool::tp20::MODULE_GATEWAY, 0x19);
    assert_eq!(modules::by_address(0x19).map(|m| m.name), Some("Gateway"));
    assert_eq!(modules::by_address(0x19).map(|m| m.symbol), Some("DID_19"));
    assert_eq!(modules::by_address(0x1F), None);
}

#[test]
fn a_module_can_be_named_or_numbered() {
    assert_eq!(modules::parse("engine"), Ok(0x01));
    assert_eq!(modules::parse("gateway"), Ok(0x19));
    assert_eq!(modules::parse("Gateway"), Ok(0x19));
    assert_eq!(modules::parse("DID_19"), Ok(0x19));
    assert_eq!(modules::parse("0x19"), Ok(0x19));
    // Bare numbers stay decimal, which is what -m 1 has always meant.
    assert_eq!(modules::parse("1"), Ok(0x01));
    assert_eq!(modules::parse("25"), Ok(25));
    assert_eq!(modules::parse(" engine "), Ok(0x01));

    let error = modules::parse("carburettor").unwrap_err();
    assert!(error.contains("vag-tool modules"), "{error}");

    assert_eq!(modules::describe(0x01), "01 Engine Control Module 1");
    assert_eq!(modules::describe(0xFE), "FE");
}

#[test]
fn works_without_a_label_table() {
    assert_eq!(
        faults::Fault::new(0x4802, 0x00).describe(None),
        "P2002 (VAG 18434) static"
    );
}

#[test]
fn label_table_binary_searches_for_a_key() {
    // Enough neighbours that the search has to walk, and keys of both
    // lengths VAG uses, since a 7-byte key sorts before an 8-byte one with
    // the same prefix.
    let mut records: Vec<String> = (0..500)
        .map(|n| format!("en\tVAG{n:05}\tfault number {n}"))
        .collect();
    records.push("en\tP047100\tExhaust Pressure Sensor \"A\"\\nRange/Performance".to_string());
    records.push("en\tP04710\tseven character key".to_string());
    records.push("en\tUNIT0005\tbar".to_string());
    records.sort();
    let borrowed: Vec<&str> = records.iter().map(String::as_str).collect();
    let path = label_file("search", &borrowed);

    let table = faults::LabelTable::open(&path).unwrap();
    assert_eq!(table.len(), 503);
    assert_eq!(table.lang(), "en");
    assert_eq!(table.get("UNIT0005").as_deref(), Some("bar"));
    assert_eq!(table.get("VAG00000").as_deref(), Some("fault number 0"));
    assert_eq!(table.get("VAG00499").as_deref(), Some("fault number 499"));
    assert_eq!(table.get("P04710").as_deref(), Some("seven character key"));
    // Not in there, before the first key, and after the last.
    assert_eq!(table.get("VAG00500"), None);
    assert_eq!(table.get("AAAAAAAA"), None);
    assert_eq!(table.get("ZZZZZZZZ"), None);

    let fault = faults::Fault::new(0x41D7, 0x08);
    let text = fault.describe(table.get(&fault.text_key()).as_deref());
    assert!(text.contains("Exhaust Pressure Sensor"), "{text}");
    std::fs::remove_dir_all(path.parent().unwrap()).ok();
}

/// The binary search snaps each probe back to the start of a line, which is
/// the one piece of this that could silently return the wrong neighbour. Check
/// it against a plain map over a table big enough to need many probes, with
/// keys of both lengths and gaps everywhere.
#[test]
fn binary_search_agrees_with_a_plain_map() {
    let mut expected = std::collections::HashMap::new();
    let mut records = Vec::new();
    // Every third key missing, so misses land between neighbours, and a
    // handful of 7-character keys, which sort before an 8-character key with
    // the same prefix.
    for n in 0..3000u32 {
        if n % 3 == 0 {
            continue;
        }
        let key = if n % 97 == 0 {
            format!("VAG{:04}", n % 10000)
        } else {
            format!("VAG{n:05}")
        };
        let text = format!("description {n}");
        records.push(format!("en\t{key}\t{text}"));
        expected.insert(key, text);
    }
    records.sort();
    records.dedup();
    let borrowed: Vec<&str> = records.iter().map(String::as_str).collect();
    let path = label_file("agrees", &borrowed);
    let table = faults::LabelTable::open(&path).unwrap();

    // Every key that is in there, and every key that is not.
    for n in 0..3100u32 {
        for key in [format!("VAG{n:05}"), format!("VAG{:04}", n % 10000)] {
            assert_eq!(
                table.get(&key).as_deref(),
                expected.get(&key).map(String::as_str),
                "key {key}"
            );
        }
    }
    // Before the first record, after the last, and a prefix of one.
    assert_eq!(table.get(""), None);
    assert_eq!(table.get("A"), None);
    assert_eq!(table.get("VAG"), None);
    assert_eq!(table.get("ZZZZZZZZ"), None);
    std::fs::remove_dir_all(path.parent().unwrap()).ok();
}

#[test]
fn escapes_come_back_as_the_characters_they_stand_for() {
    let path = label_file(
        "escapes",
        &["en\tVAG00001\tfirst\\nsecond\\ttabbed\\\\backslash\\qunknown"],
    );
    let table = faults::LabelTable::open(&path).unwrap();
    assert_eq!(
        table.get("VAG00001").as_deref(),
        // An unknown escape is left as written rather than swallowed.
        Some("first\nsecond\ttabbed\\backslash\\qunknown")
    );
    std::fs::remove_dir_all(path.parent().unwrap()).ok();
}

#[test]
fn a_table_in_another_language_is_used_anyway() {
    let path = label_file("danish", &["da\tP047100\tUdstodningstryksensor"]);
    let table = faults::LabelTable::open(&path).unwrap();
    assert_eq!(table.lang(), "da");
    assert_eq!(
        table.get("P047100").as_deref(),
        Some("Udstodningstryksensor")
    );
    std::fs::remove_dir_all(path.parent().unwrap()).ok();
}

#[test]
fn english_wins_when_the_table_holds_both() {
    let path = label_file(
        "both",
        &["da\tP047100\tUdstodningstryksensor", "en\tP047100\tExhaust"],
    );
    let table = faults::LabelTable::open(&path).unwrap();
    assert_eq!(table.lang(), "en");
    assert_eq!(table.get("P047100").as_deref(), Some("Exhaust"));
    std::fs::remove_dir_all(path.parent().unwrap()).ok();
}

/// `--labels` takes the directory `extract-labels` writes, and picks English
/// out of it; naming one file directly still works.
#[test]
fn a_directory_of_tables_resolves_to_english() {
    let dir = label_file_named("dir-en", "en.tsv", &["en\tP047100\tExhaust"])
        .parent()
        .unwrap()
        .to_path_buf();
    std::fs::write(
        dir.join("da.tsv"),
        table_text(&["da\tP047100\tUdstodningstryksensor"]),
    )
    .unwrap();

    let from_dir = faults::LabelTable::open(&dir).unwrap();
    assert_eq!(from_dir.lang(), "en");
    assert_eq!(from_dir.get("P047100").as_deref(), Some("Exhaust"));

    let named = faults::LabelTable::open(dir.join("da.tsv")).unwrap();
    assert_eq!(named.lang(), "da");
    assert_eq!(
        named.get("P047100").as_deref(),
        Some("Udstodningstryksensor")
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// Only another language on disk: read it rather than give up.
#[test]
fn a_directory_without_english_falls_back() {
    let dir = label_file_named("dir-da", "da.tsv", &["da\tP047100\tUdstodningstryksensor"])
        .parent()
        .unwrap()
        .to_path_buf();
    let table = faults::LabelTable::open(&dir).unwrap();
    assert_eq!(table.lang(), "da");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_empty_directory_says_what_to_run() {
    let dir = std::env::temp_dir().join(format!("vagtool-emptydir-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let error = faults::LabelTable::open(&dir).unwrap_err();
    assert!(error.to_string().contains("--lang all"), "{error}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_file_that_is_not_a_table_says_so() {
    let dir = std::env::temp_dir().join(format!("vagtool-nottsv-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("en.tsv");
    std::fs::write(&path, r#"{"P047100": "still JSON"}"#).unwrap();
    let error = faults::LabelTable::open(&path).unwrap_err();
    assert!(error.to_string().contains("not a label table"), "{error}");
    std::fs::remove_dir_all(&dir).ok();
}

/// The language and its record count come from the header, so a file without
/// one is refused rather than read as if it were empty.
#[test]
fn a_table_without_a_language_line_is_refused() {
    let dir = std::env::temp_dir().join(format!("vagtool-noheader-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("en.tsv");
    std::fs::write(&path, "#vag-labels 1\nen\tP047100\tExhaust\n").unwrap();
    let error = faults::LabelTable::open(&path).unwrap_err();
    assert!(error.to_string().contains("names no language"), "{error}");
    std::fs::remove_dir_all(&dir).ok();
}

/// The committed `assets/labels/` is what the two programs really have to
/// agree on, so read it if it is there.
#[test]
fn reads_the_committed_label_table() {
    let Some(table) = faults::LabelTable::default_table() else {
        eprintln!("skipped: no {} to read", faults::DEFAULT_LABELS);
        return;
    };
    assert!(table.len() > 70_000, "only {} records", table.len());
    assert_eq!(table.lang(), "en");
    let text = table.get("P047100").expect("P0471 text");
    assert_eq!(text, "Exhaust Pressure Sensor \"A\"\nRange/Performance");
    // A code the package does not have, folded in from assets/labels-extra.tsv.
    assert_eq!(
        table.get("VAG04244").as_deref(),
        Some("Brake temperature management")
    );
    // The five that came as German only read as English here.
    assert_eq!(
        table.get("VAG04238").as_deref(),
        Some("Brake fluid overheating")
    );
    assert_eq!(
        table.get("VAG04241").as_deref(),
        Some("Left rear spoiler motor")
    );
    // Nothing German slipped into the English table with them.
    for key in ["VAG04238", "VAG04239", "VAG04240", "VAG04241", "VAG04242"] {
        let text = table.get(key).expect(key);
        assert!(text.is_ascii(), "{key} is not English: {text:?}");
    }
}

/// The German text is kept as the source had it, under `de`.
#[test]
fn the_german_extras_are_in_the_german_table() {
    let path = std::path::Path::new(faults::DEFAULT_LABELS).join("de.tsv");
    if !path.is_file() {
        eprintln!("skipped: no {} to read", path.display());
        return;
    }
    let table = faults::LabelTable::open(&path).unwrap();
    assert_eq!(table.lang(), "de");
    assert_eq!(
        table.get("VAG04238").as_deref(),
        Some("Überhitzung der Bremsflüssigkeit")
    );
}

#[test]
fn coding_ii_layout() {
    let request =
        build_coding_ii_request(21295, "0003", &hex::decode("059738DE").unwrap(), 12345).unwrap();
    assert_eq!(hex::encode(&request), "3b9a059738de3039303030330300532f");
}

#[test]
fn coding_ii_checks_its_inputs() {
    assert!(build_coding_ii_request(21295, "0003", &[0x05, 0x97], 12345).is_err());
    assert!(build_coding_ii_request(0x800000, "0003", &[0; 4], 12345).is_err());
}

const SNIFF: &str = "\
(1000.000000) can0 200#01C00010000301
(1000.010000) can0 201#00D00003400701
(1000.020000) can0 740#A00F8AFF32FF
(1000.030000) can0 300#A10F8AFF4AFF
(1000.040000) can0 740#1000021089
(1000.050000) can0 300#B1
(1000.060000) can0 300#1000025089
(1000.070000) can0 740#B1
(1000.080000) can0 740#1100042B00532F
(1000.090000) can0 300#B2
(1000.100000) can0 300#1100026B00
(1000.110000) can0 740#B2
";

#[test]
fn log_reassembles_and_flags_the_write() {
    let mut decoder = logdecode::SessionDecoder::new(0x01);
    decoder.feed_lines(SNIFF.lines());
    assert_eq!(decoder.tester_tx, Some(0x740));
    assert_eq!(decoder.ecu_tx, Some(0x300));

    let payloads: Vec<String> = decoder
        .messages
        .iter()
        .map(|m| hex::encode(&m.payload))
        .collect();
    assert!(payloads.contains(&"1089".to_string()));
    assert!(payloads.contains(&"5089".to_string()));

    let writes: Vec<_> = decoder.messages.iter().filter(|m| m.is_write()).collect();
    assert_eq!(writes.len(), 2);
    assert_eq!(hex::encode(&writes[0].payload), "2b00532f");
    assert_eq!(writes[0].direction, logdecode::Direction::Tester);
}

#[test]
fn plain_candump_format() {
    let (_, id, data) = logdecode::parse_frame_line("  can0  740   [5]  10 00 02 21 01").unwrap();
    assert_eq!(id, 0x740);
    assert_eq!(hex::encode(data), "1000022101");
}
