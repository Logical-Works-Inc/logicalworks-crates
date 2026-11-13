//! Seeded refusals of the outside words (#783): every bound, grant and record
//! the words check before they reach outside, swept over seeds.
//!
//! `sim_outside.rs` sweeps the interleavings of faults that do reach an
//! upstream. This target sweeps the refusals that must happen *before* one: a
//! body bound, an attempt budget or a deadline outside its range, a malformed
//! URL, a missing grant, a send with nowhere durable to record its intent, a
//! resume under another payload, and model output past its token budget. The
//! seed draws the values, the tenant and the payload; every refusal is checked
//! by its typed arm and by the upstream or mailer it must not have touched.
//! Each sweep runs every seed twice and requires the same trace hash.
//!
//! | Test | Property it pins (source) |
//! |---|---|
//! | `sim_fetch_refuses_every_body_bound_outside_its_range` | a zero or past-ceiling body bound is refused typed and dials nothing (CWE-770) |
//! | `sim_fetch_word_refuses_an_attempt_count_outside_one_to_a_thousand` | an attempt budget outside `1..=1000` is refused through the word (CWE-770) |
//! | `sim_fetch_refuses_a_malformed_url_before_any_socket` | a URL that is not absolute http(s) is refused before a socket opens (RFC 3986 §4.3) |
//! | `sim_fetch_refuses_a_zero_deadline_whatever_its_budget` | a zero deadline is refused, not raced (CWE-400) |
//! | `sim_fetch_without_the_net_grant_is_blocked_and_dials_nothing` | every grant set lacking `bot.net` is blocked naming it (least privilege) |
//! | `sim_send_refuses_a_resumed_payload_under_another_intent` | a resume with other bytes never sends under the first intent (idempotency-key reuse) |
//! | `sim_send_after_a_refused_dispatch_dispatches_again_once` | a dispatch that landed nothing fails typed and a new run dispatches once more |
//! | `sim_send_with_nowhere_durable_is_refused_before_dispatch` | no store, no send: an intent that cannot outlive the process is refused |
//! | `sim_send_without_the_notify_grant_is_blocked_while_its_peer_sends` | a host lacking `bot.notify` is blocked while a granted host sends once |
//! | `sim_ask_refuses_a_zero_budget_and_decodes_whole_bytes_within_one` | a zero token budget is refused; output within its budget decodes whole |

#![cfg(all(
    feature = "script",
    feature = "ephemeral",
    feature = "time",
    feature = "net"
))]

use std::sync::Arc;
use std::time::Duration;

use lgwks_bot::proposal::{Shape, StubModel};
use lgwks_bot::script::{AskRequest, FetchOptions, FlowError, Scope};
use lgwks_bot::task::task;
use lgwks_bot::{Cap, GrantSet};

use crate::effect_policy::allowing;
use crate::outside_support::{Behavior, OutsideError, StubMailer, Upstream};
use crate::outside_support::{ask_hosted, fail, get, host_on, hosted, is_applied, mail};
use crate::outside_support::{outside_grants, run_of};
use crate::scratch::Scratch;
use crate::sim;

/// A test's result: an error fails it with the refusal behind it.
type TestResult = Result<(), OutsideError>;

/// Seeds per sweep; each runs twice.
const SEEDS: u64 = 16;

/// The largest body bound a fetch may declare, as `fetch` holds it.
const BODY_CEILING: usize = 64 * 1024 * 1024;

/// Run `body` for every seed twice and require the same, non-empty trace.
fn replayed(tag: &str, mut body: impl FnMut(&mut sim::Sim) -> TestResult) -> TestResult {
    for seed in 0..SEEDS {
        let mut first = sim::Sim::new(seed);
        body(&mut first)?;
        let mut second = sim::Sim::new(seed);
        body(&mut second)?;
        if first.trace.is_empty() || first.hash() != second.hash() {
            lgwks_std::trace::debug!(tag, seed, "replayed: the seed's trace moved");
            return fail("the same seed replays the same trace");
        }
    }
    Ok(())
}

/// A draw below `bound` as an index.
fn below(sim: &mut sim::Sim, bound: u32) -> Result<usize, OutsideError> {
    match usize::try_from(sim.rng().below(bound)) {
        Ok(drawn) => Ok(drawn),
        Err(error) => {
            lgwks_std::trace::debug!(error = %error, "below: a draw fits an index");
            fail("a draw fits an index")
        }
    }
}

/// The seeded tenant name.
fn tenant(sim: &mut sim::Sim) -> &'static str {
    if sim.rng().below(2) == 0 {
        "acme"
    } else {
        "zenith"
    }
}

#[test]
fn sim_fetch_refuses_every_body_bound_outside_its_range() -> TestResult {
    replayed("bound", |sim| {
        let past = below(sim, 4_096)?;
        let bound = match below(sim, 2)? {
            0 => 0,
            _ => BODY_CEILING.saturating_add(1).saturating_add(past),
        };
        let scratch = Scratch::new("outside-sim-bound")?;
        let host = host_on(
            scratch.path(),
            tenant(sim),
            GrantSet::empty().grant(Cap::net()),
        )?;
        let upstream = Upstream::serve(Behavior::Ok, 0, Duration::ZERO)?;
        let fetching = task(
            "bound",
            |scope: Scope, (url, bound): (String, usize)| async move {
                get(&scope, &url, &FetchOptions::default().max_body_bytes(bound)).await
            },
        )?;
        let report = lgwks_bot::block_on(host.run(&fetching, (upstream.url(), bound)));
        let hits = upstream.hits();
        upstream.join();
        let Some(&FlowError::InvalidBound { value, max, .. }) = report.error() else {
            return fail("an out-of-range body bound is refused as a bound");
        };
        if hits != 0 || max != u64::try_from(BODY_CEILING).map_err(std::io::Error::other)? {
            return fail("the refusal names the ceiling and dials nothing");
        }
        sim.record(&format!("bound:{value}"));
        Ok(())
    })
}

#[test]
fn sim_fetch_word_refuses_an_attempt_count_outside_one_to_a_thousand() -> TestResult {
    replayed("attempts", |sim| {
        let attempts = match below(sim, 3)? {
            0 => 0,
            1 => 1_001,
            _ => u32::MAX.saturating_sub(sim.rng().below(1_000)),
        };
        let scratch = Scratch::new("outside-sim-attempts")?;
        let host = host_on(scratch.path(), "acme", GrantSet::empty().grant(Cap::net()))?;
        let fetching = task("attempts", |scope: Scope, attempts: u32| async move {
            let options = FetchOptions::default().attempts(attempts);
            get(&scope, "http://127.0.0.1:9/", &options).await
        })?;
        let refused = lgwks_bot::block_on(host.run(&fetching, attempts));
        match refused.error() {
            Some(&FlowError::InvalidBound { value, .. }) if value == u64::from(attempts) => {
                sim.record(&format!("attempts:{attempts}"));
                Ok(())
            }
            other => {
                lgwks_std::trace::debug!(error = ?other, attempts, "the attempt budget was not refused");
                fail("an attempt budget outside 1..=1000 is refused naming its value")
            }
        }
    })
}

#[test]
fn sim_fetch_refuses_a_malformed_url_before_any_socket() -> TestResult {
    const MALFORMED: [&str; 6] = [
        "",
        "not a url",
        "ftp://127.0.0.1/",
        "http//x/",
        "://",
        "/a/b",
    ];
    replayed("url", |sim| {
        let index = below(sim, 6)?;
        let Some(url) = MALFORMED.get(index) else {
            return fail("the draw names a malformed URL");
        };
        let refused = hosted(
            tenant(sim),
            GrantSet::empty().grant(Cap::net()),
            |scope: Scope| async move {
                lgwks_bot::script::fetch(&scope, "url", url, &FetchOptions::default()).await
            },
        )?;
        if !matches!(refused, Err(FlowError::Failed { .. })) {
            return fail("a malformed URL fails typed");
        }
        sim.record(&format!("url:{index}"));
        Ok(())
    })
}

#[test]
fn sim_fetch_refuses_a_zero_deadline_whatever_its_budget() -> TestResult {
    replayed("deadline", |sim| {
        let budget =
            u32::try_from(below(sim, 3)?.saturating_add(1)).map_err(std::io::Error::other)?;
        let scratch = Scratch::new("outside-sim-deadline")?;
        let host = host_on(
            scratch.path(),
            "zenith",
            GrantSet::empty().grant(Cap::net()),
        )?;
        let upstream = Upstream::serve(Behavior::Ok, 0, Duration::ZERO)?;
        let url = upstream.url();
        let fetching = task("deadline", move |scope: Scope, url: String| async move {
            let zero = FetchOptions::default()
                .attempts(budget)
                .deadline(Duration::ZERO);
            lgwks_bot::script::fetch(&scope, "deadline", &url, &zero).await
        })?;
        let report = lgwks_bot::block_on(host.run(&fetching, url));
        let hits = upstream.hits();
        upstream.join();
        let refused = matches!(
            report.error(),
            Some(&FlowError::InvalidBound { value: 0, .. })
        );
        if !refused || hits != 0 {
            return fail("a zero deadline is refused before a dial");
        }
        sim.record(&format!("deadline:{budget}"));
        Ok(())
    })
}

#[test]
fn sim_fetch_without_the_net_grant_is_blocked_and_dials_nothing() -> TestResult {
    replayed("net", |sim| {
        let grants = match below(sim, 4)? {
            0 => GrantSet::empty(),
            1 => GrantSet::empty().grant(Cap::new("bot.model")),
            2 => GrantSet::empty().grant(Cap::notify()),
            _ => GrantSet::empty()
                .grant(Cap::new("bot.model"))
                .grant(Cap::notify()),
        };
        let scratch = Scratch::new("outside-sim-net")?;
        let host = host_on(scratch.path(), "acme", grants)?;
        let upstream = Upstream::serve(Behavior::Ok, 0, Duration::ZERO)?;
        let fetching = task("net", |scope: Scope, url: String| async move {
            get(&scope, &url, &FetchOptions::default()).await
        })?;
        let report = lgwks_bot::block_on(host.run(&fetching, upstream.url()));
        let hits = upstream.hits();
        upstream.join();
        let Some(blocked) = report.error() else {
            return fail("a fetch without bot.net is blocked");
        };
        if !matches!(*blocked, FlowError::Blocked { .. }) {
            return fail("a fetch without bot.net is blocked, not failed");
        }
        if !blocked.to_string().contains("bot.net") || hits != 0 {
            return fail("the block names bot.net and nothing dials");
        }
        sim.record("net:blocked");
        Ok(())
    })
}

#[test]
fn sim_send_refuses_a_resumed_payload_under_another_intent() -> TestResult {
    replayed("intent", |sim| {
        let salt = sim.rng().below(1_000_000);
        let first = format!("order-{salt}").into_bytes();
        let other = format!("order-{}", salt.saturating_add(1)).into_bytes();
        let scratch = Scratch::new("outside-sim-intent")?;
        let mailer = Arc::new(StubMailer::new());
        let sending = task(
            "send",
            |scope: Scope, (mailer, payload): (Arc<StubMailer>, Vec<u8>)| async move {
                mail(&scope, &mailer, payload).await
            },
        )?;
        let host = host_on(scratch.path(), "acme", outside_grants())?;
        mailer.fail_next_read();
        let crashed =
            lgwks_bot::block_on(allowing(host.run(&sending, (Arc::clone(&mailer), first))));
        let run = run_of(&crashed)?;
        let resumed = lgwks_bot::block_on(allowing(host.resume(
            run,
            &sending,
            (Arc::clone(&mailer), other),
        )));
        if !matches!(resumed.error(), Some(FlowError::Failed { .. })) || mailer.calls() != 0 {
            return fail("other bytes under a recorded intent are refused and never sent");
        }
        sim.record(&format!("intent:{salt}"));
        Ok(())
    })
}

#[test]
fn sim_send_after_a_refused_dispatch_dispatches_again_once() -> TestResult {
    replayed("refused", |sim| {
        let name = tenant(sim);
        let payload = format!("{name}-{}", sim.rng().below(1_000)).into_bytes();
        let scratch = Scratch::new("outside-sim-refused")?;
        let host = host_on(scratch.path(), name, outside_grants())?;
        let mailer = Arc::new(StubMailer::new());
        let sending = task(
            "send",
            |scope: Scope, (mailer, payload): (Arc<StubMailer>, Vec<u8>)| async move {
                mail(&scope, &mailer, payload).await
            },
        )?;
        mailer.refuse_next_dispatch();
        let refused = lgwks_bot::block_on(allowing(
            host.run(&sending, (Arc::clone(&mailer), payload.clone())),
        ));
        if refused.error().is_none() || mailer.calls() != 1 {
            return fail("a refused dispatch fails the run after one attempt");
        }
        let again =
            lgwks_bot::block_on(allowing(host.run(&sending, (Arc::clone(&mailer), payload))));
        match again.output() {
            Some(outcome) if is_applied(outcome) && mailer.calls() == 2 => {
                sim.record(&format!("refused:{name}"));
                Ok(())
            }
            _ => fail("a new run after a refusal dispatches once more and settles"),
        }
    })
}

#[test]
fn sim_send_with_nowhere_durable_is_refused_before_dispatch() -> TestResult {
    replayed("storeless", |sim| {
        let size = below(sim, 512)?.saturating_add(1);
        let payload = vec![0x5a_u8; size];
        let mailer = StubMailer::new();
        let (mailer_ref, payload_ref) = (&mailer, &payload);
        let refused = hosted(tenant(sim), outside_grants(), |scope: Scope| async move {
            lgwks_bot::script::send(&scope, "send", mailer_ref, payload_ref).await
        })?;
        if !matches!(refused, Err(FlowError::Store { .. })) {
            return fail("a send with no store is refused as a store failure");
        }
        if mailer.calls() != 0 {
            return fail("a storeless send dispatches nothing");
        }
        sim.record(&format!("storeless:{size}"));
        Ok(())
    })
}

#[test]
fn sim_send_without_the_notify_grant_is_blocked_while_its_peer_sends() -> TestResult {
    replayed("notify", |sim| {
        let scratch = Scratch::new("outside-sim-notify")?;
        let lacking = GrantSet::empty()
            .grant(Cap::net())
            .grant(Cap::new("bot.model"));
        let blocked_host = host_on(scratch.path(), "acme", lacking)?;
        let granted_host = host_on(scratch.path(), "zenith", outside_grants())?;
        let mailer = Arc::new(StubMailer::new());
        let sending = task(
            "send",
            |scope: Scope, (mailer, payload): (Arc<StubMailer>, Vec<u8>)| async move {
                mail(&scope, &mailer, payload).await
            },
        )?;
        let payload = sim.rng().below(1_000).to_be_bytes().to_vec();
        let blocked = lgwks_bot::block_on(allowing(
            blocked_host.run(&sending, (Arc::clone(&mailer), payload.clone())),
        ));
        let named = blocked.error().is_some_and(|error| {
            matches!(error, FlowError::Blocked { .. }) && error.to_string().contains("bot.notify")
        });
        let sent = lgwks_bot::block_on(allowing(
            granted_host.run(&sending, (Arc::clone(&mailer), payload)),
        ));
        let settled = sent.output().is_some_and(is_applied);
        if !named || !settled || mailer.calls() != 1 {
            return fail("only the granted tenant sends, once");
        }
        sim.record("notify:one");
        Ok(())
    })
}

#[test]
fn sim_ask_refuses_a_zero_budget_and_decodes_whole_bytes_within_one() -> TestResult {
    replayed("ask", |sim| {
        let mut model_seed = u64::from(sim.rng().below(1_000));
        let model = loop {
            let candidate = StubModel::from_seed(model_seed);
            if candidate.shape() == Shape::WellFormed {
                break candidate;
            }
            model_seed = model_seed.saturating_add(1);
        };
        let zero = AskRequest::default().max_tokens(0);
        let refused: Result<Vec<u8>, FlowError> = ask_hosted("zero", &model, &zero)?;
        if !matches!(refused, Err(FlowError::InvalidBound { value: 0, .. })) {
            return fail("a zero token budget is refused");
        }
        let budget = u32::try_from(model.output().len())
            .map_err(std::io::Error::other)?
            .saturating_add(1);
        let within = AskRequest::default().max_tokens(budget);
        let whole: Vec<u8> = ask_hosted("within", &model, &within)??;
        if whole != model.output() {
            return fail("output within its budget decodes whole");
        }
        sim.record(&format!("ask:{model_seed}:{}", whole.len()));
        Ok(())
    })
}
