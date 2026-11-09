# The docs catch up, 2026-11-08

No public item changed in these releases: each exists so the page crates.io and docs.rs show is true of the code.

An audit found published pages describing crates several releases old. A reader of any of them was told the wrong default features, and the `lgwks_bot` quickstart did not compile.

## lgwks_bot 8.1.1

- The README's quickstart is now the program `published-consumer` compiles. The block it replaces named types that exist nowhere.
- The features table lists `ecs`, `trace`, `wire`, `ron`, `edge`, `testing`, `sealed` and `legacy`, and says which are on by default.
- `lgwks_bot::kernel` is named as supported beside `script!`, and it has its own guide.
- `recorded-instant` and `broker-fence` read an empty line as a usage error with its own message.

## lgwks_std 4.1.1

- The README's feature map copies `version = "4"`, not `"3"`.

## lgwks_ast 6.0.1

- The README states the true dependency chain: this crate reaches `lgwks_std` only through `tool`, and `lgwks_deps` depends on this crate.

## lgwks_deps 6.2.1

- The README and the manifest stop saying no feature is on by default: `debug-doctor` and `lgwks` are, and the storefront's default selection is empty.
- `lgwks new` pins `lgwks_bot` 8.1.1 and `lgwks_std` 4.1.1.

## For an agent in this repository

`SKILLS.md` is the start page, routing a task to one skill under `skills/` ([ADR 0022](../adr/0022-an-agents-start-page-held-by-a-test.md)). Suite tests hold those pages, the publish order, the bot's feature guide, the front page's versions and each manifest line's major to the tree.
