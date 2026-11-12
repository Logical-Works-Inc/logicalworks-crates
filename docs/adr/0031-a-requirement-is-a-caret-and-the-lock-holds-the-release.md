# ADR 0031: a requirement is a caret, and the lock holds the release

Status: **accepted**, 2026-11-12. The Director's order was "it and u have a deps problem, fix it for good".

Stakeholders: the Director; a maintainer or agent landing a dependency update; a consumer whose own lockfile resolves these crates.
Concerns: an update pull request that arrives red and is closed instead of landed; a lockfile that falls behind its advisories; a consumer held to one release it cannot build.
Requirements: R13
Viewpoint: the dependency-admission viewpoint, one row per external edge and the file that fixes its version.
Elements and relations: the requirement lines of `crates/lgwks-deps/Cargo.toml` and `crates/lgwks-std/Cargo.toml`; their rungs in `contract/APPROVED.toml`; `Cargo.lock` and the fifteen nested lockfiles under `bench/` and `crates/lgwks-deps/tests/fixtures/`; the expiring rows of `contract/PATCHES.toml`.

## Context

Seventeen storefront edges were written as exact requirements, such as `=0.41.0`. ADR 0015 pinned twelve of them, and the other five were pinned one at a time (#696, #785, #787). The reason given was reproducibility, which `Cargo.lock` and `--locked` already supply.

An exact requirement puts the version in three places: the manifest, the register rung and every lockfile. A patch release then edits all three. Dependabot edits the manifest and the root lockfile only, so its pull request arrives with `RequirementDrift` and stale nested lockfiles.

Pull requests #904 and #922 arrived red that way, each touching only the root lockfile and that one manifest. Agents had closed #810 and #811 rather than land them. On 2026-11-12 `lgwks-deps freshness` on main's lockfile named 375 of 1,035 packages behind. The exact pins also bind consumers: `=0.5.5` of appcui cannot build beside its newer macro crate (#841).

## Decision

1. **Every registry edge is a caret requirement at the floor the code needs.** The exact release lives in `Cargo.lock`, and every gate builds `--locked`. The register rung is the same caret, so a compatible release moves no rung.
2. **A compatible update is a lockfile change.** Dependabot's `rust-patch-minor` pull request changes `Cargo.lock` alone, and the suites judge it.
3. **A major is still an admission.** It moves the requirement, the rung with its date, and the nested lockfiles, in one pull request.
4. **An exact pin is an expiring exception.** It needs an `upstream-pin` row in `contract/PATCHES.toml` with its upstream issue and expiry, and the gate refuses the row once it lapses.
5. **An update pull request is landed, never closed.** `skills/lgwks-dependency-update/SKILL.md` is the route for both kinds.

## Alternatives rejected

- **Keep the exact pins and re-lock by hand each week.** Every release stays a three-file edit, which is the work that was skipped until the lock was 375 behind.
- **A bot that rewrites the register on each Dependabot pull request.** The register is a human decision, and a machine that edits it removes the review it records.
- **Tilde requirements.** They refuse the minor releases that carry most fixes, and Cargo's own guidance for a library is the caret.
- **Refusing `=` inside `lgwks-deps check` for every repository.** Consumers run the same gate, and their pins are theirs to decide; this record binds this repository's register.

## Consequences

- The seventeen pins are carets, and three floors moved: appcui to 0.5.7, rusqlite to 0.41.1 and genai to 0.7.4. `rsa` leaves its release candidate for 0.10.
- The root lockfile moved 89 packages, and all fifteen nested lockfiles were re-locked.
- A consumer resolves any compatible release of an edge, so two consumers may build different patch releases until each updates its own lockfile.
- A breaking change published as a patch release reaches this repository through Dependabot's pull request, where the suites meet it before it merges.
- The majors named in #904 are their own pull request under decision 3.

## Correspondence

Refines decision 1 of ADR 0015, whose "pinned exactly" clause this record replaces for the twelve agent-standard edges. Relates to ADR 0003, whose default-off costs are unchanged, and to ADR 0028, whose dependency cache key moves whenever the lockfile does.

## Verification

- `ledger::tests::the_committed_register_pins_no_registry_edge_exactly` reads the committed register and ledger and names every unexcused exact pin.
- `ledger::tests::an_exact_pin_is_named_unless_an_upstream_pin_excuses_it` holds the judgement on a planted pin, with and without its ledger row.
- `lgwks-deps check .` holds each manifest requirement to its rung (`RequirementDrift`) and refuses a lapsed pin (`PatchExpired`).
- The `lockfile` lane and the suites that build each nested root `--locked` refuse a stale lockfile.
