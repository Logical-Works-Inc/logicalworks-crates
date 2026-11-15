//! Workspace-layout sims: seeded valid layouts round-trip the wire, every
//! single fault is named with its exact issue and path, the strict loader
//! refuses unknown and mistyped fields and every cut of the bytes, and the
//! web projection is deterministic and free of native-only surfaces.

use drishti::canvas_protocol::workspace_layout::{
    WorkspaceChrome, WorkspaceLayout, WorkspaceLayoutError, WorkspaceLayoutIssue,
    WorkspaceLoadError, WorkspacePresentation, WorkspaceSignalPattern, WorkspaceSurface,
    WorkspaceWidgetDescriptor, load_validated,
};
use drishti::canvas_protocol::{
    Capability, ChromePlacement, WidgetSurface, WorkspaceSlot, WorkspaceSurfaceRole, WorkspaceTheme,
};

use crate::canvas_layout_faults::{Faults, base, tool};
use crate::motion_kit::{describe, ensure, with_member};
use crate::rng::SimRng;

/// Lowercase letters.
const LETTERS: &[u8] = b"abcdefghijklmnopqrstuvwxyz";

/// A seeded lowercase word of one to six letters.
fn word(random: &mut SimRng) -> String {
    let count = random.below_len(6).saturating_add(1);
    (0..count)
        .map(|_| char::from(LETTERS[random.below_len(LETTERS.len())]))
        .collect()
}

/// A seeded valid layout.
fn random_layout(random: &mut SimRng) -> Result<WorkspaceLayout, String> {
    let placements = [
        ChromePlacement::Top,
        ChromePlacement::Right,
        ChromePlacement::Bottom,
        ChromePlacement::Left,
    ];
    let default = placements[random.below_len(4)];
    let mut allowed = vec![default];
    for placement in placements {
        if placement != default && random.below(2) == 0 {
            allowed.push(placement);
        }
    }
    let theme = if random.below(2) == 0 {
        WorkspaceTheme::Calm
    } else {
        WorkspaceTheme::Retro
    };
    let chrome = WorkspaceChrome::new(default, allowed, random.below(2) == 0, theme);
    let surface_count = random.below_len(6).saturating_add(1);
    let mut surfaces = Vec::new();
    for index in 0..surface_count {
        let kind = format!("{}-{}", word(random), word(random));
        let version = format!(
            "{}.{}.{}",
            random.below(20),
            random.below(20),
            random.below(20)
        );
        let mut capabilities = Vec::new();
        if random.below(2) == 0 {
            capabilities.push(Capability::SignalEmit);
        }
        if random.below(2) == 0 {
            capabilities.push(Capability::SignalSubscribe);
        }
        let surface_kind = match random.below(3) {
            0 => WidgetSurface::Web,
            1 => WidgetSurface::Native,
            _ => WidgetSurface::Both,
        };
        let emit_count = random.below_len(4);
        let mut emits = Vec::new();
        for _ in 0..emit_count {
            emits.push(WorkspaceSignalPattern::new(format!("{}.*", word(random))));
        }
        let descriptor = WorkspaceWidgetDescriptor::new(
            kind,
            format!("widget.{}{index}", word(random)),
            version,
            surface_kind,
            emits,
            Vec::new(),
            capabilities,
        );
        let (role, slot) = if random.below(4) == 0 {
            (WorkspaceSurfaceRole::Context, WorkspaceSlot::Overlay)
        } else {
            (WorkspaceSurfaceRole::Panel, WorkspaceSlot::Secondary)
        };
        let order = u8::try_from(index.saturating_mul(3).saturating_add(random.below_len(3)))
            .map_err(describe)?;
        surfaces.push(WorkspaceSurface::new(
            descriptor,
            WorkspacePresentation::new(word(random), role, slot, order),
        ));
    }
    let tool_count = random.below_len(4).saturating_add(1);
    let mut tools = Vec::new();
    for index in 0..tool_count {
        tools.push(tool(&format!("canvas.{}-{index}", word(random))));
    }
    Ok(WorkspaceLayout::new(
        1,
        word(random),
        format!("workspace.{}", word(random)),
        word(random),
        chrome,
        surfaces,
        tools,
    ))
}

/// The wire text of a layout.
fn wire(layout: &WorkspaceLayout) -> Result<String, String> {
    Ok(lgwks_std::json::to_value(layout)
        .map_err(describe)?
        .to_string())
}

/// Bug caught: a valid layout the strict loader refuses, or one that changes
/// on a round trip, so a saved workspace does not reopen as it was.
#[test]
fn sim_layout_seeded_layouts_validate_and_survive_the_wire() -> Result<(), String> {
    ensure(base().validate().is_ok(), "the base layout must be valid")?;
    for seed in 0..300_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(500_000));
        let layout = random_layout(&mut random)?;
        ensure(
            layout.validate().is_ok(),
            &format!(
                "seed {seed}: a generated layout was refused: {:?}",
                layout.validate()
            ),
        )?;
        let text = wire(&layout)?;
        let loaded = load_validated(text.as_bytes()).map_err(describe)?;
        ensure(
            loaded == layout,
            &format!("seed {seed}: the layout changed on the wire"),
        )?;
        ensure(
            wire(&loaded)? == text,
            &format!("seed {seed}: the wire text is not stable"),
        )?;
    }
    Ok(())
}

/// Bug caught: a layout fault reported under the wrong issue or path, or not
/// reported at all, so a renderer receives a workspace the contract forbids.
#[test]
fn sim_layout_every_single_fault_is_named_with_its_path() -> Result<(), String> {
    let table = Faults::everything();
    ensure(table.len() > 90, "the fault table must be broad")?;
    for (label, layout, issue, at) in table {
        ensure(
            layout.validate() == Err(WorkspaceLayoutError::new(issue, at)),
            &format!(
                "{label}: got {:?}, want {issue:?} at {at}",
                layout.validate()
            ),
        )?;
        ensure(
            layout.web_renderer_input().is_err(),
            &format!("{label}: the web projection accepted an invalid layout"),
        )?;
    }
    let mut edges = base();
    edges.set_title("x".repeat(80));
    edges.set_subtitle("\u{e9}".repeat(120));
    edges.surfaces_mut()[0].presentation_mut().set_order(31);
    ensure(
        edges.validate().is_ok(),
        "the exact maxima must be accepted",
    )
}

/// Bug caught: a loader that accepts an unknown key, a bool for an integer,
/// an out-of-range order, a cut document or non-UTF-8 bytes, so a hand-edited
/// or truncated file opens as a layout.
#[test]
fn sim_layout_loader_refuses_unknown_mistyped_and_cut_documents() -> Result<(), String> {
    let text = wire(&base())?;
    ensure(
        load_validated(text.as_bytes()).is_ok(),
        "the clean document must load",
    )?;
    for (key, value) in [
        ("schema_version", "\"1\""),
        ("schema_version", "true"),
        ("schema_version", "1.5"),
        ("schema_version", "65536"),
        ("schema_version", "-1"),
        ("order", "256"),
        ("order", "true"),
        ("order", "-1"),
        ("order", "\"0\""),
        ("collapsible", "1"),
        ("collapsible", "\"true\""),
        ("self_listen", "0"),
        ("pinned", "null"),
    ] {
        let mutated = with_member(&text, key, value)?;
        ensure(
            matches!(
                load_validated(mutated.as_bytes()),
                Err(WorkspaceLoadError::Decode(_))
            ),
            &format!("{key} = {value} must be a decode failure"),
        )?;
    }
    for anchor in [
        "\"chrome\":{",
        "\"descriptor\":{",
        "\"presentation\":{",
        "\"tools\":[{",
    ] {
        let mutated = text.replacen(anchor, &format!("{anchor}\"zzz\":1,"), 1);
        ensure(
            matches!(
                load_validated(mutated.as_bytes()),
                Err(WorkspaceLoadError::Decode(_))
            ),
            &format!("an unknown key after {anchor} was accepted"),
        )?;
    }
    let top = text.replacen('{', "{\"zzz\":1,", 1);
    ensure(
        load_validated(top.as_bytes()).is_err(),
        "an unknown top-level key was accepted",
    )?;
    for cut in 0..text.len() {
        ensure(
            load_validated(&text.as_bytes()[..cut]).is_err(),
            &format!("the first {cut} bytes loaded as a layout"),
        )?;
    }
    for junk in [
        Vec::new(),
        vec![0xff, 0xfe],
        b"null".to_vec(),
        b"[]".to_vec(),
        b"{}".to_vec(),
        b"\xef\xbb\xbf{}".to_vec(),
        [text.as_bytes(), b"x"].concat(),
        [b"[", text.as_bytes(), b"]"].concat(),
    ] {
        ensure(
            load_validated(&junk).is_err(),
            &format!("{junk:02x?} loaded"),
        )?;
    }
    let invalid = with_member(&text, "schema_version", "7")?;
    ensure(
        matches!(
            load_validated(invalid.as_bytes()),
            Err(WorkspaceLoadError::Invalid(error))
                if error.issue() == WorkspaceLayoutIssue::UnsupportedSchemaVersion
        ),
        "a well-typed wrong version is an Invalid, not a Decode",
    )
}

/// Bug caught: a web projection that keeps a native-only surface, reorders
/// the rest, differs between runs, or yields a layout that itself fails
/// validation (an all-native workspace has nothing for a web renderer, and
/// must say so rather than hand back an empty layout).
#[test]
fn sim_layout_web_projection_is_deterministic_and_native_free() -> Result<(), String> {
    let mut projected = 0_usize;
    let mut refused = 0_usize;
    for seed in 0..300_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(510_000));
        let layout = random_layout(&mut random)?;
        let want: Vec<&str> = layout
            .surfaces()
            .iter()
            .filter(|each| each.descriptor().surface() != WidgetSurface::Native)
            .map(|each| each.descriptor().widget_id())
            .collect();
        if want.is_empty() {
            ensure(
                layout.web_renderer_input()
                    == Err(WorkspaceLayoutError::new(
                        WorkspaceLayoutIssue::InvalidSurfaceCount,
                        "surfaces",
                    )),
                &format!(
                    "seed {seed}: an all-native layout projected to something other than a refusal"
                ),
            )?;
            refused = refused.saturating_add(1);
            continue;
        }
        let web = layout.web_renderer_input().map_err(describe)?;
        let got: Vec<&str> = web
            .surfaces()
            .iter()
            .map(|each| each.descriptor().widget_id())
            .collect();
        ensure(
            got == want,
            &format!("seed {seed}: the projection reordered or kept a native surface"),
        )?;
        projected = projected.saturating_add(1);
        ensure(
            web.validate().is_ok(),
            &format!("seed {seed}: the projection is not itself valid"),
        )?;
        ensure(
            wire(&web)? == wire(&layout.web_renderer_input().map_err(describe)?)?,
            &format!("seed {seed}: two projections differ"),
        )?;
        ensure(
            web.web_renderer_input().map_err(describe)? == web,
            &format!("seed {seed}: projecting twice changed the layout"),
        )?;
        ensure(
            web.title() == layout.title()
                && web.layout_id() == layout.layout_id()
                && web.tools() == layout.tools()
                && web.chrome() == layout.chrome(),
            "chrome, title and tools pass through unchanged",
        )?;
    }
    ensure(
        projected > 30 && refused > 3,
        "the corpus must hold projectable and all-native layouts",
    )
}
