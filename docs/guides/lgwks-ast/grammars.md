# lgwks_ast grammar selection

One cargo feature per grammar forwards to `ast-grep-language`. The default set
holds `full`, which enables all 28, so a default build parses every language in
the table. To carry fewer, turn the default off and name each grammar:

```toml
[dependencies]
lgwks_ast = { version = "6", default-features = false, features = ["lang-c", "lang-cpp", "lang-scala", "lang-kotlin", "lang-tsx"] }
```
`Language::C` is spelled `Language::CLang`. The workspace lint contract forbids
single-character identifiers, and that lint is `forbid` rather than `deny`, so it
cannot be lowered from source by an `#[allow]` on the variant. `Language::name()`
reports `"c"`, which is the stable identity findings match on, and the feature is
`lang-c`.

| Feature | Language | Extensions |
|---------|----------|------------|
| `lang-rust` | Rust | `rs` |
| `lang-python` | Python | `py`, `pyi` |
| `lang-typescript` | TypeScript | `ts`, `mts`, `cts` |
| `lang-javascript` | JavaScript | `js`, `jsx`, `mjs`, `cjs`, `vue`, `svelte` |
| `lang-go` | Go | `go` |
| `lang-java` | Java | `java` |
| `lang-swift` | Swift | `swift` |
| `lang-tsx` | TSX (parsed by the TypeScript grammar; shares it with `lang-typescript`) | `tsx` |
| `lang-c` | C | `c`, `h` |
| `lang-cpp` | C++ | `cpp`, `hpp`, `cc`, `cxx`, `hh`, `hxx` |
| `lang-csharp` | C# | `cs` |
| `lang-scala` | Scala | `scala`, `sc` |
| `lang-kotlin` | Kotlin | `kt`, `kts` |
| `lang-bash` | Bash / POSIX shell | `sh`, `bash` |
| `lang-css` | CSS | `css` |
| `lang-dart` | Dart | `dart` |
| `lang-elixir` | Elixir | `ex`, `exs` |
| `lang-haskell` | Haskell | `hs` |
| `lang-hcl` | HCL / Terraform | `hcl`, `tf` |
| `lang-html` | HTML | `html`, `htm` |
| `lang-json` | JSON | `json` |
| `lang-lua` | Lua | `lua` |
| `lang-md` | Markdown | `md`, `markdown` |
| `lang-nix` | Nix | `nix` |
| `lang-php` | PHP | `php` |
| `lang-ruby` | Ruby | `rb` |
| `lang-solidity` | Solidity | `sol` |
| `lang-yaml` | YAML | `yaml`, `yml` |
| `full` | all of the above | — |

A language whose feature is off does not exist in `Language::ALL` and is never
returned by `Language::of_path`, so a consumer never compiles a grammar it
cannot select.

Extension lookup is a filename heuristic, not a syntax verdict. In particular,
`.vue` and `.svelte` select the JavaScript grammar; they do not enable dedicated
container grammars. An extensionless script is identified by its `#!` line
through `Language::of_shebang`, which `detect` does not consult.

`try_detect_content` returns `ContentDetection::NoMatch`, `Unique(language)`, or
`Ambiguous`; non-syntax parser failures return `Err` because they leave a
required candidate uninspected.
