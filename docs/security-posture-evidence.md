# Security posture: The evidence packet

Part of [Security posture](security-posture.md).

## The evidence packet

**None of these twelve artifacts ship today.** They are the packet this project
would have to be able to produce, ranked by evidence value over cost, and the
table is here so the gap is legible rather than implied. "Implied" marks the
ones whose substrate already exists and which therefore need an emitter rather
than a new mechanism — read that column as distance, not as availability. A
reviewer asking for any row is asking for work, not for a file.

| # | Artifact | Proves | Does not prove | Cost | Implied? |
|---|---|---|---|---|---|
| 1 | **OpenVEX document** over the advisory position already written in prose in `SECURITY.md` | A named CVE in a named component is `not_affected` here, with justification and reopen condition | That the reasoning is correct | Hours — the argument exists, only the format is missing | **Yes** |
| 2 | **SBOM per release** (SPDX 3.0.1 or CycloneDX 1.6), from the `Cargo.lock` CI already verifies, targeting the 2026 minimum elements incl. SBOM Generation Context and Component Hash Algorithm | Component inventory by hash | Nothing about vulnerabilities or completeness | One release-job step | **Yes** |
| 3 | **Build provenance, SLSA Build L2** | Which commit, workflow and builder produced these bytes | Anything about behaviour; not L3 | Two workflow lines, no key management | Partly |
| 4 | **Signed execution transcript + journal export with replay** | No vendor code entered the consumer's browser; the exact DOM state acted upon; an independently reproducible outcome | What any given consumer's browser received; that the resolver chose a benign target | A verb plus a versioned export format | Partly — the bounded `session` journal and replay-from-journal exist; signing and a versioned export format do not |
| 5 | **Authority grant and revocation receipt** | The exact authority held, for how long, and that it was revoked | That it was well-scoped or well-granted | A serializable signed manifest | **No** — and the name is wrong for what exists. `cap`/`gate::GrantSet` hold an in-process capability *snapshot* with no revoke operation, so there is nothing to emit a revocation receipt for. See the snapshot boundary in `crates/lgwks-bot/README.md` |
| 6 | **in-toto/DSSE envelopes over 4 and 5**, with estate-owned predicate types | That receipt and artifact are bound together and signed by one identity | Predicate truth — the envelope enforces nothing | Build-time only | No |
| 7 | **Sigstore bundles, persisted** | Which OIDC identity signed, and that the entry is in a public append-only log | Anything else | Low-medium; the discipline is persisting bundles | No |
| 8 | **Script inventory with per-script written justification**, one row per observed script | The literal 6.4.3 third element, on its face | That any listed script is benign or the list complete | Derived from 4 | No |
| 9 | **Per-run digest over the script set and security-impacting headers** | The raw material for 6.4.3's inventory and 11.6.1's change/add/delete alerting in one artifact | What executed after runtime mutation | Derived from 4 | No |
| 10 | **OpenSSF Scorecard + OSPS Baseline self-assessment** keyed to published control IDs | Repo hygiene and branch/artifact posture, re-runnable at a public URL | Code quality | An Action plus a maintained table | Partly |
| 11 | **SSDF-mapped questionnaire answers** — each row answered by a practice ID plus an artifact above | Control coverage in the reviewer's vocabulary | Nothing new | Low, once 1–10 exist | No |
| 12 | **SOC 2 Type II / ISO 27001** | That the organisation's *process* operated over a period | That this binary came from that commit | High — audit fees plus a 3–12 month window | No — pursue under deal pressure |

Three ordering decisions worth stating:

**Item 7 is what makes items 3, 6 and 8 mean anything to a stranger.** Without a
pinned identity policy and the persisted bundle, the signature is an assertion
rather than evidence.

**Items 1, 2, 3 and 10 are the cheap ones** — a reviewer can check them without
cooperating with the vendor.

**Items 4, 5 and 8 are where the verifiability properties are
differentiated**, and they are the two things no standard questionnaire asks
about. A packet that produced only commodity provenance would be
indistinguishable from any other vendor's.

Note on Sigstore specifically: the log went to a tile-backed v2 and **stopped
serving lookup APIs** — the inclusion proof is handed over at upload and not
retrievable later. The bundle is therefore the artifact to persist, in the
hash-pinned content store, which is exactly the persistence layer the log
deliberately removed.

## The five questions this component trips

A browser-driving automation component trips these regardless of which
questionnaire is used (CAIQ v4.1 / CCM v4.1 domain codes, SIG 2025):

| Question | Domain | Answered by |
|---|---|---|
| Where are session cookies, tokens and credentials stored, and are they encrypted at rest? | IAM, CEK | Item 5; the consumer's own credential seams, because this crate stores no session material |
| What network egress is the runner permitted, and how is that enforced? | DCS | `GrantSet`; declared observation intents |
| Can the customer revoke the automation's access, and how quickly? | IAM | **By narrowing** — item 5. `GrantSet` has no revoke operation and a built bot holds a snapshot of the set it was admitted with. `EcsBot::replace_grants` narrows a running bot between ticks, `cap_token`'s `RevocationList::revoke` refuses a token (#779), `broker::fence` withdraws an operation by name (ADR 0017), and `dist_lease` revokes a lease. The two-clock model in `docs/frontier.md` stays a design note |
| Are actions against our application logged in a form we can retrieve and verify? | LOG | Items 4 and 9 — answered as a *customer-facing export*, not an internal capability |
| What data leaves our boundary? | DCS | Declared observation intents; the egress grant |

Answer in two columns and say which is which: **technically verifiable** (SSDF
PS.3, PW.4, RV.1; items 1–10) versus **process, evidenced by report** (item 12).
A reviewer handed only the second column concludes there is no technical story.

## Sources, and what was not retrieved

Primary: PCI DSS v4.0.1 and the PCI SSC Information Supplement *Payment Page
Security and Preventing E-Skimming — Guidance for PCI DSS Requirements 6.4.3 and
11.6.1* (v1.0, March 2025). Also PCI SSC *Summary of Changes r1* (August 2024),
W3C SRI Level 2, CSP Level 3 (WD-CSP3-20260729), and Trusted Types
(WD-trusted-types-20251103). MDN and web-platform-dx give shipping status, with Chrome
Web Store *Additional Requirements for Manifest V3* and the `chrome.debugger`
reference. Supply chain rests on SLSA v1.2, the in-toto attestation spec v1.1.1, CISA's *2026 Minimum
Elements for SBOM* and CISA's VEX minimum requirements. Process rests on NIST SP 800-218 SSDF, the
CISA Secure Software Development Attestation Form, OpenSSF Scorecard and OSPS
Baseline (2025-10-10).

**Not retrieved, and no assertion here should be repeated to a QSA without
them:**

- **Table 7 of the PCI supplement** — the per-element testing procedures and
  expected evidence for 6.4.3 — and the ROC Template's verbatim 11.6.1 testing
  procedures. This is the single most load-bearing gap.
- Mozilla's `Integrity-Policy` and import-map-integrity positions were read as
  issue titles, not bodies.
- `chrome.userScripts` world-isolation semantics and developer-mode gating.
- Whether CycloneDX has shipped past 1.6, and ISO/IEC 27001's current edition
  as of 2026-09.

**The one claim with no precedent behind it** is the load-bearing one: that an
operator-controlled browser is outside 6.4.3's scope. It rests on the
requirement's own repeated "consumer browser" wording. It is a textual argument.
Present it as one.
