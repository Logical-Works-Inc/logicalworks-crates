# ADR 0013: serve emits an OpenAPI 3.1 document from the route registry

Status: **accepted**, 2026-11-05. Follows #830 and #782 for typed route reflection and documentation emission.

Stakeholders: the Director; API consumers; developers deploying estate serve.
Concerns: runtime reflection overhead on HTTP endpoints; manual documentation drift from route implementation.
Requirements: R1, R2, R13
Viewpoint: the HTTP edge contract viewpoint, mapping in-memory routes to machine-readable OpenAPI specifications.
Elements and relations: `lgwks_bot::rt::serve::openapi`, `RouteDescriptor`, `emit_openapi`, and `estate serve`.

## Context

The HTTP edge in `lgwks_bot::rt::serve` routes requests by tenant and path. Clients and upstream gateways need machine-readable contracts to discover route schemas, methods, authentication posture, and parameters. Reflection at runtime on each request adds latency, while manual OpenAPI maintenance drifts from the actual handler implementation.

## Decision

1. **In-memory RouteDescriptor.** Each typed route registers an explicit `RouteDescriptor` carrying method, path, tags, operation id, summary, parameters, request body schema, responses, and security posture (`x-auth-posture`).
2. **Deterministic emission.** `lgwks_bot::rt::serve::openapi::emit_openapi` builds a valid OpenAPI 3.1 document from the in-memory route registry using `lgwks_std::json`.
3. **Fail-closed startup gate.** `estate serve` checks that every registered route has a valid descriptor before binding ports. Duplicate routes, relative paths, and undescribed operations are refused at startup.
4. **Standard contract endpoint.** The edge exposes `GET /t/{tenant}/openapi.json` returning the generated document.

## Alternatives rejected

- **Runtime reflection per request.** Adds per-request serialization overhead and defers misconfiguration discovery to runtime traffic.
- **Manual OpenAPI specification maintenance.** Vulnerable to specification drift when handlers or route paths change.

## Consequences

- Route metadata is declared once alongside handler definitions.
- The route registry and emitted OpenAPI document agree in both directions.
- Misconfigured or undescribed routes fail closed at server initialization.
- Maximum route count is bounded to 256 entries to prevent unbounded memory growth.

## Correspondence

Refines ADR 0006: the HTTP edge driver serves its typed contract directly. Relates to ADR 0012 for complete API documentation.

## Verification

- `crates/lgwks-bot/tests/it/serve_openapi.rs` validates route registration, OpenAPI 3.1 schema emission, and unknown-app 404 responses.
- Parity tests confirm parity boundaries refuse undescribed routes, relative paths, and duplicate methods.
