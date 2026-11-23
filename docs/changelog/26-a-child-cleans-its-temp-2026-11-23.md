# lgwks_bot 10.0.1 / lgwks_deps 8.0.1

Cut 2026-11-23. A supervised child no longer leaves temp files behind (#1127, ADR 0077). On 2026-11-23 the build Mac held 110,965 `xcrun_db-*` files (86 GiB) from children that were killed or cut short. Both crates change no public item, so both are patches.

## What a consumer notices

- Each child that `lgwks_bot` starts gets its own `TMPDIR`, `TMP` and `TEMP`. The directory goes when the process group ends: on exit, failure, deadline, and with a background writer still running.
- A spec that sets `TMPDIR` or clears the environment keeps its own choice.
- `lgwks new` pins `lgwks_bot` 10.0.1.

## lgwks_bot 10.0.1 (patch: no public item)

- The private dirs sit under `lgwks-tmp-<random>/` in the system temp dir. Each root holds a locked `.owner` file. The first root a process opens removes every root whose lock is free, so the roots of a supervisor killed with `SIGKILL` go too (INV-BOT-TEMP-1).
- `xcrun` writes its cache beside itself, whatever `TMPDIR` says. While a group runs and when it ends, at most once each 2 s, `xcrun_db-XXXXXXXX` files idle for 10 s or more are removed from the own temp dir and the Darwin user temp dir.
- The process-wide start lock, live-leader set and orphan queue sit behind one owner type.

## lgwks_deps 8.0.1 (patch: no public item)

- `lgwks new` pins `lgwks_bot` 10.0.1 and `lgwks_std` 5.0.0.
