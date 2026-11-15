//! Protocol-vocabulary sims: every capability and closed enum round-trips its
//! one wire spelling and refuses near misses, the node taxonomy keeps its
//! order, and the session invariant names the first bad field at the bounds.

use drishti::canvas_protocol::{
    ActorType, Capability, ChromePlacement, CompensationStrategy, EgressDestination,
    LocalActionStatus, NodeKind, OfflineBehavior, SessionInvariant, SessionInvariantError,
    SyncStatus, TrustBin, WidgetSurface, WorkspaceSlot, WorkspaceSurfaceRole, WorkspaceTheme,
    WorkspaceToolIcon,
};

use crate::motion_kit::{describe, ensure};
use crate::rng::SimRng;

/// The JSON text of a value.
fn json_of<T: lgwks_std::json::serde::Serialize>(value: &T) -> Result<String, String> {
    lgwks_std::json::to_string(value).map_err(describe)
}

/// Bug caught: a capability whose Display, FromStr and serde spellings drift
/// apart, or one that parses from a near miss, so a grant check and a stored
/// grant disagree about what was granted.
#[test]
fn sim_protocol_capability_has_one_wire_spelling() -> Result<(), String> {
    let all = Capability::all();
    for (index, capability) in all.iter().enumerate() {
        let text = capability.to_string();
        ensure(
            json_of(capability)? == format!("\"{text}\""),
            &format!("{text}: serde and Display spell it differently"),
        )?;
        ensure(
            text.parse::<Capability>().as_ref() == Ok(capability),
            &format!("{text}: FromStr does not invert Display"),
        )?;
        let back: Capability =
            lgwks_std::json::from_str(&format!("\"{text}\"")).map_err(describe)?;
        ensure(
            back == *capability,
            &format!("{text}: serde does not invert"),
        )?;
        for other in all.iter().skip(index.saturating_add(1)) {
            ensure(
                other.to_string() != text,
                &format!("{text}: two capabilities share a spelling"),
            )?;
        }
        for near in [
            text.to_uppercase(),
            format!(" {text}"),
            format!("{text} "),
            format!("{text}\0"),
            format!("{text}s"),
            text.replace('.', "_"),
            text.replace('.', ":"),
            text.chars().take(text.len().saturating_sub(1)).collect(),
            String::new(),
        ] {
            ensure(
                near.parse::<Capability>().is_err(),
                &format!("{near:?}: a near miss of {text} parsed"),
            )?;
            ensure(
                lgwks_std::json::from_str::<Capability>(&format!("{near:?}")).is_err(),
                &format!("{near:?}: serde accepted a near miss of {text}"),
            )?;
        }
    }
    ensure(
        all.len() == 13 && "signal.emit".parse::<Capability>() == Ok(Capability::SignalEmit),
        "the closed set holds thirteen capabilities",
    )?;
    ensure(
        lgwks_std::json::from_str::<Capability>("7").is_err()
            && lgwks_std::json::from_str::<Capability>("null").is_err()
            && lgwks_std::json::from_str::<Capability>("[\"signal.emit\"]").is_err(),
        "a non-string is not a capability",
    )
}

/// Round-trip a value and refuse every near-miss spelling.
pub fn closed_set<T>(values: &[T], names: &[&str]) -> Result<(), String>
where
    T: lgwks_std::json::serde::Serialize
        + lgwks_std::json::serde::de::DeserializeOwned
        + PartialEq
        + core::fmt::Debug,
{
    ensure(values.len() == names.len(), "values and names pair up")?;
    for (value, name) in values.iter().zip(names) {
        ensure(
            json_of(value)? == format!("\"{name}\""),
            &format!("{value:?}: wire spelling is not {name}"),
        )?;
        let back: T = lgwks_std::json::from_str(&format!("\"{name}\"")).map_err(describe)?;
        ensure(back == *value, &format!("{name}: decode does not invert"))?;
        for near in [
            name.to_uppercase(),
            name.to_lowercase().replace(char::is_lowercase, "x"),
            format!(" {name}"),
            format!("{name} "),
            format!("{name}s"),
            String::from(&name[..name.len().saturating_sub(1)]),
            String::new(),
        ] {
            if near == *name {
                continue;
            }
            ensure(
                lgwks_std::json::from_str::<T>(&format!("{near:?}")).is_err(),
                &format!("{near:?}: a near miss of {name} decoded"),
            )?;
        }
        ensure(
            lgwks_std::json::from_str::<T>("0").is_err()
                && lgwks_std::json::from_str::<T>("null").is_err(),
            &format!("{name}: a non-string decoded"),
        )?;
    }
    Ok(())
}

/// Bug caught: a closed vocabulary that accepts a re-cased, padded or
/// truncated name, so a renderer and the validator disagree on a placement,
/// slot, status or destination.
#[test]
fn sim_protocol_closed_enums_roundtrip_and_refuse_near_misses() -> Result<(), String> {
    closed_set(
        &[
            ChromePlacement::Top,
            ChromePlacement::Right,
            ChromePlacement::Bottom,
            ChromePlacement::Left,
        ],
        &["top", "right", "bottom", "left"],
    )?;
    closed_set(
        &[WorkspaceTheme::Calm, WorkspaceTheme::Retro],
        &["calm", "retro"],
    )?;
    closed_set(
        &[
            WidgetSurface::Web,
            WidgetSurface::Native,
            WidgetSurface::Both,
        ],
        &["web", "native", "both"],
    )?;
    closed_set(
        &[WorkspaceSurfaceRole::Panel, WorkspaceSurfaceRole::Context],
        &["panel", "context"],
    )?;
    closed_set(
        &[
            WorkspaceSlot::Primary,
            WorkspaceSlot::Secondary,
            WorkspaceSlot::Tertiary,
            WorkspaceSlot::Overlay,
        ],
        &["primary", "secondary", "tertiary", "overlay"],
    )?;
    closed_set(
        &[
            WorkspaceToolIcon::Cursor,
            WorkspaceToolIcon::Compose,
            WorkspaceToolIcon::Duplicate,
            WorkspaceToolIcon::Remove,
        ],
        &["cursor", "compose", "duplicate", "remove"],
    )?;
    closed_set(
        &[EgressDestination::CloudRelay, EgressDestination::AuditVault],
        &["cloudrelay", "auditvault"],
    )?;
    closed_set(
        &[ActorType::Human, ActorType::Agent, ActorType::System],
        &["Human", "Agent", "System"],
    )?;
    closed_set(
        &[
            TrustBin::System,
            TrustBin::Managed,
            TrustBin::Generated,
            TrustBin::Untrusted,
        ],
        &["System", "Managed", "Generated", "Untrusted"],
    )?;
    closed_set(
        &[
            LocalActionStatus::Pending,
            LocalActionStatus::Running,
            LocalActionStatus::Completed,
            LocalActionStatus::Failed,
            LocalActionStatus::Compensating,
            LocalActionStatus::Compensated,
        ],
        &[
            "Pending",
            "Running",
            "Completed",
            "Failed",
            "Compensating",
            "Compensated",
        ],
    )?;
    closed_set(
        &[
            SyncStatus::NotSynced,
            SyncStatus::Uploading,
            SyncStatus::Acknowledged,
            SyncStatus::ConflictDetected,
        ],
        &["NotSynced", "Uploading", "Acknowledged", "ConflictDetected"],
    )?;
    closed_set(
        &[
            CompensationStrategy::None,
            CompensationStrategy::Rollback,
            CompensationStrategy::Compensate,
            CompensationStrategy::Abandon,
        ],
        &["None", "Rollback", "Compensate", "Abandon"],
    )?;
    closed_set(
        &[
            OfflineBehavior::QueueLocally,
            OfflineBehavior::FailClosed,
            OfflineBehavior::DegradeToCache,
            OfflineBehavior::Unsupported,
        ],
        &[
            "QueueLocally",
            "FailClosed",
            "DegradeToCache",
            "Unsupported",
        ],
    )
}

/// Bug caught: a node taxonomy whose action tiers or data-exposure levels lose
/// their order, so a vision layer can reach more data than an app, or a relay
/// can act.
#[test]
fn sim_protocol_node_taxonomy_keeps_both_axes_ordered() -> Result<(), String> {
    let order = [
        NodeKind::Vault,
        NodeKind::DataEngine,
        NodeKind::App,
        NodeKind::VisionLayer,
        NodeKind::Relay,
    ];
    for pair in order.windows(2) {
        ensure(
            pair[0].data_exposure_level() > pair[1].data_exposure_level(),
            &format!(
                "{:?} must hold strictly more data than {:?}",
                pair[0], pair[1]
            ),
        )?;
        ensure(
            pair[0].max_action_tier() >= pair[1].max_action_tier(),
            &format!("{:?} must not act less than {:?}", pair[0], pair[1]),
        )?;
    }
    ensure(
        NodeKind::App.max_action_tier() == NodeKind::VisionLayer.max_action_tier()
            && NodeKind::App.data_exposure_level() > NodeKind::VisionLayer.data_exposure_level(),
        "an app and a vision layer act alike but see differently",
    )?;
    ensure(
        NodeKind::Relay.max_action_tier() == 0 && NodeKind::Relay.data_exposure_level() == 0,
        "a relay neither acts nor sees",
    )?;
    for kind in order {
        ensure(
            kind.max_action_tier() <= 3 && kind.data_exposure_level() <= 4,
            "tiers stay inside their documented ranges",
        )?;
    }
    closed_set(
        &order,
        &["Vault", "DataEngine", "App", "VisionLayer", "Relay"],
    )
}

/// A well-formed canonical UUID of a seeded value.
pub fn seeded_uuid(random: &mut SimRng) -> String {
    let high = random.next_u64();
    let low = random.next_u64();
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        high.wrapping_shr(32),
        high.wrapping_shr(16) & 0xffff,
        high & 0xffff,
        low.wrapping_shr(48),
        low & 0xffff_ffff_ffff
    )
}

/// A malformed identity of a seeded flavour.
fn broken_uuid(random: &mut SimRng) -> String {
    let good = seeded_uuid(random);
    match random.below(8) {
        0 => String::new(),
        1 => good.replace('-', ""),
        2 => format!("{{{good}}}"),
        3 => format!("urn:uuid:{good}"),
        4 => good.chars().take(35).collect(),
        5 => format!("{good}0"),
        6 => good.replacen(|digit: char| digit.is_ascii_hexdigit(), "g", 1),
        _ => "x".repeat(600),
    }
}

/// Bug caught: a session invariant that binds a malformed identity, names a
/// later bad field instead of the first, or moves the 512-byte tenant bound.
#[test]
fn sim_protocol_session_invariant_names_the_first_bad_field() -> Result<(), String> {
    for seed in 0..400_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(650_000));
        let mut parts: Vec<String> = Vec::new();
        let mut broken = [false; 5];
        for (slot, flag) in broken.iter_mut().enumerate() {
            if slot == 3 {
                let length = [0_usize, 1, 511, 512, 513, 600, 7][random.below_len(7)];
                *flag = length == 0 || length > 512;
                parts.push("t".repeat(length));
            } else if random.below(3) == 0 {
                *flag = true;
                parts.push(broken_uuid(&mut random));
            } else {
                parts.push(seeded_uuid(&mut random));
            }
        }
        let want = broken.iter().position(|flag| *flag).map(|slot| match slot {
            0 => SessionInvariantError::InvalidWorkObjectId,
            1 => SessionInvariantError::InvalidSessionId,
            2 => SessionInvariantError::InvalidPolicyBundleId,
            3 => SessionInvariantError::InvalidTenantId,
            _ => SessionInvariantError::InvalidUserSub,
        });
        let built = SessionInvariant::new(
            parts[0].clone(),
            parts[1].clone(),
            parts[2].clone(),
            parts[3].clone(),
            parts[4].clone(),
        );
        match (built, want) {
            (Ok(session), None) => {
                ensure(
                    session.work_object_id() == parts[0]
                        && session.session_id() == parts[1]
                        && session.policy_bundle_id() == parts[2]
                        && session.tenant_id() == parts[3]
                        && session.user_sub() == parts[4],
                    &format!("seed {seed}: an accessor returned another field"),
                )?;
                let text = json_of(&session)?;
                let back: SessionInvariant = lgwks_std::json::from_str(&text).map_err(describe)?;
                ensure(
                    back == session,
                    &format!("seed {seed}: the wire changed the session"),
                )?;
            }
            (Err(got), Some(expected)) => ensure(
                got == expected,
                &format!("seed {seed}: named {got:?}, the first bad field is {expected:?}"),
            )?,
            (outcome, expected) => {
                return Err(format!(
                    "seed {seed}: built {outcome:?}, reference says {expected:?}"
                ));
            }
        }
    }
    Ok(())
}
