# canvas-protocol motion conformance fixtures

Language-neutral JSON fixtures for the motion engine protocol. These fixtures validate that any consumer (TypeScript broker, Host-Native Logic kernel, Rust) agrees on envelope validation.

## Structure

| Path | Shape | Expected outcome |
|------|-------|-----------------|
| `envelope/intent-*.json` | Full `motion.intent` envelope | Must pass validation |
| `envelope/lifecycle-*.json` | `motion.lifecycle.*` event | Must pass validation |
| `envelope/rejected/*.json` | Malformed envelopes | Must FAIL validation |

## Declared schemas

- `motion.intent` → `MotionIntent.json`
- `motion.patch` → `MotionPatch.json`
- `motion.lifecycle.*` → `MotionLifecycle.json`

## Usage in TypeScript

See `src/__tests__/motion-conformance.test.ts`.
