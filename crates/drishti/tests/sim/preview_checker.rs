//! Preview-host sims: the scripted headless traversal over seeded sources
//! must satisfy the independent trace checker, and the checker must name
//! each tamper of a valid trace with its exact typed refusal.

use std::io::Write;

use drishti::canvas_scene::{Action, Artifact, Canvas, Delta, Limits};
use drishti::cli::Failure;
use drishti::preview::{TraceCheckError, check_records, run_traversal};
use lgwks_std::json::Value;

use crate::motion_kit::{describe, ensure, with_member};
use crate::rng::SimRng;

/// Pieces a seeded source is cut from: ASCII, accents, wide scripts, blank
/// lines and tabs.
const PIECES: &[&str] = &[
    "alpha ",
    "beta\n",
    "\u{e9}t\u{e9} ",
    "\u{65e5}\u{672c}\u{8a9e}\n",
    "\n",
    "\t",
    "x",
    "line two\n",
];

/// A seeded source text, possibly empty.
fn source_text(random: &mut SimRng) -> String {
    let count = random.below_len(40);
    let mut text = String::new();
    for _ in 0..count {
        text.push_str(PIECES[random.below_len(PIECES.len())]);
    }
    text
}

/// Run the headless traversal over one source and return its JSONL bytes.
fn trace_for(
    source: &str,
    proposal: Option<&str>,
    evidence: Option<&str>,
    reduced: bool,
) -> Result<Vec<u8>, String> {
    let limits = Limits::default();
    let artifact = Artifact::new(
        crate::fixtures::WORK_OBJECT,
        1,
        source,
        proposal,
        evidence,
        limits,
    )
    .map_err(describe)?;
    let mut canvas = Canvas::new(
        crate::fixtures::session("local-preview")?,
        1,
        artifact,
        limits,
    )
    .map_err(describe)?;
    if reduced {
        canvas
            .enqueue(Delta::new(
                canvas.session().clone(),
                1,
                0,
                Action::ReducedMotion(true),
            ))
            .map_err(describe)?;
    }
    let mut bytes = Vec::new();
    run_traversal(canvas, &mut bytes).map_err(describe)?;
    Ok(bytes)
}

/// The lines of a trace.
fn lines_of(bytes: Vec<u8>) -> Result<Vec<String>, String> {
    Ok(String::from_utf8(bytes)
        .map_err(describe)?
        .lines()
        .map(String::from)
        .collect())
}

/// Parse trace lines into records.
fn records_of(lines: &[String]) -> Result<Vec<Value>, String> {
    let mut records = Vec::new();
    for line in lines {
        let record: Value = lgwks_std::json::from_str(line).map_err(describe)?;
        records.push(record);
    }
    Ok(records)
}

/// The checker's verdict on trace lines.
fn judge(
    lines: &[String],
    source_bytes: u64,
    reduced: bool,
) -> Result<Result<usize, TraceCheckError>, String> {
    Ok(check_records(&records_of(lines)?, source_bytes, reduced))
}

/// Bug caught: a traversal whose frames the independent analytic checker
/// rejects (a spring that drifts, a frame that claims native presentation, a
/// wrong byte count, a lost phase), or one whose bytes differ between runs.
#[test]
fn sim_preview_traversal_satisfies_the_checker_for_seeded_sources() -> Result<(), String> {
    for seed in 0..40_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(600_000));
        let source = source_text(&mut random);
        let proposal = (random.below(2) == 0).then(|| source_text(&mut random));
        let evidence = (random.below(2) == 0).then(|| source_text(&mut random));
        let source_bytes = u64::try_from(source.len()).map_err(describe)?;
        for reduced in [false, true] {
            let bytes = trace_for(&source, proposal.as_deref(), evidence.as_deref(), reduced)?;
            let again = trace_for(&source, proposal.as_deref(), evidence.as_deref(), reduced)?;
            ensure(
                bytes == again,
                &format!("seed {seed}: two runs wrote different bytes"),
            )?;
            let lines = lines_of(bytes)?;
            let frames = judge(&lines, source_bytes, reduced)?;
            ensure(
                frames == Ok(lines.len().saturating_sub(1)),
                &format!("seed {seed} reduced {reduced}: the checker said {frames:?}"),
            )?;
            ensure(
                judge(&lines, source_bytes, !reduced)? == Err(TraceCheckError::ModeMismatch),
                &format!("seed {seed}: the wrong motion mode was accepted"),
            )?;
            ensure(
                judge(&lines, source_bytes.saturating_add(1), reduced)?
                    == Err(TraceCheckError::NativeOrBytes),
                &format!("seed {seed}: the wrong byte count was accepted"),
            )?;
        }
    }
    Ok(())
}

/// Replace member `key` of line `at` with the raw JSON `value`.
fn edit(lines: &[String], at: usize, key: &str, value: &str) -> Result<Vec<String>, String> {
    let mut out = lines.to_vec();
    let line = out.get(at).ok_or("no such line")?;
    let changed = with_member(line, key, value)?;
    out[at] = changed;
    Ok(out)
}

/// Point the summary's two counters at `frames`.
fn recount(lines: &[String], frames: usize) -> Result<Vec<String>, String> {
    let last = lines.len().saturating_sub(1);
    let first = edit(lines, last, "prepared", &frames.to_string())?;
    edit(&first, last, "acknowledged", &frames.to_string())
}

/// Index of the first frame line of a script phase.
fn first_of(lines: &[String], step: &str) -> Result<usize, String> {
    let needle = format!("\"step\":\"{step}\"");
    lines
        .iter()
        .position(|line| line.contains(&needle))
        .ok_or_else(|| format!("no {step} frame"))
}

/// Expect one verdict on tampered lines.
fn expect(
    label: &str,
    lines: &[String],
    source_bytes: u64,
    reduced: bool,
    want: TraceCheckError,
) -> Result<(), String> {
    let got = judge(lines, source_bytes, reduced)?;
    ensure(
        got == Err(want.clone()),
        &format!("{label} (reduced {reduced}): got {got:?}, want {want:?}"),
    )
}

/// Bug caught: a checker that passes a tampered trace, or that names the wrong
/// defect, so a forged or truncated journey reads as headless evidence.
#[test]
fn sim_preview_checker_names_each_tamper() -> Result<(), String> {
    let source = "alpha\nbeta\n";
    let len = u64::try_from(source.len()).map_err(describe)?;
    for reduced in [false, true] {
        let lines = lines_of(trace_for(source, Some("p"), None, reduced)?)?;
        let last = lines.len().saturating_sub(1);
        let tail = last.saturating_sub(1);
        let toward = first_of(&lines, "toward-evidence")?;
        let settle = first_of(&lines, "settle")?;
        expect(
            "no records",
            &[],
            len,
            reduced,
            TraceCheckError::MissingSummary,
        )?;
        expect(
            "no summary",
            &lines[..last],
            len,
            reduced,
            TraceCheckError::MissingSummary,
        )?;
        expect(
            "summary alone",
            &lines[last..],
            len,
            reduced,
            TraceCheckError::MissingFrames,
        )?;
        let short = [&lines[..3], &lines[last..]].concat();
        expect(
            "three frames",
            &short,
            len,
            reduced,
            TraceCheckError::MissingFrames,
        )?;
        let kind = edit(&lines, 2, "kind", "\"note\"")?;
        expect(
            "a note among frames",
            &kind,
            len,
            reduced,
            TraceCheckError::MissingFrames,
        )?;
        let step = edit(&lines, toward, "step", "\"sideways\"")?;
        expect(
            "unknown phase",
            &step,
            len,
            reduced,
            TraceCheckError::UnknownStep(String::from("sideways")),
        )?;
        let mode = edit(&lines, 0, "reduced", if reduced { "false" } else { "true" })?;
        expect(
            "flipped mode",
            &mode,
            len,
            reduced,
            TraceCheckError::ModeMismatch,
        )?;
        for (key, value) in [
            ("target", "1.0"),
            ("content_revision", "2"),
            ("object_key", "\"other\""),
        ] {
            let drift = edit(&lines, toward, key, value)?;
            expect(key, &drift, len, reduced, TraceCheckError::IdentityDrift)?;
        }
        let native = edit(&lines, 1, "native_presented", "true")?;
        expect(
            "native claim",
            &native,
            len,
            reduced,
            TraceCheckError::NativeOrBytes,
        )?;
        let bytes = edit(
            &lines,
            1,
            "source_bytes",
            &len.saturating_add(1).to_string(),
        )?;
        expect(
            "byte count",
            &bytes,
            len,
            reduced,
            TraceCheckError::NativeOrBytes,
        )?;
        let moved = edit(&lines, tail, "position", "0.5")?;
        expect(
            "last position",
            &moved,
            len,
            reduced,
            TraceCheckError::Trajectory {
                step: String::from("settle"),
                index: tail,
            },
        )?;
        let fast = edit(&lines, toward, "velocity", "0.5")?;
        expect(
            "velocity",
            &fast,
            len,
            reduced,
            TraceCheckError::Trajectory {
                step: String::from("toward-evidence"),
                index: toward,
            },
        )?;
        let renamed = lines[3].replacen("\"velocity\"", "\"velocitz\"", 1);
        let mut missing = lines.clone();
        missing[3] = renamed;
        expect(
            "missing field",
            &missing,
            len,
            reduced,
            TraceCheckError::MissingFrames,
        )?;
        let typed = edit(&lines, 3, "position", "\"x\"")?;
        expect(
            "string position",
            &typed,
            len,
            reduced,
            TraceCheckError::MissingFrames,
        )?;
        for key in ["idle_no_frame", "revocation_refused"] {
            expect(
                key,
                &edit(&lines, last, key, "false")?,
                len,
                reduced,
                TraceCheckError::IdleBoundary,
            )?;
        }
        let claimed = edit(&lines, last, "native_presented", "true")?;
        expect(
            "summary native claim",
            &claimed,
            len,
            reduced,
            TraceCheckError::IdleBoundary,
        )?;
        for key in ["prepared", "acknowledged"] {
            let drifted = edit(&lines, last, key, &last.saturating_add(1).to_string())?;
            expect(
                key,
                &drifted,
                len,
                reduced,
                TraceCheckError::AccountingDrift,
            )?;
        }
        expect(
            "string counter",
            &edit(&lines, last, "prepared", "\"x\"")?,
            len,
            reduced,
            TraceCheckError::MissingFrames,
        )?;
        let mut twice = lines.clone();
        twice.insert(1, lines[0].clone());
        expect(
            "a second initial frame",
            &recount(&twice, last.saturating_add(1))?,
            len,
            reduced,
            TraceCheckError::PhaseOrder,
        )?;
        let cut = [&lines[..settle], &lines[last..]].concat();
        expect(
            "no settle phase",
            &recount(&cut, settle)?,
            len,
            reduced,
            // A reduced journey holds one frame per phase, so losing one
            // leaves fewer than the four frames any journey needs.
            if reduced {
                TraceCheckError::MissingFrames
            } else {
                TraceCheckError::IncompleteJourney
            },
        )?;
        if reduced {
            let mut more = lines.clone();
            more.insert(toward, lines[toward].clone());
            expect(
                "a repeated phase frame",
                &recount(&more, last.saturating_add(1))?,
                len,
                reduced,
                TraceCheckError::PhaseCount,
            )?;
        }
    }
    Ok(())
}

/// Raw replacements per numeric member for the perturbation sweep.
const PERTURBED: &[(&str, &[&str])] = &[
    ("position", &["0.4375", "-3.5", "1e300"]),
    ("velocity", &["0.4375", "-3.5"]),
    ("target", &["0.5", "7.0", "-1.0"]),
    ("content_revision", &["0", "2", "18446744073709551615"]),
    ("source_bytes", &["0", "999999"]),
];

/// Bug caught: a checker blind to one number of one frame, so a single forged
/// position, velocity, target, revision or byte count passes.
#[test]
fn sim_preview_checker_rejects_every_single_number_perturbed() -> Result<(), String> {
    let source = "one\ntwo\nthree\n";
    let len = u64::try_from(source.len()).map_err(describe)?;
    for reduced in [false, true] {
        let lines = lines_of(trace_for(source, None, None, reduced)?)?;
        let mut records = records_of(&lines)?;
        ensure(
            check_records(&records, len, reduced).is_ok(),
            "the clean trace must pass",
        )?;
        let frames = lines.len().saturating_sub(1);
        let mut tried = 0_usize;
        for index in 0..frames {
            for &(key, values) in PERTURBED {
                for &value in values {
                    let changed = edit(&lines, index, key, value)?;
                    if changed[index] == lines[index] {
                        continue;
                    }
                    let record: Value =
                        lgwks_std::json::from_str(&changed[index]).map_err(describe)?;
                    let original = std::mem::replace(&mut records[index], record);
                    tried = tried.saturating_add(1);
                    ensure(
                        check_records(&records, len, reduced).is_err(),
                        &format!("reduced {reduced}: frame {index} {key}={value} was accepted"),
                    )?;
                    records[index] = original;
                }
            }
        }
        ensure(
            tried > frames,
            "the sweep must try several values per frame",
        )?;
    }
    Ok(())
}

/// A random scalar as raw JSON.
fn scalar(random: &mut SimRng) -> String {
    match random.below(6) {
        0 => String::from("true"),
        1 => String::from("null"),
        2 => random.below(5).to_string(),
        3 => String::from("\"frame\""),
        4 => String::from("\"summary\""),
        _ => String::from("1.5"),
    }
}

/// Bug caught: a checker that passes, or fails untyped, on records that are
/// not a trace at all (random members, wrong kinds, short runs).
#[test]
fn sim_preview_checker_refuses_random_records_with_the_right_head_error() -> Result<(), String> {
    let keys = [
        "kind", "step", "position", "velocity", "reduced", "target", "prepared",
    ];
    for seed in 0..300_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(610_000));
        let count = random.below_len(9);
        let mut records = Vec::new();
        let mut kinds: Vec<&str> = Vec::new();
        for _ in 0..count {
            let kind = ["frame", "summary", "other"][random.below_len(3)];
            let mut members = vec![format!("\"kind\":\"{kind}\"")];
            for key in keys.iter().skip(1) {
                if random.below(2) == 0 {
                    members.push(format!("\"{key}\":{}", scalar(&mut random)));
                }
            }
            let record: Value = lgwks_std::json::from_str(&format!("{{{}}}", members.join(",")))
                .map_err(describe)?;
            records.push(record);
            kinds.push(kind);
        }
        let verdict = check_records(&records, 3, random.below(2) == 0);
        let want = if kinds.last() != Some(&"summary") {
            TraceCheckError::MissingSummary
        } else if kinds.len() < 5
            || kinds[..kinds.len().saturating_sub(1)]
                .iter()
                .any(|kind| *kind != "frame")
        {
            TraceCheckError::MissingFrames
        } else {
            ensure(
                verdict.is_err(),
                &format!("seed {seed}: random records passed"),
            )?;
            continue;
        };
        ensure(
            verdict == Err(want.clone()),
            &format!("seed {seed}: got {verdict:?}, want {want:?}"),
        )?;
    }
    Ok(())
}

/// A sink that accepts a budget of bytes and then fails every write.
struct Choke {
    /// Bytes still accepted.
    budget: usize,
    /// Bytes taken so far.
    taken: Vec<u8>,
}

impl Write for Choke {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let take = (self.budget > 0)
            .then_some(buffer.len().min(self.budget))
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "choked"))?;
        self.taken.extend_from_slice(&buffer[..take]);
        self.budget = self.budget.saturating_sub(take);
        Ok(take)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Run the traversal over a fresh scene into a sink.
fn traverse_into(sink: &mut Choke) -> Result<Result<(), Failure>, String> {
    let limits = Limits::default();
    let artifact = Artifact::new(crate::fixtures::WORK_OBJECT, 1, "text", None, None, limits)
        .map_err(describe)?;
    let canvas = Canvas::new(
        crate::fixtures::session("local-preview")?,
        1,
        artifact,
        limits,
    )
    .map_err(describe)?;
    Ok(run_traversal(canvas, sink))
}

/// Bug caught: a full or closed sink swallowed mid-trace, so the host
/// reports success for a trace that was cut short.
#[test]
fn sim_preview_traversal_surfaces_every_sink_failure() -> Result<(), String> {
    let mut full = Choke {
        budget: usize::MAX,
        taken: Vec::new(),
    };
    ensure(
        traverse_into(&mut full)?.is_ok(),
        "an open sink must succeed",
    )?;
    let total = full.taken.len();
    ensure(total > 1_000, "the trace must be long enough to cut")?;
    let mut random = SimRng::seed(620_000);
    let mut budgets = vec![0, 1, total.saturating_sub(1)];
    budgets.extend((0..40).map(|_| random.below_len(total)));
    for budget in budgets {
        let mut sink = Choke {
            budget,
            taken: Vec::new(),
        };
        let outcome = traverse_into(&mut sink)?;
        ensure(
            matches!(outcome, Err(Failure::Io(_) | Failure::Json(_))),
            &format!("budget {budget} of {total}: the cut was reported as {outcome:?}"),
        )?;
        ensure(
            full.taken.starts_with(&sink.taken),
            &format!("budget {budget}: the bytes before the cut differ from the full trace"),
        )?;
    }
    let mut exact = Choke {
        budget: total,
        taken: Vec::new(),
    };
    ensure(
        traverse_into(&mut exact)?.is_ok() && exact.taken == full.taken,
        "a sink with exactly enough room must succeed",
    )
}
