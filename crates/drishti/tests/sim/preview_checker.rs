//! Preview-host sims: the scripted headless traversal over seeded sources
//! must satisfy the independent trace checker, and the checker must name
//! each tamper of a valid trace with its exact typed refusal.

use std::io::Write;
use std::slice::SliceIndex;

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

/// Record kinds the random-record sim draws from.
const KINDS: [&str; 3] = ["frame", "summary", "other"];

/// A seeded source text, possibly empty.
fn source_text(random: &mut SimRng) -> String {
    let count = random.below_len(40);
    let mut text = String::new();
    for _ in 0..count {
        if let Some(piece) = PIECES.get(random.below_len(PIECES.len())) {
            text.push_str(piece);
        }
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
    let slot = out.get_mut(at).ok_or("no such line")?;
    *slot = with_member(slot, key, value)?;
    Ok(out)
}

/// The lines in `range`, or a failure naming the short trace.
fn part<R: SliceIndex<[String], Output = [String]>>(
    lines: &[String],
    range: R,
) -> Result<&[String], String> {
    lines
        .get(range)
        .ok_or_else(|| String::from("the trace is too short for this cut"))
}

/// The line at `at`, cloned, or a failure naming the short trace.
fn line_at(lines: &[String], at: usize) -> Result<String, String> {
    lines
        .get(at)
        .cloned()
        .ok_or_else(|| format!("the trace has no line {at}"))
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

/// One valid trace and the positions the tamper cases aim at.
struct Case {
    /// The valid trace.
    lines: Vec<String>,
    /// Source bytes the trace claims.
    len: u64,
    /// Motion mode of the trace.
    reduced: bool,
    /// Index of the summary line.
    last: usize,
    /// Index of the last frame line.
    tail: usize,
    /// Index of the first toward-evidence frame.
    toward: usize,
    /// Index of the first settle frame.
    settle: usize,
}

impl Case {
    /// Trace `source` in one motion mode and locate the phases.
    fn build(source: &str, reduced: bool) -> Result<Self, String> {
        let len = u64::try_from(source.len()).map_err(describe)?;
        let lines = lines_of(trace_for(source, Some("p"), None, reduced)?)?;
        let last = lines.len().saturating_sub(1);
        let tail = last.saturating_sub(1);
        let toward = first_of(&lines, "toward-evidence")?;
        let settle = first_of(&lines, "settle")?;
        Ok(Self {
            lines,
            len,
            reduced,
            last,
            tail,
            toward,
            settle,
        })
    }

    /// Demand one exact verdict on tampered lines.
    fn refuses(&self, label: &str, lines: &[String], want: TraceCheckError) -> Result<(), String> {
        let got = judge(lines, self.len, self.reduced)?;
        let message = format!(
            "{label} (reduced {}): got {got:?}, want {want:?}",
            self.reduced
        );
        ensure(got == Err(want), &message)
    }
}

/// Bug caught: a checker that passes a tampered trace, or that names the wrong
/// defect, so a forged or truncated journey reads as headless evidence.
#[test]
fn sim_preview_checker_names_each_tamper() -> Result<(), String> {
    for reduced in [false, true] {
        let case = Case::build("alpha\nbeta\n", reduced)?;
        tamper_shape(&case)?;
        tamper_frames(&case)?;
        tamper_summary(&case)?;
    }
    Ok(())
}

/// Cuts and kind or mode swaps: the trace is the wrong shape.
fn tamper_shape(case: &Case) -> Result<(), String> {
    let lines = &case.lines;
    case.refuses("no records", &[], TraceCheckError::MissingSummary)?;
    case.refuses(
        "no summary",
        part(lines, ..case.last)?,
        TraceCheckError::MissingSummary,
    )?;
    case.refuses(
        "summary alone",
        part(lines, case.last..)?,
        TraceCheckError::MissingFrames,
    )?;
    let short = [part(lines, ..3)?, part(lines, case.last..)?].concat();
    case.refuses("three frames", &short, TraceCheckError::MissingFrames)?;
    let kind = edit(lines, 2, "kind", "\"note\"")?;
    case.refuses("a note among frames", &kind, TraceCheckError::MissingFrames)?;
    let step = edit(lines, case.toward, "step", "\"sideways\"")?;
    case.refuses(
        "unknown phase",
        &step,
        TraceCheckError::UnknownStep(String::from("sideways")),
    )?;
    let mode = edit(
        lines,
        0,
        "reduced",
        if case.reduced { "false" } else { "true" },
    )?;
    case.refuses("flipped mode", &mode, TraceCheckError::ModeMismatch)
}

/// One tampered number or member of a single frame.
fn tamper_frames(case: &Case) -> Result<(), String> {
    let lines = &case.lines;
    for (key, value) in [
        ("target", "1.0"),
        ("content_revision", "2"),
        ("object_key", "\"other\""),
    ] {
        let drift = edit(lines, case.toward, key, value)?;
        case.refuses(key, &drift, TraceCheckError::IdentityDrift)?;
    }
    let native = edit(lines, 1, "native_presented", "true")?;
    case.refuses("native claim", &native, TraceCheckError::NativeOrBytes)?;
    let bytes = edit(
        lines,
        1,
        "source_bytes",
        &case.len.saturating_add(1).to_string(),
    )?;
    case.refuses("byte count", &bytes, TraceCheckError::NativeOrBytes)?;
    let moved = edit(lines, case.tail, "position", "0.5")?;
    case.refuses(
        "last position",
        &moved,
        TraceCheckError::Trajectory {
            step: String::from("settle"),
            index: case.tail,
        },
    )?;
    let fast = edit(lines, case.toward, "velocity", "0.5")?;
    case.refuses(
        "velocity",
        &fast,
        TraceCheckError::Trajectory {
            step: String::from("toward-evidence"),
            index: case.toward,
        },
    )?;
    let mut missing = lines.clone();
    let fourth = missing.get_mut(3).ok_or("the trace has no line 3")?;
    *fourth = fourth.replacen("\"velocity\"", "\"velocitz\"", 1);
    case.refuses("missing field", &missing, TraceCheckError::MissingFrames)?;
    let typed = edit(lines, 3, "position", "\"x\"")?;
    case.refuses("string position", &typed, TraceCheckError::MissingFrames)
}

/// The summary line and the phase accounting.
fn tamper_summary(case: &Case) -> Result<(), String> {
    let lines = &case.lines;
    let last = case.last;
    for key in ["idle_no_frame", "revocation_refused"] {
        case.refuses(
            key,
            &edit(lines, last, key, "false")?,
            TraceCheckError::IdleBoundary,
        )?;
    }
    let claimed = edit(lines, last, "native_presented", "true")?;
    case.refuses(
        "summary native claim",
        &claimed,
        TraceCheckError::IdleBoundary,
    )?;
    for key in ["prepared", "acknowledged"] {
        let drifted = edit(lines, last, key, &last.saturating_add(1).to_string())?;
        case.refuses(key, &drifted, TraceCheckError::AccountingDrift)?;
    }
    case.refuses(
        "string counter",
        &edit(lines, last, "prepared", "\"x\"")?,
        TraceCheckError::MissingFrames,
    )?;
    let mut twice = lines.clone();
    twice.insert(1, line_at(lines, 0)?);
    case.refuses(
        "a second initial frame",
        &recount(&twice, last.saturating_add(1))?,
        TraceCheckError::PhaseOrder,
    )?;
    let cut = [part(lines, ..case.settle)?, part(lines, last..)?].concat();
    case.refuses(
        "no settle phase",
        &recount(&cut, case.settle)?,
        // A reduced journey holds one frame per phase, so losing one
        // leaves fewer than the four frames any journey needs.
        if case.reduced {
            TraceCheckError::MissingFrames
        } else {
            TraceCheckError::IncompleteJourney
        },
    )?;
    if case.reduced {
        let mut more = lines.clone();
        more.insert(case.toward, line_at(lines, case.toward)?);
        case.refuses(
            "a repeated phase frame",
            &recount(&more, last.saturating_add(1))?,
            TraceCheckError::PhaseCount,
        )?;
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
                    let new_line = line_at(&changed, index)?;
                    if new_line == line_at(&lines, index)? {
                        continue;
                    }
                    let record: Value = lgwks_std::json::from_str(&new_line).map_err(describe)?;
                    let slot = records
                        .get_mut(index)
                        .ok_or("the trace has no such record")?;
                    let original = std::mem::replace(slot, record);
                    tried = tried.saturating_add(1);
                    ensure(
                        check_records(&records, len, reduced).is_err(),
                        &format!("reduced {reduced}: frame {index} {key}={value} was accepted"),
                    )?;
                    let slot = records
                        .get_mut(index)
                        .ok_or("the trace has no such record")?;
                    *slot = original;
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
            let kind = *KINDS
                .get(random.below_len(KINDS.len()))
                .ok_or("kind index out of range")?;
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
        } else if kinds.len() < 5 || kinds.iter().rev().skip(1).any(|kind| *kind != "frame") {
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
        let accepted = buffer.get(..take).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "take past the buffer")
        })?;
        self.taken.extend_from_slice(accepted);
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
