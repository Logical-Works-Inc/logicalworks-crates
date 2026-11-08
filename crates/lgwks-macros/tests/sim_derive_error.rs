//! The derive as a consumer writes it, judged against a model of the text it
//! must print and the source it must report.
//!
//! Source: `std::error::Error`'s contract for `source`, and the documented
//! attributes of `#[derive(lgwks_macros::Error)]`. Requirement: 6.0.0 of
//! `lgwks_ast` took away the re-export consumers derived through, and this is
//! the path the guides now teach, so it must compile and answer outside this
//! crate. The fools are the weird-bytes payload, the empty payload and the
//! monkey drawing variants.

use std::error::Error;
use std::io;

use lgwks_std::seeded::Seeded;

type TestResult = Result<(), Box<dyn Error>>;

/// Seeds the sweep replays, named so a failure prints what reproduces it.
const SEEDS: [u64; 4] = [
    0x5EED_0000_0000_0001,
    0x5EED_0000_0000_0002,
    0x5EED_0000_FFFF_FFFF,
    0xDEAD_BEEF_CAFE_0001,
];

/// Draws per seed.
const DRAWS: u64 = 256;

/// Payload pieces, among them text a format string would read as a placeholder.
const PIECES: [&str; 6] = ["", "plain", "{0}", "{cause}", "ünï\u{1F980}", "%s\n\t"];

#[derive(lgwks_macros::Error, Debug)]
enum Diagnostic {
    #[error("parse refused: {0}")]
    Refused(String),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("at line {line}: {cause}")]
    Located {
        line: u64,
        #[source]
        cause: io::Error,
    },
    #[error("gone")]
    Gone,
}

/// One draw: the diagnostic, the text it must print, the source it must report.
fn draw(stream: &mut Seeded) -> Result<(Diagnostic, String, Option<String>), Box<dyn Error>> {
    let piece = *PIECES
        .get(stream.index(PIECES.len())?)
        .ok_or("an index lands inside its list")?;
    let line = stream.next_u64();
    Ok(match stream.below(4)? {
        0 => (
            Diagnostic::Refused(piece.to_owned()),
            format!("parse refused: {piece}"),
            None,
        ),
        1 => (
            Diagnostic::from(io::Error::other(piece.to_owned())),
            piece.to_owned(),
            None,
        ),
        2 => (
            Diagnostic::Located {
                line,
                cause: io::Error::other(piece.to_owned()),
            },
            format!("at line {line}: {piece}"),
            Some(piece.to_owned()),
        ),
        _ => (Diagnostic::Gone, "gone".to_owned(), None),
    })
}

/// The printed text of every draw of `seed`, each checked against the model.
fn trace(seed: u64) -> Result<Vec<String>, Box<dyn Error>> {
    let mut stream = Seeded::from_seed(seed);
    let mut printed = Vec::new();
    for _ in 0..DRAWS {
        let (diagnostic, text, source) = draw(&mut stream)?;
        assert_eq!(diagnostic.to_string(), text, "seed {seed:#x}");
        assert_eq!(
            diagnostic.source().map(ToString::to_string),
            source,
            "seed {seed:#x}: {diagnostic:?}"
        );
        printed.push(text);
    }
    Ok(printed)
}

#[test]
fn sim_derive_error_monkey_every_variant_prints_and_sources_as_the_model_says() -> TestResult {
    for seed in SEEDS {
        trace(seed)?;
    }
    Ok(())
}

#[test]
fn sim_derive_error_the_same_seed_replays_and_distinct_seeds_diverge() -> TestResult {
    for seed in SEEDS {
        assert_eq!(trace(seed)?, trace(seed)?, "seed {seed:#x}");
    }
    assert_ne!(trace(0x5EED_0000_0000_0001)?, trace(0x5EED_0000_0000_0002)?);
    Ok(())
}

#[test]
fn a_from_conversion_lets_the_question_mark_carry_the_cause() {
    fn refused() -> Result<(), Diagnostic> {
        Err(io::Error::other("disk"))?;
        Ok(())
    }
    assert!(
        matches!(refused(), Err(Diagnostic::Io(ref cause)) if cause.to_string() == "disk"),
        "`?` converts through #[from]"
    );
}
