//! T12: evidence carrying the wrong identity is refused, and refuses leave the
//! journal byte-identical.
//!
//! The row names seven identity fields and two properties: each wrong field is
//! refused without mutation, and a checked counter's exhaustion never aliases an
//! identity it has already issued.
//!
//! What existed is real but partial. `ecs_tick.rs` refuses a stale *binding*
//! (the action digest) and `durable_dispatch.rs` covers a readable journal with no
//! such outcome. Neither sweeps the seven fields, so nothing asserted that each
//! one is refused by the check that is *about* it rather than by an earlier check
//! that happens to catch it too.
//!
//! This file sweeps them. One held entry, one correct identity, and seven
//! deliveries — the correct one and six that each differ in exactly one field.
//! Each wrong delivery must be refused by the variant its field calls for, the
//! journal must not move, the action must not be re-entered, and the correct
//! delivery must still succeed. That last clause is what makes the sweep a test
//! of the identity checks: a blanket "refuse everything" passes five of the seven
//! assertions and fails that one.
//!
//! The rows live beside this file by subject: `ledger` (the sweep, the duplicate
//! and the foreign run), `late_evidence` (T11), `file_journal` (the real device)
//! and `counter` (the checked counters). This file holds the one held entry they
//! all start from and the seven rewrites they all apply.
//!
//! # What the refusals actually are, and why
//!
//! Asserting "an error" would be worthless here, because an earlier check often
//! catches the evidence first. The arms group by the *reason*, and the grouping
//! is the interesting output of writing the test:
//!
//! - An `ActionId` is derived from the bot's name, its position and its action's
//!   domain — **not** from the run, the flow revision or the environment. So a
//!   key that gets one of those three wrong still names an action this bot does
//!   declare, and is refused one level later: the ledger speaks for one run, and
//!   a key naming another is not its work.
//! - None of those four refusals is `NoSuchWork`. That variant means "there is
//!   no held effect at this address", and telling a caller that about work it
//!   *does* hold is the confusion the variant exists to prevent.
//!
//! The counter clause needs no bot and is asserted against the values the types
//! expose rather than by manufacturing `u64::MAX` appends.

use crate::sim;

use std::cell::{Cell, RefCell};
use std::error::Error;
use std::rc::Rc;

use lgwks_bot::ecs::EcsError;
use lgwks_bot::effect::{
    ActionDigest, ActionId, EffectKey, EnvironmentEpoch, EnvironmentId, FlowRevision, RunId,
};
use lgwks_bot::error::KernelError;
use lgwks_bot::gate::GrantSet;
use lgwks_bot::journal::MemoryJournal;
use lgwks_bot::spec::{Bot, EffectIdentity};

use sim::rig::{FixedSource, NeverSettles, RUN_NAME, is_value, scope};

mod counter;
mod file_journal;
mod late_evidence;
mod ledger;

/// What a row reports.
///
/// A `Result` rather than a panic because the workspace forbids the panicking
/// path everywhere, test code included: a row that unwound would report a line
/// number inside a macro rather than which of the facts it was checking had
/// moved.
type TestResult = Result<(), Box<dyn Error>>;

/// The environment this file's ledger acts on, as a parsed value.
fn environment() -> Result<EnvironmentId, Box<dyn Error>> {
    Ok(EnvironmentId::from_hex(sim::rig::ENV)?)
}

/// The run the ledger under test speaks for.
fn ledger_run() -> Result<RunId, Box<dyn Error>> {
    Ok(RunId::from_hex("0102030405060708090a0b0c0d0e0f10")?)
}

/// The flow revision the ledger under test was admitted under.
fn ledger_flow() -> Result<FlowRevision, Box<dyn Error>> {
    Ok(FlowRevision::from_tagged(
        "blake3_256",
        "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
    )?)
}

/// One of the seven identity fields a settlement can get wrong.
///
/// An enum rather than seven closures, because the row is about *which field*
/// disagreed and an assertion that only said "refused" would pass for a refusal
/// that named the wrong one. Each arm rewrites exactly one field of a correct
/// key, so a key this produces differs from the correct one in that field and
/// nowhere else — which is what makes the test discriminating rather than a
/// blanket "a wrong key is refused".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WrongField {
    /// A key whose run is not the ledger's.
    Run,
    /// A key whose action the ledger does not declare.
    Action,
    /// A key bound to a different payload than the live generation.
    Payload,
    /// A key admitted under a different flow revision.
    Revision,
    /// A key naming an environment the broker never issued.
    Environment,
    /// A key naming a different generation of the right environment.
    Epoch,
    /// A key naming a later attempt on the same entry.
    Attempt,
}

impl WrongField {
    /// Every arm, in a fixed order, so a family that sweeps them cannot reorder
    /// its draws and renumber another family's trace.
    const ALL: [Self; 7] = [
        Self::Run,
        Self::Action,
        Self::Payload,
        Self::Revision,
        Self::Environment,
        Self::Epoch,
        Self::Attempt,
    ];

    /// A stable tag for the trace, so a trace hash does not move when an arm's
    /// `Debug` spelling does.
    const fn tag(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Action => "action",
            Self::Payload => "payload",
            Self::Revision => "revision",
            Self::Environment => "environment",
            Self::Epoch => "epoch",
            Self::Attempt => "attempt",
        }
    }

    /// The rewrite this field means, applied to `held`.
    fn key(self, held: EffectKey) -> Result<EffectKey, Box<dyn Error>> {
        match self {
            Self::Run => replace_run(held),
            Self::Action => replace_action(held),
            Self::Payload => replace_digest(held),
            Self::Revision => replace_flow(held),
            Self::Environment => replace_environment(held),
            Self::Epoch => replace_epoch(held),
            Self::Attempt => replace_attempt(held),
        }
    }
}

/// The identity `key` was minted under: its run, environment and flow.
const fn identity_of(key: EffectKey) -> EffectIdentity {
    EffectIdentity::new(key.run(), key.environment(), key.flow())
}

/// `key`'s own attempt — its action, attempt, digest and epoch — minted under
/// `identity`, which is what a rewrite of the run or the flow asks for.
const fn under(identity: EffectIdentity, key: EffectKey) -> EffectKey {
    identity.key(key.action(), key.attempt(), key.digest(), key.epoch())
}

/// An action id this run's bots do not declare.
///
/// An action id is a digest over the bot's name, its position and its action's
/// domain, so no hex constant is *guaranteed* undeclared — a test that picked one
/// and got lucky would be asserting something for a reason it does not control.
/// The declared set here is one action and every sweep below asserts the
/// rewritten key differs from the held one before it asserts anything about the
/// refusal, so a future change to what the bot derives fails the row rather than
/// quietly making an arm test the held action a second time. The assertion, not
/// the constant, is what carries the guarantee.
const UNHELD_ACTION: &str = "ee00112233445566778899aabbccddee";

/// A key that differs from `key` in its run alone.
///
/// Every rewrite function is named after the enum arm it serves and documented
/// only by the field it moves: the arm's own doc already says which field that
/// is, and a second copy of the sentence is a second statement to keep in step.
fn replace_run(key: EffectKey) -> Result<EffectKey, Box<dyn Error>> {
    let run = RunId::from_hex("1112131415161718191a1b1c1d1e1f20")?;
    Ok(under(
        EffectIdentity::new(run, key.environment(), key.flow()),
        key,
    ))
}

/// A key that differs from `key` in its action alone.
fn replace_action(key: EffectKey) -> Result<EffectKey, Box<dyn Error>> {
    let action = ActionId::from_hex(UNHELD_ACTION)?;
    Ok(identity_of(key).key(action, key.attempt(), key.digest(), key.epoch()))
}

/// A key that differs from `key` in its payload digest alone.
fn replace_digest(key: EffectKey) -> Result<EffectKey, Box<dyn Error>> {
    let digest = ActionDigest::from_tagged(
        "blake3_256",
        "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e2f",
    )?;
    Ok(identity_of(key).key(key.action(), key.attempt(), digest, key.epoch()))
}

/// A key that differs from `key` in its flow revision alone.
fn replace_flow(key: EffectKey) -> Result<EffectKey, Box<dyn Error>> {
    let flow = FlowRevision::from_tagged(
        "blake3_256",
        "1f1e1d1c1b1a191817161514131211100f0e0d0c0b0a09080706050403020100",
    )?;
    Ok(under(
        EffectIdentity::new(key.run(), key.environment(), flow),
        key,
    ))
}

/// A key that differs from `key` in its environment alone.
fn replace_environment(key: EffectKey) -> Result<EffectKey, Box<dyn Error>> {
    let environment = EnvironmentId::from_hex("2122232425262728292a2b2c2d2e2f31")?;
    Ok(EffectIdentity::new(key.run(), environment, key.flow()).at_epoch(key, key.epoch()))
}

/// A key naming a different generation of the right environment.
///
/// A *fixed* generation would be wrong half the time: the held key is already at
/// whatever generation the broker minted, and writing `1` again would produce the
/// same key whenever that generation *is* one. Two generations past is always a
/// different value, and it is the stale direction the fence cares about — a key
/// from a generation the environment has been replaced past.
fn replace_epoch(key: EffectKey) -> Result<EffectKey, Box<dyn Error>> {
    let ahead = key
        .epoch()
        .checked_next()
        .and_then(EnvironmentEpoch::checked_next)
        .filter(|ahead| *ahead != key.epoch())
        .or_else(|| (key.epoch() != EnvironmentEpoch::FIRST).then_some(EnvironmentEpoch::FIRST))
        .ok_or("no generation differs from the held one")?;
    Ok(identity_of(key).at_epoch(key, ahead))
}

/// A key naming a later attempt on the same entry, and no other change.
fn replace_attempt(key: EffectKey) -> Result<EffectKey, Box<dyn Error>> {
    let attempt = key
        .attempt()
        .checked_next()
        .ok_or("an attempt counter with one value left cannot be advanced")?;
    Ok(key.with_attempt(attempt))
}

// ── The bot-shaped rows ─────────────────────────────────────────────────────

/// A bot whose one action is held as an unknown outcome, with its live binding.
///
/// One constructor for every scenario in this file, because all of them need the
/// same arrangement — a held entry, a live binding, and a journal holding only
/// the two write-ahead rungs — and a copy per row would be six places that
/// arrangement could drift out from under the assertions about it.
///
/// The counter is what makes "held, not retried" and "refused" distinguishable:
/// every row asserts the action's body did *not* run again, which a refusal that
/// re-entered it would satisfy by looking like a retry.
struct Held {
    /// The bot under test.
    bot: Bot,
    /// The journal its effects are recorded in.
    store: Rc<RefCell<MemoryJournal>>,
    /// The key that settles the held entry.
    binding: EffectKey,
    /// How many times the effect's body has run.
    entered: Rc<Cell<u32>>,
}

/// A held bot over `store`, and the binding its held entry reports.
///
/// The action is [`NeverSettles`], so the tick's own error is
/// `EffectIndeterminate` rather than a tick that fired: that is the state the
/// whole ledger exists for, and an entry that was merely undecided would not be
/// settleable at all.
fn held_under(store: Rc<RefCell<MemoryJournal>>) -> Result<Held, Box<dyn Error>> {
    let entered = Rc::new(Cell::new(0));
    let mut bot = Bot::builder(RUN_NAME)
        .observe(FixedSource)
        .on(
            is_value,
            NeverSettles {
                entered: Rc::clone(&entered),
            },
        )
        .with_effects(scope(
            EffectIdentity::new(ledger_run()?, environment()?, ledger_flow()?),
            Rc::clone(&store),
        )?)
        .build(&GrantSet::empty())?;
    match bot.tick() {
        Ok(fired) => {
            let refusal =
                Err(format!("an indeterminate effect was reported as {fired} fired").into());
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "held_under: returning an error to the caller");
            return refusal;
        }
        Err(error) => {
            if !matches!(
                error,
                EcsError::Kernel(KernelError::EffectIndeterminate { .. })
            ) {
                let refusal = Err(format!("expected the action's own error, got {error:?}").into());
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "held_under: returning an error to the caller");
                return refusal;
            }
        }
    }
    let binding = bot
        .pending()
        .first()
        .ok_or("the held entry must be reported")?
        .key()
        .ok_or("a held entry is held by an attempt, so it must carry its key")?;
    Ok(Held {
        bot,
        store,
        binding,
        entered,
    })
}
