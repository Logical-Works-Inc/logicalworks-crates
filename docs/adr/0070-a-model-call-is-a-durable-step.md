# ADR 0070: a model call is a durable step

Status: **accepted**, 2026-11-20.

Stakeholders: the Director; an agent harness that resumes a run after a crash; a tenant who pays per token.
Concerns: `ask_in` called the model on every resume. A crash after the answer spent the tokens again, and the
resume could act on another answer than the one the run already acted on.
Requirements: INV-BOT-198, INV-BOT-184, INV-BOT-182.
Viewpoint: a flow's `ask` step, as a resume reads it.
Elements and relations: `script::ask`, `script::run_store::remember_at`, `task::Drift::Input`.

## Context

Temporal keeps an activity's result in the workflow history, Restate keeps `ctx.run` output in its journal, and
DBOS keeps step outputs in Postgres. Each replays a non-deterministic call from the record. `remember` gave a flow
that property by hand; `ask`, the costliest call a flow makes, did not have it. The lexicon guaranteed nothing about
replay. The measured failure: four crashes after one `ask` called the model five times
(`sim_a_resumed_ask_never_calls_the_model_twice`, before this change).

## Decision

1. **`ask_in` runs the call through `remember_at`.** Under a run store the step records `(digest, whole output)`.
   With no store, the call runs as before and claims nothing.
2. **The digest covers the prompt and the token budget, framed under `lgwks.bot.ask.v1`.** The deadline is left out:
   it bounds the wait, never the answer.
3. **A recorded digest that differs from the live one is `FlowError::Incompatible` with `Drift::Input`**, raised
   before the model is called.
4. **A truncated output is refused inside the step**, so it is never recorded and a resume asks again.
5. **The bytes are recorded, not the decoded value.** The decode runs on every replay.

## Alternatives rejected

- **Record the decoded `T`.** Every `AskDecode` type would owe an archive bound, and a decoder fix would replay a
  stale decode. Reverse this if a measured decode cost on replay exceeds the model call it replaces.
- **Leave replay to a `remember` block the author writes.** The author forgets it, and the lexicon is what an agent
  writing `script!` reads. A guarantee the word does not keep is a guarantee no flow has.

## Consequences

- A resumed run pays one model call per `ask` step, whatever crashed after it.
- A flow whose prompt depends on the clock or a random draw is refused on resume. That is the drift the run would
  otherwise act on unseen.
- An `ask` under a run store writes one record per call, as `remember` does.

## Correspondence

Refines ADR 0016 (the run store) and #783 (the outside words). The model-usage budget is the next step on this one.

## Verification

`crates/lgwks-bot/tests/it/sim_ask_replay.rs`: `sim_a_resumed_ask_never_calls_the_model_twice`,
`sim_a_resume_under_another_prompt_is_refused_before_the_model`, `sim_a_cut_answer_is_never_remembered` and
`sim_without_a_store_every_run_calls_the_model`.
