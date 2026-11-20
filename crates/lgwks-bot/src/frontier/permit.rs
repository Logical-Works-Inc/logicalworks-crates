//! The two halves of an [`Admission`]: the [`Verdict`] selection reaches without
//! taking a slot, and the [`InFlightPermit`] reservation mints when it does.

use std::sync::Arc;
use std::time::Duration;

#[cfg(doc)]
use super::Frontier;
use super::{Admission, ConstraintKey, DeferralKind, RejectKind};

/// Identity of one frontier's reservation ledger.
///
/// Compared by allocation identity, never by value, so a permit minted by one
/// frontier is refused by another with the same ordinal in its ledger. This is
/// why `Frontier` is not `Clone`: copying the ledger would produce two
/// accountants for one set of reservations, and a permit — which exists only
/// once — would settle one copy while the other held its slots forever.
#[derive(Debug)]
pub(super) struct Issuer;

/// A reserved admission, and the only key that releases it.
///
/// Minted by [`Frontier::admit`], consumed by [`Frontier::complete`], and
/// borrowed by [`Frontier::observe_retry_after`]. It names the host as spelled
/// when the request was admitted, the origin that host belonged to *then*, and
/// the egress — the exact keys whose slots this reservation holds.
///
/// # Why it is not `Clone`, and why `Drop` does not release
///
/// One reservation means one slot set, so one handle releases it. A second
/// handle would either double-decrement or be a lie about which request owns
/// which slot, and the type makes both unrepresentable.
///
/// Dropping a permit leaves its slots held. That is the conservative direction
/// and it is deliberate: the handle going out of scope says the *caller* has
/// stopped looking, not that the request stopped. A frontier that assumed
/// otherwise would over-admit against hosts that are still being contacted, and
/// `docs/bot-on-ecs.md` §8.2's rule — *the intent must be durable before the
/// effect* — is the same asymmetry.
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use lgwks_bot::frontier::{Admission, ConstraintKey, Frontier, Outcome};
/// use lgwks_bot::frontier::{PolitenessPolicy, Resolved, RulesState};
///
/// let mut frontier = Frontier::new(PolitenessPolicy::DEFAULT, "eth0");
/// frontier.observe_resolution("a.example", Resolved::Origin("203.0.113.7".to_owned()));
/// frontier.observe_rules(&ConstraintKey::host("a.example"), RulesState::Allowed);
/// let Admission::Admit(permit) = frontier.admit("a.example", Duration::ZERO) else {
///     return Err("a ready host is admitted".into());
/// };
/// assert_eq!(permit.host(), "a.example");
/// frontier.complete(&permit, Outcome::Success)?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug)]
#[must_use = "a permit dropped without `Frontier::complete` keeps its slots reserved; dropping \
              the handle is not evidence that the request left flight"]
pub struct InFlightPermit {
    /// The ledger this permit settles against.
    pub(super) issuer: Arc<Issuer>,
    /// This reservation's ordinal within the issuing frontier. Unique among
    /// live reservations, which is what makes the ledger check meaningful.
    pub(super) ordinal: u64,
    /// The host, as spelled when the request was admitted.
    pub(super) host: String,
    /// The origin the host belonged to at admission time, if it had one. Not
    /// re-read from the topology when the request completes.
    pub(super) origin: Option<ConstraintKey>,
    /// The egress this frontier admits over.
    pub(super) egress: ConstraintKey,
}

impl InFlightPermit {
    /// The host this request was admitted for, as spelled at admission time.
    ///
    /// # Examples
    ///
    /// ```
    /// # use lgwks_bot::frontier::{Admission, ConstraintKey, Frontier, PolitenessPolicy, Resolved, RulesState};
    /// # let mut frontier = Frontier::new(PolitenessPolicy::DEFAULT, "eth0");
    /// # frontier.observe_resolution("a.example", Resolved::Origin("o".to_owned()));
    /// # frontier.observe_rules(&ConstraintKey::host("a.example"), RulesState::Allowed);
    /// let admission = frontier.admit("a.example", std::time::Duration::ZERO);
    /// if let Admission::Admit(permit) = admission {
    ///     assert_eq!(permit.host(), "a.example");
    /// }
    /// ```
    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Whether this reservation holds a slot against `key`.
    ///
    /// The frontier's own tests assert on what was reserved through it rather
    /// than re-deriving it from the topology, which is the mistake the permit
    /// exists to prevent; no caller outside them asks.
    #[cfg(test)]
    #[must_use]
    pub(super) fn holds(&self, key: &ConstraintKey) -> bool {
        self.keys().iter().any(|held| held == key)
    }

    /// Every constraint this reservation holds a slot against.
    ///
    /// The host, the origin it belonged to at admission time, and the egress —
    /// built from the recorded fields, never from the frontier's current maps.
    pub(super) fn keys(&self) -> Vec<ConstraintKey> {
        let mut keys = Vec::with_capacity(3);
        keys.push(ConstraintKey::host(self.host.clone()));
        keys.extend(self.origin.clone());
        keys.push(self.egress.clone());
        keys
    }

    /// The constraints an observation *about the server* is evidence for: the
    /// host and the origin, without the egress.
    ///
    /// The egress is excluded deliberately. It is the operator's own interface,
    /// and one server asking for a pause — or rate-limiting one tenant behind a
    /// shared proxy — is not evidence about the capacity of our network.
    /// Binding the egress here would let a single target halt every unrelated
    /// host in the run, which is the failure this distinction exists to prevent.
    pub(super) fn server_keys(&self) -> Vec<ConstraintKey> {
        let mut keys = Vec::with_capacity(2);
        keys.push(ConstraintKey::host(self.host.clone()));
        keys.extend(self.origin.clone());
        keys
    }
}

/// Two permits are equal when they name the same reservation ordinal.
///
/// The issuer is deliberately not part of equality. It is an allocation
/// identity, and folding it in would make two identical schedules run against
/// two fresh frontiers produce unequal verdicts — which is the determinism
/// [`Frontier`]'s virtual clock exists to provide. Identity is enforced where it
/// belongs instead: in [`Frontier::complete`], which refuses a permit its ledger
/// does not hold.
impl PartialEq for InFlightPermit {
    fn eq(&self, other: &Self) -> bool {
        self.ordinal == other.ordinal
    }
}

impl Eq for InFlightPermit {}

/// The side-effect-free half of [`Admission`].
///
/// The same three arms, with `Admit` carrying nothing because nothing has been
/// reserved yet. Selection cannot mint a permit — it must not take a slot — and
/// reservation must not be able to act on a decision the selector would not
/// have made, so both read this one function and each maps it to what it is
/// allowed to do with it.
#[derive(Debug)]
pub(super) enum Verdict {
    /// Every constraint admits, so the caller may reserve.
    Admit,
    /// Held, with the instant and the binding constraint.
    Defer {
        /// Elapsed virtual time at which the item becomes admissible.
        resume_at: Duration,
        /// Why it was held.
        kind: DeferralKind,
        /// Which constraint bound.
        constraint: ConstraintKey,
    },
    /// Dropped for good.
    Reject {
        /// Why it is dead.
        kind: RejectKind,
    },
}

impl Verdict {
    /// This verdict as a refusal, or `None` when it admitted.
    ///
    /// `Admit` carries no refusal, so it is not representable in the output:
    /// the caller that gets `None` knows it holds a permission and not a
    /// verdict shaped like one.
    pub(super) fn refusal(self) -> Option<Admission> {
        match self {
            Self::Admit => None,
            Self::Defer {
                resume_at,
                kind,
                constraint,
            } => Some(Admission::Defer {
                resume_at,
                kind,
                constraint,
            }),
            Self::Reject { kind } => Some(Admission::Reject { kind }),
        }
    }
}

/// The later of two admissions, breaking ties on the constraint key.
///
/// A free function so the caller can hold a mutable borrow of the state map
/// while accumulating from `Admit`. The order is total over `(instant, kind,
/// key)`, so equally-late deferrals never depend on iteration order.
pub(super) fn later(current: Verdict, candidate: Verdict) -> Verdict {
    let take_candidate = match (deferral_order(&current), deferral_order(&candidate)) {
        (Some(held), Some(offered)) => offered > held,
        (None, Some(_)) => true,
        (Some(_), None) | (None, None) => false,
    };
    if take_candidate { candidate } else { current }
}

/// The sort key for a deferral. `None` for a verdict that is not a deferral,
/// which never reaches [`later`]'s comparison in practice but is modelled
/// rather than papered over with a sentinel key.
fn deferral_order(verdict: &Verdict) -> Option<(Duration, DeferralKind, &ConstraintKey)> {
    match *verdict {
        Verdict::Defer {
            resume_at,
            kind,
            ref constraint,
        } => Some((resume_at, kind, constraint)),
        Verdict::Admit | Verdict::Reject { .. } => None,
    }
}
