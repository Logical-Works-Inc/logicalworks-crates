//! Seeded fools against the runtime census, `docs/census-runtime.md` (#1014).
//!
//! The census is one table: a row per top-level module of `crates/lgwks-bot/src`,
//! a `src total` row the module rows sum to, a `tests/` row, and a `crate total`
//! row that is `src total` plus `tests/`. The doc also says how many lines are
//! blank or plain comments, which is the crate total less code, doc and test.
//!
//! The reference model is the mutation itself. The bumper adds to one module
//! cell, the dropper deletes a row, the copier writes a row twice, the renamer
//! points a row at a module that does not exist, and the tests bumper moves the
//! `tests/` row. Every seed also shuffles the module rows, which the census
//! must accept. The check names each fault and nothing else.
//!
//! Source: #1014's proof ("the table's totals equal `wc -l` over `git
//! ls-files`") and SD-7, a doc claim with no enforcement.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

use crate::seed::{Rng, Trace};

/// A test's result type.
type TestResult = Result<(), Box<dyn Error>>;

/// How many mutated tables each run judges; each is one string, no disk.
const SEEDS: u64 = 2_048;

/// The census this test reads.
const DOC: &str = "docs/census-runtime.md";

/// The source tree the census counts.
const SRC: &str = "crates/lgwks-bot/src";

/// The first cells of the table's header line.
const HEADER: &str = "| Module | Files | Lines |";

/// The columns the module rows sum to in `src total`.
const SUMMED: [&str; 8] = [
    "Files", "Lines", "Code", "Doc", "Test", "Pub", "Local", "Unref",
];

/// The columns `crate total` holds as `src total` plus `tests/`.
const CRATE: [&str; 5] = ["Files", "Lines", "Code", "Doc", "Test"];

/// The phrase before the blank and comment line count.
const OTHER: &str = "count only in Lines: ";

/// One table row: its first cell and the rest, as written.
#[derive(Clone, Debug, PartialEq)]
struct Row {
    name: String,
    cells: Vec<String>,
}

/// The census table and the blank and comment count its text states.
#[derive(Clone, Debug)]
struct Census {
    columns: Vec<String>,
    rows: Vec<Row>,
    other: Option<u64>,
}

/// The repository root.
fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The cells of one `|`-delimited line, trimmed, without the outer bars.
fn cells(line: &str) -> Vec<String> {
    let inner = line.trim().trim_start_matches('|').trim_end_matches('|');
    inner
        .split('|')
        .map(|cell| cell.trim().to_owned())
        .collect()
}

/// The count written after [`OTHER`], commas allowed.
fn stated_other(doc: &str) -> Option<u64> {
    let (_, rest) = doc.split_once(OTHER)?;
    let digits: String = rest
        .chars()
        .take_while(|ch| ch.is_ascii_digit() || *ch == ',')
        .filter(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

/// The census in `doc`: the table under [`HEADER`] and the stated count.
fn census(doc: &str) -> Census {
    let mut lines = doc.lines().skip_while(|line| !line.starts_with(HEADER));
    let columns = match lines.next() {
        Some(header) => cells(header).into_iter().skip(1).collect(),
        None => Vec::new(),
    };
    let rows = lines
        .skip(1)
        .take_while(|line| line.starts_with('|'))
        .filter_map(|line| {
            let mut all = cells(line).into_iter();
            let name = all.next()?;
            Some(Row {
                name,
                cells: all.collect(),
            })
        })
        .collect();
    Census {
        columns,
        rows,
        other: stated_other(doc),
    }
}

/// `census` written back as the doc's table and count sentence.
fn render(census: &Census) -> Result<String, std::fmt::Error> {
    let mut out = String::from(HEADER);
    for column in census.columns.iter().skip(2) {
        write!(out, " {column} |")?;
    }
    out.push('\n');
    out.push_str(&"|---".repeat(census.columns.len().saturating_add(1)));
    out.push_str("|\n");
    for row in &census.rows {
        write!(out, "| {} |", row.name)?;
        for cell in &row.cells {
            write!(out, " {cell} |")?;
        }
        out.push('\n');
    }
    if let Some(other) = census.other {
        writeln!(
            out,
            "\nBlank lines and plain `//` comments {OTHER}{other} of them."
        )?;
    }
    Ok(out)
}

/// The module name a row names, when it is a module row.
fn module_of(row: &Row) -> Option<&str> {
    let name = row.name.strip_prefix('`')?.strip_suffix('`')?;
    (name != "tests/").then_some(name)
}

/// The value of `column` in `row`, or why there is none.
fn value(census: &Census, row: &Row, column: &str) -> Result<u64, String> {
    let at = census
        .columns
        .iter()
        .position(|name| name == column)
        .ok_or_else(|| format!("the header has no {column}"))?;
    let cell = row
        .cells
        .get(at)
        .ok_or_else(|| format!("{} has no {column}", row.name))?;
    cell.parse()
        .map_err(|error| format!("{} {column} is not a number: {error}", row.name))
}

/// The row named `name`, or why there is none.
fn named<'a>(census: &'a Census, name: &str) -> Result<&'a Row, String> {
    census
        .rows
        .iter()
        .find(|row| row.name == name)
        .ok_or_else(|| format!("no {name} row"))
}

/// The module rows of `census`.
fn modules(census: &Census) -> Vec<&Row> {
    census
        .rows
        .iter()
        .filter(|row| module_of(row).is_some())
        .collect()
}

/// Every way `src total` disagrees with the module rows, into `found`.
fn src_sums(census: &Census, src: &Row, found: &mut BTreeSet<String>) {
    for column in SUMMED {
        let mut sum = 0u64;
        for row in modules(census) {
            match value(census, row, column) {
                Ok(cell) => sum = sum.saturating_add(cell),
                Err(problem) => {
                    found.insert(problem);
                }
            }
        }
        match value(census, src, column) {
            Ok(stated) => {
                if stated != sum {
                    found.insert(format!(
                        "src total {column} is {stated}, the modules sum to {sum}"
                    ));
                }
            }
            Err(problem) => {
                found.insert(problem);
            }
        }
    }
}

/// Every way `crate total` and the stated count disagree, into `found`.
fn crate_sums(census: &Census, rows: [&Row; 3], found: &mut BTreeSet<String>) {
    let [src, tests, total] = rows;
    let mut stated_cells = Vec::new();
    for column in CRATE {
        let parts = [
            value(census, src, column),
            value(census, tests, column),
            value(census, total, column),
        ];
        match parts {
            [Ok(src_cell), Ok(tests_cell), Ok(stated)] => {
                let sum = src_cell.saturating_add(tests_cell);
                if stated != sum {
                    found.insert(format!(
                        "crate total {column} is {stated}, src total and tests/ sum to {sum}"
                    ));
                }
                stated_cells.push(stated);
            }
            broken => {
                for part in broken {
                    if let Err(problem) = part {
                        found.insert(problem);
                    }
                }
            }
        }
    }
    if let (&[_, lines, code, doc, test], Some(stated)) = (stated_cells.as_slice(), census.other) {
        let left = lines.saturating_sub(code.saturating_add(doc).saturating_add(test));
        if stated != left {
            found.insert(format!(
                "the text says {stated} other lines, the crate total leaves {left}"
            ));
        }
    }
}

/// Every way the table's arithmetic fails, sorted.
fn sums(census: &Census) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut seen = BTreeSet::new();
    for row in modules(census) {
        if !seen.insert(row.name.as_str()) {
            found.insert(format!("{} appears twice", row.name));
        }
    }
    let parts = [
        named(census, "src total"),
        named(census, "`tests/`"),
        named(census, "crate total"),
    ];
    match parts {
        [Ok(src), Ok(tests), Ok(total)] => {
            src_sums(census, src, &mut found);
            crate_sums(census, [src, tests, total], &mut found);
        }
        broken => {
            for part in broken {
                if let Err(problem) = part {
                    found.insert(problem);
                }
            }
        }
    }
    found
}

/// Every module row that names nothing under the source tree, sorted.
fn strays(census: &Census, present: &BTreeSet<String>) -> BTreeSet<String> {
    census
        .rows
        .iter()
        .filter_map(module_of)
        .filter(|name| !present.contains(*name))
        .map(|name| format!("`{name}` names no module under {SRC}"))
        .collect()
}

/// The top-level module names under the source tree: each `x.rs` and each directory `x`.
fn present() -> Result<BTreeSet<String>, Box<dyn Error>> {
    let mut found = BTreeSet::new();
    for entry in fs::read_dir(root().join(SRC))? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type()?.is_dir() {
            found.insert(name);
        } else if let Some(stem) = name.strip_suffix(".rs") {
            found.insert(stem.to_owned());
        }
    }
    Ok(found)
}

/// The committed census.
fn committed() -> Result<Census, Box<dyn Error>> {
    Ok(census(&fs::read_to_string(root().join(DOC))?))
}

/// The indexes, among all rows, of the module rows.
fn module_slots(census: &Census) -> Vec<usize> {
    census
        .rows
        .iter()
        .enumerate()
        .filter(|&(_, row)| module_of(row).is_some())
        .map(|(at, _)| at)
        .collect()
}

/// A uniform draw from `slots`.
fn draw(rng: &mut Rng, slots: &[usize]) -> Result<usize, Box<dyn Error>> {
    let pick = usize::try_from(rng.below(u32::try_from(slots.len())?))?;
    Ok(*slots.get(pick).ok_or("a draw past the slots")?)
}

/// Adds `delta` to `column` of row `at`.
fn bump(census: &mut Census, at: usize, column: &str, delta: u64) -> TestResult {
    let col = census
        .columns
        .iter()
        .position(|name| name == column)
        .ok_or("no such column")?;
    let row = census.rows.get_mut(at).ok_or("no such row")?;
    let cell = row.cells.get_mut(col).ok_or("no such cell")?;
    let old: u64 = cell.parse()?;
    *cell = old.checked_add(delta).ok_or("a bump past u64")?.to_string();
    Ok(())
}

/// The expected `src total` refusal for `column` when the modules sum to `stated` moved by `cell`.
fn moved(column: &str, stated: u64, sum: Option<u64>) -> Result<String, Box<dyn Error>> {
    let sum = sum.ok_or("a sum past u64")?;
    Ok(format!(
        "src total {column} is {stated}, the modules sum to {sum}"
    ))
}

/// The refusals one module row's cells cause when they are counted `twice` or dropped.
fn row_moves(clean: &Census, row: &Row, twice: bool) -> Result<BTreeSet<String>, Box<dyn Error>> {
    let src = named(clean, "src total")?;
    let mut expected = BTreeSet::new();
    for column in SUMMED {
        let cell = value(clean, row, column)?;
        if cell > 0 {
            let stated = value(clean, src, column)?;
            let sum = if twice {
                stated.checked_add(cell)
            } else {
                stated.checked_sub(cell)
            };
            expected.insert(moved(column, stated, sum)?);
        }
    }
    Ok(expected)
}

/// The expected problems of one seed: (sums, strays).
type Expected = (BTreeSet<String>, BTreeSet<String>);

/// Applies one seeded fault to `census` and names what the check must say.
fn fault(clean: &Census, census: &mut Census, rng: &mut Rng) -> Result<Expected, Box<dyn Error>> {
    let mut sums_expected = BTreeSet::new();
    let mut strays_expected = BTreeSet::new();
    let at = draw(rng, &module_slots(census))?;
    let src = named(clean, "src total")?;
    match rng.below(5) {
        0 => {
            let column = SUMMED
                .get(usize::try_from(rng.below(8))?)
                .ok_or("a draw past the columns")?;
            let delta = u64::from(rng.between(1, 1_000));
            bump(census, at, column, delta)?;
            let stated = value(clean, src, column)?;
            sums_expected.insert(moved(column, stated, stated.checked_add(delta))?);
        }
        1 => {
            let row = census.rows.get(at).ok_or("no such row")?.clone();
            census.rows = census
                .rows
                .iter()
                .enumerate()
                .filter(|&(index, _)| index != at)
                .map(|(_, kept)| kept.clone())
                .collect();
            sums_expected = row_moves(clean, &row, false)?;
        }
        2 => {
            let row = census.rows.get(at).ok_or("no such row")?.clone();
            sums_expected = row_moves(clean, &row, true)?;
            sums_expected.insert(format!("{} appears twice", row.name));
            census.rows.push(row);
        }
        3 => {
            let gone = format!("gone_{}", rng.below(1_000_000));
            census.rows.get_mut(at).ok_or("no such row")?.name = format!("`{gone}`");
            strays_expected.insert(format!("`{gone}` names no module under {SRC}"));
        }
        _ => {
            let tests_at = census
                .rows
                .iter()
                .position(|row| row.name == "`tests/`")
                .ok_or("no tests row")?;
            let delta = u64::from(rng.between(1, 1_000));
            bump(census, tests_at, "Lines", delta)?;
            let stated = value(clean, named(clean, "crate total")?, "Lines")?;
            let sum = value(clean, src, "Lines")?
                .checked_add(value(clean, named(clean, "`tests/`")?, "Lines")?)
                .and_then(|sum| sum.checked_add(delta))
                .ok_or("a sum past u64")?;
            sums_expected.insert(format!(
                "crate total Lines is {stated}, src total and tests/ sum to {sum}"
            ));
        }
    }
    Ok((sums_expected, strays_expected))
}

/// One seed's mutated census, shuffled, and the problems the model expects of it.
fn mutate(clean: &Census, rng: &mut Rng) -> Result<(Census, Expected), Box<dyn Error>> {
    let mut census = clean.clone();
    let expected = fault(clean, &mut census, rng)?;
    let slots = module_slots(&census);
    for _ in 0..slots.len() {
        let first = draw(rng, &slots)?;
        let second = draw(rng, &slots)?;
        census.rows.swap(first, second);
    }
    Ok((census, expected))
}

/// One seed judged against the model; its trace is the problems it named.
fn one(clean: &Census, present: &BTreeSet<String>, seed: u64) -> Result<u64, Box<dyn Error>> {
    let mut rng = Rng::new(seed);
    let (mutated, (sums_expected, strays_expected)) = mutate(clean, &mut rng)?;
    let reread = census(&render(&mutated)?);
    let found_sums = sums(&reread);
    let found_strays = strays(&reread, present);
    assert_eq!(
        found_sums, sums_expected,
        "seed {seed}: the sums against the model"
    );
    assert_eq!(
        found_strays, strays_expected,
        "seed {seed}: the module names against the model"
    );
    let mut trace = Trace::new();
    trace.record_number("seed", seed);
    for problem in found_sums.iter().chain(&found_strays) {
        trace.record(problem);
    }
    Ok(trace.hash())
}

/// The committed census adds up, and every seeded bump, drop, copy, rename and
/// tests bump is judged as the model expects under a shuffle of the module
/// rows, with the same seed replaying to the same trace.
#[test]
fn sim_census_rows_sum_to_their_totals() -> TestResult {
    let clean = committed()?;
    let present = present()?;
    assert!(
        clean.rows.len() > 30 && clean.columns.len() >= SUMMED.len(),
        "the table is read: {} rows, {:?}",
        clean.rows.len(),
        clean.columns
    );
    assert!(clean.other.is_some(), "the blank and comment count is read");
    assert_eq!(
        sums(&clean),
        BTreeSet::new(),
        "the committed census adds up"
    );
    assert_eq!(
        sums(&census(&render(&clean)?)),
        BTreeSet::new(),
        "the table renders back to a table that adds up"
    );
    for seed in 0..SEEDS {
        let first = one(&clean, &present, seed)?;
        let again = one(&clean, &present, seed)?;
        assert_eq!(first, again, "seed {seed} replays to the same trace");
    }
    Ok(())
}

/// Every module the committed census names exists under the source tree, so a
/// later leaf that deletes or moves a module must edit the census; a planted
/// name is refused, and a wrong blank-line count is named.
#[test]
fn the_census_names_only_modules_that_exist() -> TestResult {
    let clean = committed()?;
    let present = present()?;
    assert!(
        present.contains("lib") && present.contains("bin"),
        "the walk reads files and directories"
    );
    assert_eq!(
        strays(&clean, &present),
        BTreeSet::new(),
        "every census row names a module"
    );
    let mut planted = clean.clone();
    planted.rows.push(Row {
        name: String::from("`ghost`"),
        cells: Vec::new(),
    });
    assert_eq!(
        strays(&planted, &present),
        BTreeSet::from([format!("`ghost` names no module under {SRC}")]),
    );
    let mut miscounted = clean;
    let stated = miscounted.other.ok_or("no stated count")?;
    let wrong = stated.checked_add(1).ok_or("a count past u64")?;
    miscounted.other = Some(wrong);
    assert_eq!(
        sums(&miscounted),
        BTreeSet::from([format!(
            "the text says {wrong} other lines, the crate total leaves {stated}"
        )]),
    );
    Ok(())
}
