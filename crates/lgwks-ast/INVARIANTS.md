# `lgwks_ast` invariants

The `lgwks_ast` part of the invariant register, one short line per rule: rule · why · enforced by. The rest is in
[`INVARIANTS.md`](../../INVARIANTS.md).

- **INV-AST-1** Checked inspection charges nodes before descending, keeps state proportional to depth and bounds it with `MAX_AST_DEPTH`. · why: #143, #277 · enforced by: `lgwks_ast::parser::check_tests::a_tree_past_the_depth_bound_refuses_as_ast_too_deep`
- **INV-AST-2** Content detection parses each candidate once; only invalid syntax is negative evidence. · why: #165, #194 · enforced by: `tests/it/content_detection.rs`, `tests/it/sim_diagnostics.rs`
- **INV-AST-3** The tree walk is a cursor walk, `O(nodes)` time and `O(depth)` state, in source order. · why: #277 · enforced by: `lgwks_ast::parser::tests::the_cursor_walk_visits_exactly_the_nodes_the_positional_walk_did`
- **INV-AST-4** Hostile input earns a typed refusal, and deep markdown is refused before the scanner can abort. · why: #277 · enforced by: `tests/it/hostile.rs`
- **INV-AST-5** A checked parse holds its thread at most its deadline plus one progress interval and answers `ParseError::TimedOut`. · why: #277 · enforced by: `tests/it/sim_parse_deadline.rs`
- **INV-SCAN-ZERO** No shipped file carries a scan finding, an unreadable file is a refusal, and every way a lint is silenced is read. · why: ADR 0001, #698, #699, #758 · enforced by: the `scan` lane, `tests/it/sim_scan_detectors.rs`, `tests/it/scan_evidence.rs`
