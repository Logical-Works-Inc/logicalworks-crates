# Framework comparison

Continued in: [What each one does](framework-comparison-rivals.md), [What is worth taking](framework-comparison-worth-taking.md).

Status: reference. Why the field looks the same, what is different, and
what to take. Companion to [`frontier.md`](frontier.md).

## Primary sources

Every external project named in this page was re-checked on **2026-10-02**
against the project's own repository, release notes or documentation, pinned to
the version shown. A claim the project itself does not support is marked
*unsourced* rather than deleted. Nothing here is a benchmark or measurement of
this crate.

| Project | Version at 2026-10-02 | Primary source |
|---|---|---|
| `spider-rs` | `v2.52.2` | repo `README.md` + release tag |
| `discord.py` | `2.7.1` | PyPI metadata + `intents.html` docs |
| `serenity` | `v0.12.5` | repo release tag |
| Stagehand | npm `@browserbasehq/stagehand@3.7.3`; docs/Go SDK `v4` | repo `README.md` |
| Crawlee (JS) | `@crawlee/core` `3.18.2` | npm registry + repo `README.md` |
| Crawlee (Python) | PyPI `crawlee` | repo `README.md` |
| Scrapy | `2.19.0` | release tag + repo `README.md` |
| WalkMe, UiPath | n/a | no public framework released |

## The claim under test

> "It is all fundamentally the same fucking idea."

The survey supports it. Every framework examined — a Rust crawler, a Discord bot
library, two browser-agent SDKs, two scraping frameworks, and two commercial
automation platforms — implements the same five parts:

| Part | What it is |
|---|---|
| **Observe** | Acquire state from a surface |
| **Decide** | Choose what to do next |
| **Act** | Emit an effect |
| **State** | Carry session, queue, cache or variables |
| **Bounds** | Limit the run: budget, capacity, retries, rate |

There is no sixth part anywhere in the field. Once that is seen, the differences
stop looking like architectural disagreement and start looking like four
parameters.

## Four axes, and only four

1. **Observation surface** — HTTP bytes, an event stream, a DOM/accessibility
   tree, pixels, or live form state.
2. **Concurrency model** — parallel-and-stateless with deduplication;
   long-lived connection per shard; in-page on the application's own thread;
   or strictly sequential.
3. **Decision source** — authored rule, fitted model, or model inference.
4. **Authority** — what an action is permitted to do, and who can prove it.

Axes 1 and 3 are where the commercial field competes. Axes 2 and 4 are where the
engineering differs — and axis 4 is empty everywhere.
