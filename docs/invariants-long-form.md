# Invariants: the long form

`INVARIANTS.md` and `crates/lgwks-bot/INVARIANTS.md` are the enforced register: one line per rule, with its reason and the tests that enforce it. These pages keep the reasoning, the measurements and the stated limits behind each line, moved word for word when the register was cut to one line per rule (#381). Where the two disagree, the register's line is the rule and the page is the defect to fix. A Director correction or an incident fix adds or replaces its register line in the same PR, and its reasoning here.

| Page |
|---|
| [Surfaces and dependencies](invariants/surfaces.md) |
| [lgwks_bot — durable execution, part 1](invariants/bot-1.md) |
| [lgwks_bot — durable execution, part 2](invariants/bot-2.md) |
| [lgwks_std](invariants/std.md) |
| [lgwks_ast](invariants/ast.md) |
| [Docs](invariants/docs.md) |
| [Governance as code, part 1](invariants/governance-1.md) |
| [Governance as code, part 2](invariants/governance-2.md) |
| [Open questions for the Director](invariants/questions.md) |
