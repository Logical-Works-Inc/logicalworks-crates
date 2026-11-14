//! The #908 dodge table, run through the shipped `lgwks-ast script check`.
//!
//! #908 reproduced fourteen lines: five controls the checker refused and nine
//! spellings of the same defects it accepted. This test is that table. Each
//! row is planted in its own flow and checked by the binary a user runs, so a
//! row's verdict is what the user sees, exit status included.
//!
//! Twelve rows are refused now. Two stay accepted until the step that owns
//! them lands: a call into a crate outside the script needs the allowlist
//! (#908 step 3), and a `run` inside a Rust-braced `for` needs the step-key
//! check (#908 step 4). When one of those lands, its row here fails and is
//! flipped to `Refused` in the same change: that is the red-to-green signal
//! the issue asks for. After the table come its neighbours: more spellings
//! of a refused class, and near misses a refusal by structure must not catch.

#![cfg(feature = "tool")]

use std::error::Error;

use crate::cli::{plant, text, tool};
use crate::scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

/// What the checker must say about one row.
#[derive(Debug, Clone, Copy)]
enum Verdict {
    /// Refused, with this fragment in the message.
    Refused(&'static str),
    /// Accepted today, and the #908 step that will refuse it.
    Open(&'static str),
    /// Accepted, and it must stay accepted: a neighbour of a refused row.
    Stands,
}

/// A panicking method, by any spelling.
const PANICS: Verdict = Verdict::Refused("ends the program on failure");
/// A task with no owner.
const SPAWNS: Verdict = Verdict::Refused("spawned task has no owner");
/// I/O around the capability gate.
const BYPASSES: Verdict = Verdict::Refused("bypasses the capability gate");
/// A fan-out bound typed as a number.
const TYPED: Verdict = Verdict::Refused("typed concurrency number");
/// A loop that never ends.
const ENDLESS: Verdict = Verdict::Refused("runs its loop forever");
/// A `use` that renames a refused call.
const RENAMES: Verdict = Verdict::Refused("`use` of `fs`");
/// Accepted until #908 step 3, the allowlist.
const ALLOWLIST: Verdict = Verdict::Open("step 3, the allowlist");
/// Accepted until #908 step 4, step keys.
const STEP_KEYS: Verdict = Verdict::Open("step 4, step keys");
/// A thread stalled in place.
const STALLS: Verdict = Verdict::Refused("stalls every sibling");
/// The process ended from a flow.
const EXITS: Verdict = Verdict::Refused("ending the process");
/// An owned value leaked.
const LEAKS: Verdict = Verdict::Refused("leaks what the value owns");
/// A near miss that must stay accepted.
const STANDS: Verdict = Verdict::Stands;

/// The table in #908's order, then its neighbours. A row ending in `:` opens
/// a block, which [`script`] gives a body.
const ROWS: [(&str, Verdict); 30] = [
    ("xs.first().unwrap()", PANICS),
    ("tokio::spawn(async {})", SPAWNS),
    ("std::fs::read(\"x\")", BYPASSES),
    ("loop { }", Verdict::Refused("a loop with no bound")),
    ("each x in xs, at most 16 at once:", TYPED),
    ("xs.first().r#unwrap()", PANICS),
    ("tokio::r#spawn(async {})", SPAWNS),
    ("std::r#fs::read(\"x\")", BYPASSES),
    ("use std::{fs}; fs::read(\"x\")", RENAMES),
    ("tokio::task::spawn_blocking(|| ())", SPAWNS),
    ("for _ in 0u64.. { }", ENDLESS),
    ("each x in xs, at most (16) at once:", TYPED),
    ("reqwest::get(\"http://x\").await", ALLOWLIST),
    ("for x in &xs { let _p = run price(x); }", STEP_KEYS),
    // Neighbours: the same classes under other spellings.
    ("std::thread::r#sleep(d)", STALLS),
    ("std::process::r#exit(0)", EXITS),
    ("std::mem::r#forget(x)", LEAKS),
    ("for _ in (0u64).. { }", ENDLESS),
    ("each x in xs, at most ((16)) at once:", TYPED),
    ("each x in xs, at most (8 * 2) at once:", TYPED),
    // Near misses: a name that is not the refused path, a bounded range, a
    // bound that names its source or says one at a time.
    ("pace::sleep(d)", STANDS),
    ("job::exit(0)", STANDS),
    ("cache::forget(x)", STANDS),
    ("xs.get(1..)", STANDS),
    ("for i in 0..xs.len() { }", STANDS),
    ("each x in xs, at most 1 at once:", STANDS),
    ("each x in xs, at most (1) at once:", STANDS),
    ("each x in xs, at most (xs.len()) at once:", STANDS),
    ("each x in xs, at most (xs.len() * 2) at once:", STANDS),
    (
        "use std::collections::BTreeMap; BTreeMap::<u8, u8>::new()",
        STANDS,
    ),
];

/// The script one row is planted in; a row that opens a block gets one.
fn script(row: &str) -> String {
    let body = match row.ends_with(':') {
        true => format!("{row}\n            x\n        give back 1"),
        false => row.to_owned(),
    };
    format!("lgwks_macros::script! {{\n    flow f(xs: Vec<u8>) -> u8:\n        {body}\n}}\n")
}

#[test]
fn every_row_of_the_dodge_table_gets_its_verdict_from_the_shipped_checker() -> TestResult {
    let tree = Scratch::new("script-dodges")?;
    let mut wrong = Vec::new();
    for (index, &(body, verdict)) in ROWS.iter().enumerate() {
        let file = format!("row{index:02}.rs");
        plant(tree.path(), &file, &script(body))?;
        let output = tool(tree.path(), &["script", "check", &file])?;
        let (out, err) = text(&output);
        let code = output.status.code();
        let accepted = code == Some(0) && format!("{out}{err}").contains("nothing refused");
        let miss = match verdict {
            Verdict::Refused(fragment) => (code != Some(2) || !out.contains(fragment))
                .then(|| format!("wanted a refusal naming `{fragment}`")),
            Verdict::Open(step) => (!accepted)
                .then(|| format!("is refused now: #908 {step} landed, flip this row to `Refused`")),
            Verdict::Stands => (!accepted).then(|| "must stay accepted".to_owned()),
        };
        if let Some(why) = miss {
            wrong.push(format!(
                "row {index} `{body}` {why}; exit {code:?}:\n{out}{err}"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} of {} rows got another verdict (an `Open` row now refused is flipped to `Refused`):\n{}",
        wrong.len(),
        ROWS.len(),
        wrong.join("\n")
    );
    Ok(())
}
