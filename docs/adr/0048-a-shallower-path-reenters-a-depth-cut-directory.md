# ADR 0048: a shallower path re-enters a depth-cut directory

Status: **accepted**, 2026-11-13, for #913 item 7.

Stakeholders: every caller of a `lgwks_std::fs` walk that follows symlinks under a `max_depth`.
Concerns: a strict walk that drops a subtree and still reports complete.
Requirements: INV-FS-2
Viewpoint: the path-based walk engine, `fs/engine.rs`.
Elements and relations: `walk_recursive` (the recursion), `track_canonical_visit` (the visited map),
`WalkContext::depth_cut` (the cut flag a walk passes up to its caller).

## Context

- The walk kept a set of canonical directories it had entered, so a link loop ends.
- A link deep in the tree can reach a directory before its real, shallower path does. `max_depth`
  then cuts that directory's subtree, counted from the link.
- The real path found the directory in the set and returned. It recorded no omission. A strict
  walk lost the subtree and reported complete.

## Decision

- The set becomes a map. Each canonical directory keeps the shallowest depth it was entered at
  and whether `max_depth` cut anything below it then.
- A directory is entered again only when that entry was cut and the new path is shallower.
- A skipped directory that was cut marks its caller cut too. A later shallower path to the caller
  then re-enters both.
- Depth falls on each re-entry, so each directory is entered at most `max_depth + 1` times. With
  no cut, nothing is entered twice, and the output is as before.

## Alternatives rejected

- **Loop detection over ancestors only**, as `find -L` does. Each alias lists its whole target.
  A tree of links that fan out grows exponentially in output, with no ceiling when no
  `WalkLimits` is set.
- **An omission at the skip.** A strict walk then refuses a tree it can read in full.

## Consequences

- A re-entered directory lists its entries again under the shallower logical path. The deep alias
  keeps the entries it listed first.
- The map adds one `bool` and one `usize` for each directory entered.

## Correspondence

Answers #913 item 7. Adds no module.

## Verification

`sim_fs_walk::faults::a_deep_link_never_hides_the_shallower_real_subtree`: 48 seeded trees, a link
at level 4 that sorts first and points at a directory that has a subdirectory, `max_depth` 3 to 6.
Every path the depth admits on the real tree is reached.
