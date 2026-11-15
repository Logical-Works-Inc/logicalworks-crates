//! The fault table for the workspace-layout sims: a base layout and every
//! single-fault variant the contract names, each with the exact issue and
//! path validation must report.

use drishti::canvas_protocol::workspace_layout::{
    WorkspaceChrome, WorkspaceLayout, WorkspaceLayoutIssue, WorkspacePresentation,
    WorkspaceSignalPattern, WorkspaceSurface, WorkspaceTool, WorkspaceWidgetDescriptor,
};
use drishti::canvas_protocol::{
    Capability, ChromePlacement, WidgetSurface, WorkspaceSlot, WorkspaceSurfaceRole,
    WorkspaceTheme, WorkspaceToolIcon,
};

/// A widget descriptor over explicit parts and no signal patterns.
fn widget(
    id: &str,
    kind: &str,
    version: &str,
    capabilities: Vec<Capability>,
) -> WorkspaceWidgetDescriptor {
    WorkspaceWidgetDescriptor::new(
        String::from(kind),
        String::from(id),
        String::from(version),
        WidgetSurface::Both,
        Vec::new(),
        Vec::new(),
        capabilities,
    )
}

/// A surface in a panel slot with the given id and order.
fn surface(id: &str, order: u8) -> WorkspaceSurface {
    WorkspaceSurface::new(
        widget(id, "kind-a", "1.0.0", vec![Capability::SignalEmit]),
        WorkspacePresentation::new(
            String::from("Panel"),
            WorkspaceSurfaceRole::Panel,
            WorkspaceSlot::Primary,
            order,
        ),
    )
}

/// A tool with the given id.
pub fn tool(id: &str) -> WorkspaceTool {
    WorkspaceTool::new(
        String::from(id),
        String::from("Label"),
        WorkspaceToolIcon::Cursor,
    )
}

/// The chrome every base layout uses.
fn chrome() -> WorkspaceChrome {
    WorkspaceChrome::new(
        ChromePlacement::Top,
        vec![ChromePlacement::Top, ChromePlacement::Left],
        true,
        WorkspaceTheme::Calm,
    )
}

/// A layout over the standard chrome and the given surfaces and tools.
fn layout_of(surfaces: Vec<WorkspaceSurface>, tools: Vec<WorkspaceTool>) -> WorkspaceLayout {
    WorkspaceLayout::new(
        1,
        String::from("T"),
        String::from("workspace.landing"),
        String::new(),
        chrome(),
        surfaces,
        tools,
    )
}

/// The base layout: two surfaces and two tools, valid.
pub fn base() -> WorkspaceLayout {
    WorkspaceLayout::new(
        1,
        String::from("Title"),
        String::from("workspace.landing"),
        String::from("Subtitle"),
        chrome(),
        vec![surface("widget.one", 0), surface("widget.two", 1)],
        vec![tool("canvas.first"), tool("canvas.second")],
    )
}

/// One fault: a label, the broken layout and the refusal it must draw.
pub type Fault = (String, WorkspaceLayout, WorkspaceLayoutIssue, &'static str);

/// Collects faults.
pub struct Faults {
    /// The table so far.
    table: Vec<Fault>,
}

impl Faults {
    /// Record one fault.
    fn add(
        &mut self,
        label: String,
        layout: WorkspaceLayout,
        issue: WorkspaceLayoutIssue,
        at: &'static str,
    ) {
        self.table.push((label, layout, issue, at));
    }

    /// The whole table.
    pub fn everything() -> Vec<Fault> {
        let mut out = Self { table: Vec::new() };
        header_faults(&mut out);
        surface_faults(&mut out);
        presentation_and_tool_faults(&mut out);
        out.table
    }
}

/// Faults in the header fields: version, id, title, subtitle, chrome.
fn header_faults(out: &mut Faults) {
    for version in [0_u16, 2, 255, u16::MAX] {
        let mut layout = base();
        layout.set_schema_version(version);
        out.add(
            format!("schema {version}"),
            layout,
            WorkspaceLayoutIssue::UnsupportedSchemaVersion,
            "schema_version",
        );
    }
    for id in [
        "",
        "workspace",
        "Workspace.landing",
        "workspace.Landing",
        "workspace..landing",
        ".landing",
        "workspace.landing\0",
        "1workspace.landing",
        "workspace.landing_x",
        "w\u{f6}rkspace.landing",
        "workspace.landing ",
        " workspace.landing",
        "workspace.",
    ] {
        let mut layout = base();
        layout.set_layout_id(String::from(id));
        out.add(
            format!("layout id {id:?}"),
            layout,
            WorkspaceLayoutIssue::InvalidLayoutId,
            "layout_id",
        );
    }
    for title in [
        "",
        &"x".repeat(81),
        "a\nb",
        &"\u{e9}".repeat(81),
        "tab\there",
    ] {
        let mut layout = base();
        layout.set_title(String::from(title));
        out.add(
            format!("title {} scalars", title.chars().count()),
            layout,
            WorkspaceLayoutIssue::InvalidTitle,
            "title",
        );
    }
    for subtitle in [&"x".repeat(121), "a\u{7f}b", "nul\0"] {
        let mut layout = base();
        layout.set_subtitle(String::from(subtitle));
        out.add(
            format!("subtitle {} scalars", subtitle.chars().count()),
            layout,
            WorkspaceLayoutIssue::InvalidSubtitle,
            "subtitle",
        );
    }
    let bad_chromes = [
        WorkspaceChrome::new(ChromePlacement::Top, Vec::new(), true, WorkspaceTheme::Calm),
        WorkspaceChrome::new(
            ChromePlacement::Top,
            vec![ChromePlacement::Top, ChromePlacement::Top],
            true,
            WorkspaceTheme::Calm,
        ),
        WorkspaceChrome::new(
            ChromePlacement::Top,
            vec![ChromePlacement::Left],
            true,
            WorkspaceTheme::Calm,
        ),
        WorkspaceChrome::new(
            ChromePlacement::Top,
            vec![
                ChromePlacement::Top,
                ChromePlacement::Right,
                ChromePlacement::Bottom,
                ChromePlacement::Left,
                ChromePlacement::Top,
            ],
            false,
            WorkspaceTheme::Retro,
        ),
    ];
    for (index, bad) in bad_chromes.into_iter().enumerate() {
        let layout = WorkspaceLayout::new(
            1,
            String::from("T"),
            String::from("workspace.landing"),
            String::new(),
            bad,
            vec![surface("widget.one", 0)],
            vec![tool("canvas.first")],
        );
        out.add(
            format!("chrome {index}"),
            layout,
            WorkspaceLayoutIssue::InvalidChrome,
            "chrome",
        );
    }
}

/// Faults in the surface list and each widget descriptor.
fn surface_faults(out: &mut Faults) {
    for count in [0_usize, 33, 1_000] {
        let surfaces: Vec<WorkspaceSurface> = (0..count)
            .map(|index| surface(&format!("widget.n{index}"), 0))
            .collect();
        out.add(
            format!("{count} surfaces"),
            layout_of(surfaces, vec![tool("canvas.first")]),
            WorkspaceLayoutIssue::InvalidSurfaceCount,
            "surfaces",
        );
    }
    let mut dup_id = base();
    dup_id.surfaces_mut()[1]
        .descriptor_mut()
        .set_widget_id(String::from("widget.one"));
    out.add(
        String::from("duplicate widget id"),
        dup_id,
        WorkspaceLayoutIssue::DuplicateWidgetId,
        "surfaces[].descriptor.widget_id",
    );
    let mut dup_order = base();
    dup_order.surfaces_mut()[1].presentation_mut().set_order(0);
    out.add(
        String::from("duplicate order"),
        dup_order,
        WorkspaceLayoutIssue::DuplicateSurfaceOrder,
        "surfaces[].presentation.order",
    );
    for id in [
        "",
        "widget",
        "Widget.one",
        "widget.One",
        "widget..one",
        "widget.one\n",
    ] {
        let mut layout = base();
        layout.surfaces_mut()[0]
            .descriptor_mut()
            .set_widget_id(String::from(id));
        out.add(
            format!("widget id {id:?}"),
            layout,
            WorkspaceLayoutIssue::InvalidWidgetDescriptor,
            "surfaces[].descriptor",
        );
    }
    for kind in ["", "Kind", "a..b", "-a", "a-", "1kind", "kind_a", "kind a"] {
        let mut layout = base();
        layout.surfaces_mut()[0]
            .descriptor_mut()
            .set_kind(String::from(kind));
        out.add(
            format!("kind {kind:?}"),
            layout,
            WorkspaceLayoutIssue::InvalidWidgetDescriptor,
            "surfaces[].descriptor",
        );
    }
    for version in [
        "", "1", "1.2", "1.2.3.4", "a.b.c", "1..3", "1.2.3 ", " 1.2.3", "1.2.-3", "v1.2.3",
        "1,2,3", "1.2.3.",
    ] {
        let mut layout = base();
        layout.surfaces_mut()[0]
            .descriptor_mut()
            .set_version(String::from(version));
        out.add(
            format!("version {version:?}"),
            layout,
            WorkspaceLayoutIssue::InvalidWidgetDescriptor,
            "surfaces[].descriptor",
        );
    }
    for grant in [
        vec![Capability::TapeRead],
        vec![Capability::IntentEmit],
        vec![Capability::SignalEmit, Capability::SignalEmit],
        vec![Capability::SignalEmit, Capability::Shred],
        vec![Capability::RemoteCompute],
    ] {
        let mut layout = base();
        layout.surfaces_mut()[0]
            .descriptor_mut()
            .capabilities_mut()
            .clone_from(&grant);
        out.add(
            format!("capabilities {grant:?}"),
            layout,
            WorkspaceLayoutIssue::PrivilegedWidgetCapability,
            "surfaces[].descriptor.capabilities",
        );
    }
    for (count, glob) in [
        (65_usize, "a.*"),
        (1, ""),
        (1, "a\0b"),
        (1, &"x".repeat(161)),
    ] {
        let patterns: Vec<WorkspaceSignalPattern> = (0..count)
            .map(|_| WorkspaceSignalPattern::new(String::from(glob)))
            .collect();
        let descriptor = WorkspaceWidgetDescriptor::new(
            String::from("kind-a"),
            String::from("widget.one"),
            String::from("1.0.0"),
            WidgetSurface::Both,
            patterns,
            Vec::new(),
            Vec::new(),
        );
        let mut layout = base();
        *layout.surfaces_mut()[0].descriptor_mut() = descriptor;
        out.add(
            format!("pattern ({count}, {} scalars)", glob.chars().count()),
            layout,
            WorkspaceLayoutIssue::InvalidWidgetDescriptor,
            "surfaces[].descriptor",
        );
    }
}

/// Faults in presentation metadata and the tool list.
fn presentation_and_tool_faults(out: &mut Faults) {
    for (role, slot, order, title) in [
        (
            WorkspaceSurfaceRole::Panel,
            WorkspaceSlot::Overlay,
            0_u8,
            "T",
        ),
        (
            WorkspaceSurfaceRole::Context,
            WorkspaceSlot::Primary,
            0,
            "T",
        ),
        (
            WorkspaceSurfaceRole::Context,
            WorkspaceSlot::Tertiary,
            0,
            "T",
        ),
        (WorkspaceSurfaceRole::Panel, WorkspaceSlot::Primary, 32, "T"),
        (
            WorkspaceSurfaceRole::Panel,
            WorkspaceSlot::Primary,
            u8::MAX,
            "T",
        ),
        (WorkspaceSurfaceRole::Panel, WorkspaceSlot::Primary, 0, ""),
        (
            WorkspaceSurfaceRole::Panel,
            WorkspaceSlot::Primary,
            0,
            "a\nb",
        ),
    ] {
        let mut layout = base();
        *layout.surfaces_mut()[0].presentation_mut() =
            WorkspacePresentation::new(String::from(title), role, slot, order);
        out.add(
            format!("presentation {role:?} {slot:?} {order} {title:?}"),
            layout,
            WorkspaceLayoutIssue::InvalidPresentation,
            "surfaces[].presentation",
        );
    }
    for count in [0_usize, 17, 500] {
        let tools: Vec<WorkspaceTool> = (0..count)
            .map(|index| tool(&format!("canvas.t{index}")))
            .collect();
        out.add(
            format!("{count} tools"),
            layout_of(vec![surface("widget.one", 0)], tools),
            WorkspaceLayoutIssue::InvalidToolCount,
            "tools",
        );
    }
    for id in [
        "canvas.",
        "tool",
        "canvas.Bad",
        "canvas.-x",
        "canvas.a b",
        "canvas.a_b",
        "Canvas.a",
        "canvas.\u{e9}",
    ] {
        let mut layout = base();
        layout.tools_mut()[0].set_tool_id(String::from(id));
        out.add(
            format!("tool id {id:?}"),
            layout,
            WorkspaceLayoutIssue::InvalidTool,
            "tools[]",
        );
    }
    let mut dup_tool = base();
    dup_tool.tools_mut()[1].set_tool_id(String::from("canvas.first"));
    out.add(
        String::from("duplicate tool id"),
        dup_tool,
        WorkspaceLayoutIssue::DuplicateToolId,
        "tools[].tool_id",
    );
    for label in ["", &"x".repeat(41), "a\nb"] {
        let labelled = WorkspaceTool::new(
            String::from("canvas.first"),
            String::from(label),
            WorkspaceToolIcon::Remove,
        );
        out.add(
            format!("tool label {} scalars", label.chars().count()),
            layout_of(vec![surface("widget.one", 0)], vec![labelled]),
            WorkspaceLayoutIssue::InvalidTool,
            "tools[]",
        );
    }
}
