//! The derive's `#[source]` on a boxed field, judged against `std::error::Error`'s contract.
//!
//! Source: `std::error::Error::source` ("the lower-level source of this error"), and the
//! 2026-11-19 defect this file pins: a `#[source]` field written `Box<E>` was returned as the
//! box itself. `&Box<E>` coerces to `&dyn Error`, so a caller's `is::<E>()` was false and a
//! source walk skipped `E`'s own layer. lgwks_bot's `FlowError` carries boxed sources, and its
//! error-layer sims saw every layer as unknown. Requirement: the source a boxed field reports is
//! the boxed value, for enums, structs, trait objects and recursive chains, and an unboxed field
//! still reports itself. The fools are the monkey drawing variants, a chain as deep as the seed
//! asks, and weird-bytes payloads.

use std::error::Error;
use std::io;

use lgwks_std::seeded::{DrawError, Seeded};

type TestResult = Result<(), Box<dyn Error>>;

/// Seeds every sweep replays, named so a failure prints what reproduces it.
const SEEDS: [u64; 4] = [
    0x5EED_B0C5_0000_0001,
    0x5EED_B0C5_0000_0002,
    0x5EED_B0C5_FFFF_FFFF,
    0xB0C5_DEAD_BEEF_0001,
];

/// Draws per seed.
const DRAWS: u64 = 128;

/// Payload pieces, among them text a format string would read as a placeholder.
const PIECES: [&str; 6] = ["", "plain", "{0}", "{cause}", "ünï\u{1F980}", "%s\n\t"];

/// Why a draw could not produce a payload piece.
#[derive(lgwks_macros::Error, Debug)]
enum PieceRefused {
    #[error("the stream refused the draw")]
    Draw(#[from] DrawError),
    #[error("index {0} is past the piece list")]
    Past(usize),
}

/// The inner error a boxed field carries.
#[derive(lgwks_macros::Error, Debug)]
#[error("inner: {0}")]
struct Inner(String);

/// An enum whose sources are boxed, unboxed and trait objects.
#[derive(lgwks_macros::Error, Debug)]
enum Outer {
    #[error("boxed")]
    Boxed(#[source] Box<Inner>),
    #[error("named boxed at {at}")]
    Named {
        at: u64,
        #[source]
        cause: Box<Inner>,
    },
    #[error("unboxed")]
    Unboxed(#[source] io::Error),
    #[error("dynamic")]
    Dynamic(#[source] Box<dyn Error + Send + Sync>),
}

/// A struct whose one source is boxed.
#[derive(lgwks_macros::Error, Debug)]
#[error("wrapped {label}")]
struct Wrapped {
    label: String,
    #[source]
    cause: Box<io::Error>,
}

/// A chain of boxed layers ending in a leaf, as deep as a draw makes it.
#[derive(lgwks_macros::Error, Debug)]
enum Chain {
    #[error("leaf {0}")]
    Leaf(String),
    #[error("link {depth}")]
    Link {
        depth: u32,
        #[source]
        next: Box<Chain>,
    },
}

/// A tuple struct whose one source is boxed.
#[derive(lgwks_macros::Error, Debug)]
#[error("tuple")]
struct Tuple(#[source] Box<Inner>);

/// A boxed field taken through `#[from]`, which is also its source.
#[derive(lgwks_macros::Error, Debug)]
enum Lifted {
    #[error("lifted")]
    Io(#[from] Box<io::Error>),
}

/// One payload piece drawn from `stream`.
fn piece(stream: &mut Seeded) -> Result<&'static str, PieceRefused> {
    let at = stream.index(PIECES.len())?;
    PIECES.get(at).copied().ok_or(PieceRefused::Past(at))
}

/// A chain `depth` links deep over a leaf carrying `text`.
fn chain(depth: u32, text: &str) -> Chain {
    let mut built = Chain::Leaf(text.to_owned());
    for level in 1..=depth {
        built = Chain::Link {
            depth: level,
            next: Box::new(built),
        };
    }
    built
}

#[test]
fn sim_box_source_monkey_every_variant_reports_the_boxed_value() -> TestResult {
    for seed in SEEDS {
        let mut stream = Seeded::from_seed(seed);
        for _ in 0..DRAWS {
            let text = piece(&mut stream)?;
            let outer = match stream.below(4)? {
                0 => Outer::Boxed(Box::new(Inner(text.to_owned()))),
                1 => Outer::Named {
                    at: stream.next_u64(),
                    cause: Box::new(Inner(text.to_owned())),
                },
                2 => Outer::Unboxed(io::Error::other(text.to_owned())),
                _ => Outer::Dynamic(Box::new(io::Error::other(text.to_owned()))),
            };
            let source = outer.source().ok_or("every variant carries a source")?;
            let (inner_typed, io_typed) = (source.is::<Inner>(), source.is::<io::Error>());
            let expected = match outer {
                Outer::Boxed(_) | Outer::Named { .. } => (true, false, format!("inner: {text}")),
                Outer::Unboxed(_) | Outer::Dynamic(_) => (false, true, text.to_owned()),
            };
            assert_eq!(
                (inner_typed, io_typed, source.to_string()),
                expected,
                "seed {seed:#x}: {outer:?}"
            );
        }
    }
    Ok(())
}

#[test]
fn sim_box_source_a_deep_chain_walks_every_layer_once() -> TestResult {
    for seed in SEEDS {
        let mut stream = Seeded::from_seed(seed);
        for _ in 0..DRAWS {
            let depth = u32::try_from(stream.below(48)?)?;
            let built = chain(depth, piece(&mut stream)?);
            let mut links = 0_u32;
            let mut layer: &dyn Error = &built;
            while let Some(next) = layer.source() {
                assert!(
                    next.is::<Chain>(),
                    "seed {seed:#x}: layer {links} is not a Chain"
                );
                links = links.saturating_add(1);
                layer = next;
            }
            assert_eq!(
                links, depth,
                "seed {seed:#x}: the walk skipped or repeated a layer"
            );
            assert!(
                layer.to_string().starts_with("leaf "),
                "seed {seed:#x}: ended off the leaf"
            );
        }
    }
    Ok(())
}

#[test]
fn sim_box_source_the_source_is_the_heap_value_not_the_box() -> TestResult {
    let mut stream = Seeded::from_seed(0x5EED_B0C5_0000_00AA);
    for _ in 0..DRAWS {
        let boxed = Box::new(Inner(piece(&mut stream)?.to_owned()));
        let heap: *const Inner = &raw const *boxed;
        let outer = Outer::Boxed(boxed);
        let source = outer.source().ok_or("a boxed variant carries a source")?;
        let reported = source
            .downcast_ref::<Inner>()
            .ok_or("the source is the Inner")?;
        assert!(
            std::ptr::eq(reported, heap),
            "the source moved off the heap value"
        );
    }
    Ok(())
}

#[test]
fn sim_box_source_a_struct_field_reports_the_boxed_io_error() -> TestResult {
    let mut stream = Seeded::from_seed(0x5EED_B0C5_0000_00BB);
    for _ in 0..DRAWS {
        let (label, text) = (piece(&mut stream)?, piece(&mut stream)?);
        let wrapped = Wrapped {
            label: label.to_owned(),
            cause: Box::new(io::Error::other(text.to_owned())),
        };
        assert_eq!(wrapped.to_string(), format!("wrapped {label}"));
        let source = wrapped
            .source()
            .ok_or("a struct with #[source] reports it")?;
        let cause = source
            .downcast_ref::<io::Error>()
            .ok_or("the source is the io::Error")?;
        assert_eq!(cause.to_string(), text);
    }
    Ok(())
}

#[test]
fn sim_box_source_a_trait_object_source_downcasts_to_what_was_boxed() -> TestResult {
    let mut stream = Seeded::from_seed(0x5EED_B0C5_0000_00CC);
    for _ in 0..DRAWS {
        let text = piece(&mut stream)?;
        let nested: Box<dyn Error + Send + Sync> = if stream.below(2)? == 0 {
            Box::new(io::Error::other(text.to_owned()))
        } else {
            Box::new(Inner(text.to_owned()))
        };
        let wants_io = nested.is::<io::Error>();
        let outer = Outer::Dynamic(nested);
        let source = outer.source().ok_or("a dynamic variant carries a source")?;
        assert_eq!(
            (source.is::<io::Error>(), source.is::<Inner>()),
            (wants_io, !wants_io),
            "the trait object's own type is what a caller downcasts"
        );
    }
    Ok(())
}

#[test]
fn sim_box_source_weird_bytes_leave_display_and_the_inner_text_intact() -> TestResult {
    for seed in SEEDS {
        let mut stream = Seeded::from_seed(seed);
        for _ in 0..DRAWS {
            let (text, at) = (piece(&mut stream)?, stream.next_u64());
            let named = Outer::Named {
                at,
                cause: Box::new(Inner(text.to_owned())),
            };
            assert_eq!(named.to_string(), format!("named boxed at {at}"));
            let inner = named.source().map(ToString::to_string);
            assert_eq!(inner, Some(format!("inner: {text}")), "seed {seed:#x}");
        }
    }
    Ok(())
}

#[test]
fn sim_box_source_a_tuple_struct_reports_its_boxed_field() -> TestResult {
    let mut stream = Seeded::from_seed(0x5EED_B0C5_0000_00DD);
    for _ in 0..DRAWS {
        let text = piece(&mut stream)?;
        let tuple = Tuple(Box::new(Inner(text.to_owned())));
        let source = tuple
            .source()
            .ok_or("a tuple struct with #[source] reports it")?;
        let inner = source
            .downcast_ref::<Inner>()
            .ok_or("the source is the Inner")?;
        assert_eq!(inner.0, text, "the tuple's source carries the drawn text");
    }
    Ok(())
}

#[test]
fn sim_box_source_a_from_field_converts_and_reports_the_boxed_value() -> TestResult {
    fn refused(text: &str) -> Result<(), Lifted> {
        let failed: Result<(), Box<io::Error>> = Err(Box::new(io::Error::other(text.to_owned())));
        failed?;
        Ok(())
    }
    let mut stream = Seeded::from_seed(0x5EED_B0C5_0000_00EE);
    for _ in 0..DRAWS {
        let text = piece(&mut stream)?;
        let Err(lifted) = refused(text) else {
            return Err("`?` through #[from] must carry the failure".into());
        };
        let source = lifted.source().ok_or("a #[from] field is the source")?;
        let cause = source
            .downcast_ref::<io::Error>()
            .ok_or("the source is the io::Error")?;
        assert_eq!(cause.to_string(), text);
    }
    Ok(())
}

#[test]
fn sim_box_source_the_double_clicker_gets_the_same_source_every_time() -> TestResult {
    let mut stream = Seeded::from_seed(0x5EED_B0C5_0000_00FF);
    for _ in 0..DRAWS {
        let depth = u32::try_from(stream.below(8)?)?.saturating_add(1);
        let built = chain(depth, piece(&mut stream)?);
        let first: *const dyn Error = built.source().ok_or("a link has a source")?;
        for _ in 0..stream.below(16)? {
            let again: *const dyn Error = built.source().ok_or("a link has a source")?;
            assert!(
                std::ptr::addr_eq(first, again),
                "source moved between calls"
            );
        }
    }
    Ok(())
}

#[test]
fn sim_box_source_a_hundred_neighbour_threads_walk_their_own_chains() -> TestResult {
    let walked: Vec<Result<(u32, u32), String>> = std::thread::scope(|scope| {
        let mut workers = Vec::with_capacity(100);
        for tenant in 0..100_u32 {
            workers.push(scope.spawn(move || {
                let built = chain(tenant, &format!("tenant {tenant}"));
                let mut links = 0_u32;
                let mut layer: &dyn Error = &built;
                while let Some(next) = layer.source() {
                    links = links.saturating_add(1);
                    layer = next;
                }
                if layer.to_string() == format!("leaf tenant {tenant}") {
                    Ok((tenant, links))
                } else {
                    Err(format!("tenant {tenant} reached another tenant's leaf"))
                }
            }));
        }
        workers
            .into_iter()
            .map(|worker| match worker.join() {
                Ok(walk) => walk,
                Err(panic) => Err(format!("a walker panicked: {panic:?}")),
            })
            .collect()
    });
    for walk in walked {
        let (tenant, links) = walk?;
        assert_eq!(
            links, tenant,
            "tenant {tenant}'s walk visited another depth"
        );
    }
    Ok(())
}
