# ADR 0091: a meta prover holds every resource kind to three proofs

Status: **accepted**, 2026-11-27.

Stakeholders: the Director; every repo built on the crates; the Mac and boxes the gates run on.
Concerns: leaks came back one kind at a time (temp files, orphans, RAM disks, fseventsd), each fixed alone.
Requirements: INV-PROVE-1.
Viewpoint: the resource-lifetime view of the workspace: what a process acquires and who releases it.
Elements and relations: the `prove` lane of `ci-local`, the checker; `contract/RESOURCES.toml`, the register; the
Kani harnesses in each owner module; the closure lanes; the census tests.

## Context

The Director, 2026-11-27: "DO NOT MAKE WHACK-A-MOLE FIXES. EVER. YOU WILL PROVE CRATES MATHEMATICALLY CANNOT CAUSE
THE MEM BUGS I KEEP SEEING THROUGH A META PROVER." #1128 lists the leak classes in prose. Nothing checks that each
class keeps its proof, or that a new kind has one at all. #1164 is the order.

## Decision

1. The prover's catalog of resource kinds is code, not data: process, mount, lease, temp, descriptor, heap, thread,
   and daemon (memory a daemon holds for our I/O). A register entry cannot add or drop a kind.
2. Each kind names one owner module and three proofs in `contract/RESOURCES.toml`:
   - **closure**: required lanes that refuse an acquisition of the kind outside its owner;
   - **model**: `#[kani::proof]` harnesses on the owner's real code, each run by a required lane whose command is
     `cargo kani` with a `--harness` that selects it, so Kani's verdict is that lane's verdict;
   - **census**: tests that drive the real code to its end and read the OS table the kind lives in.
3. `ci-local --lane prove` fails on a kind with no entry, two entries, no owner or an empty proof list. It also fails
   on a lane that is absent or not required, a name that is no harness, a harness no required Kani lane runs, or a
   census name that is no test. Its detail is one line: `N of 8 resource kinds are not proven: GAP kind: why; ...`.
4. The prover reads names and lanes and runs nothing. Each proof fails in its own lane.
5. The census reads whole OS tables, not the catalog. A resource of a kind the catalog lacks shows up as residue,
   and that fails the census test by name.

## Alternatives rejected

- **TLA+ specs of each protocol.** TLC checks a model someone wrote beside the code, and the two drift. Kani checks
  the Rust that runs.
- **One more fix per leak.** That is the method the Director banned. It finds a kind only after it costs a machine.
- **A Halstead or other complexity gate.** It predicts defect density. It cannot show that a release runs.

## Consequences

- The first models are the ledger's kernels: the torn-line rule over every text of up to 4 bytes, and the detach
  rule over every boot, holder and image. Both verify in 28.7 s wall at 3.0 GB peak RSS (Kani 0.68.0, 2026-11-27).
  At 6 bytes the same proof took 6.1 GB, and a whole-record harness did not end in 13 minutes.
- A proof holds within its bound and the catalog. Code off the rails and `unsafe` in dependencies are not covered.
- `prove` and `kani` are local and not required until the lane names no gap. Each gap it names is open work.

## Correspondence

Makes #1128 machine-checked. Uses ADR 0087 and 0090 as the owners of process, mount and lease.

## Verification

`sim_prove_judge_matches_the_model_over_hostile_registers` (2,000 seeded registers against a model of the rule) and
`sim_prove_names_every_unproven_kind_and_only_those` (the shipped `ci-local` on a fixture repository).
