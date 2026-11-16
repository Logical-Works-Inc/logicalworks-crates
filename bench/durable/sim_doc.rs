//! Simulations of `--write-doc`: a doc a human edits, handed to the binary that
//! fills the block between two marker lines.
//!
//! The markers are spelled out here, apart from `crate::doc`, so a changed
//! spelling fails these tests. The oracles are a decision table written out by
//! hand and a generator that knows by construction which of its lines are live
//! markers; neither is the function's own output.

use std::ffi::OsString;
use std::path::Path;

use crate::args::parse;
use crate::doc::{MAX_DOC_BYTES, check, splice, write};
use crate::error::{BenchError, DocFault};
use crate::sim_kit::{Dice, TestResult, Trace, driven, line, replays, sim_clock, text};
use crate::workdir::Workdir;

/// The start marker line.
const S: &str = "<!-- durable-table:start -->";

/// The end marker line.
const E: &str = "<!-- durable-table:end -->";

/// The table handed in: three lines, no final line ending.
const TABLE: &str = "| measure | lgwks |\n|---|---|\n| engine | x |";

/// The table as the block holds it in a doc with `\n` line endings.
const T_LF: &str = "| measure | lgwks |\n|---|---|\n| engine | x |\n";

/// The table as the block holds it in a doc whose start marker ends `\r\n`.
const T_CRLF: &str = "| measure | lgwks |\r\n|---|---|\r\n| engine | x |\r\n";

/// What a doc must come to: itself with the table in, or the fault.
type Want = Result<String, DocFault>;

/// The hostile docs and what each must come to, written by hand.
fn cases() -> Vec<(&'static str, String, Want)> {
    let bad = |fault| Err(fault);
    vec![
        ("empty doc", String::new(), bad(DocFault::StartMissing)),
        ("prose only", "# t\nno markers\n".to_owned(), bad(DocFault::StartMissing)),
        ("start only", format!("a\n{S}\nold\n"), bad(DocFault::EndMissing)),
        ("end only", format!("a\n{E}\n"), bad(DocFault::StartMissing)),
        ("swapped", format!("{E}\nold\n{S}\n"), bad(DocFault::OutOfOrder)),
        ("start doubled", format!("{S}\nold\n{S}\n{E}\n"), bad(DocFault::StartRepeated)),
        ("end doubled", format!("{S}\nold\n{E}\n{E}\n"), bad(DocFault::EndRepeated)),
        ("both doubled", format!("{S}\n{S}\n{E}\n{E}\n"), bad(DocFault::StartRepeated)),
        ("start doubled, end missing", format!("{S}\n{S}\n"), bad(DocFault::EndMissing)),
        ("end doubled, start missing", format!("{E}\n{E}\n"), bad(DocFault::StartMissing)),
        ("swapped and end doubled", format!("{E}\n{S}\n{E}\n"), bad(DocFault::EndRepeated)),
        (
            "crlf",
            format!("a\r\n{S}\r\nold\r\n{E}\r\nz\r\n"),
            Ok(format!("a\r\n{S}\r\n{T_CRLF}{E}\r\nz\r\n")),
        ),
        (
            "start line ends lf, end line ends crlf",
            format!("a\n{S}\nold\r\n{E}\r\nz\n"),
            Ok(format!("a\n{S}\n{T_LF}{E}\r\nz\n")),
        ),
        (
            "markers shown in a backtick fence, then the real pair",
            format!("```\n{S}\n{E}\n```\n{S}\nold\n{E}\nend\n"),
            Ok(format!("```\n{S}\n{E}\n```\n{S}\n{T_LF}{E}\nend\n")),
        ),
        ("markers only in a fence", format!("```\n{S}\n{E}\n```\n"), bad(DocFault::StartMissing)),
        (
            "tilde fence",
            format!("~~~\n{S}\n~~~\n{S}\n{E}\n"),
            Ok(format!("~~~\n{S}\n~~~\n{S}\n{T_LF}{E}\n")),
        ),
        (
            "a short fence does not close a longer one",
            format!("````\n```\n{S}\n```\n{S}\n{E}\n"),
            bad(DocFault::StartMissing),
        ),
        ("an unclosed fence swallows the end", format!("{S}\n```\n{E}\n"), bad(DocFault::EndMissing)),
        (
            "a closing fence with text after it does not close",
            format!("```\n``` x\n{S}\n{E}\n```\n"),
            bad(DocFault::StartMissing),
        ),
        (
            "a fence indented four spaces is code, not a fence",
            format!("    ```\n{S}\nold\n{E}\n"),
            Ok(format!("    ```\n{S}\n{T_LF}{E}\n")),
        ),
        ("empty block", format!("{S}\n{E}\n"), Ok(format!("{S}\n{T_LF}{E}\n"))),
        ("empty block, crlf", format!("{S}\r\n{E}\r\n"), Ok(format!("{S}\r\n{T_CRLF}{E}\r\n"))),
        ("blank lines between", format!("{S}\n\n\n{E}\n"), Ok(format!("{S}\n{T_LF}{E}\n"))),
        ("end marker ends the file", format!("{S}\nold\n{E}"), Ok(format!("{S}\n{T_LF}{E}"))),
        ("start marker ends the file", S.to_owned(), bad(DocFault::EndMissing)),
        (
            "markers indented and trailed by blanks",
            format!("  {S}\t\nold\n {E}  \n"),
            Ok(format!("  {S}\t\n{T_LF} {E}  \n")),
        ),
        ("markers inside a sentence", format!("see {S} and {E}\n"), bad(DocFault::StartMissing)),
        (
            "marker in other case",
            format!("<!-- DURABLE-TABLE:START -->\n{E}\n"),
            bad(DocFault::StartMissing),
        ),
        ("byte order mark before the start", format!("\u{feff}{S}\n{E}\n"), bad(DocFault::StartMissing)),
        ("NUL before the start", format!("\0{S}\n{E}\n"), bad(DocFault::StartMissing)),
        ("lone carriage returns are no lines", format!("{S}\rold\r{E}\r"), bad(DocFault::StartMissing)),
        (
            "text around the block is kept byte for byte",
            format!("# T\n\ntext  \n{S}\n| a | b |\n|---|---|\nstale\n{E}\n\n## After  \n"),
            Ok(format!("# T\n\ntext  \n{S}\n{T_LF}{E}\n\n## After  \n")),
        ),
    ]
}

/// The fool at the keyboard: thirty-odd hostile docs, each filled or refused as the
/// hand-written table says, and the table handed in with a final line ending
/// fills the same way.
#[test]
fn sim_doc_fat_finger_hostile_docs_fill_or_refuse_by_the_decision_table() {
    for (name, doc, want) in cases() {
        assert_eq!(splice(&doc, TABLE), want, "{name}");
        assert_eq!(splice(&doc, &format!("{TABLE}\n")), want, "{name} with a final line ending");
    }
    assert_eq!(splice(&format!("{S}\nold\n{E}\n"), ""), Ok(format!("{S}\n{E}\n")), "an empty table empties the block");
}

/// A piece of a generated doc and whether it is a live marker line.
struct Piece {
    /// The lines, each ending as the doc's lines do.
    text: String,
    /// What the generator made it.
    live: Live,
}

/// What a generated piece is, known by construction.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Live {
    /// Prose, a stale table row, or a fenced block: never a marker.
    No,
    /// The start marker on a line of its own.
    Start,
    /// The end marker on a line of its own.
    End,
}

/// A line of prose, some of it a decoy that holds a marker without being one.
fn prose(dice: &mut Dice, eol: &str) -> Result<String, String> {
    let lines = [
        "plain words".to_owned(),
        String::new(),
        format!("see {S} above"),
        format!("{E} then words"),
        "<!-- DURABLE-TABLE:START -->".to_owned(),
        "``code`` inline".to_owned(),
        "    ```".to_owned(),
        "| old | row |".to_owned(),
        "\u{200f}rtl mark".to_owned(),
        "~~ struck ~~".to_owned(),
    ];
    let found = lines.get(dice.pick(lines.len())?).ok_or("the draw left the prose")?;
    Ok(format!("{found}{eol}"))
}

/// A fenced block: its markers are decoys, and it closes unless `closed` is false.
fn fenced(dice: &mut Dice, eol: &str, closed: bool) -> Result<String, String> {
    let (glyph, other) = if dice.pick(2)? == 0 { ('`', '~') } else { ('~', '`') };
    let width = 3 + dice.pick(3)?;
    let info = ["", "rust", " sh"];
    let tag = info.get(dice.pick(info.len())?).ok_or("the draw left the info")?;
    let indent = " ".repeat(dice.pick(4)?);
    let mut block = format!("{indent}{}{tag}{eol}", glyph.to_string().repeat(width));
    for _ in 0..dice.pick(5)? {
        let decoys = [
            S.to_owned(),
            E.to_owned(),
            "prose".to_owned(),
            String::new(),
            other.to_string().repeat(3),
            glyph.to_string().repeat(width - 1),
            format!("{} x", glyph.to_string().repeat(width)),
        ];
        let decoy = decoys.get(dice.pick(decoys.len())?).ok_or("the draw left the decoys")?;
        block.push_str(&format!("{}{decoy}{eol}", " ".repeat(dice.pick(4)?)));
    }
    if closed {
        let longer = width + dice.pick(3)?;
        block.push_str(&format!("{}{}{}{eol}", " ".repeat(dice.pick(4)?), glyph.to_string().repeat(longer), " ".repeat(dice.pick(3)?)));
    }
    Ok(block)
}

/// A marker on a line of its own, with the blanks a human leaves around it.
fn live_line(dice: &mut Dice, marker: &str, eol: &str) -> Result<String, String> {
    let trail = ["", " ", "\t"];
    let after = trail.get(dice.pick(trail.len())?).ok_or("the draw left the trail")?;
    Ok(format!("{}{marker}{after}{eol}", " ".repeat(dice.pick(7)?)))
}

/// Drop one final line ending, the way an editor that adds none leaves a file.
fn trim_eol(text: &mut String) {
    if text.ends_with("\r\n") {
        text.truncate(text.len() - 2);
    } else if text.ends_with('\n') {
        text.truncate(text.len() - 1);
    }
}

/// A generated doc and what it must come to: pieces tagged live or not, counted
/// as they were made.
fn generated(dice: &mut Dice) -> Result<(String, Want), String> {
    let eol = if dice.pick(2)? == 0 { "\n" } else { "\r\n" };
    let mut pieces: Vec<Piece> = Vec::new();
    for _ in 0..3 + dice.pick(8)? {
        let text = match dice.pick(4)? {
            0 | 1 => prose(dice, eol)?,
            2 => fenced(dice, eol, true)?,
            _ => format!("| stale | row |{eol}"),
        };
        pieces.push(Piece { text, live: Live::No });
    }
    for (marker, live) in [(S, Live::Start), (E, Live::End)] {
        let counts = [0_usize, 1, 1, 2];
        for _ in 0..*counts.get(dice.pick(counts.len())?).ok_or("the draw left the counts")? {
            let at = dice.pick(pieces.len() + 1)?;
            pieces.insert(at, Piece { text: live_line(dice, marker, eol)?, live });
        }
    }
    if dice.pick(8)? == 0 {
        pieces.push(Piece { text: fenced(dice, eol, false)?, live: Live::No });
    }
    let count = |wanted| pieces.iter().filter(|piece| piece.live == wanted).count();
    let first = |wanted| pieces.iter().position(|piece| piece.live == wanted);
    let (starts, ends) = (count(Live::Start), count(Live::End));
    let mut doc: String = pieces.iter().map(|piece| piece.text.as_str()).collect();
    let mut want = match (first(Live::Start), first(Live::End)) {
        _ if starts == 0 => Err(DocFault::StartMissing),
        _ if ends == 0 => Err(DocFault::EndMissing),
        _ if starts > 1 => Err(DocFault::StartRepeated),
        _ if ends > 1 => Err(DocFault::EndRepeated),
        (Some(start), Some(end)) if end < start => Err(DocFault::OutOfOrder),
        (Some(start), Some(end)) => {
            let rows: String = TABLE.lines().map(|row| format!("{row}{eol}")).collect();
            let head: String = pieces.iter().take(start + 1).map(|piece| piece.text.as_str()).collect();
            let tail: String = pieces.iter().skip(end).map(|piece| piece.text.as_str()).collect();
            Ok(format!("{head}{rows}{tail}"))
        }
        (_, _) => {
            lgwks_std::trace::debug!("a counted marker has no position");
            return Err("a counted marker has no position".to_owned());
        }
    };
    if dice.pick(4)? == 0 {
        trim_eol(&mut doc);
        if let Ok(expected) = want.as_mut() {
            trim_eol(expected);
        }
    }
    Ok((doc, want))
}

/// The monkey: thousands of generated docs mixing prose, decoy fences, doubled and
/// swapped markers, both line endings and a missing final line ending. Each comes
/// to what its construction says. A doc that comes out valid must also be a fixed
/// point: filling it again with the same table changes nothing.
#[test]
fn sim_doc_monkey_generated_docs_fill_or_refuse_as_built() -> TestResult {
    let (mut filled, mut refused) = (0_u32, 0_u32);
    for seed in 1..=600_u64 {
        let mut dice = Dice::new(seed);
        let (doc, want) = generated(&mut dice)?;
        let got = splice(&doc, TABLE);
        assert_eq!(got, want, "seed {seed}: {doc:?}");
        match got {
            Ok(done) => {
                filled += 1;
                assert_eq!(splice(&done, TABLE), Ok(done.clone()), "seed {seed}: not a fixed point");
            }
            Err(_fault) => refused += 1,
        }
    }
    assert!(filled > 60 && refused > 60, "the draws were lopsided: {filled} filled, {refused} refused");
    Ok(())
}

/// One seed, one set of docs and verdicts: the trace hashes agree on a second run.
#[test]
fn sim_doc_replays_from_one_seed() -> TestResult {
    fn scenario(seed: u64) -> Result<u64, String> {
        let mut dice = Dice::new(seed);
        let mut trace = Trace::new();
        for _ in 0..80 {
            let (doc, _) = generated(&mut dice)?;
            trace.note(&doc);
            trace.note(&format!("{:?}", splice(&doc, TABLE)));
        }
        Ok(trace.hash())
    }
    for seed in [3, 33, 333] {
        replays(scenario, seed)?;
    }
    Ok(())
}

/// The names in `dir`, sorted.
fn names_in(dir: &Path) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(text)? {
        names.push(entry.map_err(text)?.file_name().to_string_lossy().into_owned());
    }
    names.sort();
    Ok(names)
}

/// The saboteur: through the real files, every hostile doc is either filled as the
/// table says or refused with its fault and left byte for byte as it was, and in
/// both cases the directory holds the doc and nothing else, no temp file left.
/// `check` agrees with the verdict.
#[test]
fn sim_doc_saboteur_every_verdict_leaves_the_doc_and_the_directory_clean() -> TestResult {
    let outer = Workdir::create(None).map_err(line)?;
    for (index, (name, doc, want)) in cases().into_iter().enumerate() {
        let dir = outer.path().join(format!("case-{index}"));
        std::fs::create_dir(&dir).map_err(text)?;
        let path = dir.join("doc.md");
        std::fs::write(&path, &doc).map_err(text)?;
        let written = write(&path, TABLE);
        let checked = check(&path);
        let after = std::fs::read_to_string(&path).map_err(text)?;
        assert_eq!(names_in(&dir)?, ["doc.md"], "{name}: litter");
        match want {
            Ok(expected) => {
                assert!(written.is_ok(), "{name}: {written:?}");
                assert_eq!(after, expected, "{name}");
                assert!(checked.is_ok(), "{name}: check refused a doc write filled");
            }
            Err(fault) => {
                let refusal = matches!(&written, Err(BenchError::DocMarkers { fault: got, .. }) if *got == fault);
                assert!(refusal, "{name}: {written:?}");
                assert!(matches!(&checked, Err(BenchError::DocMarkers { fault: got, .. }) if *got == fault), "{name}: {checked:?}");
                assert_eq!(after, doc, "{name}: a refused doc was changed");
            }
        }
    }
    Ok(())
}

/// The double-clicker: filling twice changes nothing the second time; a different
/// table replaces the first and never piles up; a second doc beside it is never
/// touched.
#[test]
fn sim_doc_double_clicker_second_write_is_a_fixed_point_and_a_new_table_replaces() -> TestResult {
    let outer = Workdir::create(None).map_err(line)?;
    let (path, bystander) = (outer.path().join("doc.md"), outer.path().join("other.md"));
    let original = format!("# t\n{S}\nstale\n{E}\nend\n");
    std::fs::write(&path, &original).map_err(text)?;
    std::fs::write(&bystander, &original).map_err(text)?;
    write(&path, TABLE).map_err(line)?;
    let once = std::fs::read_to_string(&path).map_err(text)?;
    write(&path, TABLE).map_err(line)?;
    assert_eq!(std::fs::read_to_string(&path).map_err(text)?, once);
    assert_eq!(once, format!("# t\n{S}\n{T_LF}{E}\nend\n"));
    write(&path, "| big |\n| a |\n| b |\n| c |\n| d |").map_err(line)?;
    write(&path, "| small |").map_err(line)?;
    assert_eq!(std::fs::read_to_string(&path).map_err(text)?, format!("# t\n{S}\n| small |\n{E}\nend\n"));
    assert_eq!(std::fs::read_to_string(&bystander).map_err(text)?, original, "a neighbour doc was touched");
    assert_eq!(names_in(outer.path())?, ["doc.md", "other.md"]);
    Ok(())
}

/// The pulled plug: a temp name that cannot be written ends the write typed, with
/// the doc as it was; once the obstacle is gone the same write goes through.
#[test]
fn sim_doc_pulled_plug_a_failed_write_leaves_the_doc_as_it_was() -> TestResult {
    let outer = Workdir::create(None).map_err(line)?;
    let path = outer.path().join("doc.md");
    let original = format!("{S}\nstale\n{E}\n");
    std::fs::write(&path, &original).map_err(text)?;
    let obstacle = outer.path().join(format!("doc.md.{}.tmp", std::process::id()));
    std::fs::create_dir(&obstacle).map_err(text)?;
    let refused = write(&path, TABLE);
    assert!(matches!(refused, Err(BenchError::Io { context: "writing the doc's temp file", .. })), "{refused:?}");
    assert_eq!(std::fs::read_to_string(&path).map_err(text)?, original);
    std::fs::remove_dir(&obstacle).map_err(text)?;
    write(&path, TABLE).map_err(line)?;
    assert_eq!(std::fs::read_to_string(&path).map_err(text)?, format!("{S}\n{T_LF}{E}\n"));
    assert_eq!(names_in(outer.path())?, ["doc.md"]);
    Ok(())
}

/// Giant and empty: a doc of exactly the bound is taken, one byte more and ten times
/// more are refused as too large, and an empty file and a missing file and a
/// directory in the doc's place each end typed.
#[test]
fn sim_doc_giant_and_empty_the_size_bound_binds() -> TestResult {
    let outer = Workdir::create(None).map_err(line)?;
    let path = outer.path().join("doc.md");
    let bound = usize::try_from(MAX_DOC_BYTES).map_err(text)?;
    let head = format!("{S}\n{E}\n");
    for (extra, taken) in [(0_usize, true), (1, false), (9 * bound, false)] {
        let doc = format!("{head}{}", "x".repeat(bound - head.len() + extra));
        std::fs::write(&path, &doc).map_err(text)?;
        let verdict = write(&path, TABLE);
        if taken {
            verdict.map_err(line)?;
            assert!(std::fs::read_to_string(&path).map_err(text)?.contains(T_LF));
        } else {
            let large = matches!(&verdict, Err(BenchError::DocMarkers { fault: DocFault::TooLarge, .. }));
            assert!(large, "{extra}: {verdict:?}");
            assert_eq!(std::fs::read_to_string(&path).map_err(text)?, doc, "a too large doc was changed");
        }
    }
    std::fs::write(&path, "").map_err(text)?;
    let empty = write(&path, TABLE);
    assert!(matches!(empty, Err(BenchError::DocMarkers { fault: DocFault::StartMissing, .. })), "{empty:?}");
    let absent = write(&outer.path().join("absent.md"), TABLE);
    assert!(matches!(absent, Err(BenchError::Io { .. })), "{absent:?}");
    let directory = check(outer.path());
    assert!(matches!(directory, Err(BenchError::Io { .. })), "{directory:?}");
    Ok(())
}

/// Weird bytes: a doc that is not UTF-8 is refused typed and left alone, with the
/// markers intact around the bad bytes.
#[test]
fn sim_doc_weird_bytes_a_doc_that_is_not_utf8_is_refused_unchanged() -> TestResult {
    let outer = Workdir::create(None).map_err(line)?;
    let path = outer.path().join("doc.md");
    let mut bytes = format!("{S}\n").into_bytes();
    bytes.extend_from_slice(&[0xff, 0xfe, 0xc0, 0x80, b'\n']);
    bytes.extend_from_slice(format!("{E}\n").as_bytes());
    std::fs::write(&path, &bytes).map_err(text)?;
    let refused = write(&path, TABLE);
    assert!(matches!(refused, Err(BenchError::DocMarkers { fault: DocFault::NotUtf8, .. })), "{refused:?}");
    assert_eq!(std::fs::read(&path).map_err(text)?, bytes);
    assert_eq!(names_in(outer.path())?, ["doc.md"]);
    Ok(())
}

/// The command line as the OS hands it.
fn argv(words: &[&str]) -> Vec<OsString> {
    words.iter().map(OsString::from).collect()
}

/// Fat finger: `--write-doc` with no path is a missing value, and with no rows
/// there is no table to write, each a usage refusal that exits 2.
#[test]
fn sim_doc_fat_finger_the_flag_needs_a_path_and_rows() {
    let bare = parse(argv(&["--rows", "lgwks", "--write-doc"]));
    assert!(matches!(bare, Err(BenchError::MissingValue { flag: "--write-doc" })), "{bare:?}");
    for words in [&["--write-doc", "d.md"][..], &["--write-doc", "d.md", "--json"], &["--write-doc", "d.md", "--recover"]] {
        let refused = parse(argv(words));
        assert!(matches!(&refused, Err(error) if error.exit_code() == 2), "{words:?}: {refused:?}");
    }
    let taken = parse(argv(&["--rows", "lgwks", "--write-doc", "d.md", "--json"]));
    assert!(matches!(&taken, Ok(args) if args.write_doc.is_some() && args.json), "{taken:?}");
}

/// Wrong order: a doc that cannot take the table is refused before the run starts,
/// ahead of even the work directory, and a good doc is not: the same missing work
/// directory is then the failure.
#[test]
fn sim_doc_wrong_order_a_bad_doc_is_refused_before_the_run() -> TestResult {
    let outer = Workdir::create(None).map_err(line)?;
    let (bad, good) = (outer.path().join("bad.md"), outer.path().join("good.md"));
    std::fs::write(&bad, format!("{E}\n{S}\n")).map_err(text)?;
    std::fs::write(&good, format!("{S}\n{E}\n")).map_err(text)?;
    let missing = outer.path().join("absent");
    let (missing, bad_path, good_path) = (
        missing.to_str().ok_or("a path is not UTF-8")?,
        bad.to_str().ok_or("a path is not UTF-8")?,
        good.to_str().ok_or("a path is not UTF-8")?,
    );
    let clock = sim_clock();
    for (doc, want_doc_fault) in [(bad_path, true), (good_path, false)] {
        let args = parse(argv(&["--rows", "lgwks", "--runs", "5", "--work-dir", missing, "--write-doc", doc])).map_err(line)?;
        let verdict = driven(&clock, || crate::table(&args, &clock));
        if want_doc_fault {
            assert!(matches!(verdict, Err(BenchError::DocMarkers { fault: DocFault::OutOfOrder, .. })), "{verdict:?}");
        } else {
            assert!(matches!(verdict, Err(BenchError::UnreadablePath { .. })), "{verdict:?}");
        }
    }
    assert_eq!(std::fs::read_to_string(&bad).map_err(text)?, format!("{E}\n{S}\n"));
    Ok(())
}

/// The journey: `--rows lgwks --write-doc` through the binary's own `table` runs the
/// flow and leaves the doc with the Markdown table, deploy line included, between
/// its markers and the rest of the doc untouched.
#[test]
fn sim_doc_journey_the_run_lands_in_the_doc() -> TestResult {
    let outer = Workdir::create(None).map_err(line)?;
    let (path, work) = (outer.path().join("doc.md"), outer.path().join("work"));
    std::fs::create_dir(&work).map_err(text)?;
    std::fs::write(&path, format!("# Doc\n\n{S}\nstale\n{E}\n\nafter\n")).map_err(text)?;
    let (doc_arg, work_arg) = (path.to_str().ok_or("a path is not UTF-8")?, work.to_str().ok_or("a path is not UTF-8")?);
    let args = parse(argv(&["--rows", "lgwks", "--runs", "20", "--work-dir", work_arg, "--write-doc", doc_arg])).map_err(line)?;
    let clock = sim_clock();
    driven(&clock, || crate::table(&args, &clock)).map_err(line)?;
    let after = std::fs::read_to_string(&path).map_err(text)?;
    assert!(after.starts_with(&format!("# Doc\n\n{S}\n| measure | lgwks |\n|---|---|\n")), "{after}");
    assert!(after.ends_with(&format!("{E}\n\nafter\n")), "{after}");
    assert!(!after.contains("stale"), "{after}");
    let deploy = after
        .lines()
        .find(|row| row.starts_with("| deploy: start to ready (samples) | "))
        .ok_or("the doc has no deploy line")?;
    assert!(deploy.ends_with(" ms p50 (1 sample) |"), "{deploy}");
    assert!(!after.contains("cold build"), "{after}");
    Ok(())
}
