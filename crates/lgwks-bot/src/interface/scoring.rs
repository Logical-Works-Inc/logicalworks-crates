//! How each component scores a pair of elements, and which pairs are scored at all.
//! Absent evidence is no confidence, as the parent module explains, except for
//! geometry, which has no unobserved state.

use lgwks_std::similarity::{BoundingBox, Similarity};

use super::{ElementFacts, GeometryComponent, IdentityComponent, PathComponent, TextComponent};

impl Similarity for IdentityComponent {
    type Value = ElementFacts;

    fn score(&self, left: &Self::Value, right: &Self::Value) -> f64 {
        // No identifying attribute on either side is not agreement about an
        // empty set of attributes. `Jaccard` reads it as `1.0`, correctly for
        // every consumer that means set equality, and here it would hand an
        // element with nothing to identify it by the *maximum* identity
        // confidence — half the fingerprint's weight, for a fact neither
        // snapshot observed. So the presence of the evidence is decided before
        // the metric is consulted: absent on either side is no confidence.
        if left.identity.is_empty() || right.identity.is_empty() {
            return 0.0;
        }
        self.metric.score(&left.identity, &right.identity)
    }
}

impl Similarity for PathComponent {
    type Value = ElementFacts;

    fn score(&self, left: &Self::Value, right: &Self::Value) -> f64 {
        // The same rule as identity, for the same reason: two empty paths are
        // not a structural agreement, they are a snapshot that reported no
        // structure. `PathSimilarity` scores the pair `1.0`.
        if left.path.is_empty() || right.path.is_empty() {
            return 0.0;
        }
        self.metric.score(&left.path, &right.path)
    }
}

impl Similarity for TextComponent {
    type Value = ElementFacts;

    fn score(&self, left: &Self::Value, right: &Self::Value) -> f64 {
        // An element with no visible text is ordinary — an icon-only control,
        // a spacer, an image — and two of them do not agree about their text,
        // they both have none. `EditDistance` scores the empty pair `1.0`; the
        // presence rule refuses it. An icon-only control is recognized by its
        // identity and its structure, which is what those facts are for.
        if left.text.is_empty() || right.text.is_empty() {
            return 0.0;
        }
        self.metric.score(&left.text, &right.text)
    }
}

impl Similarity for GeometryComponent {
    type Value = ElementFacts;

    fn score(&self, left: &Self::Value, right: &Self::Value) -> f64 {
        // Geometry is the one component with no absence rule, and the reason is
        // that a bounding box has no unobserved state to confuse with an
        // observed one: every snapshot reports a box for every element it
        // reports at all, and `[0.0, 0.0, 0.0, 0.0]` is the position of an
        // element at the origin rather than a fact nobody looked at. If that
        // ever stops being true the field has to become an `Option`, because a
        // sentinel value is not a presence rule.
        let left_bounds = BoundingBox::from(left.bounds);
        let right_bounds = BoundingBox::from(right.bounds);
        self.metric.score(&left_bounds, &right_bounds)
    }
}

/// Whether a candidate belongs to the target's document and kind.
///
/// This is the single eligibility gate shared by flat recognition and every
/// ladder rung. Similarity may rank eligible candidates, but it cannot grant
/// ownership across a frame boundary or between incompatible elements.
pub(super) fn is_eligible(target: &ElementFacts, candidate: &ElementFacts) -> bool {
    candidate.frames == target.frames && tags_compatible(target, candidate)
}

/// Whether two elements are the same kind of thing.
///
/// The tag is the coarsest structural fact a snapshot reports and the cheapest
/// way for two elements to be obviously different. It is a gate rather than a
/// weighted component for the same reason the frame path is: "both of these are
/// buttons" is not a degree of similarity, and a weight would let a strong
/// enough identifier score outvote a candidate that is a different HTML
/// element entirely.
///
/// An empty tag is an unreported fact rather than a claim of incompatibility,
/// so it excludes nothing. It also cannot help: the tag is not one of the
/// components, so an unknown tag leaves the candidate with exactly the evidence
/// the remaining facts supply. A *role* is not consulted here because a snapshot
/// carries it inside `identity` (`role=button`, an ARIA role attribute) and it
/// is scored there as an identifying attribute, with half the vector's weight
/// behind it.
fn tags_compatible(target: &ElementFacts, candidate: &ElementFacts) -> bool {
    target.tag.is_empty() || candidate.tag.is_empty() || target.tag == candidate.tag
}
