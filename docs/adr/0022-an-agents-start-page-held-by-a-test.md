# ADR 0022: an agent's start page, held by a test

Status: **accepted**, 2026-11-08.

Stakeholders: the Director; every agent that opens this repository; a consumer reading a guide or a crates.io page.
Concerns: agents still struggle on arrival; pages drift from the tree with no gate reading them.
Requirements: R13
Viewpoint: the first-session viewpoint: what a new reader opens, in order, and what proves it true.
Elements and relations: `SKILLS.md`, which routes to each `skills/<name>/SKILL.md`; `tests/it/agent_start_page.rs`, which reads both against the manifests and `scripts/gate-lanes.toml`.

## Context

The Director, 2026-11-08: "AGENTS STILL STRUGGLE SO ALSO ADD ROBUST SKILLS.MD FOR AN AI WHO STARTS IN THE REPO". An audit that day found guides naming 7.0.0 on an 8.1.0 tree, a publish order the manifests refuted, a bot feature guide missing seven features, and a bot README quickstart that did not compile. Nothing read any of them.

## Decision

1. **`SKILLS.md` at the root is where an agent starts.** It maps the tree, routes a task to one skill and lists the traps, and it holds no second copy of a rule.
2. **A skill is one folder with one page**, opening with its name and when to use it, so it is loaded only when its task arrives.
3. **Tests hold these pages to the tree.** A named path, lane, binary or package must exist, and a machine path is refused.
4. **Tests hold the facts that drifted.** They read the publish order against the manifests, every bot feature against its guide, the front page's versions, each manifest line's major, and the bot quickstart against the block `published-consumer` compiles.
5. **A guide states no version of its own.** The table in the root `README.md` holds them, and that table is tested.

## Alternatives rejected

- **Growing `AGENTS.md` into the onboarding page.** It sits near the 120-line law and owns the dependency doctrine, so an agent on any other task would read rules it does not need.
- **Correcting the pages once, with no test.** It is the smallest diff, and the state the audit found is what it produces.

## Consequences

- Six skills and one page are more text to keep true, and a rename now fails a suite test until they are fixed.
- The test reads named things and shapes, so a sentence can still be wrong in prose. It catches a dead name, not a false claim.
- `CLAUDE.md` is not added, because no order named it. `AGENTS.md` carries the pointer.

## Correspondence

Refines ADR 0004 and ADR 0011, which hold a page's fields and shape. This record holds its facts.

## Verification

`crates/lgwks-deps/tests/it/agent_start_page.rs` holds each decision on the live tree, and `crates/lgwks-deps/tests/it/sim_agent_start_page.rs` plants seeded faults the check must name.
