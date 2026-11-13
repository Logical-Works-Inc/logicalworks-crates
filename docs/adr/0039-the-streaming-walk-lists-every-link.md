# ADR 0039: the streaming walk lists every link

Status: **accepted**, 2026-11-13. The Director's order, relayed by logical_ci's session for #945,
was "fix this shit and ensure it never fucking happens again".

Stakeholders: the Director; logical_ci, whose A06 write detector walks a lane's subject tree with
`lgwks_std::fs::walk_dir_iter` (logical_ci#248); every caller of the streaming walk.
Concerns: a planted link that the walk silently omits.
Requirements: INV-FS-8
Viewpoint: the listing viewpoint, one entry per name a directory holds.
Elements and relations: `fs::stream::WalkIter::admit`, which decides each listed directory and
link, and `fs::stream::sim_links`, which checks it against a non-following `read_dir` recursion.

## Context

`WalkIter::admit` canonicalized every link, even with `follow_symlinks` off. A link resolving
outside the root was withheld, and a dangling one was withheld as absence under the default
`OmissionPolicy`. A lane that plants `subject/x -> /etc` or a link to nothing was invisible to a
detector that asks what the tree holds.

## Decision

- Every link is yielded as `FileKind::Symlink`, whatever it names, follow on or off.
- With follow off, no link target is resolved at all.
- With follow on, only an in-root directory link within the depth bound is descended. A link out
  of the root, to a file or to nothing is still yielded and never descended.
- A followed link is yielded under its own name, and its inside is listed beneath that name.
- A directory a link entered first is still yielded where it lives and is not descended again,
  as `walk_dir` lists it. Before this record the stream withheld it.

## Alternatives rejected

- **Report the out-of-root link as an omission.** An omission says the walk could not read a name;
  this link was read, so reporting it as unread misstates the tree.
- **Change the materializing walk too.** `walk_dir` has its own contract (R15: a strict walk
  refuses a broken link), and its callers depend on that refusal; #945 names the stream.

## Consequences

- A caller that relied on the stream hiding out-of-root links now sees them as `Symlink` entries.
  The production caller, `lgwks-ast`'s `files_under`, keeps files only. It reports every
  omission, so before this record a dangling link made it refuse the whole tree as unlistable.
  Now that link is a `Symlink` entry it passes over.
- A kind-only walk with follow off now resolves no links, so it is cheaper than before.
- A following caller that keyed entries by path saw a link under its target's name and lost the
  real directory. It now sees both, each under its own name.

## Correspondence

Refines INV-FS-5, whose canonical-root rule still holds for every yielded path. Indexed in
`docs/adr/README.md`.

## Verification

- `sim_every_link_is_listed_whatever_it_names` builds 96 seeded trees of directories, files and
  in-root, out-of-root and dangling links. Follow off must equal the `read_dir` recursion exactly.
  Follow on must list every name under some name, and must yield nothing beneath a sealed link.
- `sim_links_monkey_the_stream_is_the_model_at_every_depth_bound` adds links read before their
  target and into ancestors. The stream must yield a preorder model's exact trace at depth bounds
  0 to 3 and none, follow on and off. The mutation gate found the earlier sim never followed a link.
- `sim_links_saboteur_a_link_read_first_never_hides_or_renames_its_directory` is the shrunk case.
