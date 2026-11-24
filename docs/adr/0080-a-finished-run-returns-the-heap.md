# ADR 0080: a finished run returns the heap to its baseline

Status: **accepted**, 2026-11-23.

Stakeholders: the Director; every program that runs `script!` flows in a long-lived process.
Concerns: a run that finished holds no heap, so a host that runs a flow a million times does not grow.
Requirements: INV-BOT-HEAP-1, #1128 G1.
Viewpoint: one process, across many finished runs.
Elements and relations: `tests/heap_baseline.rs`; its counting allocator, `lgwks_deps::stats_alloc` behind the
`heap-accounting` storefront feature (gap #1133).

## Context

The borrow checker frees what leaves scope. It cannot see an `Rc` cycle, a forgotten value or a
cache that only grows. Each of those keeps its bytes for ever, and every assertion still passes.
No test counted the heap of a finished run.

## Decision

1. **Count the bytes.** One test binary installs a counting global allocator. Live bytes are the
   bytes allocated less the bytes freed, on every thread.
2. **Check the instrument first.** A buffer grown by `realloc` and then freed must read 0. An `Rc`
   cycle of 8 KiB must read at least 8 KiB.
3. **Then the runs.** After a warm-up, 512 seeded runs of each shape, then 512 mixed, must leave
   the live count at or below where it started. The shapes are fan-out, retry, deadline, cancel
   and failure.

## Alternatives rejected

- **An allocator written here.** It needs `unsafe impl GlobalAlloc`, which `unsafe_code = "forbid"`
  refuses in every crate.
- **RSS.** The allocator keeps freed pages, so RSS cannot see a leak below a page, or tell one from
  fragmentation.
- **A direct dev edge of `lgwks_bot`.** The register admits an edge only from its owner, and
  `lgwks_deps` owns every third-party edge. The storefront feature also lets an app prove its own
  flows return the heap.

## Consequences

- One more test binary. The edge costs 0.45 s of cold build and 1 crate, test only.
- The first cut added `bytes_reallocated` to the allocated bytes, which already hold it. Every
  buffer that grew read as a leak. Decision 2 holds that error out.

## Correspondence

Closes #1128 G1. ADR 0078 judges files a test leaves; this judges the heap a run leaves.

## Verification

`sim_heap_a_finished_run_returns_to_its_baseline` runs 3,072 seeded runs in about 5 s. With the
cycle check, it fails on a run that leaks.
