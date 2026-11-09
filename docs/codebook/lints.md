# Codebook: precedence and the lint contract

Part of [`CODEBOOK.md`](../../CODEBOOK.md), sections 1 and 2.

## 1. Precedence

| Rank | Source | Wins over |
|---|---|---|
| 1 | Director correction, recorded in `GOVERNANCE.md` | everything below |
| 2 | `GOVERNANCE.md` recorded decisions and ADRs | this codebook |
| 3 | This codebook | convention, habit, prior art in the tree |
| 4 | The file you are in (local idiom) | generic style guides |

A Director correction is a defect verdict. Preserve it as a regression, repair
the lowest invariant that admits the failure, and do not argue the scope down.

Conflicting historical contracts are not silently resolved. Record the conflict
in `GOVERNANCE.md` and pause the work that depends on the choice.

---

## 2. The lint contract

### 2.1 `forbid`, never `deny`

Every lint in `[workspace.lints]` is `forbid` unless this codebook records an
exception. A `deny` is defeated by any single `#[allow]` anywhere in the crate,
including one smuggled in by a macro this workspace does not own. `forbid`
cannot be lowered from source; a conflicting allow is a hard `E0453`. That
property is the only reason the contract binds without a human in the loop.

**Documented exceptions** (each is `deny`, and each is `deny` for a reason that
`forbid` cannot express):

| Lint | Why `deny` |
|---|---|
| `clippy::disallowed_methods` / `clippy::disallowed_types` | API bans with real, narrow, test-scoped exceptions: a test of the thread-parking `lgwks_std::task` must spawn a thread and sleep, so it carries a reasoned `#[expect]` at the test module |
| `missing_docs` / `rustdoc::broken_intra_doc_links` | documentation rules the workspace holds at `deny`, read by the doc lanes |

`print_stderr`, `print_stdout` and `exit` are `forbid`: a binary writes through an `io::Write` handle and returns an `ExitCode`. `unsafe_code` is `forbid` too, and `lgwks_std` alone copies the table with one line lowered to `allow` for its syscall modules, held closed by two tests (INV-STD-UNSAFE-1).

### 2.2 The four silent traps

An unknown lint name in `[lints]` produces **no diagnostic at all**. A typo is a
rule that does not exist. Four names in the anti-pattern corpus do not work as
written:

| Corpus said | Truth | Consequence as written |
|---|---|---|
| `clippy::unwrap_or_else_default` | renamed to `clippy::unwrap_or_default` | renamed-lint warning, rule does nothing |
| `clippy::improper_ctypes` | **rustc** lint | `E0602`; belongs in `[lints.rust]` |
| `clippy::improper_ctypes_definitions` | **rustc** lint | `E0602`; belongs in `[lints.rust]` |
| any unknown name | silently ignored | no diagnostic at all |

Validate every lint addition against a deliberately fake control lint in the
same run and confirm the control warned. A TOML parse error (duplicate
key, missing newline) silently voids the entire table, control included.

Two further shape-level no-ops were verified against rustc 1.98.1 on 2026-09-22
with a control crate (`exit`, `print_stderr`, `print_stdout` all at `deny`):

| Shape | Truth | Consequence |
|---|---|---|
| `std::process::exit` inside `fn main` | `clippy::exit` does not fire there under any call form (`std::process::exit`, `use std::process::exit`, `use std::process`) | a `#[expect(clippy::exit)]` on a bin that only exits from `main` is unfulfilled, and the `deny` never bites. A crash fixture that must exit calls it from a helper (`fn fail(...) -> !`), never from `main` |
| `writeln!(std::io::stderr(), ..)` | `clippy::print_stderr` does not fire | the PRINTS rule bans it and the lint does not. It is an API-ban and review rule, not a lint rule |

`clippy::exit` does fire on the same call from any other function. All three lints
have since moved to `forbid`, so a fixture cannot lower them at all.

### 2.3 The coverage ceiling

**The corpus has 75 entries and the workspace configures 75 lints, but the two
numbers are not a partition.** The table holds 68 `clippy` lints, 6 `rustc` lints
(three of them moved out of the `clippy::` namespace because they are not
clippy's) and 1 `rustdoc` lint: 70 at `forbid`, 4 at `deny` and 1 at `warn`. Other
entries of the corpus need Miri, hardware performance counters, allocation
profiling, or human review; they are not lint-expressible and no configuration
will catch them.

Claiming "all anti-patterns are enforced" is false, and so is the older
"25 of 75 name an enforceable lint" figure this replaces — 25 + 4 + 50 = 79 does
not partition 75, and 25 was never a count of anything in the tree. State the
ceiling wherever coverage is claimed. The non-expressible remainder clusters as
UB and aliasing (Miri), false sharing and cache layout (counters), allocation
and monomorphization (profilers), and API and domain-modelling shape (review).

### 2.4 API bans are half the gate

`clippy.toml` `disallowed-methods` and `disallowed-types` are warn-by-default.
Config states the *what*; `[lints]` states the *fatal*. Both must be present.
Every banned entry names the replacement, not just the prohibition.
