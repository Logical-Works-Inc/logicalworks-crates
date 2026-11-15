# drishti 0.2.0

Released 2026-11-14. The Logic OS Canvas code that lived in a private repository moved into `crates/drishti`, which is now its one home (ADR 0058).

## drishti 0.2.0 (minor)

- New modules: `canvas_protocol`, `canvas_scene`, `motion`, `native_host` and `preview`. `native_host` holds the tenant-scoped signal broker, the audit sink, the SQLite governance store, the command bar and the attestation evidence.
- New bins: `canvas_preview`, `trace_check`, `api_drive_motion`, `api_drive_scene` and `api_drive_host`. The last one walks the broker from registration to withdrawal and prints each audited decision.
- `drishti` declares no third-party crate. SQLite, P-256, float maths and the desktop preview reach it through `lgwks_deps` features `sqlite`, `p256`, `libm` and `desktop-preview`.
- The crate holds the workspace lint contract instead of its own.

`lgwks_deps` gains the default-off features `p256`, `libm` and `desktop-preview` (winit, softbuffer, cosmic-text), each with a gap issue (#840, #837, #836). Its version did not move.

`lgwks_std`, `lgwks_ast`, `lgwks_macros`, `lgwks_bot` and `lgwks_deps` did not move.
