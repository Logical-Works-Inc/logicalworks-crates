# lgwks_std 4.2.1 / lgwks_ast 6.1.0 / lgwks_deps 7.3.0 / lgwks_macros 2.1.3

Cut 2026-11-18. CI builds on a dated nightly, and a release builds on stable 1.100.0 (#1090, ADR 0066). `lgwks_ast` refuses more of what a `script!` flow may say (#908, #981).

## What a consumer notices

- Your compiler builds these crates, as before. They still declare `rust-version = "1.98"`. The release gates and `cargo +1.100.0 publish --locked` prove each crate on stable 1.100.0, and only CI and local builds use the nightly pin `nightly-2026-11-12`.
- `lgwks_ast` 6.1.0 refuses more, so a `script!` flow that compiled before can stop compiling. Of the four, only it changes what a flow may say.
- `lgwks_deps` 7.3.0 adds three storefront features, all off by default.
- `lgwks_std` 4.2.1 and `lgwks_macros` 2.1.3 change no behaviour.

## lgwks_ast 6.1.0 (minor: a stricter refusal)

The checker for `script!` flows reads structure, not spelling (#908). Each form below was a way to write a refused call under another name.

| A flow that wrote | Is now |
|---|---|
| `at most (16) at once`, `((16))` or `(8 * 2)` | refused: a bound that names nothing is a typed number |
| `at most (upstream.limit) at once` | accepted: it names its source |
| `for _ in 0u64.. { .. }` | refused: a range with no end loops forever, as `loop` does |
| `r#unwrap`, `thread::r#sleep` | refused as `unwrap` and `thread::sleep` are |
| `spawn_blocking`, `spawn_local` | refused as `spawn` is |
| `use` of `fs`, `net`, `os`, `Command`, `Child`, `Stdio`, `TcpStream`, `TcpListener` or `UdpSocket` | refused with the other imports a flow may not bring in |
| `use` of any `spawn_*` name, or a glob `*` import | refused: a glob brings in names no refusal sees |

No public item was added, removed or renamed. Each flow that now fails names the refused token and the replacement, such as `for x in xs:` over a bounded collection.

## lgwks_deps 7.3.0 (minor: new items)

- Features `p256` (ECDSA over NIST P-256, #840), `libm` (portable float maths, #837) and `desktop-preview` (`winit`, `softbuffer` and `cosmic-text`, #836) are off by default, as every storefront group is (ADR 0003). Each is reached through `lgwks_deps::storefront`.
- The `syn` edge of `lgwks expand` drops its `full` feature and keeps `parsing` and `printing`.
- `lgwks new` pins `lgwks_bot` 9.0.0 and `lgwks_std` 4.2.1, and its app calls `lgwks_bot::script::block_on`. A target directory it cannot read is refused with the reason.
- `check-std-first` reports a binary path guessed from `std::env::current_exe` (INV-CI-BRITTLE-1), and `check-gate-parity` accepts the release pin beside the nightly (INV-CI-TOOLCHAIN-1).

## lgwks_std 4.2.1 and lgwks_macros 2.1.3 (patch)

- `lgwks_std`: no public item and no behaviour changed. Doc comments name `INV-RANDOM-1` and `INV-LEB128-1`, the ids the register holds.
- `lgwks_macros`: the `script!` example in the docs now uses `each n in numbers:` with no typed bound, and its table cites `sim_within.rs`.

`lgwks_bot` 9.0.0 and `drishti` did not move.
