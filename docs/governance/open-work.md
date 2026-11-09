# Governance §6 and §7

Part of [`GOVERNANCE.md`](../../GOVERNANCE.md), which indexes every section.

## 6. Open correctness and acceptance work

Reconciled 2026-11-08 against the 34 open issues, after the 2026-11-08 cuts
(`ledger-2026-10-26.md`). Every row of the 2026-10-09 table had closed by
2026-10-19. Per §1, unwired code is a wiring defect, not a closed item.

| Priority | Work | Observed gap and completion evidence |
|---|---|---|
| P0 | Publish verification | Every crate cut on 2026-11-08 is verified with `published-consumer` against what crates.io serves |
| P1 | Target and app defects | #892 `lgwks_std` does not compile for `aarch64-apple-ios`, which fails the local `target-matrix` lane; #889 16 tenants can fill the global plan ceiling of `rt::apps`; #890 `bin/estate.rs` hard-codes a host path |
| P1 | Estate UUID (#851) | `now_v7` and the tuple UUID element shipped in lgwks_std 4.0.0 (#858); serde declined (ADR 0010). The tuple layer is on main. Open: the ECS identity surface, which lands with its first consumer |
| P1 | Source and age on every result (#778) | Nothing merged. Needs an enveloped `Observe::poll` in a major of lgwks_bot, workbook and mail readers in lgwks_ast, and a matched benchmark |
| P1 | Cold build in 90 s (#860) | 184 s cold under `ci-bot` (#857); the lgwks_bot `it`, lib-test and lib units are the critical path. A consumer's cold default build of lgwks_bot 8.1.0 measured 68.03 s against the 60 s budget |
| P1 | appcui consumer build (#841) | lgwks_deps still pins `appcui =0.5.5` without its macro crate; a fresh resolve fails |
| P1 | The app layer (#780) | #780 ceremony reference app shipped in lgwks_bot (#847); #830 OpenAPI 3.1 shipped in #832. #845's supported surface shipped as `lgwks_bot::kernel` in 8.1.0. Open: #815 durable cell log, #816 blocking provider effects, #823 2x axum |
| P1 | Journal and policy authority | #819 seeded faults shipped in #824, #820 fail-closed start in #829, #779 tokens in #843. Open: #817 model-call record/outbox, #818 pre-filter, #821 effect broker |
| P2 | Parsers in lgwks_ast | #834 SQL to a relational IR, #852 TOML and YAML; every parser lives in lgwks_ast (Director, 2026-11-04) |
| P2 | Relational projections | #835 incremental projections over the journal, #828 and #839 the embedded SQL engine |
| P2 | Crypto | #794 and #840 NIST curves (the modules that claimed them were removed as unreachable, ADR 0014), #833 X.509, CMS and PDF signing |
| P2 | Dependency gaps | #836, #837, #838, #848 and #850: each a `gap` issue, cited on the manifest line that admits a stand-in |
| P2 | Sandbox and virtualization | #846 Nova warden edges, #854 the Virtualization.framework binding; the modules that claimed them were removed as unreachable |
| P2 | Benchmarks and hygiene | #789 durable-workflow benchmark, #822 no machine-absolute paths in graph output |
| Standing | Dependency admission | Any new third-party edge goes through `skills/lgwks-dependency-admission/SKILL.md` and lands in `contract/APPROVED.toml` |

---

## 7. Decisions requiring the Director

| Decision | Conflicting authority or missing choice | Rule until decided |
|---|---|---|
| Merge without executed CI reproduction | A local PASS is not a CI run | Local receipts support source review only. Merge eligibility needs executed CI reproduction on the affected lanes unless the Director records a bounded exception naming its replacement evidence |
| rust-guard's REPETITION check | The Director authorised a bypass on 2026-09-27 for commit `b30a2716`, on the stated reason that the remaining findings are a relocated block and compiler-mandated shape, not duplication | The check is compared per file against the base commit and does not net a deletion against an addition, so a move reads as a copy. It needs three rules: net deletions against additions across the whole diff, exempt trait-impl boilerplate, and match a repeated body rather than a repeated line shape. Until then every real duplication is still extracted first and the bypass is the last step, recorded in the commit message |
| Does a lagging `Stored` fact still fence a replay? | The simulation found a fact that enters the fence 900 events behind the head and asked whether it should. Shipped answer is yes, and `sim_journal::stored_lags_history_but_still_fences_a_replay` now pins it | Until decided, the shipped behaviour stands and the pinning test fails loudly if it changes |
| Licence change or re-opening `lgwks_bot` | #73 closed it to outside contributions; the CLA instrument is deferred | `lgwks_bot` stays closed. The other five crates keep their Apache-2.0 licences (`LICENSING.md`) |
| `unsafe` for the in-process pre-exec containment hook | CODEBOOK forbids `unsafe_code` outside its named exceptions; an in-process Landlock or `sandbox_init` call (#337) needs one | No new `unsafe` until the Director rules. Stand-ins stand: `setpriv` Landlock rules on Linux and the `sandbox-exec` wrap on macOS |
| Merges on red PR check runs (Oct 8 sweep) | §3 needs hosted reproduction of the affected lanes; five of the seven PR runs (#368, #369, #370, #372, #374) read `failure` at merge | The failing jobs per run are recorded in the 2026-10-08 entry (2026-10-09). The merge rationale is not on record, and whether those merges stand as precedent is the Director's ruling. Until then it is flagged debt |

A required decision pauses dependent work. Audits, reproducible counterexamples,
and factual documentation may continue.
