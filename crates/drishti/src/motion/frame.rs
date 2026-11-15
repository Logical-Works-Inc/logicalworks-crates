//! The pure frame evaluation function.
//!
//! Absorbed from the kernel motion core. Takes the immutable graph, the
//! clock, the mutable accumulator store, and the scrub map, and returns one
//! frame of binding writes. No globals and no hidden state.
//!
//! Wall-clock inputs arrive as `f64` but tween and spring state is `f32`.
//! The conversion uses the private `narrow_f64_to_f32` helper rather than an
//! `as` cast (which this repository forbids): identical
//! round-to-nearest-even results, with subnormal results flushed to signed
//! zero as documented there.

use lgwks_deps::libm;
use lgwks_std::id::Uuid;

use super::clock::Clock;
use super::graph::MotionGraph;
use super::node::accumulator::{AccumulatorKind, AccumulatorStore};
use super::node::spring::{SpringConfig, integrate_spring};
use super::node::{MotionNode, NodeKind};
use super::scrub::ScrubSourceMap;

/// Opaque handle referencing a host-side rendering target.
///
/// # Examples
///
/// ```
/// use drishti::motion::TargetHandle;
///
/// let handle = TargetHandle::from([7_u8; 16]);
/// assert_eq!(handle.as_bytes(), &[7_u8; 16]);
/// ```
pub type TargetHandle = Uuid;

/// A single interpolated property write produced by the frame evaluator.
///
/// Fields are private: the evaluator builds writes through
/// [`BindingWrite::new`] and dispatch readers go through the accessors.
///
/// # Examples
///
/// ```
/// use drishti::motion::{BindingValue, BindingWrite, TargetHandle};
///
/// let target = TargetHandle::from([1_u8; 16]);
/// let write = BindingWrite::new(target, BindingValue::Scalar(0.5));
/// assert_eq!(write.node_id(), target);
/// assert_eq!(write.value().as_scalar(), Some(0.5));
/// ```
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct BindingWrite {
    /// Target the write belongs to.
    node_id: TargetHandle,
    /// Interpolated value.
    value: BindingValue,
}

impl BindingWrite {
    /// Assemble one binding write. The pair is stored as given; the frame
    /// evaluator is the only producer that guarantees one write per graph node.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{BindingValue, BindingWrite, TargetHandle};
    ///
    /// let write = BindingWrite::new(TargetHandle::from([2_u8; 16]), BindingValue::Scalar(1.0));
    /// assert_eq!(write.value(), &BindingValue::Scalar(1.0));
    /// ```
    #[must_use]
    pub const fn new(node_id: TargetHandle, value: BindingValue) -> Self {
        Self { node_id, value }
    }

    /// Handle of the graph node whose evaluation produced this write; the
    /// host uses it to find the rendering target.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{BindingValue, BindingWrite, TargetHandle};
    ///
    /// let target = TargetHandle::from([3_u8; 16]);
    /// let write = BindingWrite::new(target, BindingValue::Scalar(0.0));
    /// assert_eq!(write.node_id(), target);
    /// ```
    #[must_use]
    pub const fn node_id(&self) -> TargetHandle {
        self.node_id
    }

    /// The value to apply to the target property, borrowed from the write.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{BindingValue, BindingWrite, TargetHandle};
    ///
    /// let write = BindingWrite::new(TargetHandle::from([4_u8; 16]), BindingValue::Scalar(0.25));
    /// assert_eq!(write.value().as_scalar(), Some(0.25));
    /// ```
    #[must_use]
    pub const fn value(&self) -> &BindingValue {
        &self.value
    }
}

/// Discriminated interpolated value type.
///
/// # Examples
///
/// ```
/// use drishti::motion::BindingValue;
///
/// let value = BindingValue::Scalar(0.75);
/// assert_eq!(value, BindingValue::Scalar(0.75));
/// ```
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum BindingValue {
    /// Scalar binding.
    Scalar(f32),
}

impl BindingValue {
    /// Scalar value, if this is one.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::BindingValue;
    ///
    /// assert_eq!(BindingValue::Scalar(0.25).as_scalar(), Some(0.25));
    /// ```
    #[must_use]
    pub const fn as_scalar(&self) -> Option<f32> {
        match *self {
            Self::Scalar(value) => Some(value),
        }
    }
}

/// Output of one [`frame`] call, consumed by the dispatch table.
///
/// Fields are private: the evaluator builds frames and readers go through
/// the accessors.
///
/// # Examples
///
/// ```
/// use drishti::motion::{AccumulatorStore, Clock, EmptyScrubMap, MotionGraph, frame};
///
/// let out = frame(
///     &MotionGraph::default(),
///     &Clock::deterministic(8.0),
///     &mut AccumulatorStore::new(),
///     &EmptyScrubMap::new(),
/// );
/// assert_eq!(out.t_wall_ms(), 8.0);
/// assert!(out.bindings().is_empty());
/// ```
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Frame {
    /// Wall time this frame was evaluated at.
    t_wall_ms: f64,
    /// One write per graph node.
    bindings: Vec<BindingWrite>,
}

impl Frame {
    /// Clock stamp, in milliseconds, that every node of this frame was
    /// evaluated against; copied from the clock handed to [`frame`].
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{AccumulatorStore, Clock, EmptyScrubMap, MotionGraph, frame};
    ///
    /// let out = frame(
    ///     &MotionGraph::default(),
    ///     &Clock::deterministic(33.5),
    ///     &mut AccumulatorStore::new(),
    ///     &EmptyScrubMap::new(),
    /// );
    /// assert_eq!(out.t_wall_ms(), 33.5);
    /// ```
    #[must_use]
    pub const fn t_wall_ms(&self) -> f64 {
        self.t_wall_ms
    }

    /// One write per graph node, in node-handle order.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use drishti::motion::{
    ///     AccumulatorStore, Clock, Easing, EmptyScrubMap, MotionGraphBuilder, NamedCurve,
    ///     NodeHandle, NodeKind, TweenComponent, frame,
    /// };
    ///
    /// let tween = TweenComponent::new(0.0, 10.0, 0.0, 100.0, Easing::named(NamedCurve::Linear))?;
    /// let graph = MotionGraphBuilder::new()
    ///     .add_node(
    ///         NodeHandle::from([1_u8; 16]),
    ///         String::from("box"),
    ///         String::from("x"),
    ///         NodeKind::Tween(tween),
    ///     )
    ///     .build();
    /// let out = frame(
    ///     &graph,
    ///     &Clock::deterministic(50.0),
    ///     &mut AccumulatorStore::new(),
    ///     &EmptyScrubMap::new(),
    /// );
    /// assert_eq!(out.bindings().len(), 1);
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn bindings(&self) -> &[BindingWrite] {
        &self.bindings
    }
}

/// Pure frame evaluation. Allocates one `Vec` pre-sized to the node count.
///
/// # Examples
///
/// ```
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use drishti::motion::{
///     AccumulatorStore, Clock, Easing, EmptyScrubMap, MotionGraphBuilder, NamedCurve,
///     NodeHandle, NodeKind, TweenComponent, frame,
/// };
///
/// let tween = TweenComponent::new(0.0, 10.0, 0.0, 100.0, Easing::named(NamedCurve::Linear))?;
/// let graph = MotionGraphBuilder::new()
///     .add_node(
///         NodeHandle::from([1_u8; 16]),
///         String::from("box"),
///         String::from("x"),
///         NodeKind::Tween(tween),
///     )
///     .build();
/// let mut store = AccumulatorStore::new();
/// let halfway = frame(&graph, &Clock::deterministic(50.0), &mut store, &EmptyScrubMap::new());
/// let value = halfway.bindings().first().and_then(|write| write.value().as_scalar());
/// assert_eq!(value, Some(5.0));
/// # Ok(())
/// # }
/// ```
#[must_use]
pub fn frame(
    graph: &MotionGraph,
    clock: &Clock,
    accumulators: &mut AccumulatorStore,
    scrubs: &dyn ScrubSourceMap,
) -> Frame {
    let t_wall_ms = clock.wall_clock_ms();
    let mut bindings = Vec::with_capacity(graph.node_count());

    // O(n) iteration in handle order — never per-handle lookup inside loop.
    for node in graph.nodes() {
        bindings.push(evaluate_node(node, t_wall_ms, accumulators, scrubs));
    }

    Frame {
        t_wall_ms,
        bindings,
    }
}

/// Evaluate one node to a binding write.
fn evaluate_node(
    node: &MotionNode,
    t_wall_ms: f64,
    accumulators: &mut AccumulatorStore,
    scrubs: &dyn ScrubSourceMap,
) -> BindingWrite {
    let value = match *node.kind() {
        NodeKind::Tween(ref tween) => {
            BindingValue::Scalar(tween.value_at_wall_ms(narrow_f64_to_f32(t_wall_ms)))
        }
        NodeKind::Accumulator(ref accumulator) => {
            evaluate_accumulator(node.node_id(), accumulator, t_wall_ms, accumulators, scrubs)
        }
    };
    BindingWrite::new(node.node_id(), value)
}

/// Evaluate one accumulator node through the store.
fn evaluate_accumulator(
    node_id: Uuid,
    accumulator: &super::node::accumulator::AccumulatorComponent,
    t_wall_ms: f64,
    accumulators: &mut AccumulatorStore,
    scrubs: &dyn ScrubSourceMap,
) -> BindingValue {
    match accumulator.kind() {
        AccumulatorKind::Spring => evaluate_spring(node_id, accumulator, t_wall_ms, accumulators),
        AccumulatorKind::Scrub => evaluate_scrub(node_id, accumulator, accumulators, scrubs),
        AccumulatorKind::Drag => evaluate_drag(node_id, accumulators),
    }
}

/// Assumed delta for the first frame, when no previous wall time exists to
/// difference against — one 60 Hz frame. Not zero: a zero dt makes the first
/// integration step a no-op, so a spring visibly stalls for one frame.
const FIRST_FRAME_DT_SECONDS: f64 = 0.0166;

/// Frame delta above which spring integration explodes. Beyond this the sim
/// teleports to target rather than integrating.
const SPRING_EXPLODE_DT_SECONDS: f64 = 0.250;

/// Ceiling on the delta handed to the integrator, preventing divergence.
/// Distinct from [`SPRING_EXPLODE_DT_SECONDS`]: deltas between the two are
/// CLAMPED and still integrated; only deltas above the explode bound skip
/// integration entirely.
const MAX_INTEGRATION_DT_SECONDS: f64 = 0.064;

/// Springs always animate toward normalized 1.0; the binding's own range maps it.
const SPRING_TARGET: f32 = 1.0;

/// Most RK4 substeps one frame may spend on a stiff spring. Bounds the work
/// per node per frame.
const MAX_SPRING_SUBSTEPS: u16 = 64;

/// Largest `step * rate` (rate is the larger of `sqrt(tension / mass)` and
/// `friction / mass`) one RK4 step may take. RK4 diverges near 2.78 on the
/// damping axis, so one stays well under it. A spring at or under this bound
/// at its frame delta takes exactly one step, as before.
const MAX_STEP_RATE_PRODUCT: f32 = 1.0;

/// Substeps that keep every RK4 step inside the stability bound, between one
/// and [`MAX_SPRING_SUBSTEPS`].
fn substep_count(config: &SpringConfig, dt_seconds: f32) -> u16 {
    let stiffness = libm::sqrtf(config.tension() / config.mass());
    let damping = config.friction() / config.mass();
    let demand = dt_seconds * stiffness.max(damping) / MAX_STEP_RATE_PRODUCT;
    let mut steps: u16 = 1;
    while steps < MAX_SPRING_SUBSTEPS && f32::from(steps) < demand {
        steps = steps.saturating_add(1);
    }
    steps
}

/// Wall-clock delta in seconds for this frame, or [`FIRST_FRAME_DT_SECONDS`]
/// when this is the accumulator's first frame.
fn frame_dt_seconds(last_t_wall_ms: Option<f64>, t_wall_ms: f64) -> f64 {
    match last_t_wall_ms {
        Some(last) => (t_wall_ms - last) / 1000.0,
        None => FIRST_FRAME_DT_SECONDS,
    }
}

/// Integrate one spring node. A rewound or non-finite clock yields a no-op
/// step rather than garbage integration: the kernel left negative deltas to
/// the integrator, which diverges; failing safe here is deliberate.
fn evaluate_spring(
    node_id: Uuid,
    accumulator: &super::node::accumulator::AccumulatorComponent,
    t_wall_ms: f64,
    accumulators: &mut AccumulatorStore,
) -> BindingValue {
    let state = accumulators.get_or_default(node_id);
    let Some(config) = accumulator.spring_config() else {
        return BindingValue::Scalar(state.position());
    };

    // A NaN or infinite clock carries no time. Hold the state and do not
    // record the stamp: remembering it would make every later delta NaN or
    // infinite and poison the spring for good.
    if !t_wall_ms.is_finite() {
        return BindingValue::Scalar(state.position());
    }

    let raw_dt = frame_dt_seconds(state.last_t_wall_ms(), t_wall_ms);

    // Physics stability gate: a delta this large would explode the spring,
    // so teleport to target to maintain structural integrity.
    if raw_dt > SPRING_EXPLODE_DT_SECONDS {
        state.set_position(SPRING_TARGET);
        state.set_velocity(0.0);
        state.set_last_t_wall_ms(t_wall_ms);
        return BindingValue::Scalar(SPRING_TARGET);
    }

    let dt_seconds = narrow_f64_to_f32(raw_dt.clamp(0.0, MAX_INTEGRATION_DT_SECONDS));
    let steps = substep_count(config, dt_seconds);
    let step_seconds = dt_seconds / f32::from(steps);
    let (mut new_position, mut new_velocity) = (state.position(), state.velocity());
    for _ in 0..steps {
        (new_position, new_velocity) = integrate_spring(
            new_position,
            new_velocity,
            SPRING_TARGET,
            config,
            step_seconds,
        );
    }
    // A configuration so stiff that even the capped substeps diverge is
    // treated like an exploded delta: land on the target, never write NaN.
    if !new_position.is_finite() || !new_velocity.is_finite() {
        state.set_position(SPRING_TARGET);
        state.set_velocity(0.0);
        state.set_last_t_wall_ms(t_wall_ms);
        return BindingValue::Scalar(SPRING_TARGET);
    }
    state.set_position(new_position);
    state.set_velocity(new_velocity);
    state.set_last_t_wall_ms(t_wall_ms);
    BindingValue::Scalar(new_position)
}

/// Sample one scrub node from its external source, clamped to [0, 1].
fn evaluate_scrub(
    node_id: Uuid,
    accumulator: &super::node::accumulator::AccumulatorComponent,
    accumulators: &mut AccumulatorStore,
    scrubs: &dyn ScrubSourceMap,
) -> BindingValue {
    // Unwired scrub sources read as zero; the clamp below keeps the
    // binding in range.
    let scrub_value = match accumulator.scrub_source_id().and_then(|id| scrubs.get(id)) {
        // A NaN reading is no reading: `clamp` would pass it through and put
        // NaN on the binding, so it reads as zero like an unwired source.
        Some(source) if source.value().is_nan() => 0.0,
        Some(source) => source.value(),
        None => 0.0,
    };
    let state = accumulators.get_or_default(node_id);
    state.set_last_scrub_value(scrub_value);
    state.set_position(scrub_value.clamp(0.0, 1.0));
    BindingValue::Scalar(state.position())
}

/// Read one drag node's held position.
fn evaluate_drag(node_id: Uuid, accumulators: &mut AccumulatorStore) -> BindingValue {
    let state = accumulators.get_or_default(node_id);
    BindingValue::Scalar(state.position())
}

/// Round `f64` to `f32` with round-to-nearest-even.
///
/// The repository forbids `as` casts, so the conversion is spelled out with
/// integer operations on the bit pattern. Semantics match `as` exactly,
/// except that subnormal results flush to signed zero (documented below):
/// - NaN maps to NaN, infinities keep their sign;
/// - finite values beyond `f32::MAX` in magnitude map to signed infinity;
/// - finite values below the smallest normal `f32` in magnitude map to
///   signed zero rather than a subnormal (flush-to-zero; matches NEON FTZ
///   behavior and keeps every downstream comparison exact);
/// - otherwise the 53-bit mantissa rounds to 24 bits, ties to even.
fn narrow_f64_to_f32(value: f64) -> f32 {
    let bits = value.to_bits();
    let sign = bits & 0x8000_0000_0000_0000;
    // The sign bit moves from f64 position 63 to f32 position 31, exactly.
    let sign32 = if sign == 0 { 0 } else { 0x8000_0000 };
    let biased = (bits >> 52) & 0x7ff;

    // NaN and infinity keep their kind and sign.
    if biased == 0x7ff {
        if (bits & 0x000f_ffff_ffff_ffff) == 0 {
            return f32::from_bits(sign32 | 0x7f80_0000);
        }
        return f32::NAN;
    }
    // Zero (either sign) stays zero, preserving the sign bit.
    if (bits & 0x7fff_ffff_ffff_ffff) == 0 {
        return f32::from_bits(sign32);
    }
    // Overflow past f32::MAX, and any f64 subnormal, flush per policy above.
    // A normal f32 unbiased exponent spans -126..=127, i.e. f64 biased
    // 897..=1150 (1023 - 126 and 1023 + 127).
    if !(897..=1150).contains(&biased) {
        if biased > 1150 {
            return f32::from_bits(sign32 | 0x7f80_0000);
        }
        return f32::from_bits(sign32);
    }

    // Normal path: re-bias the exponent and round the mantissa to 24 bits.
    let exponent32 = biased.wrapping_sub(1023).wrapping_add(127);
    let mantissa = bits & 0x000f_ffff_ffff_ffff;
    let full = (1_u64 << 52) | mantissa;
    let kept = full >> 29;
    let round_bit = (full >> 28) & 1;
    let sticky = full & 0x0fff_ffff;
    let round_up = round_bit == 1 && (sticky != 0 || (kept & 1) == 1);
    let mut rounded = if round_up { kept.wrapping_add(1) } else { kept };
    let mut exponent = exponent32;
    if rounded >= 1_u64 << 24 {
        rounded >>= 1;
        exponent = exponent.wrapping_add(1);
    }
    // Both narrowings are provably exact: `rounded` stays below 2^24 and the
    // shifted exponent below 2^32, so the low bytes carry the whole value.
    let rounded_bytes = rounded.to_le_bytes();
    let rounded32 = u32::from_le_bytes([
        rounded_bytes[0],
        rounded_bytes[1],
        rounded_bytes[2],
        rounded_bytes[3],
    ]);
    let shifted_bytes = exponent.wrapping_shl(23).to_le_bytes();
    let exponent32 = u32::from_le_bytes([
        shifted_bytes[0],
        shifted_bytes[1],
        shifted_bytes[2],
        shifted_bytes[3],
    ]);
    f32::from_bits(sign32 | exponent32 | (rounded32 & 0x007f_ffff))
}

#[cfg(test)]
mod tests {
    use super::narrow_f64_to_f32;

    /// Assert exact bit equality between narrowed and expected `f32`.
    fn assert_narrowed(input: f64, expected: f32) -> Result<(), String> {
        let got = narrow_f64_to_f32(input);
        if got.to_bits() != expected.to_bits() {
            return Err(format!(
                "narrow({input}) = {got} ({:08x}), want {expected} ({:08x})",
                got.to_bits(),
                expected.to_bits()
            ));
        }
        Ok(())
    }

    /// Every normal `f32` survives a widen-narrow round trip bit-identically.
    #[test]
    fn narrow_round_trips_every_normal_f32() -> Result<(), String> {
        for value in [
            0.0_f32,
            -0.0_f32,
            1.0,
            -1.0,
            0.0166,
            300_000.0,
            123.456,
            -0.5,
            f32::MIN_POSITIVE,
            f32::MAX,
            1.0e-30,
            2.0,
            100.0,
        ] {
            assert_narrowed(f64::from(value), value)?;
        }
        Ok(())
    }

    /// Subnormal `f32` inputs flush to signed zero by documented policy.
    #[test]
    fn narrow_flushes_subnormal_results_to_signed_zero() -> Result<(), String> {
        assert_narrowed(f64::from(1.0e-40_f32), 0.0)?;
        assert_narrowed(f64::from(f32::MIN_POSITIVE) / 2.0, 0.0)?;
        let negative_tiny = -f64::from(1.0e-40_f32);
        let got = narrow_f64_to_f32(negative_tiny);
        if got.to_bits() != (-0.0_f32).to_bits() {
            return Err(String::from("negative tiny input must flush to -0.0"));
        }
        Ok(())
    }

    /// NaN, infinities, and overflow follow the `as` policy.
    #[test]
    fn narrow_handles_non_finite_and_overflow() -> Result<(), String> {
        if !narrow_f64_to_f32(f64::NAN).is_nan() {
            return Err(String::from("NaN must stay NaN"));
        }
        assert_narrowed(f64::INFINITY, f32::INFINITY)?;
        assert_narrowed(f64::NEG_INFINITY, f32::NEG_INFINITY)?;
        assert_narrowed(1.0e300, f32::INFINITY)?;
        assert_narrowed(-1.0e300, f32::NEG_INFINITY)?;
        Ok(())
    }

    /// Exact halfway values round to even, proving ties-to-even.
    #[test]
    fn narrow_rounds_ties_to_even() -> Result<(), String> {
        // Halfway between 1.0 and the next f32 above: mantissa is even, stays.
        let halfway_up = 1.0_f64 + 5.960464477539063e-8_f64;
        assert_narrowed(halfway_up, 1.0)?;
        // Halfway between 0x3F800001 and 0x3F800002: rounds up to even.
        let one_ulp = f64::from(f32::from_bits(0x3f80_0001));
        let two_ulp = f64::from(f32::from_bits(0x3f80_0002));
        assert_narrowed((one_ulp + two_ulp) / 2.0, f32::from_bits(0x3f80_0002))?;
        Ok(())
    }
}
