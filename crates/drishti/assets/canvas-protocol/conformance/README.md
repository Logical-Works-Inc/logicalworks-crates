# canvas-protocol conformance fixtures

Language-neutral JSON test fixtures for the canvas substrate protocol. These fixtures validate that any language implementation (TypeScript, Host-Native Logic, Rust, etc.) agrees on signal envelope validation, pattern matching, capability gates, and session invariant checks.

## Structure

| Directory | Count | Shape |
|---|---|---|
| `valid_envelopes/` | 30 | Raw envelope object, must pass `validateEnvelope` |
| `invalid_envelopes/` | 30 | `{ "input": ..., "expected_error": "..." }` — must fail `validateEnvelope` |
| `pattern_match/` | 50 | `{ "pattern": ..., "envelope": ..., "expected": bool }` |
| `capability/` | 15 | `{ "have": [...], "need": "...", "expected": "ok" | "err" }` |
| `invariant/` | 10 | `{ "input": ..., "expected": "ok" | "err" }` |

## Usage in TypeScript

See `src/__tests__/conformance.test.ts`.

## Usage in Host-Native Logic

See `surfaces/mac/Tests/CanvasProtocolTests/ConformanceTests.Host-Native Logic`.

## Adding fixtures

Name fixtures after the behaviour being tested, not the object shape. One assertion per file. Use UUIDs from the `018f5b3a-4c1d-7000-8e2f-1a2b3c4d5eXX` base with the last two hex digits varying.
