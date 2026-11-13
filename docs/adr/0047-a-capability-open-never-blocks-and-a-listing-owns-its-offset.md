# ADR 0047: a capability open never blocks, and a listing owns its offset

Status: **accepted**, 2026-11-13, for #913 items 4 and 5.

Stakeholders: every caller of `fs::capability::Dir` on a tree another process can rewrite.
Concerns: an open that hangs on a swapped name; a listing that is short but says it is complete.
Requirements: R9 (`docs/requirements/d-correctness.md`), INV-FS-4, INV-FS-6
Viewpoint: the handle-relative filesystem viewpoint, one admitted descriptor.
Elements and relations: `Dir::open_entry` and `Dir::create_new` (the opens), `Dir::entry_names`
(the listing), `Dir::try_clone` (clones share one open file description).

## Context

- `open_entry` opened with no `O_NONBLOCK` and no `O_NOCTTY`. A name swapped for a FIFO blocks
  `openat` until a peer opens the other end, which may be never.
- `entry_names` rewound and read the shared descriptor. Clones share its offset, and `Dir` is
  `Sync`. Two listings at once interleave, and each keeps a subset that reports complete.

## Decision

- `open_entry` opens `O_NONBLOCK | O_NOCTTY`, reads the kind with `fstat`, and refuses anything but
  a regular file with `Unsupported`. Then it clears `O_NONBLOCK`, so the file blocks as usual.
- `create_new` adds `O_NOCTTY`. `O_EXCL` only creates a regular file, so it needs no `O_NONBLOCK`.
- `entry_names` lists through a fresh `openat(".")`. That is a new open file description with its
  own offset, so no listing moves another's position.
- The private syscall helpers move to `capability/raw.rs`, so `capability.rs` keeps the size law
  and no public item moves.

## Alternatives rejected

- **`kind` first, then open.** The name can change between the check and the open. The check on
  the open descriptor is the only one that names what was opened.
- **A lock around the shared offset.** It serializes every listing on one directory, and a clone
  given to a child process still shares the offset outside the lock.

## Consequences

- `open_entry` no longer opens a FIFO, a device or a socket. A caller that wants one uses its own
  open.
- Each listing costs one extra `openat` and `close`.

## Correspondence

Answers #913 items 4 and 5. Adds the private module `fs::capability::raw`, named in CODEBOOK.md.

## Verification

- `fs::capability::tests::open_entry_refuses_a_fifo_without_blocking`,
  `open_entry_refuses_a_character_device` and `an_opened_regular_file_is_blocking`.
- `fs::capability::tests::concurrent_listings_through_clones_each_see_every_name`: 16 clones,
  32 rounds each, 400 names, on Linux.
