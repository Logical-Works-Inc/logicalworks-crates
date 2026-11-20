//! Persistent motion graph.
//!
//! Absorbed from the kernel motion core. Structural sharing in the kernel
//! came from `im::HashMap`; this port clones the node map on mutation
//! instead. The observable contract is unchanged: the graph is immutable
//! after construction, every mutation consumes `self` and returns a new graph
//! with `version + 1`, and iteration order is key order, which keeps replay
//! traces canonical.

use std::collections::BTreeMap;

use lgwks_std::id::Uuid;

use super::error::{MotionError, MotionErrorCode};
use super::node::{BindingTarget, MotionNode, NodeHandle, NodeKind};

/// Per-property conflict resolution strategy declared at graph build time.
///
/// # Examples
///
/// ```
/// use drishti::motion::{ConflictPolicy, MotionGraph};
///
/// let graph = MotionGraph::default()
///     .with_conflict_policy(String::from("opacity"), ConflictPolicy::Compose);
/// assert_eq!(graph.conflict_policy("opacity"), Some(&ConflictPolicy::Compose));
/// assert_ne!(ConflictPolicy::Compose, ConflictPolicy::LastWrite);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConflictPolicy {
    /// Last writer wins.
    LastWrite,
    /// Compose concurrent writes.
    Compose,
    /// Reject concurrent writes.
    Error,
}

/// Property key (for example `translateY` or `opacity`).
///
/// # Examples
///
/// ```
/// use drishti::motion::PropertyKey;
///
/// let key: PropertyKey = String::from("translateY");
/// assert_eq!(key, "translateY");
/// ```
pub type PropertyKey = String;

/// Persistent, immutable-after-construction graph of motion nodes.
///
/// # Examples
///
/// ```
/// use drishti::motion::MotionGraph;
///
/// let graph = MotionGraph::default();
/// assert_eq!(graph.version(), 0);
/// assert_eq!(graph.node_count(), 0);
/// ```
#[derive(Clone, Debug)]
pub struct MotionGraph {
    /// Nodes in handle order.
    nodes: BTreeMap<NodeHandle, MotionNode>,
    /// Snapshot identity: increments on every mutation. Private to enforce
    /// the invariant.
    version: u64,
    /// Per-property conflict policies in key order.
    conflict_policies: BTreeMap<PropertyKey, ConflictPolicy>,
}

impl MotionGraph {
    /// Empty graph at version 0. Outside the crate the same graph comes from
    /// [`MotionGraph::default`].
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            nodes: BTreeMap::new(),
            version: 0,
            conflict_policies: BTreeMap::new(),
        }
    }

    /// Snapshot identity: starts at 0 and rises by one on every mutation, so
    /// two graphs with equal versions derived from one parent are the same
    /// snapshot.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{ConflictPolicy, MotionGraph};
    ///
    /// let graph = MotionGraph::default();
    /// assert_eq!(graph.version(), 0);
    /// let graph = graph.with_conflict_policy(String::from("x"), ConflictPolicy::Error);
    /// assert_eq!(graph.version(), 1);
    /// ```
    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }

    /// Iterate all nodes in handle order. Preferred over per-handle lookup
    /// inside loops.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use drishti::motion::{
    ///     Easing, MotionGraphBuilder, NamedCurve, NodeHandle, NodeKind, TweenComponent,
    /// };
    ///
    /// let tween = TweenComponent::new(0.0, 1.0, 0.0, 100.0, Easing::named(NamedCurve::Linear))?;
    /// let graph = MotionGraphBuilder::new()
    ///     .add_node(
    ///         NodeHandle::from([9_u8; 16]),
    ///         String::from("box"),
    ///         String::from("x"),
    ///         NodeKind::Tween(tween.clone()),
    ///     )
    ///     .add_node(
    ///         NodeHandle::from([1_u8; 16]),
    ///         String::from("box"),
    ///         String::from("y"),
    ///         NodeKind::Tween(tween),
    ///     )
    ///     .build();
    /// let order: Vec<NodeHandle> = graph.nodes().map(|node| node.node_id()).collect();
    /// assert_eq!(order, vec![NodeHandle::from([1_u8; 16]), NodeHandle::from([9_u8; 16])]);
    /// # Ok(())
    /// # }
    /// ```
    pub fn nodes(&self) -> impl Iterator<Item = &MotionNode> {
        self.nodes.values()
    }

    /// Number of nodes in the graph. Adding a node whose handle is already
    /// present replaces it, so the count does not rise.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use drishti::motion::{
    ///     Easing, MotionGraphBuilder, NamedCurve, NodeHandle, NodeKind, TweenComponent,
    /// };
    ///
    /// let tween = TweenComponent::new(0.0, 1.0, 0.0, 100.0, Easing::named(NamedCurve::Linear))?;
    /// let id = NodeHandle::from([1_u8; 16]);
    /// let graph = MotionGraphBuilder::new()
    ///     .add_node(id, String::from("box"), String::from("x"), NodeKind::Tween(tween.clone()))
    ///     .add_node(id, String::from("box"), String::from("y"), NodeKind::Tween(tween))
    ///     .build();
    /// assert_eq!(graph.node_count(), 1);
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Look up one node by handle; `None` when the graph holds no such node.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use drishti::motion::{
    ///     Easing, MotionGraphBuilder, NamedCurve, NodeHandle, NodeKind, TweenComponent,
    /// };
    ///
    /// let tween = TweenComponent::new(0.0, 1.0, 0.0, 100.0, Easing::named(NamedCurve::Linear))?;
    /// let id = NodeHandle::from([1_u8; 16]);
    /// let graph = MotionGraphBuilder::new()
    ///     .add_node(id, String::from("box"), String::from("x"), NodeKind::Tween(tween))
    ///     .build();
    /// assert!(graph.get_node(&id).is_some());
    /// assert!(graph.get_node(&NodeHandle::from([2_u8; 16])).is_none());
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn get_node(&self, handle: &NodeHandle) -> Option<&MotionNode> {
        self.nodes.get(handle)
    }

    /// Conflict policy for a property, if one was declared.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{ConflictPolicy, MotionGraph};
    ///
    /// let graph = MotionGraph::default()
    ///     .with_conflict_policy(String::from("opacity"), ConflictPolicy::LastWrite);
    /// assert_eq!(graph.conflict_policy("opacity"), Some(&ConflictPolicy::LastWrite));
    /// assert_eq!(graph.conflict_policy("scale"), None);
    /// ```
    #[must_use]
    pub fn conflict_policy(&self, property: &str) -> Option<&ConflictPolicy> {
        self.conflict_policies.get(property)
    }

    /// Add a node; returns a new graph with `version + 1`. A node with an
    /// existing handle overwrites the earlier one.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use drishti::motion::{
    ///     BindingTarget, Easing, MotionGraph, MotionNode, NamedCurve, NodeHandle, NodeKind,
    ///     TweenComponent,
    /// };
    ///
    /// let tween = TweenComponent::new(0.0, 1.0, 0.0, 100.0, Easing::named(NamedCurve::Linear))?;
    /// let node = MotionNode::new(
    ///     NodeHandle::from([1_u8; 16]),
    ///     BindingTarget::new(String::from("box"), String::from("x")),
    ///     NodeKind::Tween(tween),
    /// );
    /// let graph = MotionGraph::default().add_node(node);
    /// assert_eq!(graph.node_count(), 1);
    /// assert_eq!(graph.version(), 1);
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn add_node(mut self, node: MotionNode) -> Self {
        self.nodes.insert(node.node_id(), node);
        self.version = self.version.saturating_add(1);
        self
    }

    /// Remove a node by handle; returns a new graph with `version + 1`, even
    /// when no node held the handle.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use drishti::motion::{
    ///     Easing, MotionGraphBuilder, NamedCurve, NodeHandle, NodeKind, TweenComponent,
    /// };
    ///
    /// let tween = TweenComponent::new(0.0, 1.0, 0.0, 100.0, Easing::named(NamedCurve::Linear))?;
    /// let id = NodeHandle::from([1_u8; 16]);
    /// let graph = MotionGraphBuilder::new()
    ///     .add_node(id, String::from("box"), String::from("x"), NodeKind::Tween(tween))
    ///     .build();
    /// let emptied = graph.remove_node(&id);
    /// assert_eq!(emptied.node_count(), 0);
    /// assert_eq!(emptied.version(), 2);
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn remove_node(mut self, handle: &NodeHandle) -> Self {
        self.nodes.remove(handle);
        self.version = self.version.saturating_add(1);
        self
    }

    /// Set a conflict policy for a property; returns a new graph with
    /// `version + 1`. A later call for the same property replaces the policy.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{ConflictPolicy, MotionGraph};
    ///
    /// let graph = MotionGraph::default()
    ///     .with_conflict_policy(String::from("x"), ConflictPolicy::LastWrite)
    ///     .with_conflict_policy(String::from("x"), ConflictPolicy::Error);
    /// assert_eq!(graph.conflict_policy("x"), Some(&ConflictPolicy::Error));
    /// assert_eq!(graph.version(), 2);
    /// ```
    #[must_use]
    pub fn with_conflict_policy(mut self, property: PropertyKey, policy: ConflictPolicy) -> Self {
        self.conflict_policies.insert(property, policy);
        self.version = self.version.saturating_add(1);
        self
    }

    /// Replace a node (which must already exist); returns a new graph with
    /// `version + 1`.
    ///
    /// # Errors
    ///
    /// Returns [`MotionErrorCode::NodeNotFound`] when the graph holds no node
    /// with the replacement's handle; nothing is inserted.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use drishti::motion::{
    ///     BindingTarget, Easing, MotionErrorCode, MotionGraph, MotionNode, NamedCurve, NodeHandle,
    ///     NodeKind, TweenComponent,
    /// };
    ///
    /// let tween = TweenComponent::new(0.0, 1.0, 0.0, 100.0, Easing::named(NamedCurve::Linear))?;
    /// let node = MotionNode::new(
    ///     NodeHandle::from([1_u8; 16]),
    ///     BindingTarget::new(String::from("box"), String::from("x")),
    ///     NodeKind::Tween(tween),
    /// );
    /// let refused = MotionGraph::default().replace_node(node.clone());
    /// assert_eq!(
    ///     refused.err().map(|error| error.code()),
    ///     Some(MotionErrorCode::NodeNotFound)
    /// );
    /// let replaced = MotionGraph::default().add_node(node.clone()).replace_node(node)?;
    /// assert_eq!(replaced.version(), 2);
    /// # Ok(())
    /// # }
    /// ```
    pub fn replace_node(mut self, node: MotionNode) -> Result<Self, MotionError> {
        MotionError::require(
            self.nodes.contains_key(&node.node_id()),
            MotionErrorCode::NodeNotFound,
            "replace_node: no node holds this handle",
        )?;
        self.nodes.insert(node.node_id(), node);
        self.version = self.version.saturating_add(1);
        Ok(self)
    }
}

impl Default for MotionGraph {
    /// Empty graph at version 0.
    fn default() -> Self {
        Self::new()
    }
}

/// Builder for constructing a graph from a timeline document.
///
/// # Examples
///
/// ```
/// use drishti::motion::{ConflictPolicy, MotionGraphBuilder};
///
/// let graph = MotionGraphBuilder::new()
///     .with_conflict_policy(String::from("opacity"), ConflictPolicy::Compose)
///     .build();
/// assert_eq!(graph.conflict_policy("opacity"), Some(&ConflictPolicy::Compose));
/// ```
#[derive(Debug)]
pub struct MotionGraphBuilder {
    /// Graph under construction.
    graph: MotionGraph,
}

impl MotionGraphBuilder {
    /// Start building an empty graph.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::MotionGraphBuilder;
    ///
    /// let graph = MotionGraphBuilder::new().build();
    /// assert_eq!(graph.version(), 0);
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self {
            graph: MotionGraph::new(),
        }
    }

    /// Add a node with the given identity, target, and component. Each call
    /// counts as one graph mutation toward the final version.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use drishti::motion::{
    ///     Easing, MotionGraphBuilder, NamedCurve, NodeHandle, NodeKind, TweenComponent,
    /// };
    ///
    /// let tween = TweenComponent::new(0.0, 1.0, 0.0, 100.0, Easing::named(NamedCurve::Linear))?;
    /// let id = NodeHandle::from([1_u8; 16]);
    /// let graph = MotionGraphBuilder::new()
    ///     .add_node(id, String::from("box"), String::from("x"), NodeKind::Tween(tween))
    ///     .build();
    /// assert_eq!(graph.version(), 1);
    /// assert!(graph.get_node(&id).is_some());
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn add_node(
        mut self,
        node_id: Uuid,
        handle: String,
        property: String,
        kind: NodeKind,
    ) -> Self {
        let target = BindingTarget::new(handle, property);
        let node = MotionNode::new(node_id, target, kind);
        self.graph = self.graph.add_node(node);
        self
    }

    /// Choose how competing writes to one property are arbitrated. A second
    /// choice for the same property replaces the first.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{ConflictPolicy, MotionGraphBuilder};
    ///
    /// let graph = MotionGraphBuilder::new()
    ///     .with_conflict_policy(String::from("x"), ConflictPolicy::LastWrite)
    ///     .with_conflict_policy(String::from("x"), ConflictPolicy::Error)
    ///     .build();
    /// assert_eq!(graph.conflict_policy("x"), Some(&ConflictPolicy::Error));
    /// ```
    #[must_use]
    pub fn with_conflict_policy(mut self, property: PropertyKey, policy: ConflictPolicy) -> Self {
        self.graph = self.graph.with_conflict_policy(property, policy);
        self
    }

    /// Finish building and return the graph.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::MotionGraphBuilder;
    ///
    /// let graph = MotionGraphBuilder::new().build();
    /// assert_eq!(graph.node_count(), 0);
    /// ```
    #[must_use]
    pub fn build(self) -> MotionGraph {
        self.graph
    }
}

impl Default for MotionGraphBuilder {
    /// Start building an empty graph.
    fn default() -> Self {
        Self::new()
    }
}
