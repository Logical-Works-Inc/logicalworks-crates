# lgwks_std 4.0.0

2026-11-03, released 2026-11-04 with the [09 notes](09-sealed-ids-script-edge-2026-11-04.md)

`lgwks_std::json::Value` is this crate's own tree, not a re-export of
`serde_json::Value` (#831). Parsing the 120-byte `latency_tails` document made
14 to 16 heap allocations, one per key, string, tree node and array; the
hosted `ubuntu-24.04-arm` runner read the `json parse` row at 543 ns against
its 500 ns bound. The workload and the bound are unchanged.

## lgwks_std 4.0.0 (breaking)

- `json::Value` keeps serde_json's variant names (`Null`, `Bool`, `Number`,
  `String`, `Array`, `Object`) and is `#[non_exhaustive]`, so a `match` from
  another crate needs a wildcard arm.
- `Value::String` holds a `json::Text`: a string of up to 22 bytes is
  stored inline with no allocation, a
  longer one in one exact `Box<str>`. It reads as `&str` (`Deref`, `AsRef`,
  `Borrow`, `as_str`) and converts from and into `String`.
- `json::Map` is not generic: one vector sorted by key, so an object is one
  allocation, a lookup is a binary search, keys serialize in the order the
  `BTreeMap` map did, and a repeated key keeps its last value. Iteration
  yields `(&str, &Value)`; `into_iter` yields `(String, Value)`.
- `json!` builds the new `Value`; `From` converts between `Value` and
  `serde_json::Value` both ways; `from_value` and `to_value` keep their
  signatures. `Number` and `Error` are still serde_json's.
- Measured on the Director's Mac, three runs each of
  `cargo run --release --locked -p lgwks_std --example latency_tails`:
  `json parse` p50 302 to 310 ns (p99 374 to 388) before, 195 to 196 ns
  (p99 214 to 247) after; `json write` p50 100 to 103 ns before, 124 to
  125 ns after, because reading an inline string checks its UTF-8.
- `lgwks new` scaffolds pin `lgwks_std = "=4.0.0"`.

Dependents that return or re-export `json::Value` (`lgwks_ast`'s legacy and
script verdicts, `lgwks_bot::json`) take the new type with no source change
to their callers beyond `Map<String, Value>` becoming `Map`.
