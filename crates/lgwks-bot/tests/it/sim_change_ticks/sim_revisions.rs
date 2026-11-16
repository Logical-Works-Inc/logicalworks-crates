//! The pinned rows: a wrap, a regression, a forced refresh, a single movement
//! and the cost of a quiet tick.

use std::cell::Cell;
use std::rc::Rc;

use lgwks_bot::cap::{Auth, Cap};
use lgwks_bot::ecs::EcsError;
use lgwks_bot::gate::GrantSet;
use lgwks_bot::spec::Bot;
use lgwks_bot::verb::{Observe, RefreshReason};

use super::{Subject, Tally, TestResult, scope};

/// A source whose revision runs off the end of `u64` and comes back at zero.
///
/// The comparison is equality, so `u64::MAX` → `0` is a change rather than an
/// ordering a wrap could invalidate. If this ever became an ordering the value
/// would still move and the digest engine would still fire — which is why the
/// assertion is on the fired sequence and not on the revision arithmetic.
#[test]
fn a_revision_that_wraps_is_a_change() -> TestResult {
    let mut subject = Subject::build(0, 0, true)?;
    // Sit the committed revision at the end of the counter before the run, so
    // the wrap is a step *off* the end rather than a step onto it.
    subject.place(u64::MAX, 0);
    let _outcome = subject.bot.tick();

    let mut fired = Vec::new();
    // Off the end of the counter and back to zero, with the value moving: the
    // wrap. An ordering comparison would call this smaller rather than different
    // and skip it.
    subject.current[0].set(0);
    subject.value[0].set(1);
    fired.push(subject.bot.tick()?);
    // The same revision held while nothing moves: the skip.
    fired.push(subject.bot.tick()?);
    // And on from there, so the wrap is not the only movement and the tick after
    // it is a quiet one rather than the end of the run.
    subject.current[0].set(1);
    subject.value[0].set(2);
    fired.push(subject.bot.tick()?);

    assert_eq!(
        fired,
        vec![1, 0, 1],
        "the wrap from the maximum back to zero must be a change, and a revision held \
         over a held value must not be: {fired:?}"
    );
    Ok(())
}

/// A revision that steps backwards, and one that returns to a revision already
/// committed.
///
/// Both are handled, by two different rules, which is why both are here. A
/// backwards step is a change because the comparison is equality. A revision that
/// returns to one already committed while the *value* moves is not a revision
/// change at all, and it is caught by the value comparison: the skip is
/// suppressed by the revision and the value decides.
#[test]
fn a_regressing_revision_is_a_change() -> TestResult {
    let mut subject = Subject::build(0, 0, true)?;
    subject.place(10, 0);
    let _outcome = subject.bot.tick();

    let mut fired = Vec::new();
    // Forward, so the substrate commits revision 11 against value 1.
    subject.current[0].set(11);
    subject.value[0].set(1);
    fired.push(subject.bot.tick()?);
    // Backwards to 9, which it has never committed.
    subject.current[0].set(9);
    subject.value[0].set(2);
    fired.push(subject.bot.tick()?);
    // Back to 11, which it *has* committed: the revision is quiet, so the value
    // comparison is all that is left, and the value moved.
    subject.current[0].set(11);
    subject.value[0].set(3);
    fired.push(subject.bot.tick()?);
    // A revision it has never seen, holding a value it has already fired on: the
    // revision says change, the value says quiet, and the revision wins because
    // it is the cheaper answer and it is not contradicted.
    subject.current[0].set(3);
    subject.value[0].set(3);
    fired.push(subject.bot.tick()?);

    assert_eq!(
        fired,
        vec![1, 1, 1, 0],
        "a backwards step and a fresh revision are changes, a committed revision with a \
         moved value is caught by the value comparison, and a fresh revision over an \
         already-seen value fires nothing: {fired:?}"
    );
    Ok(())
}

/// A source that declares its baseline unsound is read whatever its revision.
///
/// The case where believing a revision would be exactly wrong: the substrate has
/// been told its held value is not what the source would report now, and a
/// revision is a claim about a value rather than a repair of one. Without this
/// the skip would turn a forced read into a skipped tick, and the report's
/// `forced` row is what tells a caller its cache was rebuilt.
#[test]
fn a_forced_refresh_beats_a_quiet_revision() -> TestResult {
    let mut subject = Subject::build(0, 0, true)?;
    subject.place(7, 0);
    let _outcome = subject.bot.tick();

    // Nothing moves and nothing is declared: the skip.
    let before = subject.polls[0].get();
    assert_eq!(subject.bot.tick()?, 0, "a quiet revision fires nothing");
    assert_eq!(
        subject.polls[0].get(),
        before,
        "and a quiet revision is not polled, which is the whole saving"
    );

    // The same revision, still quiet as a value, but the source declares its
    // cache unsound. The revision is unchanged, so the skip would apply — and
    // must not.
    subject.declared[0].set(Some(RefreshReason::StaleRemoteKey));
    let _outcome = subject.bot.tick();
    subject.clear_declarations();
    assert!(
        subject.polls[0].get() > before,
        "a source that declared its baseline unsound must be polled even when its \
         revision is unchanged, or a forced read is never taken"
    );
    let report = subject.bot.tick_report();
    assert!(
        !report.forced().is_empty(),
        "and the report must name the chain and the cause, because that is what tells \
         a caller its cache was rebuilt rather than skipped"
    );
    Ok(())
}

/// A movement fires once, and then the tick is quiet again.
///
/// The other half of the equivalence: a skip that fired once and then kept
/// firing would pass a comparison over tick *counts* while being wrong on every
/// tick after the first.
#[test]
fn the_change_tick_never_fires_twice_for_one_movement() -> TestResult {
    let mut subject = Subject::build(0, 0, true)?;
    subject.place(0, 0);
    let _outcome = subject.bot.tick();

    subject.current[0].set(1);
    subject.value[0].set(1);
    assert_eq!(subject.bot.tick()?, 1, "the movement fires once");
    let after_movement = subject.polls[0].get();

    for tick in 0..5 {
        assert_eq!(
            subject.bot.tick()?,
            0,
            "tick {tick} after the movement fired again from a movement that already \
             happened"
        );
    }
    assert_eq!(
        subject.polls[0].get(),
        after_movement,
        "five quiet ticks after a movement poll nothing: a skip that re-polls is not a \
         skip"
    );
    Ok(())
}

/// A source that counts how often the substrate asks what it declares about its
/// own cache, and otherwise sits still at one revision.
struct Asked {
    /// How many times `cache_state` was read.
    reads: Rc<Cell<usize>>,
}

impl Observe for Asked {
    type Output = u64;

    fn required_caps(&self) -> &[Cap] {
        &[]
    }

    fn revision(&self) -> Option<u64> {
        Some(1)
    }

    fn cache_state(&self) -> Option<RefreshReason> {
        self.reads.set(self.reads.get().saturating_add(1));
        None
    }

    async fn poll(&self, _call: (Auth, ())) -> Result<u64, EcsError> {
        Ok(1)
    }

    fn domain_id(&self) -> &str {
        "sim::asked"
    }
}

/// A tick that polls nothing asks a source for its cache state once.
///
/// Requirement (#279, the quiet-tick cost): the question is asked before the polls
/// to decide which sources may be skipped, and asked again after them to catch a
/// failure a poll recorded. A tick on which every source was skipped has no poll
/// in which a failure could have been recorded, so the second ask repeats the first
/// and is not made. The two asks per source per tick were two dynamic calls on the
/// path every always-on watcher runs every interval. Discriminating: the count over
/// five quiet ticks is five, where two asks per tick made it ten.
#[test]
fn a_quiet_tick_asks_each_source_for_its_cache_state_once() -> TestResult {
    let reads = Rc::new(Cell::new(0));
    let (tally, _log) = Tally::new();
    let mut bot = Bot::builder("sim-change-ticks-asked")
        .observe(Asked {
            reads: Rc::clone(&reads),
        })
        .on(|_: &u64| true, tally)
        .with_effects(scope()?)
        .build(&GrantSet::empty())?;
    // Two ticks settle the value and the work it opened, so what follows is quiet.
    let _first = bot.tick();
    let _second = bot.tick();
    let warmed = reads.get();
    for tick in 0..5 {
        assert_eq!(bot.tick()?, 0, "quiet tick {tick} fired");
    }
    assert_eq!(
        reads.get().saturating_sub(warmed),
        5,
        "five quiet ticks must ask once each, not before and after polls that never ran"
    );
    Ok(())
}
