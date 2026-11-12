# lgwks_bot 8.3.0

Released 2026-11-09. A route can now answer with what a durable write did, its request held open while the write runs off every worker.

## lgwks_bot 8.3.0 (minor)

- `Effect::Command` runs `Provider::command` on a bounded pool under the `app.command` cap, and its `on` handler answers the request (#898, ADR 0025).
- `CommandLimits` and `App::command_limits` set the threads, the commands a host and a tenant may hold, and the deadline. The defaults are estimates, so measure your own service.
- Past a ceiling the client hears `503` and nothing ran. Past the deadline it hears `504`, and a late answer still folds. A held request nothing answers is a `500`.
- A refused answer now settles its waiter with a `503` that names a retry, where it used to hang the client.
- The asker reaches the provider, and stays with a chain of commands.
- A rebuild never runs a command again. A kill between the write and the reply fails the request and leaves the write, so a service keys its writes.
- The `command-journal` binary shows that pattern: a key per write, a group commit per tenant, and a real kill between the sync and the reply. `edge-load app-command` times the round trip.

`lgwks_std`, `lgwks_ast`, `lgwks_deps`, `lgwks_macros` and `drishti` did not move.
