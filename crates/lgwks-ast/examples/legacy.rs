//! Explain a COBOL program the way `lgwks legacy explain` does (#777).
//!
//! ```sh
//! cargo run -p lgwks_ast --example legacy
//! ```
//!
//! The output line is written through `std::io::Write` rather than `println!`
//! because `clippy::print_stdout` is forbidden workspace-wide, with no
//! test/example carve-out; the lint is what is enforced, and an example that
//! writes explicitly says the same thing fallibly.

use std::io::Write;

use lgwks_ast::legacy::LegacyLimits;
use lgwks_ast::legacy::cobol::parse_program;
use lgwks_ast::legacy::json::program_json;
use lgwks_std::json::Value;

/// What the example reports a failure through: the refusal's own text.
type Outcome<T> = Result<T, Box<dyn std::error::Error>>;

/// The array the JSON verdict holds at `path`, or a refusal naming the path.
fn array_at<'verdict>(verdict: &'verdict Value, path: &[&str]) -> Outcome<&'verdict Vec<Value>> {
    let mut at = verdict;
    for key in path {
        at = at.get(*key).ok_or(format!("the verdict holds no {key}"))?;
    }
    Ok(at.as_array().ok_or(format!("{path:?} is not an array"))?)
}

/// `Result` from `main` is how the example reports a failure: the refusal's own
/// `Debug` reaches the caller, and no `expect` sits in the tree.
fn main() -> Outcome<()> {
    let source = "       IDENTIFICATION DIVISION.\n       PROGRAM-ID. LEDGER.\n       PROCEDURE DIVISION.\n       MAIN-PARA.\n           CALL \"POST\" USING WS-ENTRY.\n           PERFORM CLOSE-OUT.\n       CLOSE-OUT.\n           CLOSE LEDGER-FILE.\n";
    let limits = LegacyLimits::new(1_048_576, 10_000, 80_000, 80_000, 4_096, 64, 10_000);
    let program = parse_program(source, &limits, &|_name: &str| None)?;
    let verdict = program_json(&program, &limits)?;
    let name = verdict
        .get("name")
        .and_then(Value::as_str)
        .ok_or("the verdict names no program")?;
    assert_eq!(name, "LEDGER", "the PROGRAM-ID names the program");
    assert_eq!(
        array_at(&verdict, &["graphs", "perform"])?.len(),
        1,
        "one PERFORM names CLOSE-OUT"
    );
    assert_eq!(
        array_at(&verdict, &["graphs", "call"])?.len(),
        1,
        "one CALL names POST"
    );
    let mut stdout = std::io::stdout();
    for sentence in array_at(&verdict, &["sentences"])? {
        let text = sentence
            .get("text")
            .and_then(Value::as_str)
            .ok_or("a sentence carries no text")?;
        writeln!(stdout, "{text}")?;
    }
    Ok(())
}
