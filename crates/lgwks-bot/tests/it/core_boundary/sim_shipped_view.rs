//! Hostile seeded sweeps over the shipped view the layer scan reads (#1096).
//!
//! Each sim holds the reader to a model the sim builds itself: which bytes the
//! generator planted as shipped code, as test code or as a comment, and on which
//! line. None compares the reader with its own earlier output.

use std::error::Error;
use std::thread;
use std::time::Duration;

use lgwks_ast::ParseError;

use super::layer_edges::{pick, upward_refusals};
use super::sim_test_cfg::{RT, plant, refusals};
use super::test_cfg::{read, read_within};
use crate::sim::{Rng, Trace};

/// One file the weird-bytes generator wrote: its text, how many shipped `Keep`
/// imports it planted, and the 1-based lines that hold one.
struct Weird {
    text: String,
    keeps: usize,
    keep_lines: Vec<usize>,
}

/// A valid Rust file of shipped imports, test items, comments and strings, cut
/// across lines at random, with LF or CRLF line ends and multi-byte text.
fn weird_file(rng: &mut Rng) -> Weird {
    let eol = if rng.below(2) == 0 { "\n" } else { "\r\n" };
    let mut text = String::new();
    let mut keeps = 0_usize;
    let mut keep_lines = Vec::new();
    for n in 0..rng.between(1, 30) {
        let line = text.matches('\n').count().saturating_add(1);
        let mut needs_eol = false;
        let item = match rng.below(6) {
            0 => {
                keeps = keeps.saturating_add(1);
                if keep_lines.last() != Some(&line) {
                    keep_lines.push(line);
                }
                format!("use crate::task::Keep{n};")
            }
            1 => format!(
                "#[cfg(test)] mod t{n} {{ const S: &str = \"é//}}{{\"; use crate::script::Hide{n}; }}"
            ),
            2 => format!(
                "#[cfg(all(test, feature = \"x\"))]{eol}mod u{n} {{{eol}    use crate::script::Hide{n};{eol}    const R: &str = r#\"}}\"#;{eol}}}"
            ),
            3 => {
                needs_eol = true;
                format!("// \u{1f980} crate::script::Hide{n} ü")
            }
            4 => format!("/* crate::script::Hide{n} /* nested */ é */"),
            _ => format!("const U{n}: &str = \"http://x/é//\";"),
        };
        text.push_str(&item);
        if needs_eol || rng.below(2) == 0 {
            text.push_str(eol);
        } else {
            text.push(' ');
        }
    }
    Weird {
        text,
        keeps,
        keep_lines,
    }
}

#[test]
/// Weird-bytes, wrong-order and giant: CRLF, emoji, nested block comments, `//`
/// in strings, and test items sharing lines with shipped imports. The shipped
/// view keeps every line and every character's place, blanks only test and
/// comment bytes, keeps every planted `Keep` import, holds no `Hide`, and the
/// scan reports exactly the lines the generator planted a `Keep` on.
fn sim_weird_bytes_shipped_view_keeps_every_line_and_only_shipped_code() {
    let mut hashes = Vec::new();
    for seed in [0x1096_E000_u64, 0x1096_E001, 0x1096_E000] {
        let mut rng = Rng::new(seed);
        let mut trace = Trace::new();
        for draw in 0..300_u32 {
            let weird = weird_file(&mut rng);
            let reading = read(&weird.text);
            assert!(
                reading.refused.is_none(),
                "seed {seed:#x} draw {draw}: a valid file was refused:\n{}",
                weird.text
            );
            let planted: Vec<&str> = weird.text.split('\n').collect();
            let shipped: Vec<&str> = reading.shipped.split('\n').collect();
            assert_eq!(
                planted.len(),
                shipped.len(),
                "seed {seed:#x} draw {draw}: lines kept"
            );
            for (was, now) in planted.iter().zip(&shipped) {
                assert_eq!(
                    was.chars().count(),
                    now.chars().count(),
                    "seed {seed:#x} draw {draw}: every character keeps its place"
                );
                assert!(
                    was.chars()
                        .zip(now.chars())
                        .all(|(old, new)| old == new || new == ' '),
                    "seed {seed:#x} draw {draw}: a byte changed into code: {was:?} -> {now:?}"
                );
            }
            assert!(
                !reading.shipped.contains("Hide"),
                "seed {seed:#x} draw {draw}: test or comment text shipped:\n{}",
                reading.shipped
            );
            assert_eq!(
                reading.shipped.matches("use crate::task::Keep").count(),
                weird.keeps,
                "seed {seed:#x} draw {draw}: every shipped import is kept"
            );
            let want: Vec<String> = weird
                .keep_lines
                .iter()
                .map(|line| format!("rt/sync.rs:{line}: rt module names crate::task"))
                .collect();
            let found = upward_refusals("rt/sync.rs", &weird.text);
            assert_eq!(found, want, "seed {seed:#x} draw {draw}:\n{}", weird.text);
            for edge in &found {
                trace.record(edge);
            }
        }
        hashes.push(trace.hash());
    }
    assert_eq!(hashes.first(), hashes.get(2), "the first seed replays");
    assert_ne!(hashes.first(), hashes.get(1), "two seeds draw two traces");
}

/// The line and module of one refusal, `file:line: layer module names crate::m`.
fn line_and_module(edge: &str) -> Option<(usize, &str)> {
    let (_, rest) = edge.split_once(':')?;
    let (line, rest) = rest.split_once(':')?;
    let (_, module) = rest.rsplit_once("crate::")?;
    Some((line.parse().ok()?, module))
}

/// The 1-based count of lines of `text` that end at or before byte `cut`.
fn complete_lines(text: &str, cut: usize) -> usize {
    let mut complete = 0_usize;
    let mut start = 0_usize;
    for (index, line) in text.split('\n').enumerate() {
        let end = start.saturating_add(line.len());
        if end <= cut {
            complete = index.saturating_add(1);
        }
        start = end.saturating_add(1);
    }
    complete
}

#[test]
/// Pulled-plug and saboteur: a planted file cut at a random character, mid
/// import, mid attribute, mid comment or mid module. The scan never invents an
/// edge: every refusal's line names that module. A cut file that still parses
/// reports exactly the shipping imports on its complete lines; one the parser
/// refuses reports every import on them. The same seed replays.
fn sim_pulled_plug_cut_files_never_invent_or_hide_an_edge() -> Result<(), Box<dyn Error>> {
    let mut hashes = Vec::new();
    for seed in [0x1096_F100_u64, 0x1096_F101, 0x1096_F100] {
        let mut rng = Rng::new(seed);
        let mut trace = Trace::new();
        let mut refused = [0_u32; 2];
        for draw in 0..400_u32 {
            let planted = plant(&mut rng, &RT.1)?;
            let mut cuts: Vec<usize> = planted.text.char_indices().map(|(at, _)| at).collect();
            cuts.push(planted.text.len());
            let cut = pick(&mut rng, &cuts)?;
            let prefix = planted
                .text
                .get(..cut)
                .ok_or_else(|| Box::<dyn Error>::from("the cut is not a character boundary"))?;
            let complete = complete_lines(&planted.text, cut);
            let found = upward_refusals(RT.0, prefix);
            let lines: Vec<&str> = prefix.lines().collect();
            for edge in &found {
                let (line, module) = line_and_module(edge)
                    .ok_or_else(|| Box::<dyn Error>::from("a refusal without a line"))?;
                let text = lines.get(line.saturating_sub(1)).copied();
                assert!(
                    text.is_some_and(|text| text.contains(&format!("crate::{module}"))),
                    "seed {seed:#x} draw {draw}: invented {edge}:\n{prefix}"
                );
            }
            let on_complete: Vec<String> = found
                .iter()
                .filter(|edge| line_and_module(edge).is_some_and(|(line, _)| line <= complete))
                .cloned()
                .collect();
            let parsed = read(prefix).refused.is_none();
            let marks = if parsed {
                &planted.edges
            } else {
                &planted.imports
            };
            let kept: Vec<(usize, String)> = marks
                .iter()
                .filter(|mark| mark.0 <= complete)
                .cloned()
                .collect();
            assert_eq!(
                on_complete,
                refusals(RT.0, &kept, 0),
                "seed {seed:#x} draw {draw} (parsed: {parsed}):\n{prefix}"
            );
            if let Some(count) = refused.get_mut(usize::from(!parsed)) {
                *count = count.saturating_add(1);
            }
            trace.record_number("cut", cut);
            for edge in &found {
                trace.record(edge);
            }
        }
        assert!(
            refused.iter().all(|count| *count > 0),
            "seed {seed:#x}: both parsed and refused cuts drawn, {refused:?}"
        );
        hashes.push(trace.hash());
    }
    assert_eq!(hashes.first(), hashes.get(2), "the first seed replays");
    assert_ne!(hashes.first(), hashes.get(1), "two seeds draw two traces");
    Ok(())
}

/// How many tenant threads read at once. Each thread owns its parser, so 100
/// covers every thread nextest gives one test on this host; the scan itself
/// runs on one.
const TENANTS: u64 = 100;

/// One tenant's reads: before each one, a parse forced past a zero deadline on
/// the same thread, then a planted file held to the generator's model.
fn tenant_reads(tenant: u64, nested: &str) -> Result<Vec<String>, String> {
    let mut rng = Rng::new(0x1096_F200 ^ tenant);
    let mut seen = Vec::new();
    for draw in 0..5_u32 {
        let sabotage = read_within(nested, Duration::ZERO);
        assert!(
            matches!(sabotage.refused, Some(ParseError::TimedOut { .. })),
            "tenant {tenant} draw {draw}: the sabotage parse did not time out"
        );
        let planted = plant(&mut rng, &RT.1).map_err(|error| error.to_string())?;
        let found = upward_refusals(RT.0, &planted.text);
        let want = refusals(RT.0, planted.expected(), 0);
        assert_eq!(
            found, want,
            "tenant {tenant} draw {draw}: a read after a timeout:\n{}",
            planted.text
        );
        seen.extend(found);
    }
    Ok(seen)
}

#[test]
/// Saboteur, neighbour and double-clicker: 100 tenant threads at once, each
/// forcing a parse past a zero deadline before every real read on the same
/// thread. Every read after a timeout gives the generator's model, so a timed
/// out parser never leaks state into the next file, and the same seeds give the
/// same trace in tenant order.
fn sim_saboteur_timeouts_on_every_thread_leave_the_next_parse_clean() -> Result<(), Box<dyn Error>>
{
    let nested = format!(
        "fn f() -> u8 {{ {}1{} }}\n",
        "(".repeat(4_000),
        ")".repeat(4_000)
    );
    let nested = nested.as_str();
    let mut hashes = Vec::new();
    for _ in 0..2 {
        let outcomes: Vec<Result<Vec<String>, String>> = thread::scope(|scope| {
            // Every tenant starts before the first join, so all 100 run at once.
            let mut handles = Vec::new();
            for tenant in 0..TENANTS {
                handles.push(scope.spawn(move || tenant_reads(tenant, nested)));
            }
            handles
                .into_iter()
                .map(|handle| match handle.join() {
                    Ok(outcome) => outcome,
                    Err(_) => Err(String::from("a tenant thread panicked")),
                })
                .collect()
        });
        let mut trace = Trace::new();
        for outcome in outcomes {
            for edge in outcome? {
                trace.record(&edge);
            }
        }
        hashes.push(trace.hash());
    }
    assert_eq!(hashes.first(), hashes.get(1), "the seeds replay");
    Ok(())
}
