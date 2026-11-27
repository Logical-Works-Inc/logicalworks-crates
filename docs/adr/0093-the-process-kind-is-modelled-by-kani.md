# ADR 0093: the process kind is modelled by Kani on an allocation-free host

Status: **accepted**, 2026-11-27.

Stakeholders: the Director; every repo built on the crates.
Concerns: the meta prover (ADR 0091) counts the process kind unproven until a Kani harness models its owner.
Requirements: INV-STD-COMMAND-1, INV-PROVE-1.
Viewpoint: the resource-lifetime view: the kill rounds that end a process group.
Elements and relations: `end_group_on` and its `GroupHost` seam in `process/command/child.rs`; the harness in
`child/proof.rs`; the `kani` lane; `contract/RESOURCES.toml`.

## Context

ADR 0092 ends a group by `killpg`, and on a macOS `EPERM` by kill rounds. The rounds only run under load, so no
real-kernel test reaches them. A seeded model drives them, but the prover needs a proof for every value.

## Decision

1. `end_group_on` takes its round bound. Production passes 64; the proof passes 3.
2. `GroupHost::members` returns an owned iterator (`type Members`). The kernel host returns a `Vec`, and the Kani host
   a fixed array, so the proof models no heap.
3. `group_end_kills_every_live_member_and_nothing_else` covers every group of up to two members, and every state,
   race, refusal, fork and group-kill answer. It proves that no kill reaches a zombie or a stranger, that an end leaves
   no live member, and that the shield's rounds end a group exactly when no live member refuses its kill.
4. The `kani` lane selects `group_end_`, and the register names the harness as the process model.

## Alternatives rejected

- **Prove the production bound of 64.** CBMC must unwind 64 rounds; the round logic does not change with the bound,
  and `sim_child_group_end_holds_every_round_bound` drives 64 and every bound to 8.
- **A `Vec` listing in the proof.** Measured: 418 s and 13.1 GB peak RSS for the harness, past the 5 min gate.

## Consequences

- The harness verifies in 39.6 s; the `kani` lane runs in 77.1 s wall at 3.4 GB peak RSS on the Mac.
- The process kind's gap is now only that the `kani` lane is not required. It turns required with `prove`.

## Correspondence

Refines ADR 0092. Serves ADR 0091.

## Verification

The `kani` lane (`VERIFICATION:- SUCCESSFUL`, 3 of 3 harnesses). `sim_prove_the_live_register_models_the_process_kind`
holds the register to the harnesses the lane selects. `sim_group_end_proof_holds_for_every_case` runs the proof's
`check` over all 832 cases under `cargo test`.
