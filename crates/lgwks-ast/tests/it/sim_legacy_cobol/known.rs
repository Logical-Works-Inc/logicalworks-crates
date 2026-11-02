//! Known answers for the legacy reader (#777): hand-written GnuCOBOL-style
//! programs and JCL jobs whose edges, read and write sets, DISP directions,
//! COPY records, REPLACING and dialect are pinned exactly, every summary
//! sentence checked against the source lines it rests on.

use std::error::Error;

use lgwks_ast::legacy::cobol::parse_program;
use lgwks_ast::legacy::jcl::parse_job;
use lgwks_ast::legacy::json::{job_json, program_json};
use lgwks_std::json::Value;

use super::{BOUNDS, TestResult, fixed, json_array, json_sentences, no_copy};

/// The GnuCOBOL-style month-end program the known answers read.
const MONTHEND: &str = "       IDENTIFICATION DIVISION.\n       PROGRAM-ID. MONTHEND.\n       ENVIRONMENT DIVISION.\n       INPUT-OUTPUT SECTION.\n       FILE-CONTROL.\n           SELECT CUST-FILE ASSIGN TO \"CUST.DAT\".\n       DATA DIVISION.\n       FILE SECTION.\n       FD CUST-FILE.\n       01 CUST-REC PIC X(80).\n       WORKING-STORAGE SECTION.\n       01 WS-TOTAL PIC 9(7)V99.\n       PROCEDURE DIVISION.\n       MAIN-PARA.\n           OPEN INPUT CUST-FILE.\n           PERFORM READ-LOOP UNTIL WS-EOF.\n           CALL \"CALC-TAX\" USING WS-TOTAL.\n           MOVE WS-TOTAL TO WS-REPORT.\n           CLOSE CUST-FILE.\n       READ-LOOP.\n           READ CUST-FILE INTO CUST-REC.\n           ADD 1 TO WS-COUNT.\n";

/// Reads the JSON verdict for `source` with no copybook.
fn verdict_of(source: &str) -> Result<Value, Box<dyn Error>> {
    let program = parse_program(source, &BOUNDS, &no_copy)?;
    Ok(program_json(&program, &BOUNDS)?)
}

/// Proves the PERFORM and CALL edges match the GnuCOBOL-style sample (#777).
#[test]
fn sim_known_answers_pin_perform_and_call_edges() -> TestResult {
    let verdict = verdict_of(MONTHEND)?;
    let names = verdict
        .get("paragraphs")
        .and_then(Value::as_array)
        .map(|paragraphs| {
            paragraphs
                .iter()
                .filter_map(|paragraph| paragraph.get("name"))
                .filter_map(Value::as_str)
                .map(String::from)
                .collect::<Vec<String>>()
        })
        .ok_or("the JSON names no paragraphs")?;
    assert_eq!(
        names,
        vec![String::from("MAIN-PARA"), String::from("READ-LOOP")],
        "the sample declares two paragraphs, got {names:?}"
    );
    let performed = verdict
        .get("graphs")
        .and_then(|graphs| graphs.get("perform"))
        .and_then(Value::as_array)
        .ok_or("the JSON names no PERFORM edges")?;
    assert_eq!(performed.len(), 1, "one PERFORM names a target");
    let edge = performed
        .first()
        .ok_or("one PERFORM edge must be present")?;
    assert_eq!(
        edge.get("from").and_then(Value::as_str),
        Some("MAIN-PARA"),
        "the PERFORM sits in MAIN-PARA"
    );
    assert_eq!(
        edge.get("to").and_then(Value::as_str),
        Some("READ-LOOP"),
        "the PERFORM names READ-LOOP"
    );
    assert_eq!(
        edge.get("via").and_then(Value::as_str),
        Some("perform"),
        "the edge moves by PERFORM"
    );
    let called = verdict
        .get("graphs")
        .and_then(|graphs| graphs.get("call"))
        .and_then(Value::as_array)
        .ok_or("the JSON names no CALL edges")?;
    assert_eq!(called.len(), 1, "one CALL names a target");
    let call = called.first().ok_or("one CALL edge must be present")?;
    assert_eq!(
        call.get("to").and_then(Value::as_str),
        Some("CALC-TAX"),
        "the CALL names the CALC-TAX literal"
    );
    Ok(())
}

/// Proves the read and write sets match the month-end sample (#777).
#[test]
fn sim_known_answers_pin_read_and_write_sets() -> TestResult {
    let verdict = verdict_of(MONTHEND)?;
    let mut reads: Vec<String> = verdict
        .get("graphs")
        .and_then(|graphs| graphs.get("reads"))
        .and_then(Value::as_array)
        .map(|reads| {
            reads
                .iter()
                .filter_map(|read| read.get("file"))
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .ok_or("the JSON names no reads")?;
    reads.sort();
    reads.dedup();
    assert_eq!(
        reads,
        vec![String::from("CUST-FILE")],
        "only CUST-FILE is read, got {reads:?}"
    );
    let writes = verdict
        .get("graphs")
        .and_then(|graphs| graphs.get("writes"))
        .and_then(Value::as_array)
        .ok_or("the JSON names no writes")?;
    assert!(
        writes.is_empty(),
        "the sample writes no record, got {writes:?}"
    );
    Ok(())
}

/// Proves a SELECT-only declaration never counts as a read (#777).
#[test]
fn sim_select_only_declarations_never_read() -> TestResult {
    let source = "       IDENTIFICATION DIVISION.\n       PROGRAM-ID. SELONLY.\n       ENVIRONMENT DIVISION.\n       INPUT-OUTPUT SECTION.\n       FILE-CONTROL.\n           SELECT ONLY-FILE ASSIGN TO \"ONLY.DAT\".\n       PROCEDURE DIVISION.\n       MAIN-PARA.\n           DISPLAY \"HELLO\".\n";
    let verdict = verdict_of(source)?;
    let reads = verdict
        .get("graphs")
        .and_then(|graphs| graphs.get("reads"))
        .and_then(Value::as_array)
        .ok_or("the JSON names no reads")?;
    assert!(
        reads.is_empty(),
        "a SELECT with no OPEN or READ is never a read, got {reads:?}"
    );
    Ok(())
}

/// Proves ambiguous facts say they cannot tell, with their lines (#777).
#[test]
fn sim_known_answers_pin_the_honest_gaps() -> TestResult {
    let source = "       IDENTIFICATION DIVISION.\n       PROGRAM-ID. GAPS.\n       PROCEDURE DIVISION.\n       MAIN-PARA.\n           CALL WS-PROG.\n           05 WS-NEW REDEFINES WS-OLD PIC 9(10).\n           COPY NOPE.\n       >>BOGUS FROBNICATE.\n";
    let program = parse_program(source, &BOUNDS, &no_copy)?;
    let verdict = program_json(&program, &BOUNDS)?;
    let texts = json_sentences(&verdict)?;
    assert!(
        texts
            .iter()
            .any(|text| text.contains("cannot tell") && text.contains("dynamic")),
        "the dynamic CALL must say it cannot tell, got {texts:?}"
    );
    assert!(
        texts
            .iter()
            .any(|text| text.contains("cannot tell") && text.contains("REDEFINES")),
        "the REDEFINES overlay must say it cannot tell, got {texts:?}"
    );
    assert!(
        texts.iter().any(|text| text.contains("NOPE")),
        "the unresolved COPY must be named, got {texts:?}"
    );
    assert!(
        texts
            .iter()
            .any(|text| text.contains("cannot tell") && text.contains("directive")),
        "the unknown directive must say it cannot tell, got {texts:?}"
    );
    check_sentences(&verdict, source)
}

/// Proves the month-end job graph and DISP directions match the cards (#777).
#[test]
fn sim_known_answers_pin_job_graph_and_disp() -> TestResult {
    let source = "//MONTHEND JOB (ACCT),'MONTH END',CLASS=A\n//SETUP    SET HLQ=PROD.MONTH\n//STEP1    EXEC PGM=PAYROLL\n//IN       DD DSN=&HLQ..CUST,DISP=SHR\n//OUT      DD DSN=&HLQ..REPORT,DISP=(NEW,CATLG)\n//STEP2    EXEC PROC=PROCDEF\n//PROCDEF  PROC SUFFIX=RPT\n//PSTEP    EXEC PGM=REPORTER\n//POUT     DD DSN=&HLQ..&SUFFIX,DISP=MOD\n//         PEND\n";
    let job = parse_job(source, &BOUNDS)?;
    let verdict = job_json(&job, &BOUNDS)?;
    assert_eq!(
        verdict.get("name").and_then(Value::as_str),
        Some("MONTHEND"),
        "the JOB card names the job"
    );
    let mut edges: Vec<(String, String)> = verdict
        .get("edges")
        .and_then(Value::as_array)
        .map(|edges| {
            edges
                .iter()
                .filter_map(|edge| {
                    let from = edge.get("from")?.as_str()?.to_owned();
                    let to = edge.get("to")?.as_str()?.to_owned();
                    Some((from, to))
                })
                .collect()
        })
        .ok_or("the JSON names no job edges")?;
    edges.sort();
    let mut expected: Vec<(String, String)> = vec![
        (String::from("MONTHEND"), String::from("PSTEP")),
        (String::from("MONTHEND"), String::from("STEP1")),
        (String::from("STEP1"), String::from("PAYROLL")),
        (String::from("PROD.MONTH.CUST"), String::from("STEP1")),
        (String::from("STEP1"), String::from("PROD.MONTH.REPORT")),
        (String::from("PSTEP"), String::from("REPORTER")),
        (String::from("PSTEP"), String::from("PROD.MONTH.RPT")),
    ];
    expected.sort();
    assert_eq!(
        edges, expected,
        "the job graph must match the cards exactly"
    );
    check_sentences(&verdict, source)
}

/// Proves every DISP status and stream earns its exact direction (#777).
#[test]
fn sim_disp_table_earns_exact_directions() -> TestResult {
    let cases: &[(&str, bool, bool)] = &[
        ("//S1 EXEC PGM=A\n//D DD DSN=X,DISP=SHR\n", true, false),
        ("//S1 EXEC PGM=A\n//D DD DSN=X,DISP=OLD\n", true, true),
        ("//S1 EXEC PGM=A\n//D DD DSN=X,DISP=NEW\n", false, true),
        ("//S1 EXEC PGM=A\n//D DD DSN=X,DISP=MOD\n", false, true),
        ("//S1 EXEC PGM=A\n//D DD DSN=X\n", false, true),
        ("//S1 EXEC PGM=A\n//D DD *\n", true, false),
        ("//S1 EXEC PGM=A\n//D DD DATA\n", true, false),
        ("//S1 EXEC PGM=A\n//D DD SYSOUT=*\n", false, true),
    ];
    for &(cards, reads, writes) in cases.iter() {
        let source = format!("//J JOB X\n{cards}");
        let job = parse_job(source.as_str(), &BOUNDS)?;
        let verdict = job_json(&job, &BOUNDS)?;
        let datasets = verdict
            .get("steps")
            .and_then(Value::as_array)
            .and_then(|steps| steps.first())
            .and_then(|step| step.get("datasets"))
            .and_then(Value::as_array)
            .ok_or("the JSON names no datasets")?;
        let dataset = datasets.first().ok_or("one dataset must be present")?;
        assert_eq!(
            dataset.get("reads").and_then(Value::as_bool),
            Some(reads),
            "DISP case {cards:?} reads"
        );
        assert_eq!(
            dataset.get("writes").and_then(Value::as_bool),
            Some(writes),
            "DISP case {cards:?} writes"
        );
    }
    Ok(())
}

/// Proves `&&TEMP` stays literal and `&HLQ..DATA` substitutes (#777).
#[test]
fn sim_symbolics_keep_temp_and_substitute_hlq() -> TestResult {
    let source = "//J JOB X\n//SETUP SET HLQ=PROD\n//S1 EXEC PGM=A\n//T DD DSN=&&TEMP,DISP=SHR\n//H DD DSN=&HLQ..DATA,DISP=SHR\n";
    let job = parse_job(source, &BOUNDS)?;
    let verdict = job_json(&job, &BOUNDS)?;
    let dsns: Vec<String> = verdict
        .get("steps")
        .and_then(Value::as_array)
        .and_then(|steps| steps.first())
        .and_then(|step| step.get("datasets"))
        .and_then(Value::as_array)
        .map(|datasets| {
            datasets
                .iter()
                .filter_map(|dataset| dataset.get("dsn"))
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .ok_or("the JSON names no datasets")?;
    assert!(
        dsns.contains(&String::from("&&TEMP")),
        "a temporary dataset keeps its && name, got {dsns:?}"
    );
    assert!(
        dsns.contains(&String::from("PROD.DATA")),
        "&HLQ..DATA substitutes with its dot, got {dsns:?}"
    );
    let texts = json_sentences(&verdict)?;
    assert!(
        !texts
            .iter()
            .any(|text| text.contains("&&TEMP") && text.contains("cannot tell")),
        "&&TEMP is resolved, never unresolved, got {texts:?}"
    );
    Ok(())
}

/// The dialect a program headed by `directive` reads as, and how many unknown
/// directives its verdict lists.
fn dialect_of(directive: &str) -> Result<(String, usize), Box<dyn Error>> {
    let source = format!(
        "{directive}\n       IDENTIFICATION DIVISION.\n       PROGRAM-ID. D.\n       PROCEDURE DIVISION.\n"
    );
    let verdict = verdict_of(source.as_str())?;
    let name = verdict
        .get("dialect")
        .and_then(Value::as_str)
        .ok_or("the verdict names no dialect")?;
    Ok((
        String::from(name),
        json_array(&verdict, &["unknown_directives"])?.len(),
    ))
}

/// Proves directives detect each dialect, fixed sequence columns stripped (#777).
#[test]
fn sim_known_answers_pin_dialect_detection() -> TestResult {
    let cases: [(&str, &str, &str); 7] = [
        (
            "       PROCESS APOST.",
            "ibm-enterprise",
            "PROCESS is IBM's own",
        ),
        (
            ">>DEFINE DEBUG-MODE",
            "gnucobol",
            "DEFINE is GnuCOBOL's own",
        ),
        (
            ">>CALL-CONVENTION 0",
            "micro-focus",
            "CALL-CONVENTION is Micro Focus's own",
        ),
        (
            "       $SET MF.",
            "unknown",
            "a shared spelling ties rather than guessing",
        ),
        (
            "000100 $SET MF.",
            "unknown",
            "a sequence-numbered shared spelling still ties",
        ),
        (
            "000200>>SOURCE FORMAT FREE.",
            "unknown",
            "a sequence-numbered >>SOURCE ties",
        ),
        (
            "000100 PROCESS APOST.",
            "ibm-enterprise",
            "a sequence-numbered PROCESS scores IBM",
        ),
    ];
    for (directive, expected, why) in cases {
        assert_eq!(dialect_of(directive)?.0, expected, "{why}");
    }
    let (none, unknown) = dialect_of(">>BOGUS FROBNICATE.")?;
    assert_eq!(none, "unknown", "no hint is unknown, not a guess");
    assert_eq!(unknown, 1, "one unknown directive is one diagnostic");
    Ok(())
}

/// The verdict for `cards` after a PROCEDURE DIVISION header, where only
/// `copybook` resolves, to `body`.
fn with_copybook(copybook: &str, body: &str, cards: &[&str]) -> Result<Value, Box<dyn Error>> {
    let resolve = |name: &str| (name == copybook).then(|| String::from(body));
    let mut lines = vec![fixed("PROCEDURE DIVISION.")];
    lines.extend(cards.iter().map(|card| fixed(card)));
    let program = parse_program(lines.join("\n").as_str(), &BOUNDS, &resolve)?;
    Ok(program_json(&program, &BOUNDS)?)
}

/// The statement texts of a verdict; COPY cards are left out unless `cards`.
fn statement_texts(verdict: &Value, cards: bool) -> Result<Vec<String>, Box<dyn Error>> {
    let mut texts = Vec::new();
    for statement in json_array(verdict, &["statements"])? {
        if !cards && statement.get("kind").and_then(Value::as_str) == Some("copy") {
            continue;
        }
        let text = statement
            .get("text")
            .and_then(Value::as_str)
            .ok_or("a statement carries no text")?;
        texts.push(String::from(text));
    }
    Ok(texts)
}

/// Proves resolved and unresolved COPY nodes both list, with REPLACING (#777).
#[test]
fn sim_copy_resolved_and_unresolved_both_list() -> TestResult {
    let verdict = with_copybook(
        "FIELDS",
        "           MOVE WS-A TO WS-B.\n           PERFORM WORK.\n",
        &["COPY FIELDS REPLACING ==WS-A== BY ==WS-X==.", "COPY NOPE."],
    )?;
    let listed = json_array(&verdict, &["copies"])?;
    assert_eq!(listed.len(), 2, "both COPY cards list");
    let resolved = listed
        .iter()
        .find(|copy| copy.get("name").and_then(Value::as_str) == Some("FIELDS"))
        .ok_or("FIELDS must list")?;
    assert_eq!(
        resolved.get("resolved").and_then(Value::as_bool),
        Some(true),
        "FIELDS resolves"
    );
    let gapped = listed
        .iter()
        .find(|copy| copy.get("name").and_then(Value::as_str) == Some("NOPE"))
        .ok_or("NOPE must list")?;
    assert_eq!(
        gapped.get("resolved").and_then(Value::as_bool),
        Some(false),
        "NOPE stays unresolved"
    );
    let texts = statement_texts(&verdict, true)?;
    assert!(
        texts.iter().any(|text| text.contains("WS-X")),
        "REPLACING rewrites the body, got {texts:?}"
    );
    let sentences = json_sentences(&verdict)?;
    assert!(
        sentences
            .iter()
            .any(|text| text.contains("FIELDS") && text.contains("includes")),
        "the resolved COPY says it includes, got {sentences:?}"
    );
    Ok(())
}

/// Proves bare-word REPLACING rewrites `:PFX:-ID` to `WS-ID` (#777).
#[test]
fn sim_replacing_rewrites_bare_text_words() -> TestResult {
    let verdict = with_copybook(
        "CUSTREC",
        "       05 :PFX:-ID PIC 9(5).\n",
        &["COPY CUSTREC REPLACING ==:PFX:== BY ==WS==."],
    )?;
    let texts = statement_texts(&verdict, false)?;
    assert!(
        texts.iter().any(|text| text.contains("WS-ID")),
        "bare :PFX: rewrites to WS inside :PFX:-ID, got {texts:?}"
    );
    assert!(
        !texts.iter().any(|text| text.contains(":PFX:")),
        "no bare :PFX: survives outside the COPY card, got {texts:?}"
    );
    Ok(())
}

/// Proves `XTAG` never matches `TAG`: word boundaries hold (#777).
#[test]
fn sim_replacing_respects_word_boundaries() -> TestResult {
    let verdict = with_copybook(
        "W",
        "       05 XTAG PIC X(5).\n       05 TAG PIC X(5).\n",
        &["COPY W REPLACING ==TAG== BY ==OUT==."],
    )?;
    let texts = statement_texts(&verdict, true)?;
    assert!(
        texts.iter().any(|text| text.contains("05 OUT ")),
        "the standalone TAG word rewrites to OUT, got {texts:?}"
    );
    assert!(
        texts.iter().any(|text| text.contains("XTAG")),
        "XTAG never matches TAG, got {texts:?}"
    );
    Ok(())
}

/// One line bound of a JSON range, as a one-based line number.
fn bound(range: &Value, end: &str) -> Result<usize, Box<dyn Error>> {
    let number = range
        .get(end)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("a range carries no {end}"))?;
    Ok(usize::try_from(number)?)
}

/// Checks every summary sentence of a verdict against the source: every
/// range sits inside the source, and the covered lines share a word with the
/// sentence, so a sentence with no lines, or lines that prove nothing, fails.
fn check_sentences(verdict: &Value, source: &str) -> TestResult {
    for sentence in json_array(verdict, &["sentences"])? {
        let text = sentence
            .get("text")
            .and_then(Value::as_str)
            .ok_or("a sentence carries no text")?;
        check_sentence(text, json_array(sentence, &["lines"])?, source)?;
    }
    Ok(())
}

/// Checks one summary sentence against the source lines its ranges cover.
fn check_sentence(text: &str, ranges: &[Value], source: &str) -> TestResult {
    assert!(!ranges.is_empty(), "a sentence must rest on lines: {text}");
    let physical: Vec<&str> = source.lines().collect();
    for range in ranges.iter() {
        let (start, end) = (bound(range, "start")?, bound(range, "end")?);
        assert!(start >= 1, "a range starts at one or past it: {text}");
        assert!(end >= start, "a range ends at or past its start: {text}");
        assert!(
            end <= physical.len(),
            "a range ends inside the source: {text}"
        );
        let covered: &[&str] = physical
            .get(start.saturating_sub(1)..end)
            .ok_or_else(|| format!("range {start}-{end} is outside the source"))?;
        let upper = text.to_uppercase();
        let words: Vec<&str> = upper
            .split(|cell: char| !cell.is_ascii_alphanumeric())
            .filter(|word| word.len() >= 3)
            .collect();
        let joined = covered.join(" ").to_uppercase();
        assert!(
            words.iter().any(|word| joined.contains(word)),
            "the lines of {text} share no word with it"
        );
    }
    Ok(())
}
