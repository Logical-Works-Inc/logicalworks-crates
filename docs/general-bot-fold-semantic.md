# Folding the recorder: the semantic tier and what already exists

Pages of this guide: [scope and finding](general-bot-fold.md) · [what is new](general-bot-fold-new.md) · [recorder, settlement, language](general-bot-fold-language.md) · [semantic tier and mapping](general-bot-fold-semantic.md) · [reaching a page](general-bot-fold-reach.md) · [evasion and injection](general-bot-fold-evasion.md) · [sequence 1 to 5](general-bot-fold-sequence.md) · [sequence 6 to 14](general-bot-fold-frontier.md)

## 3.6 The semantic tier: **landed in this change**

§3.5 states a limit: no string distance derives `"the usual"` from `"Repeat last
order"`. That is true, and it is what a lexicon is for. This section is the layer
that answers it, and the one place a model belongs in this design.

`semantic.rs` adds `Embedder`: a seam, not a dependency. The crate still carries
no model, no tensor library and no tokenizer. The weights are not source, and
compiling them in would make every consumer pay for them. A consumer with a model
supplies it. `lgwks_std::similarity` gains `Cosine` beside the other metrics, so
the comparison is not reinvented here.

Three properties make it safe to add to a running system, not a second opinion
that can disagree with the first.

**It cannot change a verdict the lexicon reached.** The tier is consulted only
when the lexicon returns `Absent`. Every other lexicon verdict, `Ambiguous`
included, is returned verbatim. A model is a tie-breaker of last resort, not an
override: if it could outvote an exact match, it eventually would, and an exact
match is the one verdict that cannot be wrong. So enabling this tier changes no
behaviour for any utterance that already resolved.

**Its contribution is named.** `MatchTier` gains `Semantic`, so a resolution says
whether a model produced it. `EmbedderIdentity` carries the model's name, a
content digest of its weights, and its vector width. The digest makes two runs
comparable: the same name with other weights is another model. The embedder
supplies it, because only the code that loaded the weights knows them. A verdict
a model produced is declarable only if the model is named. So the identity
travels on the verdict and into the decision receipt, not into a resolver
accessor that a caller holding a `Box<dyn Resolver>` cannot reach.

**It reports and does not guess.** A failed embedder, or one that contradicts its
own declared width, produces `Resolution::Degraded` (§6 item 5), not `Absent`.
Re-asking is right for both, but the record tells them apart, so an operator can
tell an unclear person from a dependency that is down.

**It says what it decided and on whose authority.** Each accepted answer is
recorded as a versioned `DecisionReceipt` through a required, fallible
`Journal::record_decision`. It holds the session and the flow revision, the node
and the identity of the *ordered* option set, the verdict with its tier and
score, the policy in force, and the model when one was consulted. The transcript
renders the conversation; the receipt is the audit record. A sink that refuses
the write aborts the answer, so no cursor moves with nothing to say why.

Three things are absent on purpose:

- The policy constants (`0.72`, `0.05`) are **declared, not fitted**. They are
  constructor arguments, because the right values depend on the model. The
  margin is small on purpose: two phrasings of one option can score within
  hundredths of each other.
- There is **no cache**. An option list is authored and short. A cache is a second
  piece of state that needs a bound, an eviction policy and a test for both, and
  its seam would be `Embedder`.
- There is **no logging**. The cause travels in the verdict, and the verdict
  reaches the decision receipt, the run's record. A log line would be a second
  unmanaged copy of the same fact.

## 4. What maps onto something that already exists

| Described subsystem | This workspace | Note |
|---|---|---|
| REST gateway with JWT and scopes | `Cap` / `GrantSet`, proof-carrying | Different, not strictly stronger. A scope checked once at a gateway is ambient for the rest of the request; `Auth` is re-proved on every call, against the grant set the bot was built with. That set is a **snapshot** (`lib.rs`, `README.md`): `GrantSet` has no revoke operation, so narrowing a running bot needs a mechanism this crate does not have |
| Socket.io stateful multiplexer | `rt::sync` bounded channels | Bounded by construction; `unbounded_channel` is forbidden workspace-wide |
| Browser pool manager, lease/release | `rt::supervise`, `rt::cancel` | A supervised bounded pool is exactly what these already are |
| Playwright Chromium behind a WebSocket | the CDP allow-list in `docs/security-posture.md` | The allow-list is the contract; the driver is an implementation of it |
| Workflow queue (`SKIP LOCKED`, `graphile-worker`) | `rt::task` (`JoinSet` + `CancellationToken`) for in-process; a storefront edge for durable | The durable half is a database client, which is a `lgwks_deps` optional feature, not a new surface |
| Cron / schedule worker | `Bot::tick()` under a `Time<Virtual>` clock | Scheduling is already the tick; §7 of `bot-on-ecs.md` states the bors rule: derive queue membership, never store it |
| Artifact offload to object storage | `domain::fs` plus a storefront edge | The bot hands the payload to a sink; it does not own the sink |
| Webhooks, Sheets, Airtable dispatch | `domain::net`, `domain::notify` | Each is an HTTP+JSON domain impl, not a new architecture |
| The replayer UI (React, rrweb) | **not the bot's job** | The bot is a library. `docs/bot-on-ecs.md` §6 already concluded a console belongs on `gpui` and that the bot must not grow a UI |
| MCP stdio JSON-RPC server | an adapter over the four verbs | An adapter, not a fifth verb. The four-verb contract is `INV-BOT-FOUR-VERBS` and is not negotiable |
| Autonomous agent: page analyzer, indexed handles, screenshot, strict JSON | `domain::chat` + `domain::eval` | See §5 for the two constraints that survive the fold |
