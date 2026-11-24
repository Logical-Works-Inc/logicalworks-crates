# ADR 0083: a denied child keeps its devices and loses the user manager

Status: **accepted**, 2026-11-24. Amends the Linux half of `Confinement::deny_beneath` (#689).

Stakeholders: the Director; a CI engine that hides its signing seed from the lanes it runs (logical_ci #85).
Concerns: a confined Linux child can run a real build, and nothing it asks for reads a denied subtree.
Requirements: INV-BOT-DENY-1.
Viewpoint: the denial wrappers in `lgwks_bot::rt::process::confine::deny`.
Elements and relations: `deny_platform` builds `denial_wrapper`, which feeds `bwrap_deny` or `unshare_deny`.

## Context

Measured on Ubuntu 26.04, kernel 7.0, bubblewrap (#1136):

- `bwrap --bind / /` mounts the root `nodev`. The child cannot write `/dev/null`, so no build runs.
- `systemd-run --user` from the child asks the user manager through `/run/user/<uid>`. The manager starts the command
  outside the namespace, and it prints the hidden seed.

## Decision

1. `bwrap_deny` binds the root with `--dev-bind`, so the child keeps `/dev`.
2. Every Linux denial also overmounts the caller's `/run/user/<uid>` when it is a directory. The uid is the owner of
   `/proc/self`. A host that will not say fails the spawn.

## Alternatives rejected

- **Document that a caller must deny `/run/user/<uid>`.** Every caller would have to know the escape, and one that
  forgets runs open.
- **A network namespace.** It does not stop the manager's path socket, and it takes the network from every build.

## Consequences

- A confined child has no `XDG_RUNTIME_DIR`: no user bus, no user services. A build needs neither.
- A confined child cannot open a namespace of its own, so a denial it asks for is refused as unsupported.

## Correspondence

Amends #689. Closes #1136.

## Verification

`sim_deny_a_bwrap_denial_keeps_the_devices` writes `/dev/null` under the denial.
`sim_deny_saboteur_the_user_manager_cannot_read_a_denied_file` runs a control that reads the seed, then the confined
`systemd-run --user`, which must not. `a_denial_adds_the_user_runtime_dir_only_where_it_exists` checks the directory.
