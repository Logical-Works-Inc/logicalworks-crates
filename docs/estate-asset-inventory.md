# Adjacent asset inventory

Continued in: [Bring in now](estate-asset-inventory-bring-in-now.md), [The other repos](estate-asset-inventory-repos.md).

Status: reference, written 2026-09-20 and kept as that survey's record. Companion
to [`guidance-runner-spec.md`](guidance-runner-spec.md), whose workstreams have since
landed; the repositories named here are other repositories and their state is theirs.

## Why this exists

The guidance runner described in the companion specification needs six
capabilities. Before writing any of them, the surrounding repositories were
searched for an existing implementation, so that the workspace ports rather than
reinvents, and so that a later reader can see what was considered and rejected.

Method: a cross-repository graph query over the shared concept index, structural
queries against this workspace, and a targeted sweep of sibling repositories for
module documentation matching the six capability terms. Third-party vendored
code was excluded.

The six capabilities searched for: flow graph as data; session state; evidence
journal; declarative document compilation; element and selector resolution;
invariant registers.
