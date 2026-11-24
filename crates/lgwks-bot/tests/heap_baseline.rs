//! #1128 G1: the heap of a finished `script!` run returns to its baseline.
//!
//! The borrow checker frees what goes out of scope. It cannot see an `Rc`
//! cycle, a forgotten value or a cache that only grows: each keeps its bytes
//! for ever and every assertion still passes. So this binary counts the bytes
//! itself. A counting global allocator (`lgwks_deps::stats_alloc`, feature
//! `heap-accounting`, gap #1133) sees every allocation on every thread, and
//! the live count is allocated minus freed.
//!
//! The oracle is that count, not the code's own output. After a warm-up of
//! each shape (one-time state: the process pool, trace callsites), a seeded
//! window of each shape alone, then of all of them mixed, must leave the live
//! count where it started. Two checks of the instrument come first: an `Rc`
//! cycle must show, and a buffer grown by `realloc` then freed must not.
//!
//! Fools: the saboteur (the cycle); the double-clicker (runs back to back on
//! one runtime); the clock-fool (a deadline that always fires mid-wait); the
//! wrong-order (a body that cancels its own fan-out halfway). The deadline
//! cuts a step that never ends, so no wait races the scheduler.
//!
//! One test body holds every window, so no other test thread allocates while
//! the counters are read.

#![cfg(feature = "script")]

use std::alloc::System;
use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};

use lgwks_bot::script::{FlowError, Scope, Tenant, block_on};
use lgwks_deps::stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};
use lgwks_std::seeded::Seeded;

#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

/// What a test reports when a precondition did not hold.
type TestResult = Result<(), Box<dyn Error>>;

/// How many seeded runs each window holds.
const RUNS: u64 = 512;

/// How many runs of each shape come before the windows open.
const WARM: u64 = 4;

/// The shapes a run is drawn from, by number.
const SHAPES: [&str; 5] = ["fan-out", "retry", "deadline", "cancel", "failure"];

/// A buffer a fan-out body allocates and drops.
fn buffer(item: u16) -> Vec<u8> {
    vec![0; usize::from(item).saturating_add(64)]
}

/// Fails until `fails` attempts have failed, then answers the attempt count.
fn attempt(tries: &AtomicU32, fails: u32) -> Result<u32, String> {
    let made = tries.fetch_add(1, Ordering::Relaxed).saturating_add(1);
    if made > fails {
        Ok(made)
    } else {
        Err(format!("attempt {made} of {fails} fails"))
    }
}

/// One node of the saboteur's cycle.
struct Node {
    /// The other node, set after both exist.
    next: RefCell<Option<Rc<Node>>>,
    /// What the leak holds.
    _payload: Vec<u8>,
}

/// Two nodes that own each other: neither count reaches zero, so neither is
/// freed, and nothing the compiler checks says so.
fn leak_a_cycle(bytes: usize) {
    let first = Rc::new(Node {
        next: RefCell::new(None),
        _payload: vec![0; bytes],
    });
    let second = Rc::new(Node {
        next: RefCell::new(Some(Rc::clone(&first))),
        _payload: vec![0; bytes],
    });
    first.next.replace(Some(second));
}

lgwks_bot::script! {
    /// Every item allocates a buffer, and every buffer ends with its body.
    flow fan(items: Vec<u16>) -> usize:
        let sizes = each item in items:
            buffer(item).len()
        give back sizes.iter().sum()

    /// A step that fails transiently `fails` times, then answers.
    flow flaky(fails: u32) -> u32:
        let tries = AtomicU32::new(0)
        let made = retry up to 4 times, waiting 1ms:
            attempt(&tries, fails).or_retry()?
        give back made

    /// A step that never ends on its own: the deadline drops it mid-wait.
    flow late(tag: u64) -> u64:
        let waited = within 2ms:
            std::future::pending::<()>().await
            tag
        give back waited

    /// A fan-out that its fourth body cancels.
    flow stopped(items: Vec<u16>) -> usize:
        let sizes = each item in items:
            if item == 3:
                scope.cancel()
            buffer(item).len()
        give back sizes.iter().sum()

    /// A step that fails for good.
    flow refused(reason: String) -> u8:
        fail with reason

    /// The saboteur: a run that leaks.
    flow cyclic(bytes: usize):
        leak_a_cycle(bytes)
}

/// Items `0..count`.
fn items(count: u64) -> Result<Vec<u16>, Box<dyn Error>> {
    Ok((0..count).map(u16::try_from).collect::<Result<_, _>>()?)
}

/// Run shape `shape` once with sizes from `draw`, and check its outcome.
fn run(shape: usize, draw: &mut Seeded) -> TestResult {
    let scope = Scope::root(Tenant::new("heap")?);
    match shape {
        0 => {
            let count = draw.below(64)?;
            let got = block_on(fan(&scope, items(count)?));
            assert!(got.is_ok(), "fan of {count}: {got:?}");
        }
        1 => {
            let fails = u32::try_from(draw.below(4)?)?;
            let got = block_on(flaky(&scope, fails));
            assert_eq!(
                got.ok(),
                Some(fails.saturating_add(1)),
                "flaky after {fails}"
            );
        }
        2 => {
            let tag = draw.below(1_000)?;
            let got = block_on(late(&scope, tag));
            assert!(
                matches!(got, Err(FlowError::TimedOut { .. })),
                "late {tag}: {got:?}"
            );
        }
        3 => {
            let count = draw.below(64)?;
            let got = block_on(stopped(&scope, items(count)?));
            assert_eq!(got.is_err(), count > 3, "stopped of {count}: {got:?}");
        }
        _ => {
            let got = block_on(refused(&scope, format!("refused {}", draw.below(1_000)?)));
            assert!(
                matches!(got, Err(FlowError::Failed { .. })),
                "refused: {got:?}"
            );
        }
    }
    Ok(())
}

/// Live bytes since `region` opened: allocated less freed. `stats_alloc`
/// already counts a `realloc`'s growth as allocated and its shrink as freed,
/// so its `bytes_reallocated` is not added again. Negative is a free of state
/// made before the region.
fn live(region: &Region<'_, System>) -> Result<isize, Box<dyn Error>> {
    let change = region.change();
    let allocated = isize::try_from(change.bytes_allocated)?;
    let freed = isize::try_from(change.bytes_deallocated)?;
    Ok(allocated.saturating_sub(freed))
}

#[test]
fn sim_heap_a_finished_run_returns_to_its_baseline() -> TestResult {
    let mut warm = Seeded::from_seed(0);
    for shape in 0..SHAPES.len() {
        for _ in 0..WARM {
            run(shape, &mut warm)?;
        }
    }

    // The instrument, both ways. A buffer that grows by `realloc` and is freed
    // holds nothing: a count that added the growth twice reads it as a leak.
    let grown = Region::new(GLOBAL);
    let held = {
        let mut buffer: Vec<u64> = Vec::new();
        for value in 0..10_000_u64 {
            buffer.push(value);
        }
        buffer.len()
    };
    let regrown = live(&grown)?;
    assert_eq!(
        regrown, 0,
        "a buffer of {held} grown by realloc and freed read {regrown} live bytes"
    );
    // An `Rc` cycle of 2 x 4 KiB must show, or a zero below means nothing.
    let sabotaged = Region::new(GLOBAL);
    let scope = Scope::root(Tenant::new("heap")?);
    block_on(cyclic(&scope, 4_096))?;
    let leaked = live(&sabotaged)?;
    assert!(
        leaked >= 8_192,
        "the census missed an Rc cycle of 8192 bytes: it saw {leaked}"
    );

    for (shape, name) in SHAPES.iter().enumerate() {
        let window = Region::new(GLOBAL);
        for seed in 0..RUNS {
            run(shape, &mut Seeded::from_seed(seed))?;
        }
        let moved = live(&window)?;
        assert!(
            moved <= 0,
            "{RUNS} finished {name} runs left {moved} live bytes"
        );
    }

    let window = Region::new(GLOBAL);
    for seed in 0..RUNS {
        let mut draw = Seeded::from_seed(seed);
        let shape = draw.index(SHAPES.len())?;
        run(shape, &mut draw)?;
    }
    let moved = live(&window)?;
    assert!(
        moved <= 0,
        "{RUNS} finished runs of every shape, mixed, left {moved} live bytes"
    );
    Ok(())
}
