# Product requirements

Status: **immutable.** This file is the product's normative spine. Everything
else in the repository is measured against it: issues, pull requests, the
release gate, and any readiness claim.

A requirement here is **superseded, never edited**. To change one, add an entry
to the Supersession log naming its id and why the previous statement was wrong
or incomplete, then run `cargo run -p lgwks_deps --bin check-requirements -- --update`. The
gate refuses a changed normative sentence with no supersession entry, and the
entry is permanent. Silence is not consent to drift.

Every requirement carries four things, and a requirement without all four is
not written yet:

- **Invariants** that must hold for it to be true.
- **Callers** whose lives it changes.
- **Falsifier**: one discriminating observation that would refute it. Not a
  checklist. One test.
- **Status**: `met`, `partial`, `unmet`, or `future`, with the evidence class
  named as measured, proved, argued, or unmeasured.

---

## The vision

**The problem.** Code orchestration is brittle at every level. Doing a small
amount of real work requires too much downstream machinery: a queue, a
supervisor, a tick loop, a retry controller, a cancellation tree, a process
manager, a record of what happened. Each is a dependency. Each is re-implemented
wrong. The logic is rarely the failure; the plumbing around it is.

**The promise.** The bot contract does that work. A developer writes code and
their own logic, strings it together, and is done. The seven lifecycle
obligations that every brittle codebase re-derives sit below the task surface,
where one audited implementation owns them.

**Three shapes call the contract.**

1. A developer writes code and strings it together.
2. An AI needs to poll. It calls the bot. The bot owns observation and change
   detection; the agent does not keep a shadow previous-value cache.
3. An AI needs risky code checked. The bot checks the code **in process**, over
   its structure. It does not execute the code to find out what the code does.

**The execution model.** A worker substrate for ordinary code, at native
speed. In-process is the default tier and costs nothing to reach. A VM or a
runtime is invoked only when the developer declares `N` parallel things, or
when the work is computationally bounded onto another lane. Escalation is
declared at construction. Nothing escalates on a heuristic.

**The hard part is not the work.** The hard part is meeting every abstract,
weird, human and in-real-life complexity and slop: the target that is gone
rather than moved, the dialog that stole focus, the file that was half-written,
the clock that disagrees, the credential that expired mid-run, the operator who
cancelled, the intermediary that retried a non-idempotent effect. Those are
requirements. A demo that only handles well-formed inputs is not evidence.

---

## The requirements

R1 to R13 live on their own pages, and the gate reads them with this one: [A. Authoring](docs/requirements/a-authoring.md) · [B. Execution tiers](docs/requirements/b-execution-tiers.md) · [C. Call shapes](docs/requirements/c-call-shapes.md) · [D. Correctness](docs/requirements/d-correctness.md) · [E. The mess](docs/requirements/e-the-mess.md) · [F. Evidence](docs/requirements/f-evidence.md).

---

## Supersession log

Newest first. One entry per requirement whose normative sentence changed.

- **R12, 2026-11-09.** The register held rows M1 to M12 and named three further
  classes. Rows M13 to M17 add those three and two the evidence had already met, and
  M7 and M10 to M12 move to `handled` on the tests they name.
- **R11, 2026-11-09.** Its invariants say no `unsafe`. The Director admitted direct
  syscalls in `lgwks_std` on 2026-10-12 (#345, #263), in two named modules held closed
  by INV-STD-UNSAFE-1.

The requirements' normative sentences stand as first written.

---

## Relationship to the rest of the repository

`docs/production-readiness.md` scores the axes against these requirements; it
is an assessment and is allowed to be wrong. `docs/declarative-orchestration.spec.md`
and `docs/async-sdk-shape.md` specify the surfaces R1, R2 and R6 need;
`docs/effect-kernel.md` specifies the surface R10 needs;
`docs/framework-comparison.md` and `docs/frontier.md` establish the field R9's
axis sits in. Where those documents disagree with this one, this one is the
requirement and the others are the work.

Issue [#109](https://github.com/Logical-Works-Inc/logicalworks-crates/issues/109) was
the release gate for R10 and
[#87](https://github.com/Logical-Works-Inc/logicalworks-crates/issues/87) the
implementation of R1, R2 and R6, and both are closed. What stands against the verdict
is `docs/production-readiness/7-verdict.md`.
