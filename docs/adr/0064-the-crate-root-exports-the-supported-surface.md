# ADR 0064: the crate root exports the supported surface

Status: **accepted**, 2026-11-16. Implements #1016 (B3 of #986), absorbs #896.

Stakeholders: the Director; a consumer that writes flows and hosts them.
Concerns: about 100 hidden root re-exports gave one item two paths; a supported signature named a type only a hidden path reached.
Requirements: R9, R10
Viewpoint: the consumer's viewpoint, naming items from outside the crate.
Elements and relations: the `lgwks_bot` crate root; `script`, which now carries `block_on`; `kernel`, which gains `task`, `cap` and `sealed`.

## Context

B1 (#1014) counted the root: `script!`, `try_join!`, `script`, `kernel`, `app` and a hundred hidden re-exports of
items that already had a module path. A consumer could not run a flow, host a durable one or grant an app a cap
without naming a hidden path, because `block_on`, `task::Host` and `cap::Cap` were reached only that way.

## Decision

1. **The root exports `script!`, `try_join!`, `script`, `kernel` and `app`.** `domains!` stays at the root because
   `#[macro_export]` puts it there. Every other root item is a `#[doc(hidden)]` module.
2. **An item has one path: its module.** `ecs`, `registry` and `app_error` become hidden `pub mod`s so their items
   keep a path. `BoxFuture` moves to `verb`; `join!` and `select!` move to `rt`.
3. **`script::block_on` runs a flow.** It is the runtime's own `block_on`, reached through the flow module.
4. **`kernel` gains `task`, `cap` and `sealed`**, as globs of the support modules (ADR 0021, decision 2), so a durable
   host, a cap and a sealed store are reachable from the supported surface.

## Alternatives rejected

- **Keeping `block_on`, `Runtime` and `Cap` at the root.** Three exceptions to "the root is the surface" grow back
  into a hundred.
- **Unhiding `task` and `cap`.** It works, but ADR 0021 already states the promise in one module, `kernel`.

## Consequences

- A consumer that named `lgwks_bot::X` moves to the module path. The moves are listed on #1016.
- `kernel::task` promises the host front door, so a new public item in `task` is a promise too.
- The 18 B1 methods no caller reached are deleted. The `lgwks_bot` `pub` ceilings sum to 2634, down from 2654.
  Its doc-example ceilings rise from 170 to 182.
- The `continuation` naming and watermark helpers had no caller outside the crate, so they are `pub(crate)`.
  `continues`, `event_headroom` and `byte_headroom` had no caller at all, so they are deleted.
- `join_all_bounded` returns `Result<Vec<T>, FanOutError>`. A panicking input is a typed `Panicked`, not a
  re-raised panic, so the fan-out no longer calls `resume_unwind`. `FanOutError` is the one new public item.

## Correspondence

Refines ADR 0021 and the `script!` surface of #526. B4 (#1017) and the carves of ADR 0052 act on what is left.

## Verification

`tests/it/kernel_surface.rs` holds the root list and names `kernel::task`, `kernel::cap` and `kernel::sealed`.
`tests/it/supported_surface.rs` builds a durable two-step flow and a two-route app through the supported paths only.
`check_std_first::surface::tests::a_new_pub_item_inside_a_hidden_module_is_refused_by_its_own_file` holds #896's
cross-file claim.
