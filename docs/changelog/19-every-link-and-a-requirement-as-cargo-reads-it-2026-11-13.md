# lgwks_std 4.1.2 / lgwks_ast 6.0.3 / lgwks_deps 7.1.0

Released 2026-11-13. Advisories gate a pre-release as Cargo does, the streaming walk lists every link, and Windows liveness reads the process handle.

## lgwks_deps 7.1.0 (minor)

- `lgwks-deps advisories` reads an advisory requirement exactly as Cargo's `VersionReq` does, pre-release gate included (#942, ADR 0038).
- `patched = [">= 1.2.3"]` no longer reads `2.0.0-alpha.1` as patched, so a lockfile on that pre-release is reported. The judgement is stricter, so this is a minor bump.
- Sparse-index pins (`sparse+https://index.crates.io/`) are matched like git-index pins.
- A requirement RustSec would not load is a typed refusal (exit 2), never a clean report. So is an empty, misnamed or oversized database.
- `lgwks_deps::advisories::audit` and `audit_files` give an engine typed matches and typed errors without spawning the CLI. They are support code, hidden from the rendered docs (INV-SURFACE-1).

## lgwks_std 4.1.2 (patch)

- `fs::walk_dir_iter` lists every symlink as `Symlink`, whatever it names. A link out of the root or to nothing was withheld; now only its descent is (#945, ADR 0039).
- With `follow_symlinks`, a followed link keeps its own name for itself and its inside. A directory a link entered first is still listed, where the walk used to drop it.
- Windows process liveness reads the process handle, not exit code 259, so a child that exits with 259 reads as exited (#941).
- CCITT G3 and G4 fax decoding reads codes from a 64-bit accumulator (#943).
- From earlier merges: SSE framing across reads, RSA-4096 keys and a tree stop past a refusal (#913). A linear FAT cycle check, an `F` sign for unsigned COMP-3 (#937) and decimal digits folded in `u64` chunks (#939).

## lgwks_ast 6.0.3 (patch)

Its gates read what ships, and a gate step cannot swallow its failure (#911). Its registry requirements are carets (#925). No public item changed.

`lgwks_macros`, `lgwks_bot` and `drishti` did not move: their requirements already admit these versions.
