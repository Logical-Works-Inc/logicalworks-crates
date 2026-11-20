//! Atomic graph patches.
//!
//! Absorbed from the kernel motion core. A patch either fully applies (all
//! paths valid) or fully rejects (graph unchanged). The version increments on
//! success. Immutable fields (kind, node id) are rejected.

use lgwks_std::id::Uuid;

use super::easing::Easing;
use super::error::{MotionError, MotionErrorCode};
use super::graph::MotionGraph;
use super::node::accumulator::AccumulatorStore;
use super::node::tween::TweenComponent;
use super::node::{MotionNode, NodeKind};

/// A single field mutation within a patch.
///
/// Fields are private: construction runs through [`PatchOp::new`] and reads
/// go through the accessors.
///
/// # Examples
///
/// ```
/// use drishti::motion::{PatchOp, PatchValue};
///
/// let op = PatchOp::new(String::from("to"), PatchValue::Float(0.75));
/// assert_eq!(op.path(), "to");
/// assert_eq!(op.value(), &PatchValue::Float(0.75));
/// ```
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct PatchOp {
    /// Dotted field path (for example `duration_ms`, `easing`, `to`).
    path: String,
    /// Replacement value.
    value: PatchValue,
}

impl PatchOp {
    /// Assemble one field mutation. The path is not checked here; the patch
    /// engine refuses unknown or immutable paths when the patch is applied.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{PatchOp, PatchValue};
    ///
    /// let op = PatchOp::new(String::from("duration_ms"), PatchValue::Float(250.0));
    /// assert_eq!(op.value(), &PatchValue::Float(250.0));
    /// ```
    #[must_use]
    pub fn new(path: String, value: PatchValue) -> Self {
        Self { path, value }
    }

    /// Name of the tween field this operation rewrites, such as `to` or
    /// `easing`.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{PatchOp, PatchValue};
    ///
    /// let op = PatchOp::new(String::from("start_ms"), PatchValue::Float(0.0));
    /// assert_eq!(op.path(), "start_ms");
    /// ```
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The value the field is rewritten to; its variant must match the field's
    /// type or the patch is refused.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{PatchOp, PatchValue};
    ///
    /// let op = PatchOp::new(String::from("to"), PatchValue::Bool(true));
    /// assert_eq!(op.value(), &PatchValue::Bool(true));
    /// ```
    #[must_use]
    pub const fn value(&self) -> &PatchValue {
        &self.value
    }
}

/// Typed patch value variants.
///
/// # Examples
///
/// ```
/// use drishti::motion::{Easing, NamedCurve, PatchValue};
///
/// let value = PatchValue::Easing(Easing::named(NamedCurve::SineInOut));
/// assert_ne!(value, PatchValue::Float(1.0));
/// ```
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum PatchValue {
    /// Floating-point field replacement.
    Float(f32),
    /// Easing replacement.
    Easing(Easing),
    /// Boolean field replacement.
    Bool(bool),
}

/// Patch envelope targeting one node within a graph.
///
/// Fields are private: construction runs through [`MotionPatch::new`] and
/// reads go through the accessors, so a patch cannot be rebound to another
/// node after validation.
///
/// # Examples
///
/// ```
/// use drishti::motion::{MotionPatch, NodeHandle, PatchOp, PatchValue};
///
/// let patch = MotionPatch::new(
///     NodeHandle::from([2_u8; 16]),
///     NodeHandle::from([1_u8; 16]),
///     vec![PatchOp::new(String::from("to"), PatchValue::Float(0.75))],
///     false,
/// );
/// assert_eq!(patch.ops().len(), 1);
/// ```
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct MotionPatch {
    /// Patch identity.
    patch_id: Uuid,
    /// Target node identity.
    node_id: Uuid,
    /// Field mutations, applied in order.
    ops: Vec<PatchOp>,
    /// When true, preserve position/velocity on accumulator nodes.
    preserve_velocity: bool,
}

impl MotionPatch {
    /// Assemble a patch envelope. Nothing is validated until
    /// [`apply_patch`] runs it against a graph.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{MotionPatch, NodeHandle};
    ///
    /// let patch = MotionPatch::new(
    ///     NodeHandle::from([2_u8; 16]),
    ///     NodeHandle::from([1_u8; 16]),
    ///     Vec::new(),
    ///     true,
    /// );
    /// assert!(patch.preserve_velocity());
    /// ```
    #[must_use]
    pub fn new(patch_id: Uuid, node_id: Uuid, ops: Vec<PatchOp>, preserve_velocity: bool) -> Self {
        Self {
            patch_id,
            node_id,
            ops,
            preserve_velocity,
        }
    }

    /// Caller-chosen identity of this patch, kept for receipts and replay.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{MotionPatch, NodeHandle};
    ///
    /// let id = NodeHandle::from([2_u8; 16]);
    /// let patch = MotionPatch::new(id, NodeHandle::from([1_u8; 16]), Vec::new(), false);
    /// assert_eq!(patch.patch_id(), id);
    /// ```
    #[must_use]
    pub const fn patch_id(&self) -> Uuid {
        self.patch_id
    }

    /// Node the patch rewrites; fixed at construction and checked against the
    /// graph when the patch is applied.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{MotionPatch, NodeHandle};
    ///
    /// let target = NodeHandle::from([1_u8; 16]);
    /// let patch = MotionPatch::new(NodeHandle::from([2_u8; 16]), target, Vec::new(), false);
    /// assert_eq!(patch.node_id(), target);
    /// ```
    #[must_use]
    pub const fn node_id(&self) -> Uuid {
        self.node_id
    }

    /// Field mutations, applied in order.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{MotionPatch, NodeHandle, PatchOp, PatchValue};
    ///
    /// let ops = vec![PatchOp::new(String::from("to"), PatchValue::Float(2.0))];
    /// let patch = MotionPatch::new(
    ///     NodeHandle::from([2_u8; 16]),
    ///     NodeHandle::from([1_u8; 16]),
    ///     ops.clone(),
    ///     false,
    /// );
    /// assert_eq!(patch.ops(), ops.as_slice());
    /// ```
    #[must_use]
    pub fn ops(&self) -> &[PatchOp] {
        &self.ops
    }

    /// When true, preserve position/velocity on accumulator nodes.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{MotionPatch, NodeHandle};
    ///
    /// let id = NodeHandle::from([1_u8; 16]);
    /// assert!(MotionPatch::new(id, id, Vec::new(), true).preserve_velocity());
    /// assert!(!MotionPatch::new(id, id, Vec::new(), false).preserve_velocity());
    /// ```
    #[must_use]
    pub const fn preserve_velocity(&self) -> bool {
        self.preserve_velocity
    }
}

/// Immutable field paths, rejected by the patch engine.
static IMMUTABLE_PATHS: &[&str] = &["kind", "node_id", "accumulator_kind"];

/// Apply a patch atomically. Returns the new graph on success and the
/// untouched graph's error on any invalid op.
///
/// # Errors
///
/// Returns [`MotionErrorCode::PatchPathImmutable`]
/// for immutable paths, [`MotionErrorCode::NodeNotFound`]
/// for unknown nodes, [`MotionErrorCode::PatchPathUnknown`]
/// for unknown paths, and [`MotionErrorCode::PatchInvalid`]
/// for invalid values or accumulator field patches (always refused).
///
/// # Examples
///
/// ```
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use drishti::motion::{
///     AccumulatorStore, Easing, MotionErrorCode, MotionGraphBuilder, MotionPatch, NamedCurve,
///     NodeHandle, NodeKind, PatchOp, PatchValue, TweenComponent, apply_patch,
/// };
///
/// let id = NodeHandle::from([1_u8; 16]);
/// let tween = TweenComponent::new(0.0, 1.0, 0.0, 100.0, Easing::named(NamedCurve::Linear))?;
/// let graph = MotionGraphBuilder::new()
///     .add_node(id, String::from("box"), String::from("x"), NodeKind::Tween(tween))
///     .build();
/// let patch_id = NodeHandle::from([2_u8; 16]);
///
/// let good = MotionPatch::new(
///     patch_id,
///     id,
///     vec![PatchOp::new(String::from("to"), PatchValue::Float(20.0))],
///     false,
/// );
/// let patched = apply_patch(graph.clone(), &good, &mut AccumulatorStore::new())?;
/// assert_eq!(patched.version(), graph.version() + 1);
/// let to = match patched.get_node(&id).map(|node| node.kind()) {
///     Some(NodeKind::Tween(tween)) => Some(tween.to_value()),
///     _ => None,
/// };
/// assert_eq!(to, Some(20.0));
///
/// // An immutable path is refused and the graph is left alone.
/// let bad = MotionPatch::new(
///     patch_id,
///     id,
///     vec![PatchOp::new(String::from("kind"), PatchValue::Float(1.0))],
///     false,
/// );
/// let refused = apply_patch(graph, &bad, &mut AccumulatorStore::new());
/// assert_eq!(
///     refused.err().map(|error| error.code()),
///     Some(MotionErrorCode::PatchPathImmutable)
/// );
/// # Ok(())
/// # }
/// ```
pub fn apply_patch(
    graph: MotionGraph,
    patch: &MotionPatch,
    accumulators: &mut AccumulatorStore,
) -> Result<MotionGraph, MotionError> {
    // Reject immutable paths before touching any state.
    validate_patch_paths(patch)?;

    let node = graph.get_node(&patch.node_id).ok_or_else(|| {
        MotionError::new(MotionErrorCode::NodeNotFound)
            .with_detail("patch references unknown node_id")
    })?;

    // Validate all ops against the node kind before applying any.
    for op in &patch.ops {
        validate_op_against_node(op, node)?;
    }

    let updated_node = apply_ops_to_node(node.clone(), patch)?;

    // Only an accumulator holds position/velocity to preserve; a tween has no
    // slot, and creating one would leave phantom state for the handle.
    if patch.preserve_velocity && matches!(updated_node.kind(), NodeKind::Accumulator(_)) {
        preserve_accumulator_state(&patch.node_id, accumulators);
    }

    graph.replace_node(updated_node)
}

/// Reject patches that address immutable fields.
fn validate_patch_paths(patch: &MotionPatch) -> Result<(), MotionError> {
    for op in &patch.ops {
        MotionError::require(
            !IMMUTABLE_PATHS.contains(&op.path.as_str()),
            MotionErrorCode::PatchPathImmutable,
            "attempted to patch an immutable field",
        )?;
    }
    Ok(())
}

/// Validate one op against the target node's kind.
fn validate_op_against_node(op: &PatchOp, node: &MotionNode) -> Result<(), MotionError> {
    match *node.kind() {
        NodeKind::Tween(_) => validate_tween_op(op),
        NodeKind::Accumulator(_) => validate_accumulator_op(op),
    }
}

/// Tween paths: from, to, duration_ms, start_ms, easing.
fn validate_tween_op(op: &PatchOp) -> Result<(), MotionError> {
    const VALID_PATHS: &[&str] = &["from", "to", "duration_ms", "start_ms", "easing"];
    MotionError::require(
        VALID_PATHS.contains(&op.path.as_str()),
        MotionErrorCode::PatchPathUnknown,
        "unknown patch path for tween node",
    )
}

/// Accumulator paths: tension, friction, mass, preserve_velocity,
/// scrub_source_id. Validation accepts them; application still rejects them
/// (see [`apply_ops_to_node`]).
fn validate_accumulator_op(op: &PatchOp) -> Result<(), MotionError> {
    const VALID_PATHS: &[&str] = &[
        "tension",
        "friction",
        "mass",
        "preserve_velocity",
        "scrub_source_id",
    ];
    MotionError::require(
        VALID_PATHS.contains(&op.path.as_str()),
        MotionErrorCode::PatchPathUnknown,
        "unknown patch path for accumulator node",
    )
}

/// Apply validated ops to a cloned node.
fn apply_ops_to_node(mut node: MotionNode, patch: &MotionPatch) -> Result<MotionNode, MotionError> {
    match *node.kind() {
        NodeKind::Tween(ref tween) => {
            let updated = apply_tween_ops(tween, &patch.ops)?;
            node.set_kind(NodeKind::Tween(updated));
            Ok(node)
        }
        NodeKind::Accumulator(_) => Err(MotionError::new(MotionErrorCode::PatchInvalid)
            .with_detail("accumulator field patching is refused")),
    }
}

/// A tween's patchable field set, held together while a patch sequence is
/// applied. A struct rather than five parallel `mut` locals: all five are
/// needed by both the patch loop and the rebuild, and five same-typed `f32`
/// locals in scope at once is exactly where a patch writing the wrong field
/// hides — `start_ms` and `duration_ms` are indistinguishable to the compiler.
struct TweenFields {
    /// Start value.
    from: f32,
    /// End value.
    to: f32,
    /// Start time in milliseconds.
    start_ms: f32,
    /// Duration in milliseconds.
    duration_ms: f32,
    /// Easing.
    easing: Easing,
}

impl TweenFields {
    /// Snapshot the component's current fields.
    fn from_component(tween: &TweenComponent) -> Self {
        Self {
            from: tween.from_value(),
            to: tween.to_value(),
            start_ms: tween.start_ms(),
            duration_ms: tween.duration_ms(),
            easing: tween.easing().clone(),
        }
    }

    /// Apply one patch op. An unknown path is an ERROR, never a silent no-op:
    /// a caller that misspells a field would otherwise get success back and a
    /// tween that did not change, indistinguishable from an applied no-effect
    /// patch.
    fn apply(&mut self, op: &PatchOp) -> Result<(), MotionError> {
        let path = op.path.as_str();
        if path == "easing" {
            self.easing = extract_easing(&op.value, path)?;
            return Ok(());
        }
        // Resolve the target slot FIRST, so an unknown path reports
        // PatchPathUnknown before the value is parsed. Order matters: parsing
        // first would answer a misspelled path with a complaint about its
        // value type instead.
        let slot: &mut f32 = match path {
            "from" => Some(&mut self.from),
            "to" => Some(&mut self.to),
            "start_ms" => Some(&mut self.start_ms),
            "duration_ms" => Some(&mut self.duration_ms),
            _ => None,
        }
        .ok_or_else(|| {
            MotionError::new(MotionErrorCode::PatchPathUnknown)
                .with_detail("unexpected path in apply_tween_ops")
        })?;
        *slot = extract_float(&op.value, path)?;
        Ok(())
    }

    /// Rebuild the component through its constructor, re-running validation.
    /// A patch can produce an invalid combination (a non-positive duration),
    /// and only the constructor rejects it.
    fn into_component(self) -> Result<TweenComponent, MotionError> {
        TweenComponent::new(
            self.from,
            self.to,
            self.start_ms,
            self.duration_ms,
            self.easing,
        )
    }
}

/// Apply tween ops in order, then rebuild through the constructor.
fn apply_tween_ops(tween: &TweenComponent, ops: &[PatchOp]) -> Result<TweenComponent, MotionError> {
    let mut fields = TweenFields::from_component(tween);
    for op in ops {
        fields.apply(op)?;
    }
    fields.into_component()
}

/// Extract a float patch value, rejecting mistyped values.
fn extract_float(value: &PatchValue, field: &str) -> Result<f32, MotionError> {
    match *value {
        PatchValue::Float(number) => Ok(number),
        _ => Err(MotionError::new(MotionErrorCode::PatchInvalid).with_detail(field)),
    }
}

/// Extract an easing patch value, rejecting mistyped values.
fn extract_easing(value: &PatchValue, _field: &str) -> Result<Easing, MotionError> {
    match *value {
        PatchValue::Easing(ref easing) => Ok(easing.clone()),
        _ => Err(MotionError::new(MotionErrorCode::PatchInvalid)
            .with_detail("easing patch requires PatchValue::Easing")),
    }
}

/// Ensure an accumulator node's state slot exists and is leased in the
/// current generation, so position/velocity survive the patch. Called for
/// accumulator nodes only.
fn preserve_accumulator_state(node_id: &Uuid, accumulators: &mut AccumulatorStore) {
    let _state = accumulators.get_or_default(*node_id);
}
