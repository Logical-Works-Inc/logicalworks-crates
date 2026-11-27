//! Rule 12 against a careless workflow author: a seeded sweep writes `curl`
//! commands with and without a deadline, split over continuation lines, behind
//! a bounded pipe stage, with the flag only in a comment, and the checker must
//! refuse exactly the commands the model calls unbounded.
//!
//! Source: run 45910843702, where one job's kache fetch hung 120 s on a 504 and
//! the run walled at 323 s with every test green. The draw is
//! `lgwks_std::seeded`, so a failing seed replays exactly.

use std::path::Path;

use lgwks_std::seeded::{DrawError, Seeded};

use super::{committed_problems, problems};
use crate::ParityError;

/// Seeds the sweep draws.
const SEEDS: u64 = 256;

/// A flag that bounds a whole transfer, or one that does not.
const FLAGS: [(&str, bool); 6] = [
    ("--max-time 15", true),
    ("-m 15", true),
    ("--max-time=15", true),
    ("--retry 5", false),
    ("--connect-timeout 5", false),
    ("-fsSL", false),
];

/// One drawn step body and whether the model says it holds an unbounded curl.
struct Case {
    /// The workflow text.
    text: String,
    /// The model's verdict.
    unbounded: bool,
}

/// The case `seed` draws: a curl with one to three flags, split over up to
/// three continuation lines, maybe behind a bounded pipe stage, maybe with a
/// deadline only in a trailing comment, which must count for nothing.
fn case(seed: u64) -> Result<Case, DrawError> {
    let mut draw = Seeded::from_seed(seed);
    let mut bounded = false;
    let mut words = vec!["curl".to_owned()];
    for _ in 0..=draw.index(3)? {
        if let Some(&(flag, bounds)) = FLAGS.get(draw.index(FLAGS.len())?) {
            bounded |= bounds;
            words.push(flag.to_owned());
        }
    }
    words.push("https://example.invalid/asset".to_owned());
    let per = words
        .len()
        .div_ceil(draw.index(3)?.saturating_add(1))
        .max(1);
    let mut lines: Vec<String> = words.chunks(per).map(|chunk| chunk.join(" ")).collect();
    let count = lines.len();
    for line in lines.iter_mut().take(count.saturating_sub(1)) {
        line.push_str(" \\");
    }
    if draw.index(2)? == 0
        && let Some(first) = lines.first_mut()
    {
        *first = format!("curl --max-time 5 https://example.invalid/sum | {first}");
    }
    if draw.index(2)? == 0
        && let Some(last) = lines.last_mut()
    {
        last.push_str(" # --max-time 15 is what this needs");
    }
    let body: Vec<String> = lines
        .iter()
        .map(|line| format!("          {line}"))
        .collect();
    Ok(Case {
        text: format!("      - run: |\n{}\n      - name: next\n", body.join("\n")),
        unbounded: !bounded,
    })
}

#[test]
fn sim_bounded_fetch_refuses_exactly_the_unbounded_curls() -> Result<(), DrawError> {
    let (mut refused, mut passed) = (0_u32, 0_u32);
    for seed in 0..SEEDS {
        let drawn = case(seed)?;
        let found = problems("ci.yml", &drawn.text);
        assert_eq!(
            !found.is_empty(),
            drawn.unbounded,
            "seed {seed}: the model says unbounded = {}, the checker said {found:?} for\n{}",
            drawn.unbounded,
            drawn.text
        );
        if drawn.unbounded {
            refused = refused.saturating_add(1);
        } else {
            passed = passed.saturating_add(1);
        }
    }
    assert!(
        refused > 0 && passed > 0,
        "the sweep drew one verdict only: {refused} {passed}"
    );
    Ok(())
}

/// A bound on one curl never covers the next one, a comment or a lookalike
/// command is no curl, and the refusal names the file and line.
#[test]
fn sim_bounded_fetch_judges_each_curl_alone_and_names_its_line() {
    let text = concat!(
        "steps:\n",
        "  # curl https://a.invalid\n",
        "  - run: curly --help\n",
        "  - run: curl --max-time 5 https://b.invalid\n",
        "  - run: |\n",
        "      curl \\\n",
        "        -fsSL https://c.invalid\n",
        "      curl -m 5 https://d.invalid && curl https://e.invalid\n",
    );
    let found = problems("action.yml", text);
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(
        found
            .first()
            .is_some_and(|problem| problem.starts_with("action.yml:6 ")),
        "{found:?}"
    );
    assert!(
        found
            .get(1)
            .is_some_and(|problem| problem.starts_with("action.yml:8 ")),
        "{found:?}"
    );
}

/// Every committed workflow and action holds: the hosted kache fetch that
/// walled run 45910843702 among them.
#[test]
fn sim_bounded_fetch_holds_for_every_committed_workflow_and_action() -> Result<(), ParityError> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let found = committed_problems(&root)?;
    assert!(found.is_empty(), "{found:#?}");
    Ok(())
}
