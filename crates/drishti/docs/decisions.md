# Decisions and unresolved obligations

Direction is not implementation acceptance. `user-direction` records the Director's instruction; `proposed` records an engineering recommendation; `blocked` requires evidence before runtime implementation. None means a runtime test passed.

## Decision ledger

| ID | Status | Decision | Rationale / rejected alternative |
| --- | --- | --- | --- |
| D01 | user-direction | Frontend is orchestration; creator vision is primary | Reject a renderer-centric or GPU-benchmark-centric product |
| D02 | user-direction | Rust-first authoring with permitted JS/Wasm glue and hybrid HTML/GPU presentation | Amended by Q01/Q02 and explicit glue approval below; do not require authors to maintain a second application in JavaScript |
| D03 | user-direction | GPUI is the presentation foundation | GPUI is Apache-2.0; rebuilding it to avoid GPL was based on a false premise |
| D04 | user-direction | logicalworks-crates is the primary dependency surface | Reuse owned primitives; admit optional third-party engines through the storefront |
| D05 | proposed | One host-owned authoritative application state; Drishti owns projections and coordination | LOK remains the constellation owner; standalone applications need not run the entire constellation |
| D06 | proposed | One public package with internal ownership modules | Reject a crate per concept and empty scaffold modules |
| D07 | user-direction | Known-dependency scheduling plus adaptive bounded speculation by default when qualified | Q29 supersedes opt-in-only direction; prediction cannot authorize effects; measured admission and pressure shutdown remain mandatory |
| D08 | proposed | Script engines are optional lowering adapters, not foundational application state or policy | Do not replace JS callback sprawl with Rhai/Lua callback sprawl; no new language/VM selected |
| D09 | proposed | Experience-as-Code supplies doctrine/contracts and later independent evaluation | Do not fork its whole runtime or rely on its currently simulated browser provider |
| D10 | user-direction | Keel eventually wired; strict obligations encoded now | Broken tooling is blocked evidence, not grounds to lower standards or report green |
| D11 | user-direction | Repository/product name Drishti | Documentation-only 0.0.1 publication explicitly authorized; no fake runtime to reserve a name |
| D12 | user-direction | Publish design package under Apache-2.0 | Public Cargo design publication approved; GitHub private by communicated default |
| D13 | proposed | Native-trusted and confined-guest profiles have separate guarantees | Rust memory safety and script API restrictions do not prove hostile-code isolation |
| D14 | proposed | Human, accessibility, and agent interactions share scoped semantics | An agent is not a privileged alternate product; visual tasks still need visual proof |
| D15 | user-direction | The Logical Works brand is drishti's default presentation language; the ink lab and GPU residency are the first realized reference core | Director, 2026-09-30: "I want drishti to feel like this as the gpu accelerated runtime and the inklab. Spec is in js but drishti remains the rust framework as specced out in repo." The JavaScript engine (S22) is the behavioural source and drishti stays the Rust framework; this supersedes the earlier documentation-only runtime prohibition for the presentation seats only |
| D16 | user-direction | A working native ink lab over the estate: GPUI admitted through `lgwks_deps`'s `gpui` storefront feature behind drishti's optional `ui` feature | Director, 2026-10-05: "I need a fully working version of this based on our estate". The `ui` feature is off by default, and a direct `gpui` or `wgpu` edge and a forked renderer were rejected |
| D17 | user-direction | Absorb kernel row 15 (protocol, motion, native scene, native host) with falsifying tests; SwiftUI views replaced by Drishti presentation | Director 2026-11-03 kernel#850: deprecate the kernel by absorption into the owning repo. The code lived in a private repository until good enough for the crates workspace; the Director ordered it folded into this crate on 2026-11-14, which publishes it (ADR 0057). This continues D15 for the Canvas seats |
| D18 | user-direction | Native-host broker decisions are scoped, grant-checked, bounded and audited (drishti#8, drishti#9 and drishti#10, issues of the former private repository). Every inspector reads through one filter bound to the caller's session plus tenant, resolved from the broker's own registration, never from a request field. `Reach::AllTenants` needs the broker-local `CrossTenantInspect` grant. The command bar needs `intent.emit` from an injected grant store, checked before any other refusal, and fails with `CapabilityError::NotGranted`. Every decision emits one typed `AuditEvent` to a sink the broker cannot be built without. Every registry has a numeric bound and a typed `LimitExceeded` refusal. Widget ids and rate buckets carry the tenant. | Rejected: a caller-supplied session id or grant slice, an admin see-all exemption, a no-op audit default, and a new `Capability` variant (the enum is schema-locked to the kernel, so a bump is separate). Departure from drishti#8 text: drishti#8 says a caller without the grant gets its own scope back. Asking for `AllTenants` without the grant is instead refused with `CrossTenantDenied` and audited, because a silent downgrade hides a bypass attempt from the caller and a wrong answer is worse than a refusal; a plain `Own` read is unchanged. Every refusal on the emit paths (invalid handle, malformed kind, oversize payload, rate limit) and every unknown-id revocation is audited as `Denied` (amended after the audit review: drishti#9 says every decision is audited, and a hidden flood is worse than a loud one; rate-limit refusals are also still counted in tap events), so the sink owns flood handling. A payload is bounded to 64 KiB serialized, ML peers to 8 per tenant, and a subscription view carries no handle (revocation acts on the owner's nonce). Not yet proved: durable audit storage, sink backpressure, concurrent total order of sink calls (`seq` gives it) |
| D19 | user-direction | Estate bump: `lgwks_std` 4.2, `lgwks_ast` 6.0, and `lgwks_macros` 2.1 (accepted by the lead, an estate crate) | `lgwks_ast` 6 no longer exports the `Error` derive; `lgwks_macros::Error` is its owner, so the old `extern crate lgwks_ast as thiserror` alias was removed. The estate `json::Value` is `#[non_exhaustive]` and has no pointer or index assignment, so conversions end in a `Null` arm and tests rebuild rows. Rejected: a third-party `thiserror` (no new third-party dependency) |

## Source conflicts resolved for this design

| Source tension | Resolution |
| --- | --- |
| Earlier bespoke/wgpu-only design versus GPUI adoption | GPUI reuse supersedes bespoke UI; shared compute/render semantics remain required and unproved |
| Old Canvas local stores versus current LOK charter | Current charter/ADR-101 owns constellation durable state; do not copy old SwiftData mirroring as new authority |
| Browser repo's agent-first vs human-browser product directions | Borrow stable semantic interaction and mediated effects, not its product exclusions or human-UI deferral |
| Earlier strict no-JS direction versus later explicit glue and hybrid approval | Later Director answers supersede the strict loader prohibition; application authoring remains Rust-first and browser integration remains unproved |
| Game-research focus/LOD proposals versus accepted implementation | ADR-099 parks focus-distance and LOD research; do not claim a solved intent metric or adopt untested salience heuristics as universal |
| EAC schemas/docs/provider status differ | Reuse truth separation, map native journeys explicitly; no schema-conformance or real-browser-pass claim from prose |
| Old Keel floors and commands versus current broken checkout | Preserve source/version provenance and strict intent; do not copy stale CLI invocations or advisory metrics as mandatory executable gates |
| Estate prose versus current feature/re-export implementation | Read actual consumer API before dependency integration; the design does not invent missing storefront exports |
| Brand's JavaScript engine versus the Rust framework scope | The brand's `lw-ink.js` is the behavioural specification only. Drishti re-implements it in Rust and does not adopt JavaScript as a runtime, copy the engine as a dependency, or change the framework's ownership contracts to match a web page |

## Blocking proof obligations

These are required research/engineering outcomes, not background TODOs. `blocked` means no runtime acceptance for the affected profile.

| ID | Status | Owner seat | Evidence required to close |
| --- | --- | --- | --- |
| G01 | blocked | hosts/browser.rs | Real hybrid HTML/GPU host with permitted glue, useful pre-Wasm HTML/forms, unified island interaction, wide desktop/mobile browser matrix and declared no-GPU compatibility mode |
| G02 | blocked | presentation/gpui.rs | Pinned usable GPUI and estate facade versions; macro/import compatibility; UI executor lifecycle; native and web package availability; complete license/dependency closure |
| G03 | blocked | presentation/residency.rs | Supported shared device/resource synchronization or an explicitly costed interop boundary; zero-hidden-transfer oracle on real compute-to-render work |
| G04 | blocked | hosts/isolation.rs | Selected isolation mechanism with host-call bounds, adversarial escape/exfiltration tests, cancellation, and no native fallback |
| G05 | blocked | scripting/boundary.rs | Decide whether a script adapter is needed; compare Rhai/Lua/Luau against typed Rust declarations on actual authoring, reload, resource, and security workloads |
| G06 | blocked | operations/lifecycle.rs | Transition model covering local atomic adoption, remote outcome uncertainty, duplicates, authority revocation, and code/state version migration |
| G07 | blocked | execution/budgets.rs | Per-profile numeric budgets, calibrated counters/timers, fairness, saturation, protected-state eviction, and worst-case dependency/parse costs |
| G08 | blocked | presentation/interaction.rs | Real keyboard, screen-reader, multilingual text/IME, selection, focus, interrupted motion, and persistent context journeys |
| G09 | blocked | assurance integration | Restored pinned Keel executable and consumer contract; complete required-check wiring and negative controls; independent runtime/experience evaluators |
| G10 | blocked | authoring/declarations.rs | Demonstrated lower coordination burden than idiomatic Dioxus/SwiftUI/React/direct GPUI without losing domain expressiveness |
| G11 | blocked | hosts/lok.rs | Current host-owned snapshot/action contract and constellation type bindings; no duplicate durable authority; independent standalone-host semantics |
| G12 | blocked | hosts/application.rs | Recovery storage, finite retention and exhaustion policy; dependency trust admission; confined-result information-flow boundary; justification/outcome enforcement and confirmation-policy reconciliation |
| G13 | blocked | presentation/interaction.rs | Shared phone/desktop UI workflows, genuine capability restriction classification, offload/skip declarations, automatic adaptation and coherent renderer switching |
| G14 | blocked | diagnostics/causality.rs | Bounded source-linked cause evidence and compact agent semantics; measured diagnostic time and egress savings without fabricated root causes or hidden data collection |
| G15 | blocked | presentation/residency.rs | A real wgpu/WebGPU backend admitted through `lgwks_deps`; a device capture comparing the modelled `BYTES_PER_SHAPE` transfers with measured ones; a supported shared device/queue residency contract; and a driver-loss replay. The deterministic model in `src/runtime.rs` is not this proof |

These gaps do not block publishing this design as a clearly labeled documentation package. They do block pretending the planned runtime is ready to implement wholesale or ship. Subsequent work must close the prerequisites of a narrowly specified executable slice without redefining the product as that slice.

## Sequence by proof, not by crate count

1. Freeze product requirements and counterexamples; review authoring examples in both the immersive fixture and unrelated forms/editor applications.
2. Resolve host/GPUI/estate dependency seams and supported isolation/browser domains. Measure feasibility rather than invent adapter signatures.
3. Formalize admitted operations, generations, budgets, and recovery, then test a pure reference transition model against independent cases.
4. Integrate a real native GPUI journey plus its full failure and accessibility obligations; do not count it as browser evidence.
5. Prove compute/render interop and measured demand scheduling. Add speculation only when matched workloads justify its cost.
6. Admit a script adapter only after its use case, sandbox profile, and ergonomic advantage are established.
7. Qualify every advertised host and restore strict Keel + independent experience release admission. No aggregate green when a required profile remains unknown.

Changes to decisions must name affected requirements, alternatives, evidence, migration consequences, and renewed acceptance cases. Rewriting a sentence to hide a failed proof is not a decision.

The finalized product contract (the 2026-09-12 amendment and its answer ledgers) is in [contract.md](contract.md).
