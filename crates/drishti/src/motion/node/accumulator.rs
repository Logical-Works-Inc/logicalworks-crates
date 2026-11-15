//! Accumulator component and store.
//!
//! Accumulators opt out of the pure `frame(time)` model because their next
//! state depends on previous state (spring velocity, drag inertia, scrub
//! position). State lives exclusively in [`AccumulatorStore`]; the component
//! is immutable data.
//!
//! The kernel backed the store with `generational-arena` plus `hashbrown`.
//! This port keeps a single `BTreeMap` keyed by node handle instead: the
//! observable contract (lease generations, reap-on-rotate, default-on-first-
//! touch) is identical, iteration order is key order rather than hash order,
//! and two dependencies disappear.

use lgwks_std::id::Uuid;
use std::collections::BTreeMap;

use super::super::error::{MotionError, MotionErrorCode};
use super::spring::SpringConfig;

/// Discriminant for accumulator physics kind. Kind is immutable after
/// construction — patching kind mid-flight causes physical discontinuity.
///
/// # Examples
///
/// ```
/// use drishti::motion::{AccumulatorComponent, AccumulatorKind};
///
/// assert_eq!(AccumulatorComponent::new_drag().kind(), AccumulatorKind::Drag);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AccumulatorKind {
    /// Damped spring toward normalized 1.0.
    Spring,
    /// Held position with no integration.
    Drag,
    /// Externally scrubbed position.
    Scrub,
}

/// Immutable component data. Mutable state lives in [`AccumulatorStore`].
///
/// # Examples
///
/// ```
/// use drishti::motion::{AccumulatorComponent, AccumulatorKind, SpringConfig};
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let spring = AccumulatorComponent::new_spring(SpringConfig::new(100.0, 10.0, 1.0)?);
/// assert_eq!(spring.kind(), AccumulatorKind::Spring);
/// assert!(spring.spring_config().is_some());
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct AccumulatorComponent {
    /// Physics kind.
    kind: AccumulatorKind,
    /// Spring parameters for [`AccumulatorKind::Spring`].
    spring_config: Option<SpringConfig>,
    /// External scrub source id for [`AccumulatorKind::Scrub`].
    scrub_source_id: Option<String>,
    /// When true, patch operations preserve position and velocity.
    preserve_velocity: bool,
}

impl AccumulatorComponent {
    /// Build a spring accumulator from validated spring parameters. It
    /// preserves velocity across patches by default.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use drishti::motion::{AccumulatorComponent, SpringConfig};
    ///
    /// let config = SpringConfig::new(100.0, 10.0, 1.0)?;
    /// let spring = AccumulatorComponent::new_spring(config);
    /// assert_eq!(spring.spring_config(), Some(&config));
    /// assert!(spring.preserve_velocity());
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub const fn new_spring(config: SpringConfig) -> Self {
        Self {
            kind: AccumulatorKind::Spring,
            spring_config: Some(config),
            scrub_source_id: None,
            preserve_velocity: true,
        }
    }

    /// Build a scrub accumulator bound to a named external source. It does not
    /// preserve velocity: a scrub position follows its source, not momentum.
    ///
    /// # Errors
    ///
    /// Returns [`MotionErrorCode::PatchInvalid`]
    /// for an empty source id.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use drishti::motion::{AccumulatorComponent, AccumulatorKind, MotionErrorCode};
    ///
    /// let scrub = AccumulatorComponent::new_scrub(String::from("pointer"))?;
    /// assert_eq!(scrub.kind(), AccumulatorKind::Scrub);
    /// assert_eq!(scrub.scrub_source_id(), Some("pointer"));
    ///
    /// let refused = AccumulatorComponent::new_scrub(String::new());
    /// assert_eq!(
    ///     refused.err().map(|error| error.code()),
    ///     Some(MotionErrorCode::PatchInvalid)
    /// );
    /// # Ok(())
    /// # }
    /// ```
    pub fn new_scrub(source_id: String) -> Result<Self, MotionError> {
        MotionError::require(
            !source_id.is_empty(),
            MotionErrorCode::PatchInvalid,
            "scrub_source_id must not be empty",
        )?;
        Ok(Self {
            kind: AccumulatorKind::Scrub,
            spring_config: None,
            scrub_source_id: Some(source_id),
            preserve_velocity: false,
        })
    }

    /// Build a drag accumulator (held position, no integration). It
    /// preserves velocity across patches by default.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{AccumulatorComponent, AccumulatorKind};
    ///
    /// let drag = AccumulatorComponent::new_drag();
    /// assert_eq!(drag.kind(), AccumulatorKind::Drag);
    /// assert!(drag.spring_config().is_none());
    /// assert!(drag.preserve_velocity());
    /// ```
    #[must_use]
    pub const fn new_drag() -> Self {
        Self {
            kind: AccumulatorKind::Drag,
            spring_config: None,
            scrub_source_id: None,
            preserve_velocity: true,
        }
    }

    /// Which physics this component runs. Fixed at construction.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{AccumulatorComponent, AccumulatorKind};
    ///
    /// assert_eq!(AccumulatorComponent::new_drag().kind(), AccumulatorKind::Drag);
    /// ```
    #[must_use]
    pub const fn kind(&self) -> AccumulatorKind {
        self.kind
    }

    /// Stiffness, damping and mass for the integrator; `None` for drag and
    /// scrub components, which run no physics.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use drishti::motion::{AccumulatorComponent, SpringConfig};
    ///
    /// let config = SpringConfig::new(100.0, 10.0, 1.0)?;
    /// assert_eq!(AccumulatorComponent::new_spring(config).spring_config(), Some(&config));
    /// assert_eq!(AccumulatorComponent::new_drag().spring_config(), None);
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub const fn spring_config(&self) -> Option<&SpringConfig> {
        self.spring_config.as_ref()
    }

    /// Key the frame evaluator uses to look up the external value feeding a
    /// scrub node; `None` for components that are not scrubbed.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use drishti::motion::AccumulatorComponent;
    ///
    /// let scrub = AccumulatorComponent::new_scrub(String::from("scroll"))?;
    /// assert_eq!(scrub.scrub_source_id(), Some("scroll"));
    /// assert_eq!(AccumulatorComponent::new_drag().scrub_source_id(), None);
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn scrub_source_id(&self) -> Option<&str> {
        self.scrub_source_id.as_deref()
    }

    /// Whether patches preserve position and velocity.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::AccumulatorComponent;
    ///
    /// assert!(AccumulatorComponent::new_drag().preserve_velocity());
    /// ```
    #[must_use]
    pub const fn preserve_velocity(&self) -> bool {
        self.preserve_velocity
    }

    /// Override the preserve-velocity flag, keeping every other field.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::AccumulatorComponent;
    ///
    /// let drag = AccumulatorComponent::new_drag().with_preserve_velocity(false);
    /// assert!(!drag.preserve_velocity());
    /// ```
    #[must_use]
    pub fn with_preserve_velocity(mut self, preserve: bool) -> Self {
        self.preserve_velocity = preserve;
        self
    }
}

/// Per-node mutable physics state. Lives outside the graph to keep the graph
/// immutable.
///
/// Fields are private: the store owns identity and generations in this
/// module, and the frame evaluator reads and advances physics through the
/// accessors, so no caller can rebind a state to another node or silently
/// drop a lease.
///
/// # Examples
///
/// ```
/// use drishti::motion::{AccumulatorState, NodeHandle};
///
/// let id = NodeHandle::from([1_u8; 16]);
/// let state = AccumulatorState::zero(id);
/// assert_eq!((state.position(), state.velocity()), (0.0, 0.0));
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct AccumulatorState {
    /// Node this state belongs to.
    node_id: Uuid,
    /// Current position.
    position: f32,
    /// Current velocity.
    velocity: f32,
    /// Last scrub value seen, if any.
    last_scrub_value: Option<f32>,
    /// Last wall time integrated, if any.
    last_t_wall_ms: Option<f64>,
    /// Generation in which this lease was last renewed.
    lease_generation: u32,
}

impl AccumulatorState {
    /// Zeroed state for a node, with a never-leased generation.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{AccumulatorState, NodeHandle};
    ///
    /// let state = AccumulatorState::zero(NodeHandle::from([1_u8; 16]));
    /// assert_eq!(state.lease_generation(), 0);
    /// assert_eq!(state.last_t_wall_ms(), None);
    /// ```
    #[must_use]
    pub const fn zero(node_id: Uuid) -> Self {
        Self {
            node_id,
            position: 0.0,
            velocity: 0.0,
            last_scrub_value: None,
            last_t_wall_ms: None,
            lease_generation: 0,
        }
    }

    /// Node this state belongs to. The store rewrites it to the key it files
    /// the state under, so it always matches the lookup handle.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{AccumulatorState, NodeHandle};
    ///
    /// let id = NodeHandle::from([1_u8; 16]);
    /// assert_eq!(AccumulatorState::zero(id).node_id(), id);
    /// ```
    #[must_use]
    pub const fn node_id(&self) -> Uuid {
        self.node_id
    }

    /// Where the node sits now, in the normalised range its binding maps.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{AccumulatorState, NodeHandle};
    ///
    /// let mut state = AccumulatorState::zero(NodeHandle::from([1_u8; 16]));
    /// assert_eq!(state.position(), 0.0);
    /// state.set_position(0.75);
    /// assert_eq!(state.position(), 0.75);
    /// ```
    #[must_use]
    pub const fn position(&self) -> f32 {
        self.position
    }

    /// Overwrite the stored value outright, with no integration; every other
    /// field of the state is left as it was.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{AccumulatorState, NodeHandle};
    ///
    /// let mut state = AccumulatorState::zero(NodeHandle::from([1_u8; 16]));
    /// state.set_position(0.5);
    /// assert_eq!(state.position(), 0.5);
    /// assert_eq!(state.velocity(), 0.0);
    /// ```
    pub const fn set_position(&mut self, position: f32) {
        self.position = position;
    }

    /// Rate of change of the position, per second.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{AccumulatorState, NodeHandle};
    ///
    /// let mut state = AccumulatorState::zero(NodeHandle::from([1_u8; 16]));
    /// state.set_velocity(2.0);
    /// assert_eq!(state.velocity(), 2.0);
    /// ```
    #[must_use]
    pub const fn velocity(&self) -> f32 {
        self.velocity
    }

    /// Overwrite the stored rate outright, with no integration; every other
    /// field of the state is left as it was.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{AccumulatorState, NodeHandle};
    ///
    /// let mut state = AccumulatorState::zero(NodeHandle::from([1_u8; 16]));
    /// state.set_velocity(-1.5);
    /// assert_eq!(state.velocity(), -1.5);
    /// assert_eq!(state.position(), 0.0);
    /// ```
    pub const fn set_velocity(&mut self, velocity: f32) {
        self.velocity = velocity;
    }

    /// Raw reading the scrub source gave on the previous frame, before
    /// clamping; `None` until a scrub frame has run.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{AccumulatorState, NodeHandle};
    ///
    /// let mut state = AccumulatorState::zero(NodeHandle::from([1_u8; 16]));
    /// assert_eq!(state.last_scrub_value(), None);
    /// state.set_last_scrub_value(1.5);
    /// assert_eq!(state.last_scrub_value(), Some(1.5));
    /// ```
    #[must_use]
    pub const fn last_scrub_value(&self) -> Option<f32> {
        self.last_scrub_value
    }

    /// Remember the raw scrub reading for the next frame.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{AccumulatorState, NodeHandle};
    ///
    /// let mut state = AccumulatorState::zero(NodeHandle::from([1_u8; 16]));
    /// state.set_last_scrub_value(0.25);
    /// assert_eq!(state.last_scrub_value(), Some(0.25));
    /// ```
    pub const fn set_last_scrub_value(&mut self, value: f32) {
        self.last_scrub_value = Some(value);
    }

    /// Wall-clock stamp of the previous integration step, in milliseconds;
    /// `None` on the first frame, when no delta exists yet.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{AccumulatorState, NodeHandle};
    ///
    /// let mut state = AccumulatorState::zero(NodeHandle::from([1_u8; 16]));
    /// assert_eq!(state.last_t_wall_ms(), None);
    /// state.set_last_t_wall_ms(16.0);
    /// assert_eq!(state.last_t_wall_ms(), Some(16.0));
    /// ```
    #[must_use]
    pub const fn last_t_wall_ms(&self) -> Option<f64> {
        self.last_t_wall_ms
    }

    /// Remember the wall-clock stamp so the next frame can take a delta.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{AccumulatorState, NodeHandle};
    ///
    /// let mut state = AccumulatorState::zero(NodeHandle::from([1_u8; 16]));
    /// state.set_last_t_wall_ms(8.0);
    /// assert_eq!(state.last_t_wall_ms(), Some(8.0));
    /// ```
    pub const fn set_last_t_wall_ms(&mut self, wall_ms: f64) {
        self.last_t_wall_ms = Some(wall_ms);
    }

    /// Store generation that last touched this state; 0 means never leased.
    /// A state that falls two generations behind is reaped.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{AccumulatorStore, NodeHandle};
    ///
    /// let mut store = AccumulatorStore::new();
    /// let state = store.get_or_default(NodeHandle::from([1_u8; 16]));
    /// assert_eq!(state.lease_generation(), 1);
    /// ```
    #[must_use]
    pub const fn lease_generation(&self) -> u32 {
        self.lease_generation
    }
}

/// Mutable store keyed by node handle, with liveness-lease generations.
///
/// # Examples
///
/// ```
/// use drishti::motion::{AccumulatorStore, NodeHandle};
///
/// let mut store = AccumulatorStore::new();
/// let id = NodeHandle::from([1_u8; 16]);
/// store.get_or_default(id).set_position(0.5);
/// assert_eq!(store.get(&id).map(|state| state.position()), Some(0.5));
/// ```
pub struct AccumulatorStore {
    /// Live states in key order.
    states: BTreeMap<Uuid, AccumulatorState>,
    /// Current generation. Starts at 1 so 0 means "never leased".
    current_generation: u32,
}

impl AccumulatorStore {
    /// Empty store at generation 1.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{AccumulatorStore, NodeHandle};
    ///
    /// let store = AccumulatorStore::new();
    /// assert!(store.get(&NodeHandle::from([1_u8; 16])).is_none());
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self {
            states: BTreeMap::new(),
            current_generation: 1,
        }
    }

    /// Liveness leases: increment the generation and reap entries that were
    /// not renewed since the previous generation.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{AccumulatorStore, NodeHandle};
    ///
    /// let mut store = AccumulatorStore::new();
    /// let id = NodeHandle::from([1_u8; 16]);
    /// store.get_or_default(id);
    /// // One rotation of grace: a state touched last generation survives.
    /// store.rotate_and_reap();
    /// assert!(store.get(&id).is_some());
    /// // A second rotation with no renewal reaps it.
    /// store.rotate_and_reap();
    /// assert!(store.get(&id).is_none());
    /// ```
    pub fn rotate_and_reap(&mut self) {
        let dead_generation = self.current_generation;
        self.current_generation = self.current_generation.saturating_add(1);
        self.states
            .retain(|_, state| state.lease_generation >= dead_generation);
    }

    /// Read-only access to a node's state, if present. Reading does not renew
    /// the lease.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{AccumulatorStore, NodeHandle};
    ///
    /// let mut store = AccumulatorStore::new();
    /// let id = NodeHandle::from([1_u8; 16]);
    /// assert!(store.get(&id).is_none());
    /// store.get_or_default(id);
    /// assert!(store.get(&id).is_some());
    /// ```
    #[must_use]
    pub fn get(&self, node_id: &Uuid) -> Option<&AccumulatorState> {
        self.states.get(node_id)
    }

    /// Mutable access to a node's state, creating and leasing it on first
    /// touch. Stamps the current generation.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{AccumulatorStore, NodeHandle};
    ///
    /// let mut store = AccumulatorStore::new();
    /// let id = NodeHandle::from([1_u8; 16]);
    /// let state = store.get_or_default(id);
    /// assert_eq!(state.position(), 0.0);
    /// assert_eq!(state.node_id(), id);
    /// state.set_position(0.5);
    /// assert_eq!(store.get_or_default(id).position(), 0.5);
    /// ```
    pub fn get_or_default(&mut self, node_id: Uuid) -> &mut AccumulatorState {
        let generation = self.current_generation;
        let state = self
            .states
            .entry(node_id)
            .or_insert_with(|| AccumulatorState::zero(node_id));
        state.lease_generation = generation;
        state
    }

    /// Store state for a node, stamping the current generation and forcing
    /// the node id to match the key.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{AccumulatorState, AccumulatorStore, NodeHandle};
    ///
    /// let mut store = AccumulatorStore::new();
    /// let id = NodeHandle::from([1_u8; 16]);
    /// let other = NodeHandle::from([2_u8; 16]);
    /// store.set(id, AccumulatorState::zero(other));
    /// let held = store.get(&id);
    /// assert_eq!(held.map(|state| state.node_id()), Some(id));
    /// assert_eq!(held.map(|state| state.lease_generation()), Some(1));
    /// ```
    pub fn set(&mut self, node_id: Uuid, mut state: AccumulatorState) {
        state.lease_generation = self.current_generation;
        state.node_id = node_id;
        self.states.insert(node_id, state);
    }

    /// Drop a node's state, if present. Removing an absent node is a no-op.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{AccumulatorStore, NodeHandle};
    ///
    /// let mut store = AccumulatorStore::new();
    /// let id = NodeHandle::from([1_u8; 16]);
    /// store.get_or_default(id);
    /// store.remove(&id);
    /// assert!(store.get(&id).is_none());
    /// store.remove(&id);
    /// ```
    pub fn remove(&mut self, node_id: &Uuid) {
        self.states.remove(node_id);
    }
}

impl Default for AccumulatorStore {
    /// Empty store at generation 1.
    fn default() -> Self {
        Self::new()
    }
}
