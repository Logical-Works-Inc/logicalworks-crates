# Requirements C. Call shapes

Part of [the product requirements](../../REQUIREMENTS.md), which state how a requirement changes.

## C. Call shapes

### R6. A developer strings code together and is done.

> The ordinary non-AI task MUST need no model client, no agent persona and no
> remote service.

**Invariants.** An in-process task's correctness depends on no network and no
model. Configuration of a host is explicit, reusable and inspectable, and its
cost counts against usability.

**Callers.** A developer.

**Falsifier.** An in-process task that imports a model client, or reaches a
network, to complete.

**Status.** `met` for the design, argued. `DX-01` states it. Usability of the
host configuration is **unmeasured**, and `docs/async-sdk-shape.md` warns that
a tiny call site hiding hundreds of orchestration lines fails this contract as
surely as a verbose one.

### R7. An AI that needs to poll calls the bot.

> Polling, change detection and the reaction that follows MUST be substrate.
> An agent MUST call them rather than reimplement them.

**Invariants.** Observation is a callable surface. Change detection is the
bot's. A source that holds still fires nothing, and the bot names which sources
moved. An event identity and a content value are distinguishable, so a
returning event retires and an unchanged source under a new run does not.

**Callers.** An AI agent that needs polling.

**Falsifier.** An agent path that maintains its own previous-value cache and
edge detector beside the bot.

**Status.** `partial`, measured. Change detection and `revisions()` exist and
are tested in `tests/ecs_tick/`; the identity split is tested in
`tests/it/durable_dispatch.rs`. A calling convention for an external agent is
**unmet**.

### R8. An AI that needs risky code checked calls the bot to check it in process.

> Untrusted code MUST be analysed in process over its structure. It MUST NOT be
> executed in order to be inspected.

**Invariants.** The check path never executes the subject code, in this process
or another. The verdict is typed, never free text. A check that cannot decide
returns undecidable rather than a guess.

**Callers.** An AI reviewing or gating code.

**Falsifier.** A hostile corpus: an infinite loop, a panic, an immediate
process exit, a filesystem delete. If any of them runs, in any process, to
produce the verdict, R8 is refuted.

**Status.** `partial`, measured. `lgwks_ast` is the substrate (28 declared grammars),
and `lgwks_bot::inspect` (INV-BOT-21, #150) parses the subject and never executes it,
with typed verdicts (`tests/it/inspect_wiring.rs`). This is the first door, and the
hostile corpus of the falsifier is the row to read against it.
