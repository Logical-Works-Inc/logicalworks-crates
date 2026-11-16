//! Simulations of the durability check on `restate-server --dump-config` and of
//! the config file the driver writes.
//!
//! The reader is a fool who hands the parser every spelling a dump could take:
//! sections in any order, comments, blank lines, CRLF, padding, a decoy key at
//! the top level, a multi-line array, a key twice. The oracle is a decision table
//! written here from the rule, not the parser's own output.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;

use crate::rival_config::{Dump, Ports, config_text, durability, pick_ports};
use crate::rival_error::RivalError;
use crate::sim_kit::{Dice, TestResult, Trace, replays, text};

/// What the rule says a case comes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// Measured: WAL on, fsync on.
    Measured,
    /// A switch is on that turns durability off.
    Off,
    /// A switch is not a boolean.
    NotBoolean,
    /// A key the rule needs is not there.
    Missing,
    /// The provider is not one the rule knows.
    Unknown,
}

/// The values a switch can take in a hostile dump.
const SWITCH: [Option<&str>; 6] = [
    None,
    Some("false"),
    Some("true"),
    Some("False"),
    Some("\"false\""),
    Some("0"),
];

/// How one switch reads, and whether it is on.
fn read(value: Option<&str>) -> Result<Option<bool>, Verdict> {
    match value {
        None => Ok(None),
        Some("true") => Ok(Some(true)),
        Some("false") => Ok(Some(false)),
        Some(_) => Err(Verdict::NotBoolean),
    }
}

/// The reference decision: the rule of the issue, one case at a time.
fn reference(provider: &str, wal: Option<&str>, fsync: Option<&str>) -> Verdict {
    let wal_needed = match provider {
        "replicated" => false,
        "local" => true,
        _ => return Verdict::Unknown,
    };
    match read(wal) {
        Err(verdict) => return verdict,
        Ok(Some(true)) => return Verdict::Off,
        Ok(None) if wal_needed => return Verdict::Missing,
        Ok(_) => {}
    }
    match read(fsync) {
        Err(verdict) => verdict,
        Ok(Some(true)) => Verdict::Off,
        Ok(None) => Verdict::Missing,
        Ok(Some(false)) => Verdict::Measured,
    }
}

/// A refusal as a message.
fn text_of(error: RivalError) -> String {
    format!("{error}")
}

/// What the parser's answer comes to.
fn verdict_of(dump_text: &str) -> Result<Verdict, String> {
    let dump = Dump::parse(dump_text).map_err(text_of)?;
    Ok(match durability(&dump) {
        Ok(_) => Verdict::Measured,
        Err(RivalError::DurabilityOff { .. }) => Verdict::Off,
        Err(RivalError::NotBoolean { .. }) => Verdict::NotBoolean,
        Err(RivalError::DumpMissing { .. }) => Verdict::Missing,
        Err(RivalError::UnknownProvider { .. }) => Verdict::Unknown,
        Err(other) => {
            lgwks_std::trace::debug!(%other, "the parser refused a dump with a variant the oracle does not name");
            return Err(format!("an unexpected refusal: {other}"));
        }
    })
}

/// One line, dressed the way a hostile dump might dress it.
fn dressed(dice: &mut Dice, line: &str) -> Result<String, String> {
    let pad = " ".repeat(dice.pick(3)?);
    let tail = match dice.pick(4)? {
        0 => " # a trailing note",
        1 => "\t# tab then note",
        _ => "",
    };
    let end = if dice.pick(2)? == 0 { "\r\n" } else { "\n" };
    let extra = match dice.pick(5)? {
        0 => "# a comment line\n",
        1 => "\n",
        _ => "",
    };
    Ok(format!("{extra}{pad}{line}{tail}{end}"))
}

/// A switch line, or nothing when the switch is absent.
fn switch_line(key: &str, value: Option<&str>) -> Vec<String> {
    match value {
        Some(found) => vec![format!("{key} = {found}")],
        None => Vec::new(),
    }
}

/// A section: its header and its switches.
fn section(name: &str, wal: Option<&str>, fsync: Option<&str>) -> Vec<String> {
    let mut lines = vec![format!("[{name}]")];
    lines.extend(switch_line("rocksdb-disable-wal", wal));
    lines.extend(switch_line("rocksdb-disable-wal-fsync", fsync));
    lines
}

/// A dump built from one case, its sections shuffled and every line dressed.
fn dump_of(
    dice: &mut Dice,
    provider: &str,
    log: (Option<&str>, Option<&str>),
    local: (Option<&str>, Option<&str>),
) -> Result<String, String> {
    let mut blocks: Vec<Vec<String>> = vec![
        vec![
            "[bifrost]".to_owned(),
            format!("default-provider = \"{provider}\""),
            "seal-retry-interval = \"2s\"".to_owned(),
        ],
        section("log-server", log.0, log.1),
        section("bifrost.local", local.0, local.1),
        vec![
            "[worker.storage]".to_owned(),
            "rocksdb-disable-wal-compression = false".to_owned(),
        ],
        vec!["[ingress]".to_owned(), "kafka-clusters = []".to_owned()],
    ];
    for at in (1..blocks.len()).rev() {
        let other = dice.pick(at + 1)?;
        blocks.swap(at, other);
    }
    let mut out = String::new();
    // Top level first: a decoy fsync that must not count, and a multi-line array.
    for line in [
        "roles = [",
        "\"http-ingress\",",
        "\"log-server\",",
        "]",
        "rocksdb-disable-wal-fsync = true",
        "cluster-name = \"localcluster\"",
    ] {
        out.push_str(&dressed(dice, line)?);
    }
    for block in blocks {
        for line in block {
            out.push_str(&dressed(dice, &line)?);
        }
    }
    Ok(out)
}

/// One switch value, drawn.
fn switch(dice: &mut Dice) -> Result<Option<&'static str>, String> {
    let at = dice.pick(SWITCH.len())?;
    SWITCH.get(at).copied().ok_or_else(|| "the draw left the switch list".to_owned())
}

/// Draw one case and say what the rule and the parser each make of it.
fn one_case(dice: &mut Dice) -> Result<(Verdict, Verdict, String), String> {
    let providers = ["replicated", "local", "kafka", "Replicated"];
    let provider = providers
        .get(dice.pick(providers.len())?)
        .ok_or("the draw left the provider list")?;
    let (log_wal, log_fsync) = (switch(dice)?, switch(dice)?);
    let (local_wal, local_fsync) = (switch(dice)?, switch(dice)?);
    let (wal, fsync) = if *provider == "local" {
        (local_wal, local_fsync)
    } else {
        (log_wal, log_fsync)
    };
    let dump = dump_of(dice, provider, (log_wal, log_fsync), (local_wal, local_fsync))?;
    Ok((reference(provider, wal, fsync), verdict_of(&dump)?, dump))
}

/// The parser and the decision table agree on thousands of hostile dumps, and the
/// draws reach every verdict.
#[test]
fn sim_dump_config_decision_matches_the_reference_table() -> TestResult {
    let mut seen: BTreeMap<String, u32> = BTreeMap::new();
    for seed in 1..=8_u64 {
        let mut dice = Dice::new(seed);
        for round in 0..400 {
            let (want, got, dump) = one_case(&mut dice)?;
            if want != got {
                return Err(format!("seed {seed} round {round}: want {want:?}, got {got:?}\n{dump}"));
            }
            *seen.entry(format!("{want:?}")).or_insert(0) += 1;
        }
    }
    for kind in ["Measured", "Off", "NotBoolean", "Missing", "Unknown"] {
        match seen.get(kind) {
            Some(count) if *count >= 10 => {}
            other => return Err(format!("the draws never reached {kind}: {other:?}")),
        }
    }
    Ok(())
}

/// Same seed, same dumps and same verdicts, run twice.
#[test]
fn sim_dump_cases_replay_from_one_seed() -> TestResult {
    fn scenario(seed: u64) -> Result<u64, String> {
        let mut dice = Dice::new(seed);
        let mut trace = Trace::new();
        for _ in 0..200 {
            let (want, got, dump) = one_case(&mut dice)?;
            trace.note(&format!("{want:?} {got:?} {dump}"));
        }
        Ok(trace.hash())
    }
    for seed in [3, 77, 9_001] {
        replays(scenario, seed)?;
    }
    Ok(())
}

/// A key twice, a section twice and nonsense lines are refused typed, never read
/// past: the first duplicate cannot hide the fsync switch.
#[test]
fn sim_duplicates_and_junk_are_refused_typed() {
    let twice = "[log-server]\nrocksdb-disable-wal-fsync = false\nrocksdb-disable-wal-fsync = true\n";
    assert!(matches!(Dump::parse(twice), Err(RivalError::DumpDuplicate { .. })));
    let reopened = "[log-server]\na = 1\n[bifrost]\nb = 2\n[log-server]\nrocksdb-disable-wal-fsync = true\n";
    assert!(matches!(Dump::parse(reopened), Err(RivalError::DumpDuplicate { .. })));
    for junk in [
        "no equals sign",
        "[unterminated",
        "[[array-of-tables]]",
        "\"quoted\" = 1",
        "key =",
        "= 1",
        "[a=b]",
        "[]",
        "bad key = 1",
    ] {
        let found = Dump::parse(junk);
        assert!(matches!(found, Err(RivalError::DumpMalformed { .. })), "{junk:?}: {found:?}");
    }
}

/// A dump shaped like the real one reads, and the row prints its setting; the
/// inactive provider's switches do not decide it.
#[test]
fn the_real_dump_shape_reads_and_prints_its_setting() -> TestResult {
    let real = "roles = [\n\"http-ingress\",\n\"worker\",\n]\n[bifrost]\ndefault-provider = \"replicated\"\n[bifrost.local]\nrocksdb-disable-wal = false\nrocksdb-disable-wal-fsync = false\n[log-server]\nrocksdb-disable-wal-fsync = false\n";
    let dump = Dump::parse(real).map_err(text_of)?;
    let found = durability(&dump).map_err(text_of)?;
    assert_eq!(found.line(), "wal on, fsync on (log-server, provider replicated)");
    let inactive = real.replace(
        "rocksdb-disable-wal = false\nrocksdb-disable-wal-fsync = false\n[log-server]",
        "rocksdb-disable-wal = true\nrocksdb-disable-wal-fsync = true\n[log-server]",
    );
    assert_ne!(inactive, real);
    let dump = Dump::parse(&inactive).map_err(text_of)?;
    assert!(durability(&dump).is_ok());
    Ok(())
}

/// The config the driver writes: a path with a quote or a control byte is refused
/// whole, any other path is one literal, and every address is loopback.
#[test]
fn sim_config_text_refuses_hostile_paths_and_binds_loopback() -> TestResult {
    let ports = Ports {
        node: 1_000,
        ingress: 2_000,
        admin: 3_000,
        worker: 4_000,
    };
    let hostile = [
        "a'b",
        "line\nbreak",
        "tab\there",
        "nul\0byte",
        "'; bind-address = \"0.0.0.0:1\" #",
    ];
    for path in hostile {
        let refused = config_text(&PathBuf::from(path), &ports);
        assert!(matches!(refused, Err(RivalError::ConfigPath { .. })), "{path:?}");
    }
    let alphabet = ["a", "B", "7", " ", "-", "é", "_"];
    let mut dice = Dice::new(42);
    for _ in 0..200 {
        let mut name = String::new();
        for _ in 0..=dice.pick(12)? {
            name.push_str(alphabet.get(dice.pick(alphabet.len())?).ok_or("the draw left the alphabet")?);
        }
        let path = PathBuf::from(format!("/work/{name}"));
        let written = config_text(&path, &ports).map_err(text_of)?;
        let dump = Dump::parse(&written).map_err(text_of)?;
        assert_eq!(dump.get("", "base-dir"), Some(format!("'{}'", path.display()).as_str()));
        for (section, port) in [("", 1_000), ("ingress", 2_000), ("admin", 3_000)] {
            let bound = dump.get(section, "bind-address").ok_or("no bind-address")?;
            let address: SocketAddr = bound.trim_matches('"').parse().map_err(text)?;
            assert!(address.ip().is_loopback(), "{bound}");
            assert_eq!(address.port(), port);
        }
    }
    Ok(())
}

/// Ports picked by binding are four distinct nonzero ports, every time, however
/// often the double-clicker picks.
#[test]
fn sim_picked_ports_are_distinct_and_nonzero() -> TestResult {
    for _ in 0..50 {
        let ports = pick_ports().map_err(|error| error.line())?;
        let mut all = [ports.node, ports.ingress, ports.admin, ports.worker];
        all.sort_unstable();
        assert!(all.iter().all(|port| *port != 0));
        assert!(all.windows(2).all(|pair| pair[0] != pair[1]), "{all:?}");
    }
    Ok(())
}
