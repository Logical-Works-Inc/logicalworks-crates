//! Scrub sources: externally-driven values sampled once per frame.
//!
//! Trait objects are appropriate here because the set of scrub source
//! implementations is open (host-supplied; it cannot be enumerated at
//! compile time). Contrast with [`crate::motion::Easing`], which is a closed
//! enum with no trait object.

use std::collections::BTreeMap;

/// A value source the frame evaluator queries each frame. Implementations
/// include pointer position, scroll offset, and audio-clock timestamps.
///
/// # Examples
///
/// ```
/// use drishti::motion::ScrubSource;
///
/// #[derive(Debug)]
/// struct Pointer(f32);
///
/// impl ScrubSource for Pointer {
///     fn value(&self) -> f32 {
///         self.0
///     }
/// }
///
/// assert_eq!(Pointer(0.25).value(), 0.25);
/// ```
pub trait ScrubSource: core::fmt::Debug {
    /// Current normalized value in [0, 1] (callers clamp; implementors need
    /// not).
    fn value(&self) -> f32;
}

/// Map from scrub source id to its source.
///
/// # Examples
///
/// ```
/// use drishti::motion::{ConstantScrub, HashScrubMap, ScrubSource, ScrubSourceMap};
///
/// let mut map = HashScrubMap::new();
/// map.insert(String::from("pointer"), Box::new(ConstantScrub::new(0.5)));
/// let hit = map.get("pointer").map(|source| source.value());
/// assert_eq!(hit, Some(0.5));
/// assert!(map.get("scroll").is_none());
/// ```
pub trait ScrubSourceMap: core::fmt::Debug {
    /// Look up a source by id.
    fn get(&self, id: &str) -> Option<&dyn ScrubSource>;
}

/// Empty map used in unit tests: every lookup misses.
///
/// The struct is `non_exhaustive`, so external code builds it through
/// [`EmptyScrubMap::new`] rather than the unit value.
///
/// # Examples
///
/// ```
/// use drishti::motion::{EmptyScrubMap, ScrubSourceMap};
///
/// let map = EmptyScrubMap::new();
/// assert!(map.get("anything").is_none());
/// ```
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct EmptyScrubMap;

impl EmptyScrubMap {
    /// Empty map: every lookup misses.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{EmptyScrubMap, ScrubSourceMap};
    ///
    /// assert!(EmptyScrubMap::new().get("pointer").is_none());
    /// ```
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl ScrubSourceMap for EmptyScrubMap {
    fn get(&self, _key: &str) -> Option<&dyn ScrubSource> {
        None
    }
}

/// Constant scrub source: returns the same value every frame.
///
/// # Examples
///
/// ```
/// use drishti::motion::{ConstantScrub, ScrubSource};
///
/// let source = ConstantScrub::new(0.75);
/// assert_eq!(source.value(), 0.75);
/// assert_eq!(source.value(), 0.75);
/// ```
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct ConstantScrub(f32);

impl ConstantScrub {
    /// Wrap a constant scrub value. The value is not clamped here; the frame
    /// evaluator clamps what it reads.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{ConstantScrub, ScrubSource};
    ///
    /// assert_eq!(ConstantScrub::new(1.5).value(), 1.5);
    /// ```
    #[must_use]
    pub const fn new(value: f32) -> Self {
        Self(value)
    }
}

impl ScrubSource for ConstantScrub {
    fn value(&self) -> f32 {
        self.0
    }
}

/// Map-backed scrub sources for tests and conformance fixtures.
///
/// Named for the kernel type; the backing map is ordered so multi-source
/// lookups iterate deterministically.
///
/// # Examples
///
/// ```
/// use drishti::motion::{ConstantScrub, HashScrubMap, ScrubSource, ScrubSourceMap};
///
/// let mut map = HashScrubMap::new();
/// map.insert(String::from("scroll"), Box::new(ConstantScrub::new(0.2)));
/// assert_eq!(map.get("scroll").map(|source| source.value()), Some(0.2));
/// ```
pub struct HashScrubMap {
    /// Sources in id order.
    sources: BTreeMap<String, Box<dyn ScrubSource>>,
}

impl core::fmt::Debug for HashScrubMap {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.debug_struct("HashScrubMap").finish()
    }
}

impl HashScrubMap {
    /// Empty source map.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{HashScrubMap, ScrubSourceMap};
    ///
    /// assert!(HashScrubMap::new().get("pointer").is_none());
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self {
            sources: BTreeMap::new(),
        }
    }

    /// Register a source under an id. A second insert under the same id
    /// replaces the first.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::{ConstantScrub, HashScrubMap, ScrubSource, ScrubSourceMap};
    ///
    /// let mut map = HashScrubMap::new();
    /// map.insert(String::from("pointer"), Box::new(ConstantScrub::new(0.1)));
    /// map.insert(String::from("pointer"), Box::new(ConstantScrub::new(0.9)));
    /// assert_eq!(map.get("pointer").map(|source| source.value()), Some(0.9));
    /// ```
    pub fn insert(&mut self, id: String, source: Box<dyn ScrubSource>) {
        self.sources.insert(id, source);
    }
}

impl Default for HashScrubMap {
    /// Empty source map.
    fn default() -> Self {
        Self::new()
    }
}

impl ScrubSourceMap for HashScrubMap {
    fn get(&self, id: &str) -> Option<&dyn ScrubSource> {
        self.sources.get(id).map(|boxed| boxed.as_ref())
    }
}
