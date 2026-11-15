//! Seeded simulation of the error vocabulary's two promises: untrusted text
//! cannot forge a log record, and a retry decision reads the variant, never
//! the message.
//!
//! One seed draws hostile text — newlines, carriage returns, tabs, ESC, NUL,
//! DEL, C1 controls, multi-byte letters — and drawn failures, and every family
//! asserts what a log reader or a retry policy sees:
//!
//! - escaped text holds no control character, so it is one physical line that
//!   cannot reach a terminal;
//! - escaping is idempotent, leaves text with no control character unchanged,
//!   and decodes back to the original exactly;
//! - a denial renders its count, then every capability and the domain that
//!   required it, each escaped, on one line;
//! - every document-sourced field of a rendered error is escaped;
//! - each dispatch certainty maps to one retry class, and a domain error's
//!   class is its certainty's whatever its cause says;
//! - a kernel failure wrapped through the world, session and app layers keeps
//!   its retry class, its certainty and its rendered line (ADR 0061);
//! - the same seed replays the same renderings and distinct seeds diverge.
//!
//! Sources: CWE-117 (improper output neutralization for logs), CVE-2021-44228's
//! lesson that log text is an attack surface, and ECMA-48 for the escape
//! sequences a terminal executes.

use std::fmt::Write as _;
use std::str::Chars;

use crate::BotError;
use crate::cap::{Cap, Demand};
use crate::ecs::EcsError;
use crate::error::{DispatchCertainty, Escaped, KernelError, RetryClass};
use crate::gate::GrantSet;
use crate::session::SessionError;
use crate::sim_stream::{DrawError, RECEIPT_START, Stream, fold};

/// The seeds every family replays.
const SEEDS: [u64; 4] = [
    0xE5C4_0000_0000_0001,
    0xE5C4_0000_0000_0002,
    0xE5C4_FFFF_0000_0003,
    0x10C5_E5C4_0000_0004,
];

/// Texts each family draws per seed.
const DRAWS: usize = 256;

/// The glyphs hostile text is drawn from: controls a log line or a terminal
/// acts on, and plain and multi-byte letters that must survive untouched.
const GLYPHS: [char; 16] = [
    'a', 'Z', '7', ' ', '.', ':', 'é', '✓', '𝄞', '\n', '\r', '\t', '\u{1b}', '\u{0}', '\u{7f}',
    '\u{85}',
];

/// A family's result.
type Outcome = Result<(), String>;

/// Up to 48 drawn glyphs.
fn hostile(stream: &mut Stream) -> Result<String, DrawError> {
    let len = stream.below(49)?;
    (0..len).map(|_| stream.choose(&GLYPHS)).collect()
}

/// Every drawn text under each seed.
fn sweep(mut check: impl FnMut(u64, &mut Stream, &str) -> Outcome) -> Outcome {
    for seed in SEEDS {
        let mut stream = Stream::new(seed);
        for _ in 0..DRAWS {
            let text = hostile(&mut stream)?;
            check(seed, &mut stream, &text)?;
        }
    }
    Ok(())
}

/// The character one escape names, read from `glyphs` just past its backslash.
fn escaped_glyph(glyphs: &mut Chars<'_>, escaped: &str) -> Result<char, String> {
    match glyphs.next() {
        Some('n') => Ok('\n'),
        Some('r') => Ok('\r'),
        Some('t') => Ok('\t'),
        Some('u') => {
            glyphs
                .next()
                .filter(|brace| *brace == '{')
                .ok_or_else(|| format!("a \\u escape without its brace in {escaped:?}"))?;
            let hex: String = glyphs.by_ref().take_while(|digit| *digit != '}').collect();
            let code = u32::from_str_radix(&hex, 16).map_err(|error| error.to_string())?;
            char::from_u32(code).ok_or_else(|| format!("{code:x} is no char"))
        }
        other => Err(format!("an unknown escape {other:?} in {escaped:?}")),
    }
}

/// The inverse of the escaping: `\n`, `\r`, `\t` and `\u{hex}` back to their
/// characters, for text that held no backslash before it was escaped.
fn unescape(escaped: &str) -> Result<String, String> {
    let mut out = String::with_capacity(escaped.len());
    let mut glyphs = escaped.chars();
    while let Some(glyph) = glyphs.next() {
        if glyph == '\\' {
            out.push(escaped_glyph(&mut glyphs, escaped)?);
        } else {
            out.push(glyph);
        }
    }
    Ok(out)
}

#[test]
/// Escaped text holds no control character.
fn sim_escaped_text_holds_no_control_character() -> Outcome {
    sweep(|seed, _, text| {
        let escaped = Escaped(text).to_string();
        assert!(
            !escaped.chars().any(char::is_control),
            "seed {seed:#x}: {text:?} escaped to {escaped:?}"
        );
        Ok(())
    })
}

#[test]
/// Escaping twice is escaping once.
fn sim_escaping_is_idempotent() -> Outcome {
    sweep(|seed, _, text| {
        let once = Escaped(text).to_string();
        assert_eq!(Escaped(&once).to_string(), once, "seed {seed:#x}: {text:?}");
        Ok(())
    })
}

#[test]
/// Text with no control character is written exactly as given.
fn sim_plain_text_is_unchanged() -> Outcome {
    sweep(|seed, _, text| {
        let plain: String = text.chars().filter(|glyph| !glyph.is_control()).collect();
        assert_eq!(Escaped(&plain).to_string(), plain, "seed {seed:#x}");
        Ok(())
    })
}

#[test]
/// The escaped text decodes back to the original exactly: nothing was lost.
fn sim_escaped_text_decodes_to_the_original() -> Outcome {
    sweep(|seed, _, text| {
        let escaped = Escaped(text).to_string();
        assert_eq!(
            unescape(&escaped)?,
            text,
            "seed {seed:#x}: {escaped:?} lost something"
        );
        Ok(())
    })
}

#[test]
/// A denial renders its count, then every capability and the domain that
/// required it, each escaped, on one line.
fn sim_a_denial_renders_every_shortage_on_one_line() -> Outcome {
    sweep(|seed, stream, text| {
        let count = stream.below(4)?.saturating_add(1);
        let required: Vec<Cap> = (0..count)
            .map(|index| Cap::new(format!("{text}#{index}")))
            .collect();
        let domain = format!("dom{text}");
        let shortages = GrantSet::empty().uncovered(&required, &Demand::new(domain.as_str()));
        let deficit = crate::cap::Deficit::from_shortages(shortages)
            .ok_or_else(|| format!("seed {seed:#x}: nothing was short"))?;
        let mut expected = format!("{count} ungranted");
        for (index, cap) in required.iter().enumerate() {
            let separator = if index == 0 { ": " } else { ", " };
            write!(
                expected,
                "{separator}{} (required by {})",
                Escaped(cap.as_str()),
                Escaped(&domain)
            )
            .map_err(|error| error.to_string())?;
        }
        let rendered = deficit.to_string();
        assert_eq!(rendered, expected, "seed {seed:#x}");
        assert!(
            !rendered.chars().any(char::is_control),
            "seed {seed:#x}: one physical line"
        );
        Ok(())
    })
}

#[test]
/// Every document-sourced field of a rendered error is escaped.
fn sim_document_sourced_fields_render_escaped() -> Outcome {
    sweep(|seed, _, text| {
        let errors = [
            EcsError::UnregisteredDomain {
                domain: text.to_owned(),
            },
            EcsError::IncompleteSpec {
                field: "name",
                cause: text.to_owned(),
            },
        ];
        for error in errors {
            let rendered = error.to_string();
            assert!(
                !rendered.chars().any(char::is_control),
                "seed {seed:#x}: {rendered:?}"
            );
            assert!(
                rendered.contains(&Escaped(text).to_string()),
                "seed {seed:#x}: the field is rendered whole: {rendered:?}"
            );
        }
        Ok(())
    })
}

/// The retry class each certainty permits, as the vocabulary states it.
fn class_of(certainty: DispatchCertainty) -> RetryClass {
    match certainty {
        DispatchCertainty::Refused | DispatchCertainty::Occurred => RetryClass::Never,
        DispatchCertainty::NotDelivered => RetryClass::Safe,
        DispatchCertainty::Unsettled => RetryClass::RequiresEvidence,
    }
}

#[test]
/// A domain error's class is its certainty's, whatever its cause says.
fn sim_a_domain_errors_class_is_its_certaintys() -> Outcome {
    sweep(|seed, stream, text| {
        let certainty = stream.choose(&[
            DispatchCertainty::Refused,
            DispatchCertainty::NotDelivered,
            DispatchCertainty::Unsettled,
            DispatchCertainty::Occurred,
        ])?;
        let error = KernelError::DomainError {
            domain: "sim::domain".to_owned(),
            certainty,
            cause: format!("retry {text} transient permanent safe"),
        };
        assert_eq!(error.dispatch_certainty(), certainty, "seed {seed:#x}");
        assert_eq!(
            error.retry_class(),
            class_of(certainty),
            "seed {seed:#x}: {certainty:?}"
        );
        assert_eq!(
            certainty.retry_class(),
            class_of(certainty),
            "seed {seed:#x}"
        );
        Ok(())
    })
}

#[test]
/// A refusal made before any dispatch is never retried, whatever it names.
fn sim_a_pre_dispatch_refusal_is_never_retried() -> Outcome {
    sweep(|seed, _, text| {
        let denied = GrantSet::empty()
            .admit(&[Cap::new(text.to_owned())])
            .err()
            .ok_or_else(|| format!("seed {seed:#x}: an empty set admitted {text:?}"))?;
        let unregistered = EcsError::UnregisteredDomain {
            domain: text.to_owned(),
        };
        for (certainty, class) in [
            (denied.dispatch_certainty(), denied.retry_class()),
            (
                unregistered.dispatch_certainty(),
                unregistered.retry_class(),
            ),
        ] {
            assert_eq!(certainty, DispatchCertainty::Refused, "seed {seed:#x}");
            assert_eq!(class, RetryClass::Never, "seed {seed:#x}");
        }
        Ok(())
    })
}

/// The shape of a drawn kernel failure: a domain error under a certainty, or
/// an indeterminate effect. `KernelError` is not `Clone`, so one draw is built
/// once per layer it is wrapped into.
#[derive(Clone, Copy)]
enum Shape {
    /// A `DomainError` carrying this certainty.
    Domain(DispatchCertainty),
    /// An `EffectIndeterminate`.
    Indeterminate,
}

impl Shape {
    /// A shape drawn from `stream`.
    fn draw(stream: &mut Stream) -> Result<Self, DrawError> {
        let certainty = stream.choose(&[
            DispatchCertainty::Refused,
            DispatchCertainty::NotDelivered,
            DispatchCertainty::Unsettled,
            DispatchCertainty::Occurred,
        ])?;
        Ok(if stream.below(4)? == 0 {
            Self::Indeterminate
        } else {
            Self::Domain(certainty)
        })
    }

    /// The kernel failure this shape names, with `text` as its cause.
    fn build(self, text: &str) -> KernelError {
        match self {
            Self::Domain(certainty) => KernelError::DomainError {
                domain: "sim::wrap".to_owned(),
                certainty,
                cause: text.to_owned(),
            },
            Self::Indeterminate => KernelError::EffectIndeterminate {
                domain: "sim::wrap".to_owned(),
                cause: text.to_owned(),
            },
        }
    }
}

#[test]
/// A kernel failure wrapped through the world layer, the session layer and the
/// app layer keeps its retry class, its certainty and its rendered line.
fn sim_a_kernel_failure_keeps_its_class_through_every_layer() -> Outcome {
    sweep(|seed, stream, text| {
        let shape = Shape::draw(stream)?;
        let bare = shape.build(text);
        let world = EcsError::from(shape.build(text));
        let session = SessionError::from(shape.build(text));
        let app_world = BotError::from(EcsError::from(shape.build(text)));
        let app_session = BotError::from(SessionError::from(shape.build(text)));
        let class = bare.retry_class();
        let certainty = bare.dispatch_certainty();
        assert!(matches!(world, EcsError::Kernel(_)), "seed {seed:#x}");
        assert!(matches!(session, SessionError::Kernel(_)), "seed {seed:#x}");
        assert_eq!(world.retry_class(), class, "seed {seed:#x}: world");
        assert_eq!(session.retry_class(), class, "seed {seed:#x}: session");
        assert_eq!(app_world.retry_class(), class, "seed {seed:#x}: app world");
        assert_eq!(
            app_session.retry_class(),
            class,
            "seed {seed:#x}: app session"
        );
        assert_eq!(world.dispatch_certainty(), certainty, "seed {seed:#x}");
        assert_eq!(session.dispatch_certainty(), certainty, "seed {seed:#x}");
        assert_eq!(app_world.dispatch_certainty(), certainty, "seed {seed:#x}");
        let line = bare.to_string();
        assert_eq!(world.to_string(), line, "seed {seed:#x}: world line");
        assert_eq!(session.to_string(), line, "seed {seed:#x}: session line");
        assert_eq!(app_world.to_string(), line, "seed {seed:#x}: app line");
        Ok(())
    })
}

/// The replay receipt of every rendering drawn under `seed`.
fn rendering_trace(seed: u64) -> Result<u64, String> {
    let mut stream = Stream::new(seed);
    let mut trace = RECEIPT_START;
    for _ in 0..DRAWS {
        let text = hostile(&mut stream)?;
        trace = fold(trace, Escaped(&text).to_string().as_bytes());
    }
    Ok(trace)
}

#[test]
/// The replay oracle: one seed, one rendering trace.
fn sim_the_same_seed_replays_the_same_renderings() -> Outcome {
    for seed in SEEDS {
        assert_eq!(
            rendering_trace(seed)?,
            rendering_trace(seed)?,
            "seed {seed:#x} replays"
        );
    }
    Ok(())
}

#[test]
/// The divergence oracle: a sweep that ignored its seed would pass the replay.
fn sim_distinct_seeds_draw_different_renderings() -> Outcome {
    assert_ne!(
        rendering_trace(SEEDS[0])?,
        rendering_trace(SEEDS[1])?,
        "two seeds drew alike"
    );
    Ok(())
}
