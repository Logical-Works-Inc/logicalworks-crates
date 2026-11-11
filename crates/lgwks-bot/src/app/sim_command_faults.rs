//! The fools who fill a queue, stop a host or pull its plug under a command.
//!
//! Source: #898's "a waiter never hangs on a refused mailbox" and "a rebuild never
//! re-runs effects", with the harness in `sim_command`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, sync_channel};
use std::sync::{Arc, Mutex, OnceLock};

use super::command::{CommandPool, CommandRequest, Feedback};
use super::sim_command::{
    Book, Gate, LONG, Outcome, Service, book_app, book_of, granted, roomy, run_command, said, send,
};
use super::{CommandLimits, Effect, EffectResult, Method, OnInput, Principal, ReplyTag};
use crate::cap::Cap;
use crate::cell::{DynHost, ReplySlot};

/// Catches one tenant's queue spending another's room, a ceiling that never binds,
/// and a permit that never comes back.
#[test]
fn sim_command_neighbour_a_tenant_at_its_limit_leaves_the_next_served() -> Outcome {
    let gate = Arc::new(Gate::default());
    let held = Arc::clone(&gate);
    let service = Service(move |tenant: &str, _: Option<&Principal>, body: &[u8]| {
        if tenant.starts_with("noisy") {
            held.hold();
        }
        Some(body.to_vec())
    });
    let host = granted(service, CommandLimits::new(8, 8, 2, LONG))?;
    let path = (Method::POST, "/do");
    let mut waiting = Vec::new();
    for _ in 0..2 {
        waiting.push(send(&host, "noisy-0", path, "x", None)?);
    }
    gate.arrived(2)?;
    assert_eq!(
        run_command(&host, "noisy-0", "over")?.status(),
        503,
        "past its own ceiling, while nothing else is full"
    );
    assert_eq!(
        run_command(&host, "quiet", "served")?,
        said(200, b"served"),
        "the neighbour is untouched"
    );
    for tenant in ["noisy-1", "noisy-2", "noisy-3"] {
        for _ in 0..2 {
            waiting.push(send(&host, tenant, path, "x", None)?);
        }
    }
    gate.arrived(8)?;
    assert_eq!(
        run_command(&host, "quiet", "past")?.status(),
        503,
        "the host's own ceiling binds a tenant holding none"
    );
    gate.open()?;
    for reply in waiting {
        assert_eq!(reply.wait_blocking()?, said(200, b"x"));
    }
    for tenant in ["noisy-0", "noisy-1", "noisy-2", "noisy-3", "quiet"] {
        assert_eq!(
            run_command(&host, tenant, "again")?,
            said(200, b"again"),
            "{tenant} recovers: every permit came back"
        );
    }
    host.close();
    Ok(())
}

/// Catches a rebuild that runs a command again: a double write on every panic.
#[test]
fn sim_command_pulled_plug_a_rebuild_never_runs_a_command_again() -> Outcome {
    let seen = Arc::new(AtomicU64::new(0));
    let counted = Arc::clone(&seen);
    let service = Service(move |_: &str, _: Option<&Principal>, body: &[u8]| {
        counted.fetch_add(1, Ordering::AcqRel);
        Some(body.to_vec())
    });
    let host = book_app(service, roomy())
        .grant(Cap::new("app.command"))
        .route(Method::POST, "/boom", |_: &mut Book, text: String| {
            assert!(text != "boom", "the handler panics on this body");
            vec![Effect::reply(200, Vec::new())]
        })
        .start(2, 64)?;
    let tenant = "plug";
    for round in 0..3 {
        run_command(&host, tenant, &round.to_string())?;
    }
    assert_eq!(seen.load(Ordering::Acquire), 3);
    let reply = send(&host, tenant, (Method::POST, "/boom"), "boom", None)?;
    assert_eq!(reply.wait_blocking()?.status(), 500);
    assert_eq!(host.counts().panicked, 1, "the rebuild ran");
    assert_eq!(
        seen.load(Ordering::Acquire),
        3,
        "the replayed routes returned commands, and none ran"
    );
    assert_eq!(
        book_of(&host, tenant)?,
        "3,0,0",
        "the state came back whole"
    );
    host.close();
    Ok(())
}

/// Catches an answer dropped with a debug line while its client waits: the gap #898
/// names first.
#[test]
fn sim_command_pulled_plug_a_refused_answer_still_answers_its_waiter() -> Outcome {
    let service_gate = Arc::new(Gate::default());
    let held = Arc::clone(&service_gate);
    let service = Service(move |_: &str, _: Option<&Principal>, body: &[u8]| {
        held.hold();
        Some(body.to_vec())
    });
    let worker_gate = Arc::new(Gate::default());
    let blocked = Arc::clone(&worker_gate);
    let host = book_app(service, roomy())
        .grant(Cap::new("app.command"))
        .route(Method::POST, "/stall", move |_: &mut Book, text: String| {
            if text == "stall" {
                blocked.hold();
            }
            vec![Effect::reply(200, Vec::new())]
        })
        .start(1, 2)?;
    let tenant = "plug";
    let command = send(&host, tenant, (Method::POST, "/do"), "x", None)?;
    service_gate.arrived(1)?;
    let stalled = send(&host, tenant, (Method::POST, "/stall"), "stall", None)?;
    worker_gate.arrived(1)?;
    let queued = send(&host, tenant, (Method::POST, "/stall"), "go", None)?;
    service_gate.open()?;
    let answer = command.wait_blocking()?;
    assert_eq!(
        answer.status(),
        503,
        "the mailbox is full, and the client hears so"
    );
    assert!(
        String::from_utf8(answer.into_body())?.contains("retry"),
        "it is told what to do about it"
    );
    worker_gate.open()?;
    assert_eq!(stalled.wait_blocking()?.status(), 200);
    assert_eq!(queued.wait_blocking()?.status(), 200);
    host.close();
    Ok(())
}

/// Catches an asker lost on a chain, or two identities sharing one.
#[test]
fn sim_command_neighbour_the_asker_stays_with_its_chain_and_nobody_else() -> Outcome {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&asked);
    let service = Service(move |tenant: &str, who: Option<&Principal>, body: &[u8]| {
        let name = who.map(|asker| asker.as_str().to_owned());
        if let Ok(mut held) = seen.lock() {
            held.push((tenant.to_owned(), name, body.to_vec()));
        }
        Some(Vec::new())
    });
    let host = book_app(service, roomy())
        .grant(Cap::new("app.command"))
        .route(Method::POST, "/chain", |_: &mut Book, _: String| {
            vec![Effect::Command {
                body: b"one".as_slice().into(),
                reply_to: ReplyTag::new("next"),
            }]
        })
        .on(ReplyTag::new("next"), |_: &mut Book, _: &OnInput| {
            vec![Effect::Command {
                body: b"two".as_slice().into(),
                reply_to: ReplyTag::new("done"),
            }]
        })
        .start(2, 64)?;
    for (tenant, who) in [("a", Some("alice")), ("a", Some("bob")), ("b", None)] {
        let reply = send(&host, tenant, (Method::POST, "/chain"), "x", who)?;
        assert_eq!(
            reply.wait_blocking()?.status(),
            200,
            "the chain keeps the slot"
        );
    }
    let held = asked.lock().map_err(|_| "the list broke")?.clone();
    let named = |tenant: &str, who: Option<&str>, body: &[u8]| {
        (tenant.to_owned(), who.map(str::to_owned), body.to_vec())
    };
    assert_eq!(
        held,
        vec![
            named("a", Some("alice"), b"one"),
            named("a", Some("alice"), b"two"),
            named("a", Some("bob"), b"one"),
            named("a", Some("bob"), b"two"),
            named("b", None, b"one"),
            named("b", None, b"two"),
        ]
    );
    host.close();
    Ok(())
}

/// Every level in flight at once past the ceilings: all answered, each 200 or 503,
/// the service ran exactly the 200s, and the ceilings recovered.
#[test]
fn sim_command_double_clicker_every_waiter_is_answered_at_every_level() -> Outcome {
    for level in [100_usize, 1_000, 10_000, 100_000] {
        let seen = Arc::new(AtomicU64::new(0));
        let counted = Arc::clone(&seen);
        let service = Service(move |_: &str, _: Option<&Principal>, body: &[u8]| {
            counted.fetch_add(1, Ordering::AcqRel);
            Some(body.to_vec())
        });
        let host = granted(service, CommandLimits::new(8, 512, 64, LONG))?;
        let path = (Method::POST, "/do");
        let mut waiting = Vec::with_capacity(level);
        for at in 0..level {
            let tenant = format!("clicker-{}", at % 16);
            waiting.push(send(&host, &tenant, path, "x", None)?);
        }
        let mut served = 0_u64;
        for reply in waiting {
            let answer = reply.wait_blocking()?;
            match answer.status() {
                200 => served = served.saturating_add(1),
                status => assert_eq!(status, 503, "level {level}"),
            }
        }
        assert_eq!(
            seen.load(Ordering::Acquire),
            served,
            "level {level}: what ran and what answered 200 are one set"
        );
        assert_eq!(
            run_command(&host, "clicker-0", "after")?,
            said(200, b"after"),
            "level {level}: the ceilings came back"
        );
        host.close();
    }
    Ok(())
}

/// Hears every answer a pool feeds back, for a sim of the pool alone.
struct Tally(SyncSender<bool>);

impl Feedback for Tally {
    fn feed(
        &self,
        _tenant: &str,
        _asker: Option<Principal>,
        _reply_to: ReplyTag,
        result: EffectResult,
        slot: Option<Arc<ReplySlot>>,
    ) {
        if let Some(waiting) = slot {
            waiting.complete(said(200, b"fed"));
        }
        if self
            .0
            .try_send(matches!(result, EffectResult::Value(_)))
            .is_err()
        {
            lgwks_std::trace::debug!("sim: the tally is full");
        }
    }
}

/// A command for the pool alone, held open by `slot`.
fn queued_for<'tenant>(tenant: &'tenant str, slot: &Arc<ReplySlot>) -> CommandRequest<'tenant> {
    CommandRequest {
        tenant,
        asker: None,
        body: Box::default(),
        reply_to: ReplyTag::new("done"),
        slot: Some(Arc::clone(slot)),
    }
}

/// Catches a queued command started after close, and a waiter close leaves hanging.
#[test]
fn sim_command_pulled_plug_a_closing_pool_answers_what_it_will_not_run() -> Outcome {
    let gate = Arc::new(Gate::default());
    let held = Arc::clone(&gate);
    let service = Service(move |_: &str, _: Option<&Principal>, _: &[u8]| {
        held.hold();
        Some(Vec::new())
    });
    let (sent, fed) = sync_channel(16);
    let pool = CommandPool::start(
        &CommandLimits::new(1, 8, 8, LONG),
        Arc::new(service),
        Arc::new(Tally(sent)),
    )?;
    let mut waiting = Vec::new();
    for _ in 0..4 {
        let slot = Arc::new(ReplySlot::new());
        assert!(
            pool.hand(queued_for("plug", &slot)).is_ok(),
            "within the ceilings"
        );
        waiting.push(slot);
    }
    gate.arrived(1)?;
    std::thread::scope(|scope| -> Outcome {
        scope.spawn(|| pool.close());
        while !pool.is_closing() {
            std::thread::yield_now();
        }
        gate.open()
    })?;
    let mut statuses = Vec::new();
    for slot in waiting {
        statuses.push(slot.wait_blocking()?.status());
    }
    assert_eq!(
        statuses,
        vec![200, 503, 503, 503],
        "one ran, three were told"
    );
    assert_eq!(
        fed.try_iter().count(),
        1,
        "and nothing else reached the service"
    );
    pool.close();
    Ok(())
}

/// Closes its own pool from the thread that served the command, as a host's last
/// handle dropped on that thread would.
#[derive(Default)]
struct ClosesItsPool(OnceLock<Arc<CommandPool>>);

impl Feedback for ClosesItsPool {
    fn feed(
        &self,
        _tenant: &str,
        _asker: Option<Principal>,
        _reply_to: ReplyTag,
        _result: EffectResult,
        slot: Option<Arc<ReplySlot>>,
    ) {
        if let Some(pool) = self.0.get() {
            pool.close();
        }
        if let Some(waiting) = slot {
            waiting.complete(said(200, b"closed"));
        }
    }
}

/// Catches a close on a pool's own thread joining itself, which hangs forever.
#[test]
fn sim_command_pulled_plug_a_close_on_a_pools_own_thread_joins_the_others() -> Outcome {
    let service = Service(|_: &str, _: Option<&Principal>, _: &[u8]| Some(Vec::new()));
    let feed = Arc::new(ClosesItsPool::default());
    let handed: Arc<dyn Feedback> = Arc::<ClosesItsPool>::clone(&feed);
    let limits = CommandLimits::new(4, 8, 8, LONG);
    let pool = Arc::new(CommandPool::start(&limits, Arc::new(service), handed)?);
    assert!(feed.0.set(Arc::clone(&pool)).is_ok());
    let slot = Arc::new(ReplySlot::new());
    assert!(pool.hand(queued_for("plug", &slot)).is_ok());
    assert_eq!(slot.wait_blocking()?, said(200, b"closed"));
    assert!(pool.is_closing());
    pool.close();
    Ok(())
}

/// Catches a closed pool taking work and losing its waiter, and a command too many
/// that comes back to its caller with its waiter untouched.
#[test]
fn sim_command_wrong_order_a_command_after_close_or_past_the_ceiling_is_answered() -> Outcome {
    let (sent, _fed) = sync_channel(4);
    let gate = Arc::new(Gate::default());
    let held = Arc::clone(&gate);
    let service = Service(move |_: &str, _: Option<&Principal>, _: &[u8]| {
        held.hold();
        Some(Vec::new())
    });
    let limits = CommandLimits::new(1, 1, 1, LONG);
    let pool = CommandPool::start(&limits, Arc::new(service), Arc::new(Tally(sent)))?;
    let first = Arc::new(ReplySlot::new());
    assert!(pool.hand(queued_for("late", &first)).is_ok());
    gate.arrived(1)?;
    let second = Arc::new(ReplySlot::new());
    let back = pool.hand(queued_for("late", &second));
    assert!(back.is_err(), "the ceiling hands it back");
    assert!(
        !second.is_settled(),
        "the caller decides what its waiter hears"
    );
    gate.open()?;
    assert_eq!(first.wait_blocking()?, said(200, b"fed"));
    pool.close();
    let third = Arc::new(ReplySlot::new());
    while pool.hand(queued_for("late", &third)).is_err() {
        std::thread::yield_now();
    }
    assert_eq!(
        third.wait_blocking()?.status(),
        503,
        "a closed pool's ticket answers for itself"
    );
    Ok(())
}
