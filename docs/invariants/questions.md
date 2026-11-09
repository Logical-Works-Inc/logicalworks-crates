# Open questions for the Director

Reasoning behind the one-line rules. [Index](../invariants-long-form.md).

- For INV-BOT-1..10: which crash/recovery test exercises each one? The ones without a named test are where the next regression will come from.
- Partial answer, 2026-09-23: INV-BOT-2, INV-BOT-3, INV-BOT-4 and the journal-before-acknowledge half of INV-BOT-1 are exercised under a real process kill against a real file store by `tests/it/durable_crash_observation.rs` (rows #100, #101, #102, #104, #106 of the #109 register). The still-unnamed remainder — INV-BOT-5's poll path under a real store (#99), INV-BOT-9's descendant tree (#107 T21/T22), and INV-BOT-10's real frame (#108) — is where the next regression will come from.
- 2026-10-05 (#280): #109 closed on 2026-09-23 with those three rows repaired and covered by in-process regression tests, not by an external observation. INV-BOT-9's T21 is now observed by `tests/it/process_escape.rs`, which shows a `setsid` descendant escaping the group and the receipt not claiming it (INV-BOT-112); stopping that descendant is #263. INV-BOT-5's real-store poll path and INV-BOT-10's real frame still have no named external test.
- 2026-10-06 (#263): the `setsid` descendant is now captured while its leader lives and stopped by pid, observed by `kill -0` against a real escape (INV-BOT-112), and shutdown no longer aborts a cleanup mid-drain (INV-BOT-156). Containment remains Unix-only, and a descendant orphaned before the cleanup ran is still out of reach.
