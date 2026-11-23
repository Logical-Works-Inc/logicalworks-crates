# lgwks_bot 10.0.2

Cut 2026-11-23. On macOS, a program that starts many processes through `lgwks_bot` runs at its 10.0.0 speed again (#1130). No public item changes, so this is a patch.

## What a consumer notices

- In 10.0.1, each process scanned the shared Darwin user temp dir at its first spawn and again every 2 s. On the build Mac that dir held 30,145 entries. logical_ci's ownership lane went from 17 s to 130 s, and its suite ran past a 10 min job.
- Now one process per machine scans that dir per interval. Linux is unchanged.

## lgwks_bot 10.0.2 (patch: no public item)

- The scan of the shared dir for idle `xcrun_db-*` files and the scan for dead `lgwks-tmp-*` roots each need a claim on a stamp file in that dir: `.lgwks-sweep-files` and `.lgwks-sweep-roots`. A process wins the claim when it creates the stamp, or locks it and the last win is 2 s old or more. It also wins when the last win is more than 2 s in the future, which means the clock stepped back.
- A process's own private temp dir is scanned as before.
- Receipt: `sim_scratch_machine_claim_matches_the_model`, 2,000 seeds with contended locks and clock steps, against a model written from the rule (INV-BOT-TEMP-1).

## lgwks_deps (unreleased)

- `lgwks new` in the tree pins `lgwks_bot` 10.0.2. The next `lgwks_deps` upload carries it; that upload waits on `gpui_platform` reaching crates.io (#1122).
