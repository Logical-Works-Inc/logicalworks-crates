# Security posture: What the design forbids

Part of [Security posture](security-posture.md).

## What the design forbids structurally

Claims enforced by convention decay, so these are to be enforced by types and
seams. The four security-facing commitments below are design, not code: this tree
holds no execution-locus type, observation-scope enforcement or policy-blocked
readiness signal yet. The first five are already binding on all element-resolution work, with their
grounds, in [`guidance-runner-spec.md`](guidance-runner-spec.md). A resolver
never asks a human to adjudicate an `Ambiguous` element; recognition state is
never mutated from user input; there is no fleet-side corpus; resolution is structural only,
with any visual path restricted to Set-of-Mark identifiers and never
coordinates; failure is refusal, not repair.

The security-facing ones added here:

**1. Execution locus is a type, not a claim.** An enum whose variants are
`ConsumerBrowser` and `OperatorControlledBrowser`, where the shipped path can
only construct the second. The transport refuses to attach to a page target
originating in a user-profile tab. The claim in this page then rests on a
type the compiler checks, not on a reviewer's reading.

**2. Structural resolution needs no injected helper — because of the CDP domain
allow-list.** The domains an extension is permitted to reach include
`DOMSnapshot`, `Accessibility`, `Page` and `Input`. Everything the resolver needs
is inside that set. There is therefore no engineering reason to reach for
`Runtime.evaluate`, which is the same conclusion the PCI analysis reaches by a
different route. **The two should be stated as one architectural commitment, not
two.**

**3. Observation scope is declared and enforced, not implied.** The bot has
`GrantSet` bounding what an action may *do*, and nothing bounding what it may
*observe*. Discord's `intents` is the nearest precedent: an enumerated,
declared subscription where capturing an undeclared surface is an error rather
than a silent success. This closes the read side of the same gap.

**4. Enterprise policy can nullify the driver, so that must be a readiness
signal.** On managed fleets, `chrome.debugger.attach()` fails outright under
`ExtensionSettings` blocked hosts and under DLP or `DisableScreenshots`
configuration — which is precisely the population an enterprise sale targets.
A policy-blocked attach needs a first-class typed failure surfaced as readiness,
so fleet-wide nullification is detectable *before* a pilot rather than during it.

## The browser-native integrity stack: proves, and does not prove

Every mechanism here is an equality or authority calculus over bytes. None of
them is a safety property, and the fastest way to lose a reviewer who knows the
specifications is to imply otherwise.

| Mechanism | Status | Proves | Does **not** prove |
|---|---|---|---|
| **SRI** | W3C SRI Level 2 WD; all engines since 2015–2018 | The fetched subresource's bytes are the bytes whose hash was declared | Anything about inline scripts, runtime-injected code, or module-graph members beyond the top-level script; that the bytes are benign |
| **CSP hash-pinning** (`script-src 'sha256-…'`) | CSP Level 3 | Inline script content matches a declared hash | Anything about what that script then loads or does |
| **`'strict-dynamic'`** | CSP Level 3, live | One hash-authorized bootstrap may load descendants | — and that is the problem: it is authority to load an unbounded descendant set, so `'strict-dynamic'` and a pinned module graph are in **tension**. Choosing one is an architectural decision |
| **`require-sri-for`** | **Dead** — absent from the CSP3 grammar entirely; an open W3C issue asks whether to revive it | Nothing; do not build a claim on it | — |
| **`Integrity-Policy`** | Shipped Chrome 138 for script destinations; `sources` currently supports only `inline` | The **browser**, not the vendor, enforced an integrity requirement and reported its own violations via the Reporting API | Anything non-inline; has one engine; a merchant-side synthetic deployment proves the synthetic environment's behaviour only |
| **Import-map integrity** | Chromium 127, Safari 18 | Static imports, top-level module scripts, `modulepreload` | The transitive graph beyond what the map declares |
| **Trusted Types** | W3C WD; **all three engines** — Chrome 83 (2020), Safari 26 (2025-09-15), Firefox 148 (2026-02-24) | DOM-XSS sink assignment (`innerHTML`, `eval`, `script.src`) throws under `require-trusted-types-for` | **Not a sanitizer.** MDN is explicit that the API supplies no transformation; a policy returning its input unchanged satisfies the type system and sanitizes nothing. It constrains *how* data reaches a sink, never *whether* the code is trustworthy |

The premise that Trusted Types is Chromium-only is stale as of roughly seven
months ago, and an enterprise browser baseline may not yet reflect the Firefox
and Safari dates — worth confirming per customer rather than asserting.

**The ceiling in one line:** a hash is provenance and equality, never safety. If
the vendor's own build pipeline is the compromise vector, SRI validates the
attack.

## What we do not claim

Collected in one place so it cannot be mistaken for an omission.

- **Not a sandbox.** In-process code can always dial out directly; capability
  gating is auditable authority, not isolation. This is already stated in
  [`SECURITY.md`](../SECURITY.md) and should be quoted *at* a reviewer rather
  than left for them to find — an overstated sandbox claim is what makes a
  whole packet untrustworthy.
- **No PCI compliance opinion.** The scope argument is textual and has no
  precedent letter.
- **SRI and `Integrity-Policy` prove unchanged, not benign.**
- **SLSA Build L2 at most, not L3.** Any signing step would share a runner with
  the build, so L3's "signing material unreachable by user build steps" is false
  unless a separate signing service holds the key. Do not write "SLSA 3" in a
  questionnaire answer.
- **A signature proves which identity signed these exact bytes at some time,
  and nothing else** — not safety, not authorization, not intent, not
  correspondence to any source. Verification is meaningful only with a pinned
  identity policy; without `--certificate-identity` and
  `--certificate-oidc-issuer`, *any* account on the issuer is a valid signer of
  your artifact.
- **An SBOM is not a vulnerability list.** No exploitability, no reachability,
  no completeness guarantee, stale at the next dependency patch.
- **`NOT_AFFECTED` in a VEX is a producer assertion** with no independent check.
- **A questionnaire has no row for what the automation did to the customer's
  application.** CAIQ and SIG ask about logging and access control in general
  terms. The strongest artifacts in the packet answer a row that does not exist,
  so they must be volunteered into the narrative rather than cited.
