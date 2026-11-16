//! Type-erased verb wrappers: what a chain stores, and how a value crosses the
//! erasure boundary. Split from `spec.rs` (#990).

use std::any::{Any, TypeId, type_name};

use crate::cap::{Auth, Cap};
use crate::ecs::EcsError;
use crate::gate::GrantSet;
use crate::verb::RefreshReason;

/// What a value's type was, captured where the type was still a type parameter.
///
/// The erasure boundary is crossed in two places — the source is boxed into
/// `Box<dyn ObserveAny>` when the chain is declared, and the value it produces is
/// boxed into `Box<dyn Any>` when it is polled — and nothing in those types
/// survives to prove the two halves still agree. `Witness` is what does.
///
/// [`TypeId`] is the identity: two types are the same type exactly when their
/// ids are equal. The name is carried beside it only so a mismatch can be
/// reported as prose, and is never compared — the name is a hint, not an
/// identity, and shortening it can make two distinct types render alike.
///
/// The id is process-local and is not serializable, so this serves the Rust
/// path only. A durable identity for the same question — which type a chain's
/// source produces, readable by a materializer that has only wire data — is a
/// schema key, and no such key exists: [`DomainRegistry`](crate::registry::DomainRegistry)
/// maps an identifier to a constructor and stops there, so the type a source
/// produces is written down nowhere a document could name. Do not reach for
/// `TypeId` to answer it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Witness {
    /// The type, as an identity. This is the comparison.
    id: TypeId,
    /// The type, as prose. This is not.
    name: &'static str,
}

impl Witness {
    /// The witness for `T`, taken where `T` is still a type parameter.
    pub(crate) fn of<T: 'static>() -> Self {
        Self {
            id: TypeId::of::<T>(),
            name: type_name::<T>(),
        }
    }

    /// Whether these two witnesses name the same type.
    pub(crate) fn agrees_with(self, other: Self) -> bool {
        self.id == other.id
    }

    /// The type's name, for a diagnostic.
    pub(crate) fn name(self) -> &'static str {
        self.name
    }
}

/// A value at the erasure boundary, with the witness of the type it was erased
/// from.
///
/// One allocation, the same `Box<dyn Any>` the value had to become anyway; the
/// witness rides alongside it in the staging vector rather than in a second box.
/// It is not an `Option`: a value that was boxed here was boxed from a concrete
/// type, so the witness always exists.
///
/// The witness travels *with the value* rather than being read off it where it
/// arrives. `dyn Any` exposes `type_id()`, so the actual type was never out of
/// reach; what the carrier adds is the producer's own statement of what it
/// produced, made at the one point where the type was still a type parameter.
/// The rendezvous then compares one claim against another — a producer's against
/// a consumer's — which is the comparison this check exists for. Reading the
/// `type_id` instead would compare ground truth against an expectation, which is
/// a different and weaker thing: it can only say whether the value fits, never
/// whether the two halves were built to agree.
///
/// Two limits, both real, neither addressed here:
///
/// - [`TypeId`] is process-local and is not serializable, so this serves the
///   Rust path only. A materializer holding wire data instead of a `Witness`
///   needs a durable identity for the same question, and this field is where
///   that key goes once one exists — [`DomainRegistry`](crate::registry::DomainRegistry)
///   is where such a key would be declared. Do not reach for `TypeId` to answer
///   it.
/// - A witness is `TypeId::of::<S::Output>()`, so it distinguishes *types*, not
///   *chains*. Two chains that both produce a `u16` are indistinguishable to it:
///   a value produced by one and delivered to the other passes this check. It
///   proves the pairing is type-correct, which is all a type can prove, and that
///   is strictly more than the index pairing proved before it — but it is not a
///   chain identity, and the next reader should not assume it is one.
pub(crate) struct Erased {
    /// The value itself.
    pub(crate) value: Box<dyn Any>,
    /// What type it was before it was erased.
    pub(crate) witness: Witness,
}

impl Erased {
    /// Box `value` and record the type it is being erased from.
    pub(crate) fn new<T: 'static>(value: T) -> Self {
        Self {
            witness: Witness::of::<T>(),
            value: Box::new(value),
        }
    }

    /// The value as an erased reference, for a consumer that only needs `Any`.
    pub(crate) fn as_any(&self) -> &dyn Any {
        self.value.as_ref()
    }
}

/// Object-safe view of [`Observe`](crate::verb::Observe) that erases
/// `Output`. The blanket impl forwards each call to the concrete verb, so the
/// erasure costs one vtable hop and no extra allocation: `poll_any` boxes the
/// domain's own value at the type-erasure boundary, which is where it must be
/// boxed anyway. The caller's [`Auth`] is checked here, before the source is
/// touched, so an erasure bug cannot bypass the gate.
pub(crate) trait ObserveAny {
    /// Forwards to [`Observe::domain_id`](crate::verb::Observe::domain_id).
    fn domain_id(&self) -> &str;
    /// Forwards to [`Observe::required_caps`](crate::verb::Observe::required_caps).
    fn required_caps(&self) -> &[Cap];
    /// Forwards to
    /// [`Observe::cache_state`](crate::verb::Observe::cache_state).
    ///
    /// Read on the same tick as `poll_any`, immediately after it resolves, so a
    /// source that reached a failure inside its own poll has already recorded it
    /// by the time this is asked. That ordering is why the reason is a
    /// *statement* about the source's caching rather than a second call: a
    /// second call could not see a poll that has not finished.
    fn cache_state(&self) -> Option<RefreshReason>;
    /// Forwards to [`Observe::revision`](crate::verb::Observe::revision).
    ///
    /// Read by the substrate before the poll it may skip, on the calling thread
    /// and never beside a pending poll of this source, so the revision it
    /// compares against is the one the substrate committed with the value it
    /// holds. A source that reports none answers `None` and the substrate
    /// compares the value itself, which is the path every shipped source takes.
    fn revision(&self) -> Option<u64>;
    /// Forwards to [`Observe::cadence`](crate::verb::Observe::cadence). Read once, when the world is built.
    #[cfg(feature = "ecs")]
    fn cadence(&self) -> crate::verb::Cadence;
    /// Issue an [`Auth`] for the observer's own caps and poll it.
    ///
    /// Returns `Ok(None)` when the source produced a value **equal** to
    /// `previous`, and `Ok(Some(_))` when it did not. Denies with
    /// [`KernelError::CapabilityDenied`](crate::error::KernelError::CapabilityDenied)
    /// before polling when the grant set does not cover the observer's caps.
    ///
    /// # Why the comparison is here and not at the caller
    ///
    /// This is the last point at which the output is a concrete `T::Output`.
    /// One frame further out it is a `Box<dyn Any>`, and `==` on two boxed
    /// values is an allocation, a memcpy and a free apiece, spent to answer a
    /// question that could be asked of the value in hand. In a steady-state bot
    /// the answer is "equal" on nearly every tick, so the box a caller-side
    /// comparison forces is pure waste: it is built, compared, and dropped.
    ///
    /// The boxed value carries its own [`Witness`], taken here, where the
    /// output type is still `T::Output`. That is the only place it can be
    /// taken: by the time the value reaches the chain that will consume it,
    /// both halves are erased and nothing remains to compare.
    ///
    /// # What `previous` is compared against
    ///
    /// `previous` is the payload the substrate is currently holding for this
    /// chain — the newest committed observation, or failing that the binding of
    /// a transition that owns it. A `None` return therefore means *equal to
    /// what the substrate already has*, which is the same question the chain's
    /// own `same` answers, and it is answered here by the same `PartialEq`.
    fn poll_any<'a>(
        &'a self,
        grants: &'a GrantSet,
        previous: Option<&'a Erased>,
    ) -> crate::verb::BoxFuture<'a, Result<Option<Erased>, EcsError>>;
}

/// The blanket impl carries `PartialEq` on the output because the comparison
/// above needs it. That is not a new requirement on a source: every chain is
/// closed by `EcsObserveBuilder::observe`, which already demands
/// `S::Output: PartialEq` for its change filter, so a source that could reach
/// this impl without it could not have been admitted to a chain anyway.
impl<T: crate::verb::Observe + 'static> ObserveAny for T
where
    T::Output: PartialEq + 'static,
{
    fn domain_id(&self) -> &str {
        crate::verb::Observe::domain_id(self)
    }

    fn required_caps(&self) -> &[Cap] {
        crate::verb::Observe::required_caps(self)
    }

    fn cache_state(&self) -> Option<RefreshReason> {
        crate::verb::Observe::cache_state(self)
    }

    fn revision(&self) -> Option<u64> {
        crate::verb::Observe::revision(self)
    }

    #[cfg(feature = "ecs")]
    fn cadence(&self) -> crate::verb::Cadence {
        crate::verb::Observe::cadence(self)
    }

    fn poll_any<'a>(
        &'a self,
        grants: &'a GrantSet,
        previous: Option<&'a Erased>,
    ) -> crate::verb::BoxFuture<'a, Result<Option<Erased>, EcsError>> {
        Box::pin(async move {
            let auth: Auth = grants.issue(crate::verb::Observe::required_caps(self))?;
            let value = self.poll((auth, ())).await?;
            // A `previous` of the wrong type is not equality and must not be
            // read as it: the downcast failing falls through to boxing the
            // value, which reports the chain as moved. That is the same answer
            // `same_output` gives for a failed downcast, so the two never
            // disagree about which one of them is authoritative.
            if let Some(previous) = previous
                && previous
                    .as_any()
                    .downcast_ref::<T::Output>()
                    .is_some_and(|previous| *previous == value)
            {
                return Ok(None);
            }
            Ok(Some(Erased::new(value)))
        })
    }
}

/// Object-safe view of [`Evaluate`](crate::verb::Evaluate) that erases the
/// evaluated type. `check_any` downcasts to the `T` the closure was registered
/// with; a mismatch is [`EcsError::EvaluateError`], never a false result, so a
/// wiring bug cannot masquerade as a condition that simply did not fire.
pub(crate) trait EvaluateAny {
    /// Downcast `value` to this condition's `T` and evaluate it. Returns
    /// [`EcsError::EvaluateError`] when the observed value is a different type,
    /// which is a chain-wiring bug rather than a domain failure.
    fn check_any(&self, value: &Erased) -> Result<bool, EcsError>;
}

/// Object-safe view of [`Execute`](crate::verb::Execute) that erases both the
/// input and the output. `run_any` issues a fresh [`Auth`] from the grant set
/// for the action's own caps and checks the downcast input before the action
/// runs, so a type mismatch fails without a side effect.
pub(crate) trait ExecuteAny {
    /// Forwards to [`Execute::required_caps`](crate::verb::Execute::required_caps).
    fn required_caps(&self) -> &[Cap];
    /// Forwards to [`Execute::effect_lifetime`](crate::verb::Execute::effect_lifetime).
    fn effect_lifetime(&self) -> crate::verb::EffectLifetime;
    /// Forwards to [`Execute::domain_id`](crate::verb::Execute::domain_id).
    ///
    /// Erased alongside the input and output, and needed here for the same
    /// reason admission needs it: a capability denial names the domain that
    /// declared the requirement, and by the time the action is a
    /// `Box<dyn ExecuteAny>` this is the only way left to ask it which domain
    /// it is.
    fn domain_id(&self) -> &str;
    /// Forwards to [`Execute::archive_input`](crate::verb::Execute::archive_input), downcasting first.
    ///
    /// `None` when the value is not this action's input, exactly as a call
    /// with the wrong type is refused rather than archived as something else:
    /// archiving a value the action would not accept would let an approval
    /// match bytes the action never runs on.
    fn archive_input(&self, _input: &Erased) -> Option<Vec<u8>> {
        None
    }
    /// Issue an [`Auth`] for the action's caps, downcast `input` to the
    /// action's `Input`, run it, and box the output as `Any`. Denies with
    /// [`KernelError::CapabilityDenied`](crate::error::KernelError::CapabilityDenied)
    /// before acting; reports a type mismatch as [`EcsError::TypeMismatch`]
    /// without acting.
    ///
    /// Takes the erased value rather than a bare `&dyn Any` so that every method
    /// on this boundary takes the same carrier. The witness belongs to the value
    /// — see [`Erased`] for why that is the shape this check needs — and the
    /// mismatch arm reports the producer's claim against what this action was
    /// built for, rather than reading a `type_id` off the value and comparing
    /// ground truth against an expectation.
    ///
    /// The mismatch arm is a backstop. The rendezvous in `observe_fold` compares
    /// the value's witness against the chain's before this is ever reached, so a
    /// mismatch here means the world moved behind the schedule's back. It still
    /// names both types when it fires, because a diagnostic that can say only
    /// "not the type this action wanted" leaves the reader to guess what it got.
    fn run_any<'a>(
        &'a self,
        grants: &'a GrantSet,
        input: &'a Erased,
    ) -> crate::verb::BoxFuture<'a, Result<Box<dyn Any>, EcsError>>;

    /// [`ExecuteAny::run_any`] on the clock of the flow that runs the action.
    ///
    /// The default is the plain call: an action that does not wait has nothing
    /// to measure. `TypedExec` overrides it to reach
    /// [`Execute::execute_action_on`](crate::verb::Execute::execute_action_on),
    /// so a flow's `act` reaches a waiting action with the flow's own clock.
    ///
    /// Gated with its only caller, `script::act`: an ECS tick carries no scope
    /// and therefore no clock, and a method nothing there can call is not part
    /// of that build's surface.
    #[cfg(feature = "script")]
    fn run_any_on<'a>(
        &'a self,
        _clock: &'a crate::clock::Clock,
        grants: &'a GrantSet,
        input: &'a Erased,
    ) -> crate::verb::BoxFuture<'a, Result<Box<dyn Any>, EcsError>> {
        self.run_any(grants, input)
    }
}

/// Erases one [`Execute`](crate::verb::Execute) to [`ExecuteAny`].
///
/// The action half of a chain is stored erased, so a concrete action is wrapped
/// before it can be boxed. This is that wrapper, and it is the only one: an
/// action built by [`typed_entry`](super::typed_entry) and one built from a registry entry reach the
/// same erasure, so a correction to one cannot leave the other behind.
pub(crate) struct TypedExec<A>(A);

impl<A> TypedExec<A> {
    /// Wrap one concrete action for erasure.
    pub(crate) const fn new(action: A) -> Self {
        Self(action)
    }
}

impl<A: crate::verb::Execute> TypedExec<A>
where
    A::Input: 'static,
    A::Output: 'static,
{
    /// The one erased call: downcast, prove authority, run on `clock` when the
    /// caller named one, and box the output.
    ///
    /// `None` is the call that carried no flow scope — the ECS tick and a
    /// pipeline built by hand — and it runs exactly as it always has. `Some` is
    /// the flow's `act`, whose scope names the clock its wait must spend.
    fn execute_erased<'a>(
        &'a self,
        clock: Option<&'a crate::clock::Clock>,
        grants: &'a GrantSet,
        input: &'a Erased,
    ) -> crate::verb::BoxFuture<'a, Result<Box<dyn Any>, EcsError>> {
        Box::pin(async move {
            match input.as_any().downcast_ref::<A::Input>() {
                Some(typed) => {
                    let auth: Auth = grants.issue(self.0.required_caps())?;
                    let value = match clock {
                        Some(clock) => self.0.execute_action_on(clock, (auth, typed)).await?,
                        None => self.0.execute_action((auth, typed)).await?,
                    };
                    let boxed: Box<dyn Any> = Box::new(value);
                    Ok(boxed)
                }
                None => Err(EcsError::TypeMismatch {
                    site: "spec::typed_entry",
                    chain: None,
                    expected: type_name::<A::Input>(),
                    observed: input.witness.name(),
                }),
            }
        })
    }
}

impl<A: crate::verb::Execute> ExecuteAny for TypedExec<A>
where
    A::Input: 'static,
    A::Output: 'static,
{
    fn required_caps(&self) -> &[Cap] {
        self.0.required_caps()
    }

    fn domain_id(&self) -> &str {
        self.0.domain_id()
    }

    fn effect_lifetime(&self) -> crate::verb::EffectLifetime {
        self.0.effect_lifetime()
    }

    fn archive_input(&self, input: &Erased) -> Option<Vec<u8>> {
        input
            .as_any()
            .downcast_ref::<A::Input>()
            .and_then(|typed| self.0.archive_input(typed))
    }

    fn run_any<'a>(
        &'a self,
        grants: &'a GrantSet,
        input: &'a Erased,
    ) -> crate::verb::BoxFuture<'a, Result<Box<dyn Any>, EcsError>> {
        self.execute_erased(None, grants, input)
    }

    #[cfg(feature = "script")]
    fn run_any_on<'a>(
        &'a self,
        clock: &'a crate::clock::Clock,
        grants: &'a GrantSet,
        input: &'a Erased,
    ) -> crate::verb::BoxFuture<'a, Result<Box<dyn Any>, EcsError>> {
        self.execute_erased(Some(clock), grants, input)
    }
}
