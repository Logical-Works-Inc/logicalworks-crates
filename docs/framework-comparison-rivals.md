# Framework comparison: What each one does

Part of [Framework comparison](framework-comparison.md).

## What each one does

### `spider-rs`: crawler, Rust

Concurrency-first, streaming-first. Pages stream out through a **bounded**
subscription channel as they arrive, and do not pile up. Two properties matter
here:

- **Tiered escalation.** HTTP first, rendering JavaScript *only* on pages that
  need it. Its managed mode "routes through proxies first and escalates to the
  unblocker only on pages that fight back, so you pay for bypass exactly when
  it's needed and never when it isn't" (`spider-rs` `README.md` at tag `v2.52.2`).
  The tail of the earlier quote here ("only where it's needed") is the current
  `main` wording, not the release's.
- **Declared limits.** "The crawler discovers links, respects boundaries, and
  stops on its own" (same README at `v2.52.2`; the earlier "stays inside the
  limits you set" is `main`'s wording again). The subscription channel is bounded
  by a capacity the caller passes to `website.subscribe(<n>)`.

This project's resolver specification reached the same architecture on its own:
a deterministic scorer by default, and the expensive component only on a low
margin. Two unrelated systems in different domains converged on tiered
escalation because the cost argument is the same in both. That is evidence, not
analogy.

### Discord bot frameworks (`discord.py`, `serenity`): event-driven bots

Five layers: a client owning connection lifecycle; a **gateway** WebSocket for
push events; an **HTTP** client for pull requests with rate-limit enforcement; a
**connection state** object owning the cache and dispatch; and an auto-sharding
client that partitions the event space to scale.

Three ideas worth naming:

- **`intents`**: an enumerated, declared subscription. The bot names the event
  buckets it wants and the gateway delivers those (`discord.py` `2.7.1`,
  `intents.html`). *Unsourced:* "a bot that receives an undeclared event errors".
  The guide describes subscription buckets and privileged intents, and states no
  such error. This is still the closest thing to a capability model in the
  survey.
- **Dual transport.** Push for events, pull for commands, with different
  guarantees on each.
- **Protocol-level resumption.** `IDENTIFY`/`RESUME` with a heartbeat, so a
  dropped connection continues a session and does not restart it.

### Stagehand: browser agent SDK

Protocol-first: a browser extension runs next to the browser as the in-browser
runtime, and the TypeScript, Python and Go bindings are generated from one schema
for cross-language parity (`browserbase/stagehand` `README.md`; current docs are
`v4`, latest npm release tag `@browserbasehq/stagehand@3.7.3`). *Unsourced at this
version:* "SDKs speak to it over CDP with ... JSON-RPC". The pinned README shows
one browser driver and one generated surface, and names neither CDP nor JSON-RPC.

This workspace applies the same discipline: one schema, generated surfaces, and
the runtime behind a protocol boundary. Its `observe` primitive returns candidates
(selectors) and the caller selects among them: the enumerate-then-select shape
the resolver needs.

### Crawlee and Scrapy: scraping frameworks

A crawler orchestrates storages, request handlers, retries, concurrency and
coordination. Crawlee (JS `3.18.2` / Python) states exactly these: automatic
scaling "with available system resources", integrated "proxy rotation and session
management", and configurable, bounded retries (`README.md`). The dedup-by-key
queue and the rotating session pool are Crawlee features; they are **not** Scrapy
core. Scrapy's `AutoThrottle` scales against observed server latency, not local
CPU or memory, and a rotating-proxy session pool is a third-party plugin. Reading
that sentence as Scrapy's design is *unsourced*.

Two details worth taking:

- Split crawler families by **observation surface** (HTTP against browser), not
  by task. The AI variant is a subclass, not a fork.
- *Unsourced:* an LLM crawler that "extracts structured data into a **validated
  Pydantic model**". Neither the pinned Crawlee (JS) nor Crawlee for Python
  `README.md` names an LLM crawler or Pydantic extraction. The rule under it
  (model output into a validated type, never free text) matches
  `forge-md-runtime`'s advisory-candidate split, reached on its own.

### WalkMe and UiPath, by contrast

Neither publishes an engineering framework, so everything below is an assessment
with no primary source to cite. How they sit on the four axes:

- **Observation surface:** live form state, in-page.
- **Concurrency:** none. The rules engine shares the application's own JavaScript
  thread, so every check competes with the application for the frame budget.
- **Decision source:** authored rule.
- **Authority:** ambient, unbounded.

**This answers "WalkMe is too slow."** Models do not make it slow. It lacks both
bound mechanisms the crawlers treat as foundational: declared limits with
self-termination, and tiered escalation. And it runs on the application's own
thread, not beside it. The crawlers solved the same problem and called it
architecture.
