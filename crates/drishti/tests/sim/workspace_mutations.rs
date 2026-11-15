//! Seeded workspace mutations: the canonical layout is mutated across every
//! validated field, and each mutant either validates with deterministic web
//! output or fails with its exact issue.
//!
//! Same seed replays the same trace hash over issues and web bytes.

use drishti::canvas_protocol::Capability;
use drishti::canvas_protocol::workspace_layout::{WorkspaceLayout, load_validated};

use crate::fixtures::ensure;
use crate::rng::{SimRng, TraceHash};

/// Load the canonical layout bytes.
fn canonical_bytes() -> Result<Vec<u8>, String> {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets/canvas-protocol/workspaces/workspace-landing.json");
    std::fs::read(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))
}

/// Mutate one decoded layout with seeded edits.
fn mutate(random: &mut SimRng, layout: &mut WorkspaceLayout) {
    match random.below(9) {
        0 => {
            let width = random.below_len(24);
            layout.set_layout_id(random.text(width));
        }
        1 => {
            let width = random.below_len(100);
            layout.set_title(random.text(width));
        }
        2 => {
            let width = random.below_len(140);
            layout.set_subtitle(random.text(width));
        }
        3 => {
            let position = random.below_len(layout.surfaces().len());
            if let Some(surface) = layout.surfaces_mut().get_mut(position) {
                let width = random.below_len(24);
                surface.descriptor_mut().set_widget_id(random.text(width));
            }
        }
        4 => {
            let position = random.below_len(layout.surfaces().len());
            if let Some(surface) = layout.surfaces_mut().get_mut(position) {
                // The draw is below 40, so the low byte is the whole
                // order, with no conversion to fail.
                let order_bytes = random.below_u32(40).to_le_bytes();
                surface.presentation_mut().set_order(order_bytes[0]);
            }
        }
        5 => {
            let position = random.below_len(layout.surfaces().len());
            if let Some(surface) = layout.surfaces_mut().get_mut(position) {
                surface
                    .descriptor_mut()
                    .capabilities_mut()
                    .push(Capability::TapeRead);
            }
        }
        6 => {
            let position = random.below_len(layout.tools().len());
            if let Some(tool) = layout.tools_mut().get_mut(position) {
                let width = random.below_len(24);
                tool.set_tool_id(random.text(width));
            }
        }
        7 => {
            // The draw is below 3, so the low two bytes are the whole
            // version, with no conversion to fail.
            let version_bytes = random.below_u32(3).to_le_bytes();
            layout.set_schema_version(u16::from_le_bytes([version_bytes[0], version_bytes[1]]));
        }
        _ => {
            layout.tools_mut().clear();
        }
    }
}

/// Run one mutation campaign from `seed` and hash issues plus web bytes.
fn run_mutations(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let bytes = canonical_bytes()?;
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    for _ in 0..120 {
        let mut layout: WorkspaceLayout = lgwks_std::json::from_slice(&bytes)
            .map_err(|error| format!("canonical must decode: {error}"))?;
        mutate(&mut random, &mut layout);
        match layout.validate() {
            Ok(()) => {
                trace.mix_u64(1);
                let input = layout
                    .web_renderer_input()
                    .map_err(|error| format!("valid layout must project: {error}"))?;
                let rendered = lgwks_std::json::to_vec(&input)
                    .map_err(|error| format!("projection must serialize: {error}"))?;
                trace.mix_len(rendered.len());
                for byte in rendered.iter().take(32) {
                    trace.mix_u64(u64::from(*byte));
                }
            }
            Err(error) => {
                trace.mix_str(issue_name(&error.issue()));
                trace.mix_str(error.at());
            }
        }
    }
    // Byte-string mutations exercise the loader too.
    let text = String::from_utf8(bytes).map_err(|error| format!("{error}"))?;
    let boundaries: Vec<usize> = text.char_indices().map(|(index, _)| index).collect();
    for _ in 0..20 {
        let Some(boundary) = boundaries.get(random.below_len(boundaries.len())) else {
            trace.mix_u64(2);
            continue;
        };
        let mut mutated = text.clone();
        mutated.insert(*boundary, 'x');
        match load_validated(mutated.as_bytes()) {
            Ok(_) => trace.mix_u64(1),
            Err(_) => trace.mix_u64(0),
        }
    }
    Ok(trace.finish())
}

/// Stable name for each validation issue.
fn issue_name(
    issue: &drishti::canvas_protocol::workspace_layout::WorkspaceLayoutIssue,
) -> &'static str {
    use drishti::canvas_protocol::workspace_layout::WorkspaceLayoutIssue as Issue;
    match *issue {
        Issue::UnsupportedSchemaVersion => "unsupported-schema-version",
        Issue::InvalidLayoutId => "invalid-layout-id",
        Issue::InvalidTitle => "invalid-title",
        Issue::InvalidSubtitle => "invalid-subtitle",
        Issue::InvalidChrome => "invalid-chrome",
        Issue::InvalidSurfaceCount => "invalid-surface-count",
        Issue::DuplicateWidgetId => "duplicate-widget-id",
        Issue::DuplicateSurfaceOrder => "duplicate-surface-order",
        Issue::InvalidWidgetDescriptor => "invalid-widget-descriptor",
        Issue::PrivilegedWidgetCapability => "privileged-widget-capability",
        Issue::InvalidPresentation => "invalid-presentation",
        Issue::InvalidToolCount => "invalid-tool-count",
        Issue::DuplicateToolId => "duplicate-tool-id",
        Issue::InvalidTool => "invalid-tool",
        // Future variants report by position; the enum is closed today.
        _ => "unrecognized-issue",
    }
}

/// Same seed replays the same hash.
#[test]
fn sim_workspace_mutations_replay_identically() -> Result<(), String> {
    for seed in [83_u64, 89, 1001] {
        if run_mutations(seed)? != run_mutations(seed)? {
            return Err(format!("seed {seed} diverged"));
        }
    }
    if run_mutations(83)? == run_mutations(84)? {
        return Err(String::from("distinct seeds must diverge"));
    }
    Ok(())
}

/// Run one privileged-capability campaign from `seed`: pushing the ML-only
/// tape capability onto a seeded surface always fails validation, and the
/// trace replays.
fn run_privileged_caps(seed: u64) -> Result<u64, String> {
    let mut random = SimRng::seed(seed);
    let bytes = canonical_bytes()?;
    let mut trace = TraceHash::new();
    trace.mix_u64(seed);
    let mut refused = 0_usize;
    for _ in 0..120 {
        let mut layout: WorkspaceLayout = lgwks_std::json::from_slice(&bytes)
            .map_err(|error| format!("canonical must decode: {error}"))?;
        ensure(!layout.surfaces().is_empty(), || {
            String::from("canonical must carry surfaces")
        })?;
        let position = random.below_len(layout.surfaces().len());
        let surface = layout
            .surfaces_mut()
            .get_mut(position)
            .ok_or_else(|| String::from("surface index out of range"))?;
        surface
            .descriptor_mut()
            .capabilities_mut()
            .push(Capability::TapeRead);
        let error = layout
            .validate()
            .err()
            .ok_or_else(|| String::from("privileged cap must fail"))?;
        trace.mix_str(issue_name(&error.issue()));
        trace.mix_str(error.at());
        refused = refused.saturating_add(1);
    }
    ensure(refused == 120, || {
        String::from("every privileged mutant must fail")
    })?;
    trace.mix_len(refused);
    Ok(trace.finish())
}

/// Same seed replays the same privileged-cap hash with identical refusals.
#[test]
fn sim_workspace_privileged_cap_replays_identically() -> Result<(), String> {
    for seed in [97_u64, 101, 1_151] {
        if run_privileged_caps(seed)? != run_privileged_caps(seed)? {
            return Err(format!("seed {seed} diverged"));
        }
    }
    if run_privileged_caps(97)? == run_privileged_caps(98)? {
        return Err(String::from("distinct seeds must diverge"));
    }
    Ok(())
}
