//! The core words' rows: `flow`, the block words `each` to `let`, `run`, the
//! domain words `observe` and `act`, and the line words `give back` and `fail`.
//!
//! [`super::LEXICON`] lists them first, in this order, ahead of the structured
//! words in `vocab`.

use super::{
    Axis, Evidence, Guarantee, Kind, Position, Word, domain_word, form, refused, script_sim_in,
};

/// A guarantee proved by a simulation in `lgwks_bot`'s `sim_script.rs`, the
/// file whose flows the real macro expanded.
const fn script_sim(axis: Axis, claim: &'static str, test: &'static str) -> Guarantee {
    script_sim_in(SCRIPT_SIMS, axis, claim, test)
}

/// The simulation file whose flows the real macro expanded.
const SCRIPT_SIMS: &str = "crates/lgwks-bot/tests/it/sim_script.rs";

/// The simulation file for the machine-sized and typed fan-outs: `each`
/// without a bound and `FanOut`.
const FANOUT_SIMS: &str = "crates/lgwks-bot/tests/it/sim_script_fanout.rs";

/// The simulation file for the step words: `together`, `step`, `for`, `let`,
/// `run` and `fail`.
const STEP_SIMS: &str = "crates/lgwks-bot/tests/it/sim_script_steps.rs";

/// The simulation file whose flows call registry domains (#388).
const DOMAIN_SIMS: &str = "crates/lgwks-bot/tests/it/sim_script_domains.rs";

/// What `observe` and `act` share: their trace is the `BotSpec` chain's.
const SAME_AS_SPEC: Guarantee = domain_sim(
    Axis::Generalized,
    "the same polls and actions, in the same order, as the `BotSpec` chain over the same registry",
    "the_script_trace_is_the_spec_trace",
);

/// What `observe` and `act` share: an undeclared identifier refuses the flow
/// at its entry.
const UNKNOWN_DOMAIN_REFUSED: Guarantee = domain_sim(
    Axis::Decoupled,
    "an identifier the registry lacks refuses the flow before its first step, naming the registry and the nearest identifier",
    "an_unknown_domain_refuses_the_flow_before_any_step",
);

/// A guarantee proved by a simulation in `lgwks_bot`'s `sim_script_domains.rs`.
const fn domain_sim(axis: Axis, claim: &'static str, test: &'static str) -> Guarantee {
    Guarantee {
        axis,
        claim,
        evidence: Evidence::Sim {
            file: DOMAIN_SIMS,
            test,
        },
    }
}

/// `if` and `else` are one construct, so they share one example.
const IF_ELSE_EXAMPLE: &str = "flow route(code: u16) -> u8:\n    if code < 400:\n        give back 0\n    else if code < 500:\n        give back 1\n    else:\n        give back 2\n";

/// The `flow` row.
pub(super) const FLOW: Word = Word {
    kind: Kind::Flow,
    position: Position::Flow,
    primitive: "`Scope::enter`",
    forms: &[form(
        "`[pub] flow name(inputs) [-> Output]:`",
        "an `async fn` taking the tenant [`Scope`] first and returning `Result<Output, FlowError>`",
    )],
    refuses: &["a promised output the last line never produces"],
    guarantees: &[
        script_sim(
            Axis::MultiTenant,
            "every step keyed by tenant and path",
            "tenants_isolated",
        ),
        refused(
            Axis::Idiomatic,
            "the panicking calls refused by resolved path",
            "unwrap by path",
        ),
    ],
    example: "flow count(items: Vec<u8>) -> usize:\n    give back items.len()\n",
};

/// The `each` row.
pub(super) const EACH: Word = Word {
    kind: Kind::Each,
    position: Position::Block,
    primitive: "`each`",
    forms: &[
        form(
            "`each x in xs:`",
            "every item, as many at once as the machine sustains, results in input order, first failure stops the rest",
        ),
        form(
            "`each x in xs, at most (limit) at once:`",
            "the same, under a limit that comes from somewhere named (an upstream's quota)",
        ),
    ],
    refuses: &[
        "a concurrency bound typed as a number",
        "a bound outside `1..=65536`",
    ],
    guarantees: &[
        script_sim(Axis::Hyperscale, "bounded fan-out", "fan_bounded"),
        script_sim_in(
            FANOUT_SIMS,
            Axis::Generalized,
            "results in input order",
            "fan_machine_sized",
        ),
        script_sim(
            Axis::Hyperscale,
            "first failure cancels the rest",
            "fan_fail_fast",
        ),
    ],
    example: "flow sizes(pages: Vec<String>) -> Vec<usize>:\n    let sizes = each page in pages:\n        page.len()\n    give back sizes\n",
};

/// The `within` row.
pub(super) const WITHIN: Word = Word {
    kind: Kind::Within,
    position: Position::Block,
    primitive: "`within`",
    forms: &[form(
        "`within 2s:`",
        "the block, or `TimedOut` when the deadline passes",
    )],
    refuses: &["a zero duration", "a duration with no unit"],
    guarantees: &[Guarantee {
        axis: Axis::Hyperscale,
        claim: "a deadline, on the scope's clock",
        evidence: Evidence::Sim {
            file: "crates/lgwks-bot/tests/it/sim_clock_wiring.rs",
            test: "a_within_budget_is_refused_by_an_advance_with_no_real_wait",
        },
    }],
    example: "flow ping(host: &Host) -> u16:\n    let code = within 2s:\n        host.ping().await.or_retry()?\n    give back code\n",
};

/// The `retry` row.
pub(super) const RETRY: Word = Word {
    kind: Kind::Retry,
    position: Position::Block,
    primitive: "`retry`",
    forms: &[form(
        "`retry up to 3 times[, waiting 100ms]:`",
        "the block again while it fails transiently, same key each attempt, within the run's retry budget",
    )],
    refuses: &["attempts outside `1..=1000`", "a zero wait"],
    guarantees: &[
        script_sim(
            Axis::Ephemeral,
            "one idempotency key across attempts",
            "retry_one_key",
        ),
        script_sim(
            Axis::Hyperscale,
            "attempts within the run's retry budget",
            "retry_budget_holds",
        ),
    ],
    example: "flow fetch(site: &Site) -> Page:\n    let page = retry up to 3 times, waiting 200ms:\n        site.get(scope.key()).await.or_retry()?\n    give back page\n",
};

/// The `together` row.
pub(super) const TOGETHER: Word = Word {
    kind: Kind::Together,
    position: Position::Block,
    primitive: "`try_join!`",
    forms: &[form(
        "`together:`",
        "each line underneath concurrently; `let x = ..` lines bind their result",
    )],
    refuses: &[
        "anything before its `:`",
        "an indent under a line that opens no block",
    ],
    guarantees: &[
        script_sim_in(
            STEP_SIMS,
            Axis::Ephemeral,
            "every branch owned by this task",
            "together_owned",
        ),
        script_sim_in(
            STEP_SIMS,
            Axis::Hyperscale,
            "first failure cancels the rest",
            "together_fail_fast",
        ),
    ],
    example: "flow both(left: &Api, right: &Api):\n    together:\n        let up = run probe(left)\n        let down = run probe(right)\n",
};

/// The `step` row.
pub(super) const STEP: Word = Word {
    kind: Kind::Step,
    position: Position::Block,
    primitive: "`Scope::enter`",
    forms: &[form(
        "`step name:`",
        "a named scope: its own key and error location",
    )],
    refuses: &["a name that is not one word"],
    guarantees: &[
        script_sim_in(
            STEP_SIMS,
            Axis::Ephemeral,
            "its own step key",
            "step_own_key",
        ),
        script_sim_in(
            STEP_SIMS,
            Axis::Decoupled,
            "failures located at the step",
            "step_failure_located",
        ),
    ],
    example: "flow deploy(target: &Host):\n    step upload:\n        target.upload().await.or_fail()?\n",
};

/// The `for` row.
pub(super) const FOR: Word = Word {
    kind: Kind::For,
    position: Position::Block,
    primitive: "`Scope::item`",
    forms: &[form(
        "`for x in xs:`",
        "every item in turn, each in its own scope",
    )],
    refuses: &["a missing pattern or missing items"],
    guarantees: &[
        Guarantee {
            axis: Axis::Generalized,
            claim: "items in order, one at a time",
            evidence: Evidence::Sim {
                file: STEP_SIMS,
                test: "for_in_order",
            },
        },
        script_sim_in(
            STEP_SIMS,
            Axis::Ephemeral,
            "one scope per item",
            "for_scope_per_item",
        ),
    ],
    example: "flow notify(users: Vec<User>):\n    for user in users:\n        run email(user)\n",
};

/// The `if` row.
pub(super) const IF: Word = Word {
    kind: Kind::If,
    position: Position::Block,
    primitive: "none: Rust `if`",
    forms: &[form("`if cond:` / `else if cond:` / `else:`", "as written")],
    refuses: &["a missing condition"],
    guarantees: &[refused(
        Axis::Idiomatic,
        "a value only when an `else:` ends the chain",
        "if with no else as the value",
    )],
    example: IF_ELSE_EXAMPLE,
};

/// The `else` row.
pub(super) const ELSE: Word = Word {
    kind: Kind::Else,
    position: Position::Block,
    primitive: "none: Rust `else`",
    forms: &[],
    refuses: &[
        "an `else` not after an `if`",
        "an `else:` that is not the last branch",
    ],
    guarantees: &[refused(
        Axis::Idiomatic,
        "only ever continues an `if` chain",
        "else without if",
    )],
    example: IF_ELSE_EXAMPLE,
};

/// The `let` row.
pub(super) const LET: Word = Word {
    kind: Kind::Let,
    position: Position::Block,
    primitive: "the block's own",
    forms: &[form(
        "`let x = <block>:`",
        "the block's last line becomes `x`",
    )],
    refuses: &["a `let` with no `=` or nothing after it"],
    guarantees: &[script_sim_in(
        STEP_SIMS,
        Axis::Idiomatic,
        "the block's failure propagates before `x` is bound",
        "let_fails_before_binding",
    )],
    example: "flow total(xs: Vec<u8>) -> u8:\n    let sum = within 1s:\n        xs.iter().sum()\n    give back sum\n",
};

/// The `run` row.
pub(super) const RUN: Word = Word {
    kind: Kind::Run,
    position: Position::Inline,
    primitive: "the callee flow",
    forms: &[form(
        "`run other(args)`",
        "call another flow in this scope, await it, propagate its failure",
    )],
    refuses: &[],
    guarantees: &[script_sim_in(
        STEP_SIMS,
        Axis::MultiTenant,
        "the callee runs in this tenant's scope",
        "run_in_callers_tenant",
    )],
    example: "flow outer(site: &Site) -> usize:\n    let pages = run crawl(site)\n    give back pages\n",
};

/// The `observe` row.
pub(super) const OBSERVE: Word = domain_word(
    Kind::Observe,
    "`observe`",
    &[form(
        "`observe domain::id of target`",
        "poll the source the host's registry declares under `domain::id`, built from `target`, as a step of its own; bind it with a type, `let n: u16 = ..`",
    )],
    &["an identifier with no `of <target>`"],
    &[
        refused(
            Axis::Idiomatic,
            "an identifier with no `of <target>` is refused at the word, naming the form",
            "observe with no target",
        ),
        UNKNOWN_DOMAIN_REFUSED,
        SAME_AS_SPEC,
        domain_sim(
            Axis::MultiTenant,
            "the source's capabilities are checked against this run's grant at the step",
            "a_domain_short_of_authority_blocks_its_step",
        ),
    ],
    "flow open(repo: &str) -> u16:\n    let open: u16 = observe github::pr_status of repo\n    give back open\n",
);

/// The `act` row.
pub(super) const ACT: Word = domain_word(
    Kind::Act,
    "`act`",
    &[form(
        "`act domain::id on target with value`",
        "run the action the host's registry declares under `domain::id`, built from `target`, on `value`, as a step of its own",
    )],
    &["an identifier with no `on <target> with <value>`"],
    &[
        refused(
            Axis::Idiomatic,
            "an identifier with no `on <target> with <value>` is refused at the word, naming the form",
            "act with no value",
        ),
        domain_sim(
            Axis::Ephemeral,
            "the action runs once, under exactly the grants it declares; an external one settles a repeat by reading its receiver back",
            "an_external_action_runs_once_under_its_grant",
        ),
        SAME_AS_SPEC,
        UNKNOWN_DOMAIN_REFUSED,
    ],
    "flow page(count: u16):\n    if count > 2:\n        act notify::page on \"oncall\" with count\n",
);

/// The `give back` row.
pub(super) const GIVE_BACK: Word = Word {
    kind: Kind::GiveBack,
    position: Position::Line,
    primitive: "none: `return Ok`",
    forms: &[form("`give back value`", "return from the flow")],
    refuses: &["`give back` inside a block that has its own value"],
    guarantees: &[refused(
        Axis::Idiomatic,
        "leaves the flow, never a nested block",
        "give back from a nested block",
    )],
    example: "flow one() -> u8:\n    give back 1\n",
};

/// The `fail` row.
pub(super) const FAIL: Word = Word {
    kind: Kind::Fail,
    position: Position::Line,
    primitive: "`FlowError::failed` / `FlowError::transient`",
    forms: &[form(
        "`fail with reason` / `fail transiently with reason`",
        "stop with a permanent / retryable failure",
    )],
    refuses: &["a reason missing after `with`"],
    guarantees: &[script_sim_in(
        STEP_SIMS,
        Axis::Decoupled,
        "a typed failure the caller can classify",
        "fail_is_classified",
    )],
    example: "flow check(ready: bool):\n    if !ready:\n        fail with \"not ready\"\n",
};
