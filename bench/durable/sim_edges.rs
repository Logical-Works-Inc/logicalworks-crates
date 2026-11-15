//! Hostile simulations of the bench's own edges: the numbers it reports, the
//! step log it judges recovery by, the worker pool every run goes through, and
//! the manifest it reads the engine's version from.
//!
//! Each oracle is a model built beside the code or an invariant the output must
//! keep, never the code's own output. One seed drives every input.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use crate::error::BenchError;
use crate::fleet::{TENANTS, pool, pool_over, tenant_of};
use crate::recover::check_steps;
use crate::report::{bot_version, code_lines};
use crate::sim_kit::{Dice, TestResult, Trace, line, replays, text};
use crate::stats::{Summary, micros, parse_ps};
use crate::world::LOGGED_STEPS;

/// The fat finger writes a source file: blank and whitespace-only lines,
/// comments of every kind and indent, and code lines that carry a line
/// comment or an inline block comment past their start, in any order with `\n` or `\r\n` endings. The count the
/// report prints equals the code lines the generator wrote. Catches a counter
/// that drops a line with a trailing comment, counts an indented comment, or
/// counts a CRLF blank as code.
#[test]
fn sim_report_fat_finger_code_lines_count_only_what_was_written_as_code() -> TestResult {
    const NOISE: [&str; 7] = [
        "",
        "   ",
        "\t",
        "// note",
        "    /// doc",
        "//! module",
        "\t// tab",
    ];
    const CODE: [&str; 6] = [
        "let x = 1;",
        "    call(a, b); // trailing",
        "let url = \"http://x\";",
        "}",
        "/* block */ let y = 2;",
        "\tfn f() {}",
    ];
    for seed in 0..2_000_u64 {
        let mut dice = Dice::new(seed);
        let ending = if dice.roll(2)? == 0 { "\n" } else { "\r\n" };
        let (mut source, mut written) = (String::new(), 0_usize);
        for _ in 0..dice.pick(80)? {
            if dice.roll(2)? == 0 {
                source.push_str(NOISE[dice.pick(NOISE.len())?]);
            } else {
                source.push_str(CODE[dice.pick(CODE.len())?]);
                written += 1;
            }
            source.push_str(ending);
        }
        let counted = code_lines(&source);
        ensure(counted == written, || {
            format!("seed {seed}: counted {counted} code lines, {written} were written")
        })?;
    }
    Ok(())
}

/// Fail with `message` unless `holds`.
fn ensure(holds: bool, message: impl FnOnce() -> String) -> TestResult {
    if holds { Ok(()) } else { Err(message()) }
}

/// Shuffle `items` in place on the dice: the wrong-order fool's hand.
fn shuffle<T>(dice: &mut Dice, items: &mut [T]) -> TestResult {
    for at in (1..items.len()).rev() {
        let other = dice.pick(at + 1)?;
        items.swap(at, other);
    }
    Ok(())
}

/// A sample value: tiny, near `u64::MAX`, or anywhere.
fn hostile_value(dice: &mut Dice) -> Result<u64, String> {
    let class = dice.roll(3)?;
    let value = match class {
        0 => dice.roll(5)?,
        1 => {
            let below_top = dice.roll(3)?;
            u64::MAX - below_top
        }
        _ => dice.roll(u64::MAX)?,
    };
    Ok(value)
}

/// Empty, single and giant samples of tiny and near-maximum values, in two
/// orders: the summary is order-blind, ordered, made of observed values, and
/// each percentile is the nearest rank. Catches an interpolated percentile, an
/// off-by-one rank, a summary that reads the input before sorting it, and a
/// wrapping sum near `u64::MAX`.
#[test]
fn sim_stats_giant_and_empty_percentiles_are_ordered_members_of_the_sample() -> TestResult {
    for seed in 0..64_u64 {
        let mut dice = Dice::new(seed);
        let length = match seed {
            0 => 0,
            1 => 1,
            _ => dice.pick(2_000)?,
        };
        let mut samples = Vec::with_capacity(length);
        for _ in 0..length {
            samples.push(hostile_value(&mut dice)?);
        }
        let mut shuffled = samples.clone();
        shuffle(&mut dice, &mut shuffled)?;
        let summary = Summary::of(&mut samples);
        ensure(summary == Summary::of(&mut shuffled), || {
            format!("seed {seed}: two orders of one sample summarise differently")
        })?;
        ensure(summary.samples == length, || {
            format!("seed {seed}: sample count")
        })?;
        let Some(largest) = samples.last() else {
            let zero = (summary.p50, summary.p95, summary.p99, summary.max) == (0, 0, 0, 0);
            ensure(zero, || {
                format!("seed {seed}: an empty sample reads {summary:?}")
            })?;
            continue;
        };
        ensure(summary.max == *largest, || {
            format!("seed {seed}: max is not the largest")
        })?;
        let ordered = summary.p50 <= summary.p95 && summary.p95 <= summary.p99;
        ensure(ordered && summary.p99 <= summary.max, || {
            format!("seed {seed}: percentiles out of order {summary:?}")
        })?;
        for (name, value, per_mille) in [
            ("p50", summary.p50, 500),
            ("p95", summary.p95, 950),
            ("p99", summary.p99, 990),
        ] {
            let need = (length * per_mille).div_ceil(1_000).max(1);
            let at_or_below = samples.iter().filter(|sample| **sample <= value).count();
            let below = samples.iter().filter(|sample| **sample < value).count();
            ensure(samples.binary_search(&value).is_ok(), || {
                format!("seed {seed}: {name} {value} was never observed")
            })?;
            ensure(at_or_below >= need && below < need, || {
                format!("seed {seed}: {name} {value} is not the nearest rank of {length}")
            })?;
        }
    }
    Ok(())
}

/// What `ps -o rss=` text should read as, digit by digit: surrounding Unicode
/// whitespace and one leading `+` allowed, then ASCII digits only, then the
/// product by 1024, each step checked.
fn ps_model(answer: &str) -> Option<u64> {
    let body = answer.trim();
    let digits = match body.strip_prefix('+') {
        Some(rest) => rest,
        None => body,
    };
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let mut kibibytes = 0_u64;
    for byte in digits.bytes() {
        kibibytes = kibibytes
            .checked_mul(10)?
            .checked_add(u64::from(byte - b'0'))?;
    }
    kibibytes.checked_mul(1_024)
}

/// Seeded `ps` answers made of digits, NUL, signs, letters, an Arabic-Indic
/// digit, a zero-width space and an ideographic space, up to thirty
/// characters so twenty-digit overflows come up: the reading equals the model,
/// and every refusal is the typed one. Catches a parse that accepts a
/// non-ASCII digit, wraps on overflow, or panics on odd bytes.
#[test]
fn sim_stats_weird_bytes_parse_ps_matches_a_digit_by_digit_model() -> TestResult {
    const ODD: [char; 9] = [' ', '\t', '\n', '+', '-', 'a', '\0', '\u{0663}', '\u{200b}'];
    const DIGITS: [char; 10] = ['0', '1', '2', '3', '4', '5', '6', '7', '8', '9'];
    let mut trace = Trace::new();
    for seed in 0..4_000_u64 {
        let mut dice = Dice::new(seed);
        let length = dice.pick(30)?;
        let mut answer = String::new();
        if dice.roll(4)? == 0 {
            answer.push('\u{3000}');
        }
        for _ in 0..length {
            if dice.roll(5)? == 0 {
                answer.push(ODD[dice.pick(ODD.len())?]);
            } else {
                answer.push(DIGITS[dice.pick(DIGITS.len())?]);
            }
        }
        let got = match parse_ps(&answer) {
            Ok(bytes) => Some(bytes),
            Err(BenchError::MemoryReading { .. }) => None,
            Err(other) => {
                return Err(format!(
                    "seed {seed}: {answer:?} refused untyped: {}",
                    line(other)
                ));
            }
        };
        let model = ps_model(&answer);
        ensure(got == model, || {
            format!("seed {seed}: {answer:?} read {got:?}, the model says {model:?}")
        })?;
        trace.note(&format!("{got:?}"));
    }
    ensure(trace.hash() != Trace::new().hash(), || {
        "nothing was traced".to_owned()
    })
}

/// Durations a clock fool hands over: small, around the point where whole
/// microseconds stop fitting a `u64`, and anywhere up to `u64::MAX` seconds,
/// with any nanoseconds. Whole microseconds equal the 128-bit reading clamped to
/// `u64::MAX`, and a longer duration never reads fewer. Catches a wrap, a
/// rounding of the sub-second part, and saturation that comes too early.
#[test]
fn sim_stats_clock_fool_micros_saturate_exactly_where_wide_arithmetic_overflows() -> TestResult {
    const EDGE: u64 = u64::MAX / 1_000_000;
    ensure(micros(Duration::MAX) == u64::MAX, || {
        "the largest duration wraps".to_owned()
    })?;
    ensure(micros(Duration::from_nanos(999)) == 0, || {
        "a sub-microsecond duration rounds up".to_owned()
    })?;
    for seed in 0..2_000_u64 {
        let mut dice = Dice::new(seed);
        let mut pair = [Duration::ZERO; 2];
        for slot in &mut pair {
            let secs = match dice.roll(3)? {
                0 => dice.roll(10_000)?,
                1 => EDGE - 1_000 + dice.roll(2_000)?,
                _ => dice.roll(u64::MAX)?,
            };
            *slot = Duration::new(secs, dice.small(1_000_000_000)?);
        }
        for duration in pair {
            let clamped = duration.as_micros().min(u128::from(u64::MAX));
            let wide = u64::try_from(clamped).map_err(text)?;
            let read = micros(duration);
            ensure(read == wide, || {
                format!("seed {seed}: {duration:?} read {read} micros, wide reads {wide}")
            })?;
        }
        let [short, long] = if pair[0] <= pair[1] {
            pair
        } else {
            [pair[1], pair[0]]
        };
        ensure(micros(short) <= micros(long), || {
            format!("seed {seed}: {long:?} reads fewer micros than {short:?}")
        })?;
    }
    Ok(())
}

/// A complete step log for runs `0..runs`, one line per (run, logged step).
fn complete_log(runs: u32) -> Vec<String> {
    let mut lines = Vec::new();
    for ordinal in 0..runs {
        for step in LOGGED_STEPS {
            lines.push(format!("{} {ordinal} {step}", tenant_of(ordinal)));
        }
    }
    lines
}

/// Every run's (tenant, ordinal), as the recovery check is told about them.
fn recovered(runs: u32) -> Vec<(&'static str, u32)> {
    (0..runs)
        .map(|ordinal| (tenant_of(ordinal), ordinal))
        .collect()
}

/// One double-clicker scenario: a shuffled complete log with some lines
/// repeated and blank lines strewn in. The judge counts each repeated triple
/// once, as the model does, and a clean log checks every pair.
fn double_clicked_log(seed: u64) -> Result<u64, String> {
    let mut dice = Dice::new(seed);
    let runs = 1 + dice.small(60)?;
    let mut lines = complete_log(runs);
    let mut repeated = 0_usize;
    for at in 0..lines.len() {
        if dice.roll(8)? == 0 {
            repeated += 1;
            for _ in 0..=dice.roll(3)? {
                let copy = lines[at].clone();
                lines.push(copy);
            }
        }
    }
    for _ in 0..dice.roll(5)? {
        lines.push(["", "   ", "\t"][dice.pick(3)?].to_owned());
    }
    shuffle(&mut dice, &mut lines)?;
    let log = lines.join("\n");
    let mut trace = Trace::new();
    match check_steps(&log, &recovered(runs)) {
        Ok(checked) => {
            let all = u64::from(runs) * 9;
            ensure(repeated == 0 && checked == all, || {
                format!("seed {seed}: {repeated} repeats passed, {checked} of {all} checked")
            })?;
            trace.note(&format!("ok {checked}"));
        }
        Err(BenchError::StepRanTwice { duplicates }) => {
            ensure(duplicates == repeated, || {
                format!("seed {seed}: {duplicates} duplicates judged, {repeated} made")
            })?;
            trace.note(&format!("twice {duplicates}"));
        }
        Err(other) => {
            let said = line(other);
            lgwks_std::trace::debug!(
                seed,
                said = said.as_str(),
                "the step judge answered another error"
            );
            return Err(format!("seed {seed}: {said}"));
        }
    }
    Ok(trace.hash())
}

/// The double-clicker repeats step lines once to four times in any order; one
/// seed replays to one verdict. Catches a judge that counts copies instead of
/// triples, misses a repeat that is not adjacent, or trips on blank lines.
#[test]
fn sim_steps_double_clicker_a_repeated_line_is_one_duplicate_per_triple() -> TestResult {
    for seed in 0..300_u64 {
        replays(double_clicked_log, seed)?;
    }
    Ok(())
}

/// The neighbour relabels one step line with the other tenant: the step counts
/// for nobody, and the judge names the run that lost it. Catches a judge that
/// keys steps by run number alone, which lets one tenant's step stand in for
/// another tenant's run.
#[test]
fn sim_steps_neighbour_a_step_logged_under_the_other_tenant_never_counts() -> TestResult {
    let [alpha, beta] = TENANTS;
    for seed in 0..300_u64 {
        let mut dice = Dice::new(seed);
        let runs = 1 + dice.small(40)?;
        let mut lines = complete_log(runs);
        let stolen = dice.pick(lines.len())?;
        let ordinal = u32::try_from(stolen / LOGGED_STEPS.len()).map_err(text)?;
        let step = LOGGED_STEPS[stolen % LOGGED_STEPS.len()];
        let owner = tenant_of(ordinal);
        let thief = if owner == alpha { beta } else { alpha };
        lines[stolen] = format!("{thief} {ordinal} {step}");
        shuffle(&mut dice, &mut lines)?;
        match check_steps(&lines.join("\n"), &recovered(runs)) {
            Err(BenchError::StepMissing {
                tenant,
                ordinal: lost,
                step: missing,
            }) => {
                ensure(
                    tenant == owner && lost == ordinal && missing == step,
                    || {
                        format!(
                            "seed {seed}: blamed {tenant} {lost} {missing}, lost {owner} {ordinal} {step}"
                        )
                    },
                )?;
            }
            Ok(checked) => {
                return Err(format!(
                    "seed {seed}: a stolen step passed, {checked} checked"
                ));
            }
            Err(other) => return Err(format!("seed {seed}: {}", line(other))),
        }
    }
    Ok(())
}

/// The fat finger writes one bad line into a good log: too few words, too many,
/// a negative, an overflowing, a non-ASCII or a NUL-carrying run number. The
/// judge refuses it typed, quoting that line. Catches a judge that skips a
/// line it cannot read, which would pass a log missing the steps it hid.
#[test]
fn sim_steps_fat_finger_one_bad_line_is_refused_and_quoted() -> TestResult {
    const BAD: [&str; 7] = [
        "alpha 3",
        "alpha 3 s1-intake extra",
        "alpha -1 s1-intake",
        "beta 4294967296 s2-validate",
        "beta \u{0663} s3-outside",
        "alpha 2\0 s4-merge",
        "s9-apply",
    ];
    for seed in 0..300_u64 {
        let mut dice = Dice::new(seed);
        let runs = 1 + dice.small(30)?;
        let mut lines = complete_log(runs);
        let bad = BAD[dice.pick(BAD.len())?];
        let at = dice.pick(lines.len() + 1)?;
        lines.insert(at, bad.to_owned());
        match check_steps(&lines.join("\n"), &recovered(runs)) {
            Err(BenchError::MalformedRecord { file, line: quoted }) => {
                ensure(file == "exec.log" && quoted == bad, || {
                    format!("seed {seed}: quoted {quoted:?} of {file}, the bad line was {bad:?}")
                })?;
            }
            Ok(checked) => return Err(format!("seed {seed}: {bad:?} passed, {checked} checked")),
            Err(other) => return Err(format!("seed {seed}: {}", line(other))),
        }
    }
    Ok(())
}

/// The monkey asks the pool for 0 to 2,000 jobs on 0 to 256 workers, or on
/// `u32::MAX` workers for at most 300 jobs (the host's thread cap bounds that
/// level), through both entry points. Every job number is handed out once and
/// only once. Catches a lost or doubled job at the counter, a pool that hangs
/// on zero jobs or zero workers, and a list pool that maps a number wrongly.
#[test]
fn sim_pool_monkey_every_job_number_is_handed_out_exactly_once() -> TestResult {
    for seed in 0..60_u64 {
        let mut dice = Dice::new(seed);
        let (total, workers) = match dice.roll(4)? {
            0 => (dice.small(4)?, dice.small(3)?),
            1 => (dice.small(300)?, u32::MAX),
            _ => (dice.small(2_001)?, dice.small(257)?),
        };
        let mut done = pool(total, workers, Ok).map_err(line)?;
        done.sort_unstable();
        ensure(done.iter().copied().eq(0..total), || {
            let came = done.len();
            format!("seed {seed}: {total} jobs on {workers} workers came back as {came} results")
        })?;
        let items: Vec<u64> = (0..total).map(|at| u64::from(at) * 7 + 3).collect();
        let mut mapped = pool_over(&items, workers, |item| Ok(*item)).map_err(line)?;
        mapped.sort_unstable();
        ensure(mapped == items, || {
            format!("seed {seed}: the list pool lost or doubled an item")
        })?;
    }
    Ok(())
}

/// The saboteur makes one job of a big pool fail. The pool answers with that
/// job's own error, no job ever ran twice, the failing job ran once, and no
/// job number past the total ran. Catches a pool that swallows the failure,
/// reports another worker's result, or hands out a number twice while stopping.
#[test]
fn sim_pool_saboteur_one_failing_job_is_the_answer_and_nothing_ran_twice() -> TestResult {
    for seed in 0..40_u64 {
        let mut dice = Dice::new(seed);
        let total = 100 + dice.small(9_901)?;
        let workers = 1 + dice.small(64)?;
        let fails = dice.small(total)?;
        let expected = usize::try_from(fails).map_err(text)?;
        let ran: Vec<AtomicU32> = (0..total).map(|_| AtomicU32::new(0)).collect();
        let outcome = pool(total, workers, |ordinal| {
            let slot = usize::try_from(ordinal).ok().and_then(|at| ran.get(at));
            let Some(slot) = slot else {
                return Err(BenchError::OutsideJobList { index: ordinal });
            };
            slot.fetch_add(1, Ordering::SeqCst);
            if ordinal == fails {
                Err(BenchError::StepRanTwice {
                    duplicates: expected,
                })
            } else {
                Ok(ordinal)
            }
        });
        match outcome {
            Err(BenchError::StepRanTwice { duplicates }) if duplicates == expected => {}
            Err(other) => return Err(format!("seed {seed}: answered {}", line(other))),
            Ok(done) => {
                let came = done.len();
                return Err(format!(
                    "seed {seed}: job {fails} failed, {came} results came back"
                ));
            }
        }
        let twice = ran
            .iter()
            .filter(|slot| slot.load(Ordering::SeqCst) > 1)
            .count();
        ensure(twice == 0, || {
            format!("seed {seed}: {twice} jobs ran twice")
        })?;
        let failing = ran.get(expected).map(|slot| slot.load(Ordering::SeqCst));
        ensure(failing == Some(1), || {
            format!("seed {seed}: the failing job {fails} ran {failing:?} times")
        })?;
    }
    Ok(())
}

/// A manifest a fat finger would write: decoy tables carrying their own
/// `version`, look-alike keys, comments and blank lines, and the package table
/// in any position with any spacing round `=`. The reader returns the package
/// version the generator put there, and with no package table refuses typed.
/// Catches a reader that takes a dependency's version, a `versionx` key or a
/// `[package.metadata]` table's.
#[test]
fn sim_report_fat_finger_bot_version_reads_only_the_package_table() -> TestResult {
    const JUNK: [&str; 6] = [
        "",
        "# version = \"6.6.6\"",
        "name = \"lgwks_bot\"",
        "versionx = \"5.5.5\"",
        "version.workspace = true",
        "edition = \"2024\"",
    ];
    const DECOYS: [&str; 3] = [
        "[dependencies]",
        "[package.metadata]",
        "[workspace.package]",
    ];
    const SPACING: [&str; 4] = [
        "version = ",
        "version=",
        "  version   =   ",
        "\tversion =\t",
    ];
    for seed in 0..1_000_u64 {
        let mut dice = Dice::new(seed);
        let version = format!("{}.{}.{}", dice.roll(30)?, dice.roll(30)?, dice.roll(300)?);
        let mut tables: Vec<Vec<String>> = Vec::new();
        for decoy in DECOYS {
            if dice.roll(2)? == 0 {
                let decoy_version = format!("version = \"0.{}.0\"", dice.roll(99)?);
                tables.push(vec![decoy.to_owned(), decoy_version]);
            }
        }
        let with_package = dice.roll(6)? != 0;
        if with_package {
            let mut package = vec!["[package]".to_owned()];
            for _ in 0..dice.roll(4)? {
                package.push(JUNK[dice.pick(JUNK.len())?].to_owned());
            }
            let spacing = SPACING[dice.pick(SPACING.len())?];
            package.push(format!("{spacing}\"{version}\""));
            tables.push(package);
        }
        shuffle(&mut dice, &mut tables)?;
        let manifest = tables.concat().join("\n");
        match (bot_version(&manifest), with_package) {
            (Ok(read), true) => ensure(read == version, || {
                format!("seed {seed}: read {read:?}, the package says {version:?}\n{manifest}")
            })?,
            (Err(BenchError::MalformedRecord { .. }), false) => {}
            (Ok(read), false) => {
                return Err(format!(
                    "seed {seed}: no package table, read {read:?}\n{manifest}"
                ));
            }
            (Err(other), _) => return Err(format!("seed {seed}: {}\n{manifest}", line(other))),
        }
    }
    Ok(())
}
