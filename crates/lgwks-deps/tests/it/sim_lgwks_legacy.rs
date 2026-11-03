//! `lgwks legacy` driven through the shipped binary under seeded inputs.
//!
//! Each codec round-trips seeded data through two invocations of the built
//! `lgwks`, the way a migration script calls it: what one command writes, the
//! next reads, and the facts printed with `--json` must name the input
//! exactly. Known answers pin the formats to their standards: CP037 spells
//! `A` as 0xC1 (IBM CDRA, CCSID 37), and IBM Enterprise COBOL packs
//! `-123.45` in a `PIC S9(5)V99 COMP-3` field as `0012345D`. Refusals carry
//! the stable kinds an agent branches on. One seed drives every draw, and the
//! same seed must replay the same trace while distinct seeds diverge.

use std::error::Error;
use std::path::Path;

use lgwks_std::json::Value;
use lgwks_std::legacy::decimal::Decimal;
use lgwks_std::seeded::Seeded;

use crate::lgwks_scratch::{Scratch, run_lgwks};
use crate::replay::refuse;

/// What this test returns.
type TestResult = Result<(), Box<dyn Error>>;

/// The seeds every sweep replays, named so a failure names its seed.
const SEEDS: [u64; 4] = [
    0x5EED_0000_0000_0001,
    0x5EED_0000_0000_0002,
    0x5EED_0000_FFFF_FFFF,
    0xDEAD_BEEF_CAFE_0001,
];

/// The FNV-1a offset basis a trace starts from.
const FNV_BASIS: u64 = 0xcbf2_9ce4_8422_2325;

/// The FNV-1a prime a trace folds with.
const FNV_PRIME: u64 = 0x0100_0000_01b3;

/// Folds `bytes` into `trace`, FNV-1a.
fn fold(trace: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(trace, |acc, byte| {
        (acc ^ u64::from(*byte)).wrapping_mul(FNV_PRIME)
    })
}

/// One draw below `bound`.
fn draw(rng: &mut Seeded, bound: u64) -> Result<u64, Box<dyn Error>> {
    Ok(rng.below(bound)?)
}

/// `len` seeded bytes.
fn bytes(rng: &mut Seeded, len: usize) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        out.push(u8::try_from(draw(rng, 256)?)?);
    }
    Ok(out)
}

/// `len` seeded letters, A to Z.
fn letters(rng: &mut Seeded, len: usize) -> Result<String, Box<dyn Error>> {
    let mut out = String::with_capacity(len);
    for _ in 0..len {
        out.push(char::from(
            b'A'.saturating_add(u8::try_from(draw(rng, 26)?)?),
        ));
    }
    Ok(out)
}

/// Runs `lgwks legacy <args> --json` in `dir` and returns its exit code and
/// the verdict it printed.
fn invoke(dir: &Path, args: &[&str]) -> Result<(Option<i32>, Value, String), Box<dyn Error>> {
    let mut argv = vec!["legacy"];
    argv.extend_from_slice(args);
    argv.push("--json");
    let outcome = run_lgwks(dir, &argv)?;
    let verdict: Value = lgwks_std::json::from_slice(&outcome.stdout).map_err(|error| {
        format!(
            "lgwks {} printed no JSON ({error}): {}",
            argv.join(" "),
            outcome.stderr
        )
    })?;
    Ok((outcome.code, verdict, argv.join(" ")))
}

/// The verdict of `lgwks legacy <args> --json`, refusing a non-zero exit.
fn legacy(dir: &Path, args: &[&str]) -> Result<Value, Box<dyn Error>> {
    let (code, verdict, line) = invoke(dir, args)?;
    if code != Some(0) {
        return refuse(format!("lgwks {line} exited {code:?}: {verdict}"));
    }
    Ok(verdict)
}

/// The kind of the first refusal of `lgwks legacy <args> --json`, which
/// must exit 2.
fn refused(dir: &Path, args: &[&str]) -> Result<String, Box<dyn Error>> {
    let (code, verdict, line) = invoke(dir, args)?;
    if code != Some(2) {
        return refuse(format!("lgwks {line} exited {code:?}, not 2: {verdict}"));
    }
    match verdict
        .get("refusals")
        .and_then(Value::as_array)
        .and_then(|refusals| refusals.first())
        .and_then(|first| first.get("kind"))
        .and_then(Value::as_str)
    {
        Some(kind) => Ok(String::from(kind)),
        None => refuse(format!("lgwks {line} refused with no kind: {verdict}")),
    }
}

/// The string `verdict` holds at `key`.
fn text_at<'value>(verdict: &'value Value, key: &str) -> Result<&'value str, Box<dyn Error>> {
    match verdict.get(key).and_then(Value::as_str) {
        Some(text) => Ok(text),
        None => refuse(format!("the verdict carries no string {key}: {verdict}")),
    }
}

/// CP037 spells `A` as 0xC1, and seeded Latin-1 text round-trips through
/// `encode` then `decode` on every page.
#[test]
fn sim_ebcdic_round_trips_on_every_page() -> TestResult {
    let scratch = Scratch::fresh("legacy-ebcdic")?;
    let dir = scratch.dir();
    std::fs::write(dir.join("a.txt"), "A")?;
    legacy(dir, &["ebcdic", "encode", "a.txt", "a.ebc"])?;
    if std::fs::read(dir.join("a.ebc"))? != [0xC1] {
        return refuse("CP037 must spell A as 0xC1");
    }
    for seed in SEEDS {
        for page in ["--page=037", "--page=500", "--page=1047", "--page=273"] {
            let mut rng = Seeded::from_seed(seed);
            let mut text = String::new();
            for _ in 0..64 {
                // Printable ASCII, then the Latin-1 supplement from NBSP.
                let drawn = u8::try_from(draw(&mut rng, 190)?)?;
                text.push(char::from(if drawn < 95 {
                    drawn.saturating_add(0x20)
                } else {
                    drawn.saturating_add(0x41)
                }));
            }
            std::fs::write(dir.join("in.txt"), &text)?;
            legacy(dir, &["ebcdic", "encode", "in.txt", "out.ebc", page])?;
            let back = legacy(dir, &["ebcdic", "decode", "out.ebc", page])?;
            if text_at(&back, "text")? != text {
                return refuse(format!(
                    "seed {seed:#x} {page}: the text did not round-trip"
                ));
            }
        }
    }
    Ok(())
}

/// `-123.45` packs to `0012345D` in seven digits and reads back exactly;
/// seeded values at seeded scales round-trip through `encode` then `decode`.
#[test]
fn sim_packed_decimals_round_trip_exactly() -> TestResult {
    let scratch = Scratch::fresh("legacy-packed")?;
    let dir = scratch.dir();
    let packed = legacy(dir, &["packed", "encode", "-123.45", "--digits=7"])?;
    if !text_at(&packed, "hex")?.eq_ignore_ascii_case("0012345D") {
        return refuse(format!("-123.45 must pack to 0012345D: {packed}"));
    }
    let read = legacy(
        dir,
        &["packed", "decode", "0012345d", "--digits=7", "--scale=2"],
    )?;
    if text_at(&read, "value")? != "-123.45" {
        return refuse(format!("0012345D must read as -123.45: {read}"));
    }
    for seed in SEEDS {
        let mut rng = Seeded::from_seed(seed);
        for _ in 0..8 {
            let magnitude = i128::from(draw(&mut rng, 1_000_000_000)?);
            let coefficient = if draw(&mut rng, 2)? == 0 {
                magnitude
            } else {
                magnitude.saturating_neg()
            };
            let scale = u32::try_from(draw(&mut rng, 5)?)?;
            let spelled = Decimal::new(coefficient, scale).to_string();
            let hex = legacy(dir, &["packed", "encode", &spelled, "--digits=11"])?;
            let scale_flag = format!("--scale={scale}");
            let digits = text_at(&hex, "hex")?;
            let back = legacy(
                dir,
                &["packed", "decode", digits, "--digits=11", &scale_flag],
            )?;
            if text_at(&back, "value")? != spelled {
                return refuse(format!(
                    "seed {seed:#x}: {spelled} did not round-trip: {back}"
                ));
            }
        }
    }
    Ok(())
}

/// A copybook record of EBCDIC text and a COMP-3 amount decodes to its
/// values and encodes back to the same bytes; a short record is refused.
#[test]
fn sim_copybook_records_decode_and_replay() -> TestResult {
    let scratch = Scratch::fresh("legacy-copybook")?;
    let dir = scratch.dir();
    std::fs::write(
        dir.join("rec.cpy"),
        "01 REC.\n05 NAME PIC X(5).\n05 AMT PIC S9(5)V99 COMP-3.\n",
    )?;
    let layout = legacy(dir, &["copybook", "rec.cpy"])?;
    if layout.get("record_len").and_then(Value::as_u64) != Some(9) {
        return refuse(format!("the layout must be 9 bytes: {layout}"));
    }
    let hello = [0xC8, 0xC5, 0xD3, 0xD3, 0xD6, 0x00, 0x12, 0x34, 0x5D];
    let mut file = hello.to_vec();
    file.extend_from_slice(&hello);
    std::fs::write(dir.join("rec.dat"), &file)?;
    let decoded = legacy(dir, &["copybook", "rec.cpy", "rec.dat"])?;
    let expected = lgwks_std::json::json!([
        { "name": "NAME", "value": "HELLO" },
        { "name": "AMT", "value": "-123.45" },
    ]);
    let Some(records) = decoded.get("records").and_then(Value::as_array) else {
        return refuse(format!("the decode carries no records: {decoded}"));
    };
    if records.len() != 2 {
        return refuse(format!("two records must decode: {decoded}"));
    }
    for record in records {
        if record.get("fields") != Some(&expected)
            || record.get("replays") != Some(&Value::Bool(true))
        {
            return refuse(format!("the record decoded wrong: {record}"));
        }
    }
    std::fs::write(
        dir.join("short.dat"),
        hello.get(..5).ok_or("no five bytes")?,
    )?;
    if refused(dir, &["copybook", "rec.cpy", "short.dat"])? != "malformed" {
        return refuse("a record cut short must be refused as malformed");
    }
    Ok(())
}

/// Seeded streams on both sides of the 4096-byte mini-stream cutoff pack
/// into a compound file whose streams read back byte for byte.
#[test]
fn sim_compound_files_pack_and_read_back() -> TestResult {
    let scratch = Scratch::fresh("legacy-cfb")?;
    let dir = scratch.dir();
    let names = ["Alpha", "Beta", "Gamma"];
    for seed in SEEDS {
        let mut rng = Seeded::from_seed(seed);
        let mut args = vec![
            String::from("cfb"),
            String::from("pack"),
            String::from("out.cfb"),
        ];
        let mut contents = Vec::new();
        for name in names {
            let len = usize::try_from(draw(&mut rng, 9000)?)?;
            let content = bytes(&mut rng, len)?;
            std::fs::write(dir.join(name), &content)?;
            args.push(format!("{name}={name}"));
            contents.push(content);
        }
        let argv: Vec<&str> = args.iter().map(String::as_str).collect();
        legacy(dir, &argv)?;
        let listed = legacy(dir, &["cfb", "list", "out.cfb"])?;
        let Some(all) = listed.get("streams").and_then(Value::as_array) else {
            return refuse(format!("seed {seed:#x}: no streams listed: {listed}"));
        };
        let mut streams: Vec<&str> = all.iter().filter_map(Value::as_str).collect();
        streams.sort_unstable();
        if streams != names {
            return refuse(format!(
                "seed {seed:#x}: the streams listed are {streams:?}"
            ));
        }
        for (name, content) in names.into_iter().zip(&contents) {
            legacy(dir, &["cfb", "read", "out.cfb", name, "stream.bin"])?;
            if std::fs::read(dir.join("stream.bin"))? != *content {
                return refuse(format!("seed {seed:#x}: stream {name} did not read back"));
            }
        }
    }
    std::fs::write(dir.join("junk.cfb"), [0_u8; 600])?;
    if refused(dir, &["cfb", "list", "junk.cfb"])? != "malformed" {
        return refuse("a file with no CFB magic must be refused as malformed");
    }
    Ok(())
}

/// Seeded VBA sources with repeats compress and decompress to themselves.
#[test]
fn sim_vba_modules_round_trip() -> TestResult {
    let scratch = Scratch::fresh("legacy-vba")?;
    let dir = scratch.dir();
    for seed in SEEDS {
        let mut rng = Seeded::from_seed(seed);
        let len = usize::try_from(draw(&mut rng, 12_000)?)?;
        let source = letters(&mut rng, len)?
            .replace('Q', "\r\n")
            .replace('Z', "End Sub ");
        std::fs::write(dir.join("module.bas"), &source)?;
        legacy(dir, &["vba", "compress", "module.bas", "module.bin"])?;
        legacy(dir, &["vba", "decompress", "module.bin", "back.bas"])?;
        if std::fs::read(dir.join("back.bas"))? != source.as_bytes() {
            return refuse(format!("seed {seed:#x}: the module did not round-trip"));
        }
    }
    Ok(())
}

/// Seeded rows of every packable kind build a database on each engine that
/// lists its table and reads every cell back exactly.
#[test]
fn sim_access_tables_pack_and_read_back() -> TestResult {
    let scratch = Scratch::fresh("legacy-access")?;
    let dir = scratch.dir();
    let schema = "name:text:100,qty:integer,price:currency,day:date,notes:memo";
    for seed in SEEDS {
        for engine in ["--engine=jet3", "--engine=jet4", "--engine=ace"] {
            let mut rng = Seeded::from_seed(seed);
            let count = usize::try_from(draw(&mut rng, 40)?)?.saturating_add(1);
            let mut lines = Vec::new();
            let mut expected = Vec::new();
            for _ in 0..count {
                let name_len = usize::try_from(draw(&mut rng, 20)?)?.saturating_add(1);
                let name = letters(&mut rng, name_len)?;
                let qty =
                    i32::try_from(draw(&mut rng, 2_000_000_000)?)?.saturating_sub(1_000_000_000);
                let whole = draw(&mut rng, 1_000_000)?;
                let cents = draw(&mut rng, 10_000)?;
                let days = i32::try_from(draw(&mut rng, 60_000)?)?;
                let millis = u32::try_from(draw(&mut rng, 86_400_000)?)?;
                let notes_len = usize::try_from(draw(&mut rng, 40)?)?;
                let notes = letters(&mut rng, notes_len)?;
                let price = format!("{whole}.{cents:04}");
                lines.push(format!("{name}\t{qty}\t{price}\t{days}:{millis}\t{notes}"));
                let note = if notes.is_empty() {
                    Value::Null
                } else {
                    Value::String(notes)
                };
                expected.push(lgwks_std::json::json!([
                    name,
                    qty,
                    price,
                    { "days": days, "millis": millis },
                    note,
                ]));
            }
            std::fs::write(dir.join("rows.tsv"), lines.join("\n"))?;
            legacy(
                dir,
                &[
                    "access", "pack", "out.mdb", "Orders", schema, "rows.tsv", engine,
                ],
            )?;
            let listed = legacy(dir, &["access", "list", "out.mdb"])?;
            let tables = lgwks_std::json::json!([{ "name": "Orders", "rows": count }]);
            if listed.get("tables") != Some(&tables) {
                return refuse(format!("seed {seed:#x} {engine}: listed {listed}"));
            }
            let read = legacy(dir, &["access", "rows", "out.mdb", "Orders"])?;
            if read.get("rows") != Some(&Value::Array(expected)) {
                return refuse(format!("seed {seed:#x} {engine}: rows read back as {read}"));
            }
        }
    }
    Ok(())
}

/// A seeded bilevel page, padding bits clear, crosses PBM to TIFF and back
/// without a bit moved under every compression, with its black count named.
#[test]
fn sim_fax_pages_round_trip_through_pbm() -> TestResult {
    let scratch = Scratch::fresh("legacy-fax")?;
    let dir = scratch.dir();
    for seed in SEEDS {
        for compression in ["--compression=g3", "--compression=g4", "--compression=raw"] {
            let mut rng = Seeded::from_seed(seed);
            let width = u32::try_from(draw(&mut rng, 300)?)?.saturating_add(1);
            let height = u32::try_from(draw(&mut rng, 60)?)?.saturating_add(1);
            let row_bytes = usize::try_from(width.div_ceil(8))?;
            let spare = row_bytes
                .saturating_mul(8)
                .saturating_sub(usize::try_from(width)?);
            let Some(mask) = 0xFF_u8.checked_shl(u32::try_from(spare)?) else {
                return refuse(format!("a row never spares {spare} bits"));
            };
            let mut rows = Vec::new();
            let mut black = 0_u32;
            for _ in 0..height {
                let mut row = bytes(&mut rng, row_bytes)?;
                if let Some(last) = row.last_mut() {
                    *last &= mask;
                }
                black = black.saturating_add(row.iter().map(|byte| byte.count_ones()).sum());
                rows.extend_from_slice(&row);
            }
            let mut pbm = format!("P4\n# seed {seed:#x}\n{width} {height}\n").into_bytes();
            pbm.extend_from_slice(&rows);
            std::fs::write(dir.join("page.pbm"), &pbm)?;
            legacy(dir, &["fax", "encode", "page.pbm", "page.tif", compression])?;
            let info = legacy(dir, &["fax", "info", "page.tif"])?;
            let facts = lgwks_std::json::json!([{
                "width": width,
                "height": height,
                "row_bytes": row_bytes,
                "black": black,
            }]);
            if info.get("images") != Some(&facts) {
                return refuse(format!("seed {seed:#x} {compression}: info {info}"));
            }
            legacy(dir, &["fax", "decode", "page.tif", "back.pbm"])?;
            let mut canonical = format!("P4\n{width} {height}\n").into_bytes();
            canonical.extend_from_slice(&rows);
            if std::fs::read(dir.join("back.pbm"))? != canonical {
                return refuse(format!(
                    "seed {seed:#x} {compression}: the page did not round-trip"
                ));
            }
        }
    }
    Ok(())
}

/// Past a ceiling is `bound-exceeded`; a corrupt input, an unknown word or a
/// missing file is `malformed`; each exits 2.
#[test]
fn sim_refusals_carry_their_kind() -> TestResult {
    let scratch = Scratch::fresh("legacy-refusals")?;
    let dir = scratch.dir();
    std::fs::write(dir.join("five.ebc"), [0xC1_u8; 5])?;
    if refused(dir, &["ebcdic", "decode", "five.ebc", "--max-bytes=4"])? != "bound-exceeded" {
        return refuse("five bytes past a four-byte ceiling must be bound-exceeded");
    }
    for args in [
        &["packed", "decode", "1a", "--digits=1"][..],
        &["packed", "encode", "12x", "--digits=3"][..],
        &["nonsense"][..],
        &["ebcdic", "decode", "five.ebc", "--page=999"][..],
        &["ebcdic", "transmogrify", "five.ebc"][..],
        &["ebcdic", "decode", "absent.ebc"][..],
    ] {
        if refused(dir, args)? != "malformed" {
            return refuse(format!("{args:?} must be refused as malformed"));
        }
    }
    Ok(())
}

/// One sweep's trace: packed and VBA outputs folded for `seed`.
fn sweep(dir: &Path, seed: u64) -> Result<u64, Box<dyn Error>> {
    let mut rng = Seeded::from_seed(seed);
    let value = Decimal::new(i128::from(draw(&mut rng, 1_000_000)?), 2).to_string();
    let packed = legacy(dir, &["packed", "encode", &value, "--digits=9"])?;
    let trace = fold(FNV_BASIS, text_at(&packed, "hex")?.as_bytes());
    let len = usize::try_from(draw(&mut rng, 4000)?)?.saturating_add(1);
    std::fs::write(dir.join("sweep.bas"), letters(&mut rng, len)?)?;
    legacy(dir, &["vba", "compress", "sweep.bas", "sweep.bin"])?;
    Ok(fold(trace, &std::fs::read(dir.join("sweep.bin"))?))
}

/// A month-end job explains itself in human text and JSON: steps, sentences, edges and COPY (#777).
#[test]
fn sim_explain_month_end_job_reports_human_and_json() -> TestResult {
    let scratch = Scratch::fresh("legacy-explain")?;
    let dir = scratch.dir();
    let job_text = "//MONTHEND JOB (ACCT),'MONTH END',CLASS=A\n//SETUP    SET HLQ=PROD.MONTH\n//PAY      EXEC PGM=PAYROLL\n//IN       DD DSN=&HLQ..CUST,DISP=SHR\n//OUT      DD DSN=&HLQ..REPORT,DISP=(NEW,CATLG)\n//RPT      EXEC PGM=REPORTER\n//LISTING  DD SYSOUT=*\n";
    std::fs::write(dir.join("job.jcl"), job_text)?;
    let pay_text = "       IDENTIFICATION DIVISION.\n       PROGRAM-ID. PAYROLL.\n       ENVIRONMENT DIVISION.\n       INPUT-OUTPUT SECTION.\n       FILE-CONTROL.\n           SELECT CUST-FILE ASSIGN TO \"CUST.DAT\".\n       DATA DIVISION.\n       FILE SECTION.\n       FD CUST-FILE.\n       01 CUST-REC PIC X(80).\n       PROCEDURE DIVISION.\n       MAIN-PARA.\n           OPEN INPUT CUST-FILE.\n           READ CUST-FILE INTO CUST-REC.\n           CALL \"CALC-TAX\" USING CUST-REC.\n           PERFORM WRITE-REPORT.\n           CLOSE CUST-FILE.\n       WRITE-REPORT.\n           WRITE CUST-REC FROM WS-LINE.\n";
    std::fs::write(dir.join("pay.cbl"), pay_text)?;
    let report_text = "       IDENTIFICATION DIVISION.\n       PROGRAM-ID. REPORTER.\n       PROCEDURE DIVISION.\n       MAIN-PARA.\n           COPY FIELDS REPLACING ==WS-A== BY ==WS-X==.\n           CALL WS-PROG.\n";
    std::fs::write(dir.join("report.cbl"), report_text)?;
    std::fs::create_dir(dir.join("copy"))?;
    std::fs::write(
        dir.join("copy").join("FIELDS"),
        "           MOVE WS-A TO WS-B.\n",
    )?;
    let outcome = run_lgwks(dir, &["legacy", "explain", "job.jcl"])?;
    if outcome.code != Some(0) {
        return refuse(format!(
            "explain job.jcl exited {:?}: {}",
            outcome.code, outcome.stderr
        ));
    }
    let human = String::from_utf8(outcome.stdout)
        .map_err(|error| format!("explain printed no UTF-8 text: {error}"))?;
    for needle in [
        "job MONTHEND",
        "reads dataset PROD.MONTH.CUST",
        "writes dataset PROD.MONTH.REPORT",
    ] {
        if !human.contains(needle) {
            return refuse(format!("the human summary names no {needle}: {human}"));
        }
    }
    let verdict = legacy(dir, &["explain", "job.jcl"])?;
    let job = verdict
        .get("job")
        .ok_or_else(|| format!("the JSON names no job: {verdict}"))?;
    let steps = job.get("steps").and_then(Value::as_array);
    let Some(steps) = steps else {
        return refuse(format!("the JSON names no steps: {verdict}"));
    };
    if steps.len() != 2 {
        return refuse(format!("two steps must be explained: {verdict}"));
    }
    check_sentences_in_source(job, job_text, "job.jcl")?;
    let program = legacy(dir, &["explain", "pay.cbl"])?;
    let named = match program.get("program") {
        Some(named) => named,
        None => return refuse(format!("the JSON names no program: {program}")),
    };
    if text_at(named, "name")? != "PAYROLL" {
        return refuse(format!("the JSON names no PAYROLL program: {program}"));
    }
    let performed = named
        .get("graphs")
        .and_then(|graphs| graphs.get("perform"))
        .and_then(Value::as_array);
    let Some(performed) = performed else {
        return refuse(format!("the JSON names no PERFORM edges: {program}"));
    };
    if performed.len() != 1 {
        return refuse(format!("one PERFORM edge must be graphed: {program}"));
    }
    check_sentences_in_source(named, pay_text, "pay.cbl")?;
    let resolved = legacy(dir, &["explain", "report.cbl", "--copy-dir=copy"])?;
    let replacing = resolved
        .get("program")
        .ok_or_else(|| format!("the JSON names no program: {resolved}"))?;
    let copies = replacing.get("copies").and_then(Value::as_array);
    let Some(copies) = copies else {
        return refuse(format!("the JSON names no copies: {resolved}"));
    };
    if copies
        .iter()
        .any(|copy| copy.get("resolved") != Some(&Value::Bool(true)))
    {
        return refuse(format!(
            "the COPY must resolve through --copy-dir: {resolved}"
        ));
    }
    let statements = replacing
        .get("statements")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("the JSON names no statements: {resolved}"))?;
    if !statements.iter().any(|statement| {
        statement
            .get("text")
            .and_then(Value::as_str)
            .is_some_and(|text| text.contains("WS-X"))
    }) {
        return refuse(format!("REPLACING must rewrite WS-A to WS-X: {resolved}"));
    }
    check_sentences_in_source(replacing, report_text, "report.cbl")?;
    let gapped = legacy(dir, &["explain", "report.cbl"])?;
    let gapped_program = gapped
        .get("program")
        .ok_or_else(|| format!("the JSON names no program: {gapped}"))?;
    let sentences = gapped_program.get("sentences").and_then(Value::as_array);
    let Some(sentences) = sentences else {
        return refuse(format!("the JSON names no sentences: {gapped}"));
    };
    if !sentences
        .iter()
        .any(|line| match line.get("text").and_then(Value::as_str) {
            Some(text) => text.contains("cannot tell"),
            None => false,
        })
    {
        return refuse(format!(
            "an unresolved COPY must say it cannot tell: {gapped}"
        ));
    }
    Ok(())
}

/// Every sentence's line ranges exist in the source file that proved them.
fn check_sentences_in_source(verdict: &Value, source: &str, name: &str) -> TestResult {
    let physical: Vec<&str> = source.lines().collect();
    let sentences = verdict
        .get("sentences")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("the JSON names no sentences for {name}: {verdict}"))?;
    if sentences.is_empty() {
        return refuse(format!("the JSON names no sentences for {name}"));
    }
    for sentence in sentences.iter() {
        let ranges = sentence
            .get("lines")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("a sentence names no lines for {name}: {sentence}"))?;
        if ranges.is_empty() {
            return refuse(format!(
                "a sentence must rest on lines for {name}: {sentence}"
            ));
        }
        for range in ranges.iter() {
            let start = range
                .get("start")
                .and_then(Value::as_u64)
                .ok_or_else(|| format!("a range names no start for {name}: {range}"))?;
            let end = range
                .get("end")
                .and_then(Value::as_u64)
                .ok_or_else(|| format!("a range names no end for {name}: {range}"))?;
            if start < 1 || end < start || end > u64::try_from(physical.len())? {
                return refuse(format!("range {start}-{end} is outside {name}"));
            }
        }
    }
    Ok(())
}

/// Past a ceiling is `bound-exceeded`; an unknown file, card or second file is
/// `malformed`; each exits 2 (#777).
#[test]
fn sim_explain_refusals_carry_their_kind() -> TestResult {
    let scratch = Scratch::fresh("legacy-explain-refusals")?;
    let dir = scratch.dir();
    std::fs::write(dir.join("notes.txt"), "just some notes\n")?;
    std::fs::write(dir.join("bad.jcl"), "//A JOB X\n//S FROB PGM=A\n")?;
    std::fs::write(dir.join("tiny.cbl"), "       PROGRAM-ID. TINY.\n")?;
    if refused(dir, &["explain", "tiny.cbl", "--max-bytes=4"])? != "bound-exceeded" {
        return refuse("four bytes past a four-byte ceiling must be bound-exceeded");
    }
    for args in [
        &["explain", "notes.txt"][..],
        &["explain", "absent.cbl"][..],
        &["explain", "bad.jcl"][..],
        &["explain", "tiny.cbl", "tiny.cbl"][..],
    ] {
        if refused(dir, args)? != "malformed" {
            return refuse(format!("{args:?} must be refused as malformed"));
        }
    }
    Ok(())
}

/// The same seed replays the same trace, and distinct seeds diverge.
#[test]
fn sim_the_same_seed_replays_the_same_trace() -> TestResult {
    let scratch = Scratch::fresh("legacy-replay")?;
    let dir = scratch.dir();
    let mut traces = Vec::new();
    for seed in SEEDS {
        let first = sweep(dir, seed)?;
        if sweep(dir, seed)? != first {
            return refuse(format!("seed {seed:#x} did not replay its trace"));
        }
        if traces.contains(&first) {
            return refuse(format!(
                "seed {seed:#x} collapsed onto another seed's trace"
            ));
        }
        traces.push(first);
    }
    Ok(())
}
