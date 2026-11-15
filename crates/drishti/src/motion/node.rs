//! Motion node entity plus the component discriminant.

pub mod accumulator;
pub mod critical;
pub mod spring;
pub mod tween;

use lgwks_std::id::Uuid;

pub use accumulator::{AccumulatorComponent, AccumulatorKind, AccumulatorState, AccumulatorStore};
pub use tween::TweenComponent;

/// Stable entity identity for a node within a [`super::graph::MotionGraph`].
///
/// # Examples
///
/// ```
/// use drishti::motion::NodeHandle;
///
/// let handle = NodeHandle::from([5_u8; 16]);
/// assert_eq!(handle.as_bytes(), &[5_u8; 16]);
/// ```
pub type NodeHandle = Uuid;

/// Target binding: what the node's output is written to.
///
/// # Examples
///
/// ```
/// use drishti::motion::BindingTarget;
///
/// let target = BindingTarget::new(String::from("#card"), String::from("opacity"));
/// assert_eq!((target.handle(), target.property()), ("#card", "opacity"));
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct BindingTarget {
    /// CSS selector or platform-specific handle.
    handle: String,
    /// Property key (for example `translateY` or `opacity`).
    property: String,
}

impl BindingTarget {
    /// Bind a target handle to a property key. Neither string is validated;
    /// the host that receives the binding decides what they mean.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::BindingTarget;
    ///
    /// let target = BindingTarget::new(String::from("#card"), String::from("translateY"));
    /// assert_eq!(target.property(), "translateY");
    /// ```
    #[must_use]
    pub fn new(handle: String, property: String) -> Self {
        Self { handle, property }
    }

    /// Host-side name of the object being animated, such as a CSS selector.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::BindingTarget;
    ///
    /// let target = BindingTarget::new(String::from("#card"), String::from("opacity"));
    /// assert_eq!(target.handle(), "#card");
    /// ```
    #[must_use]
    pub fn handle(&self) -> &str {
        &self.handle
    }

    /// Name of the property on the target that receives the node's output.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::BindingTarget;
    ///
    /// let target = BindingTarget::new(String::from("#card"), String::from("opacity"));
    /// assert_eq!(target.property(), "opacity");
    /// ```
    #[must_use]
    pub fn property(&self) -> &str {
        &self.property
    }
}

/// Discriminant: a node holds exactly one component kind.
///
/// # Examples
///
/// ```
/// use drishti::motion::{AccumulatorComponent, NodeKind};
///
/// let kind = NodeKind::Accumulator(AccumulatorComponent::new_drag());
/// assert!(matches!(kind, NodeKind::Accumulator(_)));
/// ```
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum NodeKind {
    /// Pure value-at-time tween.
    Tween(TweenComponent),
    /// Stateful spring, drag, or scrub accumulator.
    Accumulator(AccumulatorComponent),
}

/// Entity with stable UUID identity.
///
/// # Examples
///
/// ```
/// use drishti::motion::{AccumulatorComponent, BindingTarget, MotionNode, NodeHandle, NodeKind};
///
/// let id = NodeHandle::from([1_u8; 16]);
/// let target = BindingTarget::new(String::from("#card"), String::from("x"));
/// let node = MotionNode::new(id, target, NodeKind::Accumulator(AccumulatorComponent::new_drag()));
/// assert_eq!(node.node_id(), id);
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct MotionNode {
    /// Stable identity.
    node_id: NodeHandle,
    /// Output binding.
    target: BindingTarget,
    /// Component data.
    kind: NodeKind,
}

impl MotionNode {
    /// Assemble a node from identity, target, and component.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{AccumulatorComponent, BindingTarget, MotionNode, NodeHandle, NodeKind};
    ///
    /// let target = BindingTarget::new(String::from("#card"), String::from("x"));
    /// let kind = NodeKind::Accumulator(AccumulatorComponent::new_drag());
    /// let node = MotionNode::new(NodeHandle::from([1_u8; 16]), target.clone(), kind.clone());
    /// assert_eq!(node.target(), &target);
    /// assert_eq!(node.kind(), &kind);
    /// ```
    #[must_use]
    pub fn new(node_id: NodeHandle, target: BindingTarget, kind: NodeKind) -> Self {
        Self {
            node_id,
            target,
            kind,
        }
    }

    /// Identity that keys the node inside a graph; a patch addresses the node
    /// by it.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{AccumulatorComponent, BindingTarget, MotionNode, NodeHandle, NodeKind};
    ///
    /// let id = NodeHandle::from([2_u8; 16]);
    /// let target = BindingTarget::new(String::from("#card"), String::from("x"));
    /// let node = MotionNode::new(id, target, NodeKind::Accumulator(AccumulatorComponent::new_drag()));
    /// assert_eq!(node.node_id(), id);
    /// ```
    #[must_use]
    pub const fn node_id(&self) -> NodeHandle {
        self.node_id
    }

    /// Where the node's evaluated value is written, borrowed from the node.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{AccumulatorComponent, BindingTarget, MotionNode, NodeHandle, NodeKind};
    ///
    /// let target = BindingTarget::new(String::from("#card"), String::from("x"));
    /// let kind = NodeKind::Accumulator(AccumulatorComponent::new_drag());
    /// let node = MotionNode::new(NodeHandle::from([3_u8; 16]), target, kind);
    /// assert_eq!(node.target().property(), "x");
    /// ```
    #[must_use]
    pub const fn target(&self) -> &BindingTarget {
        &self.target
    }

    /// The component that computes the node's value.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{AccumulatorComponent, BindingTarget, MotionNode, NodeHandle, NodeKind};
    ///
    /// let target = BindingTarget::new(String::from("#card"), String::from("x"));
    /// let kind = NodeKind::Accumulator(AccumulatorComponent::new_drag());
    /// let node = MotionNode::new(NodeHandle::from([4_u8; 16]), target, kind.clone());
    /// assert_eq!(node.kind(), &kind);
    /// ```
    #[must_use]
    pub const fn kind(&self) -> &NodeKind {
        &self.kind
    }

    /// Replace the component kind (used by `apply_patch` for tween updates).
    /// The identity and target stay; only the component is swapped.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{AccumulatorComponent, BindingTarget, MotionNode, NodeHandle, NodeKind};
    ///
    /// let id = NodeHandle::from([5_u8; 16]);
    /// let target = BindingTarget::new(String::from("#card"), String::from("x"));
    /// let mut node = MotionNode::new(id, target, NodeKind::Accumulator(AccumulatorComponent::new_drag()));
    /// let swapped = NodeKind::Accumulator(AccumulatorComponent::new_drag().with_preserve_velocity(false));
    /// node.set_kind(swapped.clone());
    /// assert_eq!(node.kind(), &swapped);
    /// assert_eq!(node.node_id(), id);
    /// ```
    pub fn set_kind(&mut self, kind: NodeKind) {
        self.kind = kind;
    }
}
