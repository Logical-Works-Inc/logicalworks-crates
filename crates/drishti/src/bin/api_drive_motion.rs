//! Motion conformance driver: constructs every motion type, calls every
//! operation, and prints one canonical digest.
//!
//! Part of the api-drive suite covering the absorbed surface for the surface
//! gate: every new `pub` item needs a caller outside its crate, so these
//! binaries drive the full API end to end against frozen fixtures. A rejected
//! construction or a failed negative control fails the drive loudly. Exit
//! codes keep kernel parity: 0 success, 1 reported failure, 2 when the
//! report could not print.

use std::process::ExitCode;

use drishti::cli::drive::{must, note, refused, uuid};
use drishti::cli::{Failure, Streams};
use drishti::motion::interpolate::clip_path::{Inset, interp_inset};
use drishti::motion::interpolate::color_oklab::{
    OkLab, SRgb, interp_oklab, lerp_oklab, map_to_srgb_gamut, oklab_to_srgb, srgb_to_oklab,
};
use drishti::motion::interpolate::matrix::{Transform2D, interp_transform};
use drishti::motion::interpolate::path_morph::{Point2, interp_path};
use drishti::motion::interpolate::scalar::lerp_scalar;
use drishti::motion::interpolate::vec::{Vec2, Vec3, lerp_vec2, lerp_vec3};
use drishti::motion::node::accumulator::{
    AccumulatorComponent, AccumulatorKind, AccumulatorState, AccumulatorStore,
};
use drishti::motion::node::critical::CriticalSpring;
use drishti::motion::node::tween::{AnimValue, TweenComponent};
use drishti::motion::scrub::{
    ConstantScrub, EmptyScrubMap, HashScrubMap, ScrubSource, ScrubSourceMap,
};
use drishti::motion::{
    BindingTarget, BindingValue, BindingWrite, Clock, ConflictPolicy, Easing, Frame as MotionFrame,
    MotionError, MotionErrorCode, MotionGraph, MotionGraphBuilder, MotionNode, MotionPatch,
    NamedCurve, NodeHandle, NodeKind, PatchOp, PatchValue, PropertyKey, SpringConfig, TargetHandle,
    apply_patch, frame as evaluate_frame, integrate_spring,
};

/// Drive the motion core.
fn drive_motion(digest: &mut String) -> Result<(), Failure> {
    let node_id: NodeHandle = uuid(digest, "node-id", "55555555-5555-5555-5555-555555555555")?;
    let target: TargetHandle = node_id;
    let property: PropertyKey = String::from("opacity");
    note(digest, "target", &target)?;
    note(digest, "property", &property)?;
    let tween = drive_tween(digest)?;
    drive_springs(digest)?;
    let mut store = drive_store(digest, node_id)?;
    let graph = drive_graph(digest, node_id, property, tween)?;
    drive_patch(digest, node_id, graph, &mut store)?;
    drive_interpolation(digest)
}

/// Drive the clock, easing and tween, and return the tween.
fn drive_tween(digest: &mut String) -> Result<TweenComponent, Failure> {
    let clock = Clock::deterministic(0.0);
    note(digest, "clock", &clock.wall_clock_ms())?;
    let later = clock.advance_ms(16.0);
    note(digest, "clock-later", &later.wall_clock_ms())?;
    let easing = Easing::named(NamedCurve::Linear);
    note(digest, "eased", &easing.value_at(0.5))?;
    let bezier = must(digest, "bezier", Easing::cubic_bezier(0.25, 0.1, 0.25, 1.0))?;
    note(digest, "bezier-at", &bezier.value_at(0.5))?;
    refused(
        Easing::cubic_bezier(2.0, 0.0, 0.0, 0.0),
        "out-of-domain bezier must fail",
    )?;
    let tween_error = must(
        digest,
        "tween",
        TweenComponent::new(0.0, 1.0, 0.0, 100.0, easing),
    )?;
    note(digest, "tween-from", &tween_error.from_value())?;
    note(digest, "tween-to", &tween_error.to_value())?;
    note(digest, "tween-start", &tween_error.start_ms())?;
    note(digest, "tween-duration", &tween_error.duration_ms())?;
    note(
        digest,
        "tween-easing-linear",
        &tween_error.easing().value_at(1.0),
    )?;
    note(digest, "tween-at", &tween_error.value_at(0.5))?;
    note(digest, "tween-at-wall", &tween_error.value_at_wall_ms(50.0))?;
    refused(
        TweenComponent::new(0.0, 1.0, 0.0, 0.0, Easing::named(NamedCurve::Linear)),
        "zero-duration tween must fail",
    )?;
    Ok(tween_error)
}

/// Drive the animated value, the motion error, the spring and the accumulators.
fn drive_springs(digest: &mut String) -> Result<(), Failure> {
    let value = AnimValue::new(0.25);
    note(digest, "anim-value", &value.value())?;
    let code = MotionErrorCode::PatchInvalid;
    let motion_error = MotionError::new(code).with_detail("drive");
    note(digest, "motion-error-code", &motion_error.code())?;
    note(digest, "motion-error-detail", &motion_error.detail())?;
    let spring_config = must(digest, "spring-config", SpringConfig::new(100.0, 10.0, 1.0))?;
    note(digest, "tension", &spring_config.tension())?;
    note(digest, "friction", &spring_config.friction())?;
    note(digest, "mass", &spring_config.mass())?;
    refused(
        SpringConfig::new(-1.0, 10.0, 1.0),
        "negative tension must fail",
    )?;
    let (position, velocity) = integrate_spring(0.0, 0.0, 1.0, &spring_config, 0.016);
    note(digest, "integrated", &(position, velocity))?;
    let spring_comp = AccumulatorComponent::new_spring(spring_config);
    let scrub_comp = must(
        digest,
        "scrub-comp",
        AccumulatorComponent::new_scrub(String::from("drive.source")),
    )?;
    let drag_comp = AccumulatorComponent::new_drag();
    let spring_kind: AccumulatorKind = spring_comp.kind();
    note(digest, "spring-kind", &spring_kind)?;
    note(
        digest,
        "spring-config-present",
        &spring_comp.spring_config().is_some(),
    )?;
    let scrub_present = scrub_comp.scrub_source_id().is_some();
    note(digest, "scrub-present", &scrub_present)?;
    if let Some(source) = scrub_comp.scrub_source_id() {
        note(digest, "scrub-source", &source)?;
    }
    note(digest, "preserve", &drag_comp.preserve_velocity())?;
    let loosened = drag_comp.with_preserve_velocity(false);
    note(digest, "loosened", &loosened.preserve_velocity())?;
    refused(
        AccumulatorComponent::new_scrub(String::new()),
        "empty scrub source must fail",
    )?;
    Ok(())
}

/// Drive the accumulator store and the critical spring; return the store.
fn drive_store(digest: &mut String, node_id: NodeHandle) -> Result<AccumulatorStore, Failure> {
    let mut store = AccumulatorStore::new();
    let state = store.get_or_default(node_id);
    state.set_position(0.5);
    state.set_velocity(0.25);
    state.set_last_scrub_value(0.5);
    state.set_last_t_wall_ms(8.0);
    note(digest, "state-node", &state.node_id())?;
    note(digest, "state-position", &state.position())?;
    note(digest, "state-velocity", &state.velocity())?;
    note(digest, "state-scrub", &state.last_scrub_value())?;
    note(digest, "state-wall", &state.last_t_wall_ms())?;
    note(digest, "state-generation", &state.lease_generation())?;
    store.set(node_id, AccumulatorState::zero(node_id));
    note(digest, "store-hit", &store.get(&node_id).is_some())?;
    store.rotate_and_reap();
    store.remove(&node_id);
    note(digest, "store-miss", &store.get(&node_id).is_none())?;
    let mut spring = must(digest, "critical", CriticalSpring::new(0.0, 0.45))?;
    must(digest, "retarget", spring.retarget(2.0))?;
    must(digest, "advance", spring.advance(0.016))?;
    note(digest, "spring-position", &spring.position())?;
    note(digest, "spring-velocity", &spring.velocity())?;
    note(digest, "spring-target", &spring.target())?;
    note(digest, "spring-settled", &spring.is_settled())?;
    spring.snap();
    refused(CriticalSpring::new(0.0, 0.001), "fast response must fail")?;
    Ok(store)
}

/// Drive bindings, nodes and graph edits; return the built graph.
fn drive_graph(
    digest: &mut String,
    node_id: NodeHandle,
    property: PropertyKey,
    tween: TweenComponent,
) -> Result<MotionGraph, Failure> {
    let target_binding = BindingTarget::new(String::from("view.drive"), property.clone());
    note(digest, "binding-handle", &target_binding.handle())?;
    note(digest, "binding-property", &target_binding.property())?;
    let mut tween_node = MotionNode::new(node_id, target_binding, NodeKind::Tween(tween));
    note(digest, "node-id", &tween_node.node_id())?;
    note(digest, "node-kind", &tween_node.kind())?;
    note(digest, "node-target", &tween_node.target().handle())?;
    tween_node.set_kind(NodeKind::Accumulator(AccumulatorComponent::new_drag()));
    note(digest, "node-kind-swapped", &tween_node.kind())?;
    let mut graph: MotionGraph = MotionGraphBuilder::new()
        .add_node(
            node_id,
            String::from("view.drive"),
            property,
            NodeKind::Tween(
                TweenComponent::new(0.0, 1.0, 0.0, 100.0, Easing::named(NamedCurve::Linear))
                    .map_err(|error| Failure::from(format!("tween refused: {error}")))?,
            ),
        )
        .with_conflict_policy(String::from("opacity"), ConflictPolicy::LastWrite)
        .build();
    graph = graph.with_conflict_policy(String::from("x"), ConflictPolicy::Compose);
    note(digest, "graph-version", &graph.version())?;
    note(digest, "graph-count", &graph.node_count())?;
    note(digest, "graph-node", &graph.get_node(&node_id).is_some())?;
    note(
        digest,
        "graph-policy",
        &graph.conflict_policy("opacity").is_some(),
    )?;
    for node in graph.nodes() {
        note(digest, "graph-node-id", &node.node_id())?;
    }
    let grown = graph.clone().add_node(MotionNode::new(
        node_id,
        BindingTarget::new(String::from("h"), String::from("p")),
        NodeKind::Tween(
            TweenComponent::new(0.0, 1.0, 0.0, 100.0, Easing::named(NamedCurve::Linear))
                .map_err(|error| Failure::from(format!("tween refused: {error}")))?,
        ),
    ));
    let shrunk = grown.remove_node(&node_id);
    refused(
        shrunk.clone().replace_node(tween_node.clone()),
        "replacing a missing node must fail",
    )?;
    let replaced = must(
        digest,
        "replaced",
        shrunk.add_node(tween_node.clone()).replace_node(tween_node),
    )?;
    note(digest, "replaced-version", &replaced.version())?;
    Ok(graph)
}

/// Drive a patch, the scrub maps, frame evaluation and the direct write.
fn drive_patch(
    digest: &mut String,
    node_id: NodeHandle,
    graph: MotionGraph,
    store: &mut AccumulatorStore,
) -> Result<(), Failure> {
    let patch = MotionPatch::new(
        uuid(digest, "patch-id", "88888888-8888-8888-8888-888888888888")?,
        node_id,
        vec![PatchOp::new(String::from("to"), PatchValue::Float(0.75))],
        false,
    );
    note(digest, "patch-id", &patch.patch_id())?;
    note(digest, "patch-node", &patch.node_id())?;
    note(digest, "patch-ops", &patch.ops().len())?;
    note(digest, "patch-preserve", &patch.preserve_velocity())?;
    let op = patch.ops().first().ok_or("drive patch must keep its op")?;
    note(digest, "op-path", &op.path())?;
    note(digest, "op-value-float", &op.value())?;
    note(
        digest,
        "patch-easing-value",
        &PatchValue::Easing(Easing::named(NamedCurve::SineInOut)),
    )?;
    note(digest, "patch-bool-value", &PatchValue::Bool(true))?;
    let patched = must(
        digest,
        "patched",
        apply_patch(graph, &patch, &mut AccumulatorStore::new()),
    )?;
    note(digest, "patched-version", &patched.version())?;
    let mut scrubs = HashScrubMap::new();
    scrubs.insert(
        String::from("drive.source"),
        Box::new(ConstantScrub::new(0.5)),
    );
    let scrubbed: &dyn ScrubSourceMap = &scrubs;
    let found: Option<&dyn ScrubSource> = scrubbed.get("drive.source");
    note(digest, "scrub-hit", &found.is_some())?;
    if let Some(source) = found {
        note(digest, "scrub-value", &source.value())?;
    }
    let empty = EmptyScrubMap::new();
    let empty_map: &dyn ScrubSourceMap = &empty;
    note(digest, "scrub-miss", &empty_map.get("nope").is_none())?;
    let evaluated: MotionFrame =
        evaluate_frame(&patched, &Clock::deterministic(50.0), store, scrubbed);
    note(digest, "frame-wall", &evaluated.t_wall_ms())?;
    for binding in evaluated.bindings() {
        note(digest, "binding-node", &binding.node_id())?;
        note(digest, "binding-scalar", &binding.value().as_scalar())?;
    }
    let direct = BindingWrite::new(node_id, BindingValue::Scalar(0.5));
    note(digest, "direct-node", &direct.node_id())?;
    note(digest, "direct-value", &direct.value())?;
    Ok(())
}

/// Drive every interpolation family.
fn drive_interpolation(digest: &mut String) -> Result<(), Failure> {
    note(digest, "lerp", &lerp_scalar(0.0, 1.0, 0.25))?;
    let from2 = Vec2::new(0.0, 0.0);
    let to2 = Vec2::new(1.0, 2.0);
    let mixed2 = lerp_vec2(from2, to2, 0.5);
    note(digest, "vec2", &(mixed2.x(), mixed2.y()))?;
    let from3 = Vec3::new(0.0, 0.0, 0.0);
    let to3 = Vec3::new(1.0, 2.0, 3.0);
    let mixed3 = lerp_vec3(from3, to3, 0.5);
    note(digest, "vec3", &(mixed3.x(), mixed3.y(), mixed3.z()))?;
    let from_inset = Inset::new(0.0, 0.0, 0.0, 0.0);
    let to_inset = Inset::new(8.0, 8.0, 8.0, 8.0);
    let mixed_inset = interp_inset(from_inset, to_inset, 0.5);
    note(
        digest,
        "inset",
        &(
            mixed_inset.top(),
            mixed_inset.right(),
            mixed_inset.bottom(),
            mixed_inset.left(),
        ),
    )?;
    let from_matrix = Transform2D::new(0.0, 0.0, 1.0, 1.0, 0.0);
    let to_matrix = Transform2D::new(10.0, 20.0, 2.0, 2.0, 1.0);
    let mixed_matrix = interp_transform(from_matrix, to_matrix, 0.5);
    note(
        digest,
        "matrix",
        &(
            mixed_matrix.translate_x(),
            mixed_matrix.translate_y(),
            mixed_matrix.scale_x(),
            mixed_matrix.scale_y(),
            mixed_matrix.rotation(),
        ),
    )?;
    let from_path = [Point2::new(0.0, 0.0), Point2::new(1.0, 1.0)];
    let to_path = [Point2::new(2.0, 2.0), Point2::new(3.0, 3.0)];
    let mut out_path = [Point2::new(0.0, 0.0); 2];
    let written = interp_path(&from_path, &to_path, 0.5, &mut out_path);
    note(digest, "path-written", &written)?;
    let [first_point, _] = out_path;
    note(digest, "path-point", &(first_point.x(), first_point.y()))?;
    let from_color = SRgb::new(1.0, 0.0, 0.0);
    let to_color = SRgb::new(0.0, 0.0, 1.0);
    note(digest, "srgb-red", &from_color.red())?;
    note(digest, "srgb-green", &from_color.green())?;
    note(digest, "srgb-blue", &from_color.blue())?;
    note(digest, "srgb-gamut", &from_color.is_in_srgb_gamut())?;
    note(
        digest,
        "srgb-clamped",
        &SRgb::new(2.0, -1.0, 0.5).clamp().red(),
    )?;
    let lab = srgb_to_oklab(from_color);
    note(
        digest,
        "lab",
        &(lab.lightness(), lab.axis_a(), lab.axis_b()),
    )?;
    let lab_built = OkLab::new(0.5, 0.01, -0.01);
    note(
        digest,
        "lab-built",
        &(
            lab_built.lightness(),
            lab_built.axis_a(),
            lab_built.axis_b(),
        ),
    )?;
    note(
        digest,
        "lab-lerped",
        &lerp_oklab(lab, lab_built, 0.5).lightness(),
    )?;
    note(digest, "lab-round-trip", &oklab_to_srgb(lab).red())?;
    note(digest, "lab-mapped", &map_to_srgb_gamut(lab).green())?;
    note(
        digest,
        "lab-interp",
        &interp_oklab(from_color, to_color, 0.5).blue(),
    )?;
    Ok(())
}

/// Run the drive and print the digest.
fn drive(streams: &mut Streams<'_>) -> Result<u8, Failure> {
    let mut digest = String::new();
    drive_motion(&mut digest)?;
    streams.tell(&digest)?;
    Ok(0)
}

/// Driver entry point: exit code, never `process::exit`.
fn main() -> ExitCode {
    drishti::cli::run("api-drive-motion", drive)
}
