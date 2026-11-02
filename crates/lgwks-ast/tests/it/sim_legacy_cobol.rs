//! Seeded adversarial COBOL and JCL with known answers (#777).
//!
//! One seed writes whole programs and jobs from clean and broken fragments:
//! divisions, paragraphs, every recorded statement, COPY cards with and
//! without a resolver entry, truncated and mutated inputs, and JCL with
//! continuations, symbolics and in-stream procs. The one parser reads each
//! through the public JSON verdicts: no panic on any input, a typed error or
//! a value on every one, the same seed replaying the same trace while
//! distinct seeds diverge. Known answers pin hand-written GnuCOBOL-style
//! programs: exact PERFORM and CALL edges, exact read and write sets, DISP
//! directions, symbolics, COPY resolution and REPLACING, and every summary
//! sentence checked word by word against the source lines it rests on.

#![cfg(feature = "legacy")]

use std::error::Error;

use lgwks_ast::legacy::LegacyLimits;
use lgwks_ast::legacy::cobol::parse_program;
use lgwks_ast::legacy::error::LegacyError;
use lgwks_ast::legacy::jcl::parse_job;
use lgwks_ast::legacy::json::{job_json, program_json};
use lgwks_std::hash::{Digest, blake3};
use lgwks_std::json::Value;
use lgwks_std::seeded::Seeded;

/// What every test returns: a refusal reports itself.
type TestResult = Result<(), Box<dyn Error>>;

/// The seeds the COBOL and JCL sweeps replay, named so a failure names its
/// seed: the low edge, a neighbour, all ones in the low word, and 1959.
const SEEDS: [u64; 4] = [
    0x00C0_B01A_0000_0001,
    0x00C0_B01A_0000_0002,
    0x00C0_B01A_FFFF_FFFF,
    0x0000_0000_0000_07A7,
];

/// The bounds the unit sweeps run under: wide enough for the fragments.
const BOUNDS: LegacyLimits =
    LegacyLimits::new(1_048_576, 10_000, 80_000, 80_000, 4_096, 64, 10_000);

/// Appends `bytes` to the transcript a sweep hashes once, at its end, with
/// the estate's BLAKE3 (`lgwks_std::hash`), rather than a hand-rolled hash.
fn record(trace: &mut Vec<u8>, bytes: &[u8]) {
    trace.extend_from_slice(bytes);
}

/// One draw below `bound`.
fn draw(stream: &mut Seeded, bound: u64) -> Result<u64, Box<dyn Error>> {
    Ok(stream.below(bound)?)
}

/// One index into a pool of `len` fragments, genuinely fallible on narrow targets.
fn pick_index(stream: &mut Seeded, len: usize) -> Result<usize, Box<dyn Error>> {
    let bound = u32::try_from(len)?;
    let drawn = stream.below(u64::from(bound))?;
    Ok(usize::try_from(drawn)?)
}

/// One fragment of `pool`, drawn by `stream`.
fn pick<'pool>(stream: &mut Seeded, pool: &[&'pool str]) -> Result<&'pool str, Box<dyn Error>> {
    let at = pick_index(stream, pool.len())?;
    pool.get(at)
        .copied()
        .ok_or_else(|| format!("a draw of {at} names no fragment of {}", pool.len()).into())
}

/// Fixed-format COBOL: six sequence columns, a blank indicator, then the code.
fn fixed(code: &str) -> String {
    format!("       {code}")
}

/// Clean procedure-division lines the seeds assemble.
const CLEAN_COBOL: [&str; 12] = [
    "MAIN-PARA.",
    "READ-LOOP.",
    "OPEN INPUT CUST-FILE.",
    "PERFORM READ-LOOP UNTIL WS-EOF.",
    "PERFORM READ-LOOP THRU READ-EXIT.",
    "CALL \"CALC-TAX\" USING WS-TOTAL.",
    "CALL WS-PROG.",
    "MOVE WS-TOTAL TO WS-REPORT.",
    "ADD 1 TO WS-COUNT.",
    "READ CUST-FILE INTO CUST-REC.",
    "WRITE CUST-REC FROM WS-LINE.",
    "GO TO READ-LOOP.",
];

/// Broken lines: unknown verbs, stray words, and unbalanced fragments.
const BROKEN_COBOL: [&str; 6] = [
    "FROBNICATE THE WIDGET.",
    "PERFORM.",
    "CALL.",
    "MOVE TO NOWHERE.",
    "EXEC SQL SELECT A FROM T",
    "REDEFINES WS-OLD PIC 9(10).",
];

/// Clean JCL cards the seeds assemble.
const CLEAN_JCL: [&str; 9] = [
    "//STEP1    EXEC PGM=PAYROLL",
    "//IN       DD DSN=&HLQ..CUST,DISP=SHR",
    "//OUT      DD DSN=&HLQ..REPORT,DISP=(NEW,CATLG)",
    "//SYS      DD SYSOUT=*",
    "//DATAIN   DD *",
    "hello data",
    "/*",
    "//SETUP    SET HLQ=PROD.MONTH",
    "//NOTE     IF RC = 0 THEN",
];

/// Broken JCL: unknown operations and truncated cards.
const BROKEN_JCL: [&str; 4] = [
    "//STEP1    FROB PGM=PAYROLL",
    "//STEP1    EXEC",
    "not a card at all",
    "//STEP2    EXEC PGM=PAY,",
];

/// Writes one seeded COBOL program: divisions, paragraphs and fragments.
fn write_program(stream: &mut Seeded) -> Result<String, Box<dyn Error>> {
    let mut lines: Vec<String> = Vec::new();
    lines.push(fixed("IDENTIFICATION DIVISION."));
    lines.push(fixed("PROGRAM-ID. SEEDPROG."));
    lines.push(fixed("PROCEDURE DIVISION."));
    let count = usize::try_from(draw(stream, 12)?)?.saturating_add(1);
    for _ in 0..count {
        let broken = draw(stream, 5)? == 0;
        let pool: &[&str] = if broken { &BROKEN_COBOL } else { &CLEAN_COBOL };
        lines.push(fixed(pick(stream, pool)?));
    }
    Ok(lines.join("\n"))
}

/// Writes one seeded JCL job: a JOB card, SET, and fragment cards.
fn write_job(stream: &mut Seeded) -> Result<String, Box<dyn Error>> {
    let mut lines: Vec<String> = Vec::new();
    lines.push(String::from("//SEEDJOB  JOB (ACCT),'SEED',CLASS=A"));
    let count = usize::try_from(draw(stream, 8)?)?.saturating_add(1);
    for _ in 0..count {
        let broken = draw(stream, 4)? == 0;
        let pool: &[&str] = if broken { &BROKEN_JCL } else { &CLEAN_JCL };
        lines.push(String::from(pick(stream, pool)?));
    }
    Ok(lines.join("\n"))
}

/// No resolver entry: every COPY stays unresolved, honestly recorded.
fn no_copy(_name: &str) -> Option<String> {
    None
}

/// Records one program parse into `trace`: ok or typed error, then its JSON shape.
fn trace_program(trace: &mut Vec<u8>, source: &str) -> TestResult {
    let verdict = match parse_program(source, &BOUNDS, &no_copy)
        .and_then(|program| program_json(&program, &BOUNDS))
    {
        Ok(verdict) => verdict,
        Err(error) => {
            record(trace, discriminant(&error).as_bytes());
            return Ok(());
        }
    };
    record(trace, [1_u8].as_slice());
    for path in [
        &["statements"][..],
        &["graphs", "perform"],
        &["graphs", "call"],
    ] {
        record(
            trace,
            json_array(&verdict, path)?.len().to_le_bytes().as_slice(),
        );
    }
    let sentences = json_sentences(&verdict)?;
    record(trace, sentences.len().to_le_bytes().as_slice());
    for text in sentences {
        record(trace, text.as_bytes());
    }
    Ok(())
}

/// The refusal arm name: bounds sort apart from input faults.
fn discriminant(error: &LegacyError) -> String {
    match *error {
        LegacyError::SourceTooLarge { .. } => String::from("bytes"),
        LegacyError::TooManyLines { .. } => String::from("lines"),
        LegacyError::TooManyNodes { .. } => String::from("nodes"),
        LegacyError::TooManyEdges { .. } => String::from("edges"),
        LegacyError::CopyTooDeep { .. } => String::from("depth"),
        LegacyError::TooManyCopies { .. } => String::from("copies"),
        LegacyError::TooManyDatasets { .. } => String::from("datasets"),
        LegacyError::UnknownStatement { .. } => String::from("statement"),
        LegacyError::EmptySentence { .. } => String::from("empty"),
        LegacyError::BadLineRange { .. } => String::from("range"),
        _ => String::from("other"),
    }
}

/// The array the verdict holds at `path`: a missing key or a non-array is a
/// failure, never a zero, so a verdict that drops a field cannot pass a sweep.
fn json_array<'verdict>(
    verdict: &'verdict Value,
    path: &[&str],
) -> Result<&'verdict Vec<Value>, Box<dyn Error>> {
    let mut at = verdict;
    for key in path {
        at = at
            .get(*key)
            .ok_or_else(|| format!("the verdict holds no {key} on {path:?}"))?;
    }
    Ok(at
        .as_array()
        .ok_or_else(|| format!("{path:?} is not an array"))?)
}

/// The sentence texts of a program or job verdict; a sentence with no text fails.
fn json_sentences(verdict: &Value) -> Result<Vec<String>, Box<dyn Error>> {
    let mut texts = Vec::new();
    for sentence in json_array(verdict, &["sentences"])? {
        let text = sentence
            .get("text")
            .and_then(Value::as_str)
            .ok_or("a sentence carries no text")?;
        texts.push(String::from(text));
    }
    Ok(texts)
}

/// Records one JCL parse into `trace`: ok or typed error, then its JSON shape.
fn trace_job(trace: &mut Vec<u8>, source: &str) -> TestResult {
    let verdict = match parse_job(source, &BOUNDS).and_then(|job| job_json(&job, &BOUNDS)) {
        Ok(verdict) => verdict,
        Err(error) => {
            record(trace, discriminant(&error).as_bytes());
            return Ok(());
        }
    };
    record(trace, [2_u8].as_slice());
    for key in ["steps", "edges"] {
        record(
            trace,
            json_array(&verdict, &[key])?.len().to_le_bytes().as_slice(),
        );
    }
    for text in json_sentences(&verdict)? {
        record(trace, text.as_bytes());
    }
    Ok(())
}

/// One sweep's trace digest: seeded programs, truncated and mutated copies,
/// and seeded jobs, hashed once over the whole transcript.
fn sweep(seed: u64) -> Result<Digest, Box<dyn Error>> {
    let mut stream = Seeded::from_seed(seed);
    let mut trace: Vec<u8> = Vec::new();
    for _ in 0..6 {
        let source = write_program(&mut stream)?;
        trace_program(&mut trace, source.as_str())?;
        trace_program(&mut trace, truncate(source.as_str(), &mut stream)?)?;
        let mutated = mutate(source.as_str(), &mut stream)?;
        trace_program(&mut trace, mutated.as_str())?;
        let job = write_job(&mut stream)?;
        trace_job(&mut trace, job.as_str())?;
        trace_job(&mut trace, truncate(job.as_str(), &mut stream)?)?;
    }
    Ok(blake3(trace.as_slice()))
}

/// The first `draw` bytes of `text`: a truncated input.
fn truncate<'text>(text: &'text str, stream: &mut Seeded) -> Result<&'text str, Box<dyn Error>> {
    let bound = u64::try_from(text.len().saturating_add(1))?;
    let cut = usize::try_from(draw(stream, bound)?)?;
    match text.get(..cut.min(text.len())) {
        Some(kept) => Ok(kept),
        None => Ok(text),
    }
}

/// `text` with one byte replaced: a mutated input, or the text itself when
/// the replacement breaks UTF-8.
fn mutate(text: &str, stream: &mut Seeded) -> Result<String, Box<dyn Error>> {
    if text.is_empty() {
        return Ok(String::new());
    }
    let bound = u64::try_from(text.len())?;
    let at = usize::try_from(draw(stream, bound)?)?;
    let mut bytes = text.as_bytes().to_vec();
    if let Some(cell) = bytes.get_mut(at) {
        *cell = b'#';
    }
    match String::from_utf8(bytes) {
        Ok(mutated) => Ok(mutated),
        Err(_) => Ok(String::from(text)),
    }
}

/// Proves seeded programs and jobs parse without panic and replay per seed (#777).
#[test]
fn sim_seeded_sources_parse_and_replay_per_seed() -> TestResult {
    let mut traces: Vec<Digest> = Vec::new();
    for seed in SEEDS {
        let first = sweep(seed)?;
        let again = sweep(seed)?;
        assert_eq!(
            first,
            again,
            "seed {seed:#x} must replay its trace, got {} then {}",
            first.to_hex(),
            again.to_hex()
        );
        assert!(
            !traces.contains(&first),
            "seed {seed:#x} collapsed onto another seed's trace"
        );
        traces.push(first);
    }
    Ok(())
}

/// Proves past-a-bound inputs earn typed refusals, never a panic (#777).
#[test]
fn sim_bounds_refuse_with_their_typed_arm() -> TestResult {
    let tiny = LegacyLimits::new(16, 2, 4, 4, 1, 1, 1);
    let big = write_program(&mut Seeded::from_seed(SEEDS[0]))?;
    assert!(
        matches!(
            parse_program(big.as_str(), &tiny, &no_copy),
            Err(LegacyError::SourceTooLarge { .. }) | Err(LegacyError::TooManyLines { .. })
        ),
        "a program past tiny ceilings must be refused on its bound"
    );
    let cycling = |name: &str| -> Option<String> {
        if name == "SELF" {
            Some(String::from("       COPY SELF.\n"))
        } else {
            None
        }
    };
    let shallow = LegacyLimits::new(1_048_576, 10_000, 80_000, 80_000, 4_096, 0, 10_000);
    let source = format!("{}\n{}", fixed("PROCEDURE DIVISION."), fixed("COPY SELF."));
    assert!(
        matches!(
            parse_program(source.as_str(), &shallow, &cycling),
            Err(LegacyError::CopyTooDeep { .. })
        ),
        "a self-copying program past depth zero must be refused as too deep"
    );
    assert!(
        matches!(
            parse_job("//A JOB X\n//S EXEC\n", &BOUNDS),
            Err(LegacyError::UnknownStatement { .. })
        ),
        "an EXEC with neither PGM nor PROC must be refused at its line"
    );
    assert!(
        matches!(
            parse_job("//A JOB X\n//S FROB PGM=A\n", &BOUNDS),
            Err(LegacyError::UnknownStatement { .. })
        ),
        "an unknown JCL operation must be refused at its line"
    );
    Ok(())
}

/// The known answers: hand-written programs and jobs pinned exactly.
mod known;
