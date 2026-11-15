//! Policy sims: capability names, the capability guard, the widget
//! descriptor's capability filter and the command-bar pipeline are fed
//! near-miss spellings and hostile text, and compared with references written
//! from the documented rules.

use std::collections::BTreeMap;

use drishti::canvas_protocol::Capability;
use drishti::native_host::capability::{
    CapabilityError, assert_capability, is_ml_only, is_privileged,
};
use drishti::native_host::command_bar::{CommandSuggestions, IntentParseError, IntentPipeline};
use drishti::native_host::protocol::{JsonValue, WidgetDescriptor, WidgetSurface};

use crate::motion_kit::{describe, ensure};
use crate::rng::SimRng;

/// JSON string literal for the text, escaping quotes, backslashes and
/// control characters.
fn quoted(text: &str) -> String {
    let mut out = String::from("\"");
    for found in text.chars() {
        if found == '"' || found == '\\' {
            out.push('\\');
            out.push(found);
        } else if found.is_control() {
            out = format!("{out}\\u{:04x}", u32::from(found));
        } else {
            out.push(found);
        }
    }
    out.push('"');
    out
}

/// Bug caught: a capability name that parses from a near miss (case,
/// padding, a NUL tail, a truncation), or whose text and wire forms differ,
/// so a token is granted by a spelling no schema lists.
#[test]
fn sim_capability_names_roundtrip_and_refuse_near_misses() -> Result<(), String> {
    let all = Capability::all();
    let names: Vec<String> = all.iter().map(ToString::to_string).collect();
    for (capability, name) in all.iter().zip(&names) {
        ensure(
            name.parse::<Capability>() == Ok(capability.clone()),
            &format!("{name} did not parse back"),
        )?;
        ensure(
            matches!(
                lgwks_std::json::from_str::<Capability>(&quoted(name)),
                Ok(ref decoded) if decoded == capability
            ),
            &format!("{name} did not decode from the wire"),
        )?;
        let written = lgwks_std::json::to_value(capability).map_err(describe)?;
        ensure(
            JsonValue::from_json(&written).string_value() == Some(name),
            &format!("{name}: the wire form differs from the text form"),
        )?;
    }
    let mut candidates: Vec<String> = Vec::new();
    for name in &names {
        let chars: Vec<char> = name.chars().collect();
        candidates.push(name.to_uppercase());
        candidates.push(format!(" {name}"));
        candidates.push(format!("{name} "));
        candidates.push(format!("{name}\0"));
        candidates.push(format!("{name}\n"));
        candidates.push(name.replace('.', "_"));
        candidates.push(name.replace('.', "\u{ff0e}"));
        candidates.push(format!("{name}{name}"));
        candidates.push(chars[..chars.len().saturating_sub(1)].iter().collect());
        candidates.push(chars[1..].iter().collect());
        candidates.push(format!("{}\u{200b}{}", &name[..2], &name[2..]));
    }
    candidates.extend(
        ["", ".", "signal", "motion", "efface", "*", "all", "null"]
            .into_iter()
            .map(String::from),
    );
    for candidate in &candidates {
        let listed = names.contains(candidate);
        ensure(
            candidate.parse::<Capability>().is_ok() == listed
                && lgwks_std::json::from_str::<Capability>(&quoted(candidate)).is_ok() == listed,
            &format!("{candidate:?}: parse and decode must both say {listed}"),
        )?;
    }
    Ok(())
}

/// Bug caught: a guard that grants on a prefix, an order or a duplicate, or a
/// privilege table that drifts (ML-only must stay inside privileged, and the
/// privileged set is exactly three tokens).
#[test]
fn sim_capability_guard_equals_membership() -> Result<(), String> {
    let all = Capability::all();
    for capability in &all {
        let ml = matches!(capability, Capability::TapeRead | Capability::ViewInject);
        let privileged = ml || *capability == Capability::IntentEmit;
        ensure(
            is_ml_only(capability) == ml && is_privileged(capability) == privileged,
            &format!("{capability}: the privilege tables drifted"),
        )?;
        ensure(
            !is_ml_only(capability) || is_privileged(capability),
            "an ML-only token must be privileged",
        )?;
        ensure(
            assert_capability(&[], capability.clone())
                == Err(CapabilityError::NotGranted(capability.clone())),
            &format!("{capability}: an empty grant must refuse with the missing token"),
        )?;
    }
    for seed in 0..400_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(250_000));
        let held_count = random.below_len(30);
        let held: Vec<Capability> = (0..held_count)
            .map(|_| all[random.below_len(all.len())].clone())
            .collect();
        for need in &all {
            let want = held.contains(need);
            let got = assert_capability(&held, need.clone());
            ensure(
                got.is_ok() == want,
                &format!("seed {seed}: need {need} against {held:?}: got {got:?}"),
            )?;
            if !want {
                ensure(
                    got == Err(CapabilityError::NotGranted(need.clone())),
                    "a refusal must name the token that was missing",
                )?;
            }
        }
    }
    let crowd = vec![Capability::SignalEmit; 100_000];
    ensure(
        assert_capability(&crowd, Capability::SignalEmit).is_ok()
            && assert_capability(&crowd, Capability::Shred).is_err(),
        "a large grant must still answer by membership",
    )
}

/// Bug caught: a widget descriptor that keeps `intent.emit` (reserved for the
/// command bar), drops or reorders another grant, or mutates it, so a widget
/// posing as the user holds the user's token.
#[test]
fn sim_capability_widget_descriptor_never_holds_intent_emit() -> Result<(), String> {
    let all = Capability::all();
    for seed in 0..300_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(260_000));
        let count = random.below_len(20);
        let asked: Vec<Capability> = (0..count)
            .map(|_| all[random.below_len(all.len())].clone())
            .collect();
        let descriptor = WidgetDescriptor::new(
            String::from("widget.kind"),
            String::from("widget.id"),
            String::from("1.0.0"),
            WidgetSurface::Native,
            Vec::new(),
            Vec::new(),
            asked.clone(),
        );
        let want: Vec<Capability> = asked
            .iter()
            .filter(|held| **held != Capability::IntentEmit)
            .cloned()
            .collect();
        ensure(
            descriptor.capabilities() == want.as_slice(),
            &format!(
                "seed {seed}: {asked:?} became {:?}",
                descriptor.capabilities()
            ),
        )?;
        ensure(
            !descriptor.self_listen() && !descriptor.pinned(),
            "mount flags default off",
        )?;
    }
    Ok(())
}

/// What the documentation says a parse yields.
struct Expected {
    /// Intent kind.
    kind: String,
    /// Intent text.
    text: String,
    /// Payload members, all text.
    fields: BTreeMap<String, JsonValue>,
}

/// Reference parse, phrased by splitting rather than finding.
fn reference_parse(input: &str) -> Result<Expected, IntentParseError> {
    input
        .chars()
        .nth(512)
        .is_none()
        .then_some(())
        .ok_or(IntentParseError::InputTooLong)?;
    let trimmed = input.trim();
    let single = |kind: &str, field: &str, rest: &str| {
        Ok(Expected {
            kind: String::from(kind),
            text: String::from(rest),
            fields: BTreeMap::from([(String::from(field), JsonValue::String(String::from(rest)))]),
        })
    };
    let Some(body) = trimmed.strip_prefix('/') else {
        return single("intent.freeform", "text", trimmed);
    };
    let (command, rest) = match body.split_once(' ') {
        Some(pair) => pair,
        None => (body, ""),
    };
    // Command names are case-insensitive, as the suggestions are.
    let lowered = command.to_lowercase();
    match lowered.as_str() {
        "find" => single("intent.find", "query", rest),
        "pin" => single("intent.pin", "widget_id", rest),
        "unpin" | "help" => Ok(Expected {
            kind: format!("intent.{lowered}"),
            text: String::from(rest),
            fields: BTreeMap::new(),
        }),
        _ => Err(IntentParseError::UnknownCommand),
    }
}

/// Bug caught: a slash command split on the wrong delimiter or case, a
/// freeform input left untrimmed, or an unknown command promoted to a known
/// one, so typed text drives an intent the user did not ask for.
#[test]
fn sim_command_bar_parse_agrees_with_the_reference() -> Result<(), String> {
    const PIECES: &[&str] = &[
        "/",
        "find",
        "pin",
        "unpin",
        "help",
        " ",
        "  ",
        "\t",
        "\u{a0}",
        "\u{2003}",
        "\u{e9}",
        "\n",
        "x",
        "FIND",
        "\u{1f600}",
        "\0",
        "/find",
        "/pin ",
        "/unpin",
        "//",
    ];
    let mut accepted = 0_usize;
    let mut refused = 0_usize;
    for seed in 0..3_000_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(270_000));
        let count = random.below_len(8);
        let mut input = String::new();
        for _ in 0..count {
            if let Some(piece) = random.pick_str(PIECES) {
                input.push_str(piece);
            }
        }
        match (IntentPipeline::parse(&input), reference_parse(&input)) {
            (Ok(real), Ok(want)) => {
                accepted = accepted.saturating_add(1);
                ensure(
                    real.kind() == want.kind
                        && real.text() == want.text
                        && real.payload() == &JsonValue::Object(want.fields),
                    &format!("{input:?}: parsed to ({}, {:?})", real.kind(), real.text()),
                )?;
            }
            (Err(real), Err(want)) => {
                refused = refused.saturating_add(1);
                ensure(
                    real == want,
                    &format!("{input:?}: refused as {real:?}, not {want:?}"),
                )?;
            }
            (real, want) => {
                return Err(format!(
                    "{input:?}: verdicts differ ({}, {})",
                    real.is_ok(),
                    want.is_ok()
                ));
            }
        }
    }
    ensure(
        accepted > 300 && refused > 300,
        "the corpus must hold both verdicts",
    )
}

/// Bug caught: a length cap counted in bytes instead of scalars, or applied
/// after trimming, so a short non-Latin message is refused or a padded flood
/// slips through.
#[test]
fn sim_command_bar_cap_counts_scalars_before_trimming() -> Result<(), String> {
    let accepted = [
        "\u{e9}".repeat(512),
        "\u{1f600}".repeat(512),
        " ".repeat(512),
        format!("/find {}", "\u{e9}".repeat(506)),
        String::new(),
    ];
    for input in &accepted {
        ensure(
            IntentPipeline::parse(input).is_ok(),
            &format!("{} scalars must be accepted", input.chars().count()),
        )?;
    }
    let refused = [
        "\u{e9}".repeat(513),
        "\u{1f600}".repeat(513),
        " ".repeat(513),
        format!("/find {}", "\u{e9}".repeat(507)),
        format!("{}x", " ".repeat(512)),
    ];
    for input in &refused {
        ensure(
            IntentPipeline::parse(input).err() == Some(IntentParseError::InputTooLong),
            &format!("{} scalars must be too long", input.chars().count()),
        )?;
    }
    Ok(())
}

/// Bug caught: a suggestion the parser would then refuse, or a prefix of a
/// command that fails to suggest it, so the bar offers dead ends.
#[test]
fn sim_command_bar_suggestions_are_parseable_and_prefix_closed() -> Result<(), String> {
    let commands = ["/find", "/pin", "/unpin", "/help"];
    for command in commands {
        for cut in 1..=command.len() {
            let prefix = &command[..cut];
            for spelled in [String::from(prefix), prefix.to_uppercase()] {
                ensure(
                    CommandSuggestions::suggestions(&spelled).contains(&String::from(command)),
                    &format!("{spelled:?} must suggest {command}"),
                )?;
            }
        }
    }
    ensure(
        CommandSuggestions::suggestions("/") == commands.map(String::from),
        "a bare slash suggests all four in order",
    )?;
    const PIECES: &[&str] = &[
        "/", "f", "p", "u", "h", "i", "n", "e", "l", "P", "\u{130}", "x",
    ];
    for seed in 0..1_500_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(280_000));
        let count = random.below_len(7);
        let mut input = String::new();
        for _ in 0..count {
            if let Some(piece) = random.pick_str(PIECES) {
                input.push_str(piece);
            }
        }
        let suggested = CommandSuggestions::suggestions(&input);
        ensure(
            input.starts_with('/') || suggested.is_empty(),
            &format!("{input:?}: non-slash input must suggest nothing"),
        )?;
        for each in &suggested {
            ensure(
                commands.contains(&each.as_str())
                    && each.starts_with(&input.to_lowercase())
                    && IntentPipeline::parse(each).is_ok(),
                &format!("{input:?}: suggested {each:?} which is not a usable command"),
            )?;
        }
    }
    Ok(())
}
