# ADR 0084: a skill hygiene lane

Status: **accepted**, 2026-11-24.

Stakeholders: the Director; every agent that builds on the crates in Claude Code, OpenCode or Command Code.
Concerns: a skill no harness loads; a skill an agent never chooses; a Rust block that teaches an API no compiler read.
Requirements: R13
Viewpoint: the harness viewpoint: what a harness reads at session start, and what an agent reads once it chooses.
Elements and relations: `skills/<name>/SKILL.md`; `SKILLS.md`, which routes to each; `crates/lgwks-bot/src/lib.rs`, which compiles each Rust block; the builtin lane `skill-hygiene` in `crates/lgwks-deps/src/bin/ci_local/skill_hygiene.rs`.

## Context

The Director, 2026-11-24: "make a set of skill.md files for AI agents to use ... This is primarily for agents who are using the framework and how to succeed. Add skill hygiene as a new gate as well."
A harness reads only a skill's frontmatter at session start. It chooses the skill from `description` alone (the Agent Skills specification, agentskills.io).

So the frontmatter must hold exactly what that specification defines. A name that is not its folder fails, and so does a description past 1,024 characters. A plain YAML value that holds `: ` is read as a map (YAML 1.2, section 7.3.3), so the skill does not load.

ADR 0022 held a skill's names to the tree in a suite test. Nothing held its shape at the fast gate, and nothing compiled its Rust blocks.

## Decision

1. **The lane `skill-hygiene` is fast and runs on both surfaces.** It refuses each fault by name. The faults are a name that is not its folder or not 1 to 64 of a-z, 0-9 and `-`, a key the specification lacks, and a description that is absent, unsafe as plain YAML, past 1,024 characters or not opening with `Use `. The lane also refuses a page past 120 lines, an untitled page and a page no route names.
2. **Every `rust` block is a doctest of `lgwks_bot`.** `crates/lgwks-bot/src/lib.rs` includes each skill that holds one, under `cfg(doctest)`. The lane refuses a block it does not include, an `ignore`d block, an untagged fence (rustdoc compiles it as Rust) and an unclosed fence.
3. **A harness link into the tree must land on a skill of its own name.** Each entry in `~/.claude/skills`, `~/.config/opencode/skills`, `~/.commandcode/skills`, `~/.gemini/config/skills` and `~/.agents/skills` that links into `skills/` is followed. A dangling link or a renamed target is refused.
4. **ADR 0022 keeps the names.** `tests/it/agent_start_page.rs` still holds each named path, lane, binary and command to the tree. Its frontmatter test is now this lane's, so the rule has one home.

## Alternatives rejected

- **A Node check in the harness directory only.** CI could not run it on the repository, and it would be a second copy of the rules.
- **Compiling the blocks in a separate binary.** rustdoc already compiles a block with the crate's features and its hidden-line rules, and `doc-tests` already runs it.

## Consequences

- A skill author writes a `rust` block once, and the next `doc-tests` run compiles it.
- The link check reads the home of the machine that runs the lane. It refuses only links into this tree, so CI's empty home passes.

## Correspondence

Refines ADR 0022, which holds a skill's names to the tree. This record holds its shape and its install.

## Verification

`crates/lgwks-deps/tests/it/sim_skill_hygiene.rs` drives the shipped `ci-local` binary over 51 seeded trees and homes. Each fault kind is planted three times, and each tree is also run clean. `the_repository_skills_pass_the_lane` runs the lane on the committed tree.
