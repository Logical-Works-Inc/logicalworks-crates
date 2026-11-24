# lgwks_bot 10.0.3

Cut 2026-11-24. On Linux, a child under `Confinement::deny_beneath` can run a real build, and it cannot read a denied subtree through the systemd user manager (#1136). No public item changes, so this is a patch.

## What a consumer notices

- In 10.0.2, `bwrap` bound the root without devices. A confined child could not write `/dev/null`, so no build ran under a denial.
- In 10.0.2, a confined `systemd-run --user cat <denied>/seed` printed the seed: the manager ran `cat` outside the namespace.
- Now the child keeps `/dev`, and `/run/user/<uid>` is denied beside the named subtrees. A confined child has no user bus.

## lgwks_bot 10.0.3 (patch: no public item)

- `bwrap --dev-bind / /` in place of `--bind / /` (ADR 0083).
- The caller's `/run/user/<uid>` is overmounted when it is a directory. The uid is the owner of `/proc/self`; a host that will not say fails the spawn.
- A dropped supervisor's killed children are reaped with no later start (#1128 G4, ADR 0081).
- Receipts: `sim_deny_a_bwrap_denial_keeps_the_devices`, `sim_deny_saboteur_the_user_manager_cannot_read_a_denied_file` (INV-BOT-DENY-1).

## lgwks_deps (unreleased)

- `lgwks new` in the tree pins `lgwks_bot` 10.0.3. The next `lgwks_deps` upload carries it.
