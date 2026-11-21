//! Justification as a runtime WAF: allow, slow, park, block (#657).
//!
//! Safety × Capability × Justification: capability says what a run may reach,
//! and justification says whether continuing is worth what it costs. The rules
//! are ordered, like a firewall's: the first rule that does not allow wins,
//! and the whole sequence is deterministic in the run, so the same seed
//! reaches the same allow/slow/park/block sequence.
//!
//! Anything may refill an allowance (time, budget, an observed change) through
//! [`Allowance`]; the rules themselves may become learned later, while the
//! outcome of a decision stays deterministic and replayable: a rule is a pure
//! function of its context and its counters, and the run records the deciding
//! rule's name with its reason code.
//!
//! The runtime evaluates the installed rules before `observe`, `act`, `ask`
//! and every retry attempt, through [`Scope::justified`](super::Scope::justified);
//! an `ask` names its token budget as the step's cost, which [`Rule::tokens`]
//! reserves before the model is called and settles to the bill after;
//! `within` refuses Park and Block synchronously at its entry and never waits
//! there. At zero, the run stops with its reason code rather than spending.
//!
//! Traceability only: "who told you to do this" resolves to the deciding rule's
//! name and reason code, carried by [`FlowError::Justified`]. No new metric.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use super::{FlowError, Scope};

/// What one rule answers for one evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Verdict {
    /// Continue now.
    Allow,
    /// Continue after `delay`: a poll slowed from every five minutes to every
    /// ten, a retry spaced out. The delay is real time on the engine's timer.
    Slow {
        /// How long to wait before continuing.
        delay: Duration,
    },
    /// Stop now, by policy, keeping the reason code: a resume re-evaluates the
    /// rules and may proceed when they do.
    Park {
        /// The developer-defined reason code.
        reason: String,
    },
    /// Stop now, by policy, with the reason code: a resume asks the same rules
    /// and is stopped the same way unless the rules changed.
    Block {
        /// The developer-defined reason code.
        reason: String,
    },
}

/// The context a rule reads: where the run is, how many attempts this step has spent, and what it is about to
/// spend. Attempt zero is the first evaluation of a step that is not a retry.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct JustifyCtx<'a> {
    /// The step path the rules are evaluated before.
    pub at: &'a str,
    /// The attempt number, from zero.
    pub attempt: u32,
    /// What the step is about to spend, in model tokens: an `ask`'s token
    /// budget, and zero for a step that spends nothing countable.
    pub cost: u64,
}

impl<'a> JustifyCtx<'a> {
    /// The context for an evaluation before `at` on `attempt`, spending nothing.
    #[must_use]
    pub fn new(at: &'a str, attempt: u32) -> Self {
        Self {
            at,
            attempt,
            cost: 0,
        }
    }

    /// The same context for a step about to spend `cost` model tokens.
    #[must_use]
    pub(crate) fn costing(mut self, cost: u64) -> Self {
        self.cost = cost;
        self
    }
}

/// What a rule does once the step it admitted has been billed: the cost it
/// reserved, then what the provider billed.
type Settle = Box<dyn Fn(u64, u64) + Send + Sync>;

/// One ordered rule: a name and a pure decision function.
///
/// The function is pure in the sense the replay needs: given the same context
/// and the same counter history, it answers the same verdict. Counters live
/// inside the rule's own atomics ([`Allowance`], pacing), advanced only when
/// the rule is evaluated, so the sequence a seed produces never depends on
/// wall time, the scheduler, or another thread.
pub struct Rule {
    /// The name the trail and the refusal carry.
    name: &'static str,
    /// The decision function.
    decide: Box<dyn Fn(&JustifyCtx<'_>) -> Verdict + Send + Sync>,
    /// How the rule settles a cost it admitted, when it spends one.
    settle: Option<Settle>,
}

impl std::fmt::Debug for Rule {
    /// The rule's name; the decision function is a closure with nothing to show.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Rule")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl Rule {
    /// A rule named `name` that answers with `decide`.
    pub fn new(
        name: &'static str,
        decide: impl Fn(&JustifyCtx<'_>) -> Verdict + Send + Sync + 'static,
    ) -> Self {
        Self {
            name,
            decide: Box::new(decide),
            settle: None,
        }
    }

    /// Allow a step while `allowance` holds its whole [`cost`](JustifyCtx::cost),
    /// reserving it, and block with `reason` when it does not, spending nothing.
    ///
    /// The model-token budget of a run, as Pydantic AI's `UsageLimits` keeps
    /// one: an `ask` reserves its token budget before the model is called, so
    /// asks racing on one allowance can never together spend past it, and once
    /// the provider bills the call the reservation settles to the bill, a
    /// refund when it billed less and a further spend, floored at zero, when
    /// the prompt made it bill more. A costless step passes and spends nothing.
    /// A call that fails, or is cancelled before it is billed, keeps its whole
    /// reservation spent: a provider may bill a call it never answered, and a
    /// budget that refunded unseen could be spent past.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use lgwks_bot::script::{Allowance, Justify, Rule};
    ///
    /// let budget = Arc::new(Allowance::new(10_000));
    /// let rules = Justify::new(vec![Rule::tokens("model-tokens", budget, "model-budget-spent")]);
    /// assert_eq!(rules.len(), 1);
    /// ```
    pub fn tokens(
        name: &'static str,
        allowance: Arc<Allowance>,
        reason: impl Into<String>,
    ) -> Self {
        let reason = reason.into();
        let reserving = Arc::clone(&allowance);
        Self {
            name,
            decide: Box::new(move |ctx| {
                if reserving.take_n(ctx.cost) {
                    Verdict::Allow
                } else {
                    Verdict::Block {
                        reason: reason.clone(),
                    }
                }
            }),
            settle: Some(Box::new(move |reserved, billed| {
                allowance.settle(reserved, billed);
            })),
        }
    }

    /// Allow while `allowance` has tokens, spending one per evaluation, and
    /// block with `reason` once it is empty.
    ///
    /// The allowance is refillable from anywhere ([`Allowance::refill`]): time
    /// passing, a budget arriving, an observed change. At zero the run stops
    /// with the reason code rather than spending.
    pub fn allowance(
        name: &'static str,
        allowance: Arc<Allowance>,
        reason: impl Into<String>,
    ) -> Self {
        let reason = reason.into();
        Self::new(name, move |_| {
            if allowance.take() {
                Verdict::Allow
            } else {
                Verdict::Block {
                    reason: reason.clone(),
                }
            }
        })
    }

    /// Park with `reason` once more than `allowed` evaluations have happened.
    pub fn suspend(name: &'static str, allowed: u64, reason: impl Into<String>) -> Self {
        let reason = reason.into();
        let seen = Arc::new(AtomicU64::new(0));
        Self::new(name, move |_| {
            let seen = seen.fetch_add(1, Ordering::Relaxed);
            if seen < allowed {
                Verdict::Allow
            } else {
                Verdict::Park {
                    reason: reason.clone(),
                }
            }
        })
    }

    /// Slow every `every`-th evaluation by `delay` (one slows every decision).
    #[must_use]
    pub fn pace(name: &'static str, every: u64, delay: Duration) -> Self {
        let every = every.max(1);
        let seen = Arc::new(AtomicU64::new(0));
        Self::new(name, move |_| {
            let seen = seen.fetch_add(1, Ordering::Relaxed).saturating_add(1);
            if seen.is_multiple_of(every) {
                Verdict::Slow { delay }
            } else {
                Verdict::Allow
            }
        })
    }

    /// The rule's name: what the trail and the refusal carry for this rule.
    #[must_use]
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Answer for `ctx`.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::script::{JustifyCtx, Rule, Verdict};
    ///
    /// let paced = Rule::pace("pace", 2, std::time::Duration::from_millis(5));
    /// assert_eq!(paced.decide(&JustifyCtx::new("step", 0)), Verdict::Allow);
    /// assert!(matches!(paced.decide(&JustifyCtx::new("step", 0)), Verdict::Slow { .. }));
    /// ```
    #[must_use]
    pub fn decide(&self, ctx: &JustifyCtx<'_>) -> Verdict {
        (self.decide)(ctx)
    }
}

/// A refillable allowance: spending and refilling are saturating, so neither
/// wraps and neither needs a lock.
#[derive(Debug, Default)]
pub struct Allowance {
    /// Tokens left. Spent by evaluation, refilled by anything.
    remaining: AtomicU64,
}

impl Allowance {
    /// An allowance holding `tokens`.
    #[must_use]
    pub fn new(tokens: u64) -> Self {
        Self {
            remaining: AtomicU64::new(tokens),
        }
    }

    /// Spend one token, or refuse when empty. Saturating: never wraps.
    pub fn take(&self) -> bool {
        self.remaining
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |held| {
                held.checked_sub(1)
            })
            .is_ok()
    }

    /// Spend all of `tokens`, or refuse and spend nothing when fewer are left.
    pub(crate) fn take_n(&self, tokens: u64) -> bool {
        self.remaining
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |held| {
                held.checked_sub(tokens)
            })
            .is_ok()
    }

    /// Settle a reservation of `reserved` tokens to the `billed` ones: refund
    /// what the bill left over, or spend its excess down to zero.
    pub(crate) fn settle(&self, reserved: u64, billed: u64) {
        match billed.checked_sub(reserved) {
            Some(excess) => {
                let _spent =
                    self.remaining
                        .try_update(Ordering::Relaxed, Ordering::Relaxed, |held| {
                            Some(held.saturating_sub(excess))
                        });
            }
            None => self.refill(reserved.saturating_sub(billed)),
        }
    }

    /// Add `tokens` back, saturating at `u64::MAX`.
    ///
    /// A compare-exchange loop: each failure means another thread spent or
    /// refilled concurrently, so a retry observes that progress rather than
    /// losing it, and success lands exactly the saturated sum.
    pub fn refill(&self, tokens: u64) {
        let mut current = self.remaining.load(Ordering::Relaxed);
        loop {
            let next = current.saturating_add(tokens);
            match self.remaining.compare_exchange_weak(
                current,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return,
                Err(observed) => {
                    current = observed;
                }
            }
        }
    }

    /// Tokens left.
    #[must_use]
    pub fn remaining(&self) -> u64 {
        self.remaining.load(Ordering::Relaxed)
    }
}

/// What the rules answered, and who answered it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Decision {
    /// The winning verdict.
    pub verdict: Verdict,
    /// The rule that gave it, or `"allow"` when every rule allowed.
    pub rule: &'static str,
}

/// The ordered rules one scope evaluates.
///
/// Clone shares the rules: every scope descended from the scope the rules were
/// installed on answers the same sequence, and the counters inside the rules
/// are the one history every step appends to.
#[derive(Debug, Clone, Default)]
pub struct Justify {
    /// The ordered rules, shared by clone.
    rules: Arc<[Rule]>,
}

impl Justify {
    /// Evaluate `rules` in order before every async word and retry attempt.
    #[must_use]
    pub fn new(rules: Vec<Rule>) -> Self {
        Self {
            rules: rules.into(),
        }
    }

    /// No rules: every evaluation allows.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// How many rules are installed.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// Whether no rule is installed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// The first non-allow verdict in rule order, or allow when every rule
    /// allows. Deterministic in the evaluation order, which the runtime fixes:
    /// one evaluation per async word entry and one per retry attempt.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::script::{Justify, JustifyCtx, Rule, Verdict};
    ///
    /// let rules = Justify::new(vec![Rule::pace("pace", 1, std::time::Duration::from_millis(5))]);
    /// let decision = rules.decide(&JustifyCtx::new("step", 0));
    /// assert_eq!(decision.rule, "pace");
    /// assert!(matches!(decision.verdict, Verdict::Slow { .. }));
    /// assert_eq!(Justify::empty().decide(&JustifyCtx::new("step", 0)).verdict, Verdict::Allow);
    /// ```
    #[must_use]
    pub fn decide(&self, ctx: &JustifyCtx<'_>) -> Decision {
        for rule in self.rules.iter() {
            match rule.decide(ctx) {
                Verdict::Allow => {}
                verdict => {
                    return Decision {
                        verdict,
                        rule: rule.name(),
                    };
                }
            }
        }
        Decision::allowed()
    }

    /// Settle a cost every rule admitted: each rule that spends one moves its
    /// `reserved` tokens to the `billed` ones. Called only after an allow,
    /// when every rule in the chain admitted the cost.
    pub(crate) fn settle(&self, reserved: u64, billed: u64) {
        for rule in self.rules.iter() {
            if let Some(ref settle) = rule.settle {
                settle(reserved, billed);
            }
        }
    }
}

impl Decision {
    /// The decision when no rule stopped the step: allow.
    const fn allowed() -> Self {
        Self {
            verdict: Verdict::Allow,
            rule: "allow",
        }
    }
}

/// What `scope`'s rules answer for `attempt` spending `cost`; a scope with no
/// installed rules is answered like an empty rule set, which allows.
fn decision_at(scope: &Scope, attempt: u32, cost: u64) -> Decision {
    match scope.justification() {
        Some(rules) => rules.decide(&JustifyCtx::new(scope.path(), attempt).costing(cost)),
        None => Decision::allowed(),
    }
}

/// Refuse the step `scope` is entering when its justification rules already
/// stop it.
///
/// The synchronous Park/Block half of [`Scope::justified`], for entries that
/// cannot wait: `within` evaluates this before reading its deadline, with no
/// new suspension point and no state in the caller's future, so a deep nest of
/// clock-governed steps pays nothing for it. Slow is not waited out here —
/// see [`Scope::justified`].
///
/// # Errors
///
/// [`FlowError::Justified`] carrying the deciding rule and its reason code.
pub(crate) fn refuse_if_unjustified(scope: &Scope) -> Result<(), FlowError> {
    let decision = decision_at(scope, 0, 0);
    match decision.verdict {
        Verdict::Allow | Verdict::Slow { .. } => Ok(()),
        Verdict::Park { reason } | Verdict::Block { reason } => {
            Err(FlowError::justified(decision.rule, reason).located(scope))
        }
    }
}

impl Scope {
    /// Evaluate this scope's justification for `attempt`, slowing, parking or
    /// stopping the run as the rules answer.
    ///
    /// A scope with no installed rules allows at once. Called by the runtime
    /// before `observe`, `act`, and every retry attempt, so a flow names its
    /// rules once — on the scope its steps descend from — and every effectful
    /// step and retry answers to them. (`within` evaluates Park and Block
    /// synchronously at its entry and never waits there: a deadline cannot
    /// spend its own budget waiting, so pacing is observed where waiting
    /// already exists.)
    ///
    /// # Errors
    ///
    /// [`FlowError::Justified`] carrying the deciding rule and its reason code,
    /// or [`FlowError::Cancelled`] when the scope stopped while a `Slow`
    /// waited.
    pub async fn justified(&self, attempt: u32) -> Result<(), FlowError> {
        self.justified_costing(attempt, 0).await
    }

    /// [`Scope::justified`] for a step about to spend `cost` model tokens: a
    /// token rule reserves the cost on allow, and [`Scope::settle_cost`] moves
    /// it to the bill once the step is billed.
    pub(crate) async fn justified_costing(&self, attempt: u32, cost: u64) -> Result<(), FlowError> {
        let decision = decision_at(self, attempt, cost);
        match decision.verdict {
            Verdict::Allow => Ok(()),
            Verdict::Slow { delay } => {
                let slept = self
                    .token()
                    .run_until_cancelled(crate::rt::time::sleep(delay))
                    .await
                    .is_some();
                if slept {
                    Ok(())
                } else {
                    Err(FlowError::Cancelled {
                        at: std::sync::Arc::from(self.path()),
                    })
                }
            }
            Verdict::Park { reason } | Verdict::Block { reason } => {
                Err(FlowError::justified(decision.rule, reason).located(self))
            }
        }
    }

    /// Settle a cost [`Scope::justified_costing`] admitted: `reserved` tokens
    /// were held, `billed` were spent.
    pub(crate) fn settle_cost(&self, reserved: u64, billed: u64) {
        if let Some(rules) = self.justification() {
            rules.settle(reserved, billed);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::{Allowance, Decision, Justify, JustifyCtx, Rule, Verdict};

    /// The context these tests decide under.
    fn ctx() -> JustifyCtx<'static> {
        JustifyCtx::new("flow/step", 0)
    }

    /// The first non-allow verdict wins, in rule order, and an empty rule set
    /// allows.
    #[test]
    fn the_first_non_allow_verdict_wins_in_rule_order() {
        let justify = Justify::new(vec![
            Rule::new("first", |_| Verdict::Allow),
            Rule::new("second", |_| Verdict::Slow {
                delay: Duration::ZERO,
            }),
            Rule::new("third", |_| Verdict::Block {
                reason: String::from("never-reached"),
            }),
        ]);
        let Decision { verdict, rule } = justify.decide(&ctx());
        assert!(
            matches!(verdict, Verdict::Slow { .. }),
            "the second rule's slow wins over the third rule's block: {verdict:?}"
        );
        assert_eq!(rule, "second", "the refusal names the deciding rule");
        assert_eq!(
            Justify::empty().decide(&ctx()).verdict,
            Verdict::Allow,
            "no rules is allow, not a refusal"
        );
    }

    /// An allowance spends one token per evaluation, blocks at zero with its
    /// reason code, and refills from anywhere.
    #[test]
    fn an_allowance_blocks_at_zero_with_its_reason_code() {
        let allowance = Arc::new(Allowance::new(2));
        let rule = Rule::allowance("budget", Arc::clone(&allowance), "poll-budget-spent");
        assert_eq!(rule.decide(&ctx()), Verdict::Allow, "first take spends");
        assert_eq!(rule.decide(&ctx()), Verdict::Allow, "second take spends");
        assert_eq!(
            rule.decide(&ctx()),
            Verdict::Block {
                reason: String::from("poll-budget-spent"),
            },
            "at zero the run stops with its reason code rather than spending"
        );
        allowance.refill(1);
        assert_eq!(
            rule.decide(&ctx()),
            Verdict::Allow,
            "a refill admits the run again"
        );
        assert_eq!(
            allowance.remaining(),
            0,
            "the refill's token was spent by the evaluation"
        );
    }

    /// A token rule spends the step's whole cost or nothing, a costless step
    /// passes without spending, and settling moves the allowance to the bill:
    /// an under-bill refunds, an over-bill spends down to zero and never wraps.
    #[test]
    fn a_token_rule_spends_the_cost_and_settles_to_the_bill() {
        let allowance = Arc::new(Allowance::new(100));
        let rules = Justify::new(vec![Rule::tokens(
            "tokens",
            Arc::clone(&allowance),
            "model-budget-spent",
        )]);
        let asking = |cost| rules.decide(&ctx().costing(cost)).verdict;
        assert_eq!(
            asking(30),
            Verdict::Allow,
            "thirty of a hundred is admitted"
        );
        assert_eq!(allowance.remaining(), 70, "the whole cost is reserved");
        rules.settle(30, 10);
        assert_eq!(allowance.remaining(), 90, "an under-bill refunds the rest");
        assert_eq!(
            asking(91),
            Verdict::Block {
                reason: String::from("model-budget-spent"),
            },
            "one past what is left is refused"
        );
        assert_eq!(allowance.remaining(), 90, "a refused cost spends nothing");
        assert_eq!(asking(0), Verdict::Allow, "a costless step passes");
        assert_eq!(
            asking(90),
            Verdict::Allow,
            "exactly what is left is admitted"
        );
        assert_eq!(allowance.remaining(), 0, "and leaves nothing");
        rules.settle(90, 500);
        assert_eq!(allowance.remaining(), 0, "an over-bill floors at zero");
        allowance.refill(40);
        rules.settle(10, 25);
        assert_eq!(allowance.remaining(), 25, "an over-bill spends its excess");
    }

    /// Pacing slows every `every`-th evaluation and allows the rest.
    #[test]
    fn pacing_slows_every_nth_evaluation() {
        let rule = Rule::pace("poll-every-minute", 3, Duration::from_secs(60));
        let verdicts: Vec<Verdict> = (0..6).map(|_| rule.decide(&ctx())).collect();
        let slowed = verdicts
            .iter()
            .filter(|verdict| matches!(verdict, Verdict::Slow { .. }))
            .count();
        assert_eq!(
            slowed, 2,
            "the third and sixth evaluations slow: {verdicts:?}"
        );
        assert!(
            verdicts
                .iter()
                .enumerate()
                .all(|(index, verdict)| (index == 2 || index == 5)
                    == matches!(verdict, Verdict::Slow { .. })),
            "only every third evaluation slows: {verdicts:?}"
        );
    }
}
