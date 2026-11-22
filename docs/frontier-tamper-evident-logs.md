# Frontier definition: Tamper-evident logs

Part of [Frontier definition](frontier.md).

## 8. Tamper-evident logs and provenance: the minimum is small, and one detail is critical here

**Frontier claim.** Transparency-log ideas for AI agent action records are now
the most crowded standards area in agent infrastructure. At least nine IETF
drafts, two evaluated systems, an ISO standard and a formal systematization all
converge on signed hash-linked records with an external non-equivocation
commitment. Nothing is deployed at production scale, and the record schema is
contested.

**Anchors.**

- RFC 6962 / RFC 9162 (Certificate Transparency): the only production
  transparency log. Its split-view defense, gossip, **never became an RFC**. That
  is the plain statement of what CT guarantees.
- RFC 9943 (SCITT architecture, 2026): generalizes transparency services with
  receipts, the substrate every agent-audit profile now builds on.
- `draft-emirdag-scitt-ai-agent-execution-00` (Apr 2026): AgentInteractionRecord
  as a COSE_Sign1 payload with hash chain, sequence numbers, identity-binding
  levels, a **redaction receipt**, and compliance mappings to EU AI Act Arts. 12
  and 19.
- Agent Flight Recorder (IEEE BCCA 2026, arXiv 2609.01931): the first *evaluated*
  agent audit system. Eight fields, deterministic CBOR, **48 µs/event**, 100%
  detection of edit/delete/reorder/fork at zero false positives.
- `draft-sahu-agent-action-receipts-00` (Aug 2026): the tightest statement of the
  minimum. Attributable, tamper-evident *as a sequence*, verifiable offline as a
  pure function of records plus an out-of-band trust anchor.
- Cryptographic Erasure on Public Ledgers (IACR ePrint 2026/1109): systematizes
  retention against immutability, and finds the field over-invested in
  chameleon-hash chain rewriting.
- **EU AI Act Art. 12** (Reg. 2024/1689): high-risk systems "shall technically
  allow for the automatic recording of events over the lifetime of the system".
  Application 2 Dec 2027 (Annex III) / 2 Aug 2028 (Annex I), paired with Art. 19
  retention.

**Solved.** The cryptographic core: a per-record signature, a digest of the
predecessor's **exact transmitted octets**, a monotonic sequence number, Merkle
batching, and an epoch root published where the operator cannot fork it.
Crypto-shredding is solved as a technique: destroy the key, leave the bytes.

**Open.** The record schema is unsettled, so a vendor choosing today chooses a
draft. There is no *deployed* third party: no production transparency log for
agent actions, and no equivalent of CT's monitors. Completeness against a hostile
recorder is undetectable by construction, and a **bounded log makes this worse**.
Retention under conflicting regimes, and the post-compromise boundary as Schneier
and Kelsey drew it in 1998, remain open.

**What it changes.**

- **Chain each journal entry over the exact transmitted bytes of its
  predecessor**, including fields a verifier does not recognize. Verification then
  needs no agreement on canonical form. That matters here: four repositories share
  a **byte-identical BLAKE3-256 identifier, the exact convention a
  re-canonicalizing digest would silently diverge on**. Add a monotonic sequence
  number: without it, neither the bound nor an omission is provable.
- **Split authority across two seams, because a hash chain does not detect
  equivocation.** The in-process journal stays authoritative for the fold. A
  separately published epoch root becomes authoritative for non-equivocation. The
  bounded-append refusal is the right backpressure, but **the last-accepted
  sequence number and root must be committed outside the journal**. Otherwise the
  ceiling is a truncation attack, not a bound.
- **Never fold over a fact you must forget.** Record the fact shape plus a 32-byte
  commitment. Keep the payload in a separately keyed store with per-event derived
  keys, so key destruction meets retention while replay still folds. Adopt
  redaction-receipt semantics (record that a field *was* redacted and keep its
  hash), so replay tells "redacted" from "never present". **Reject chameleon-hash
  chain rewriting**: it mutates a committed leaf and would break the shared
  identifier convention.
- **The two-clock model maps onto the record.** The reducer's decision with its
  policy version is a *recorded* field. The revocation fence's freshness must
  **never enter the fold**: a folded freshness is the yesterday's-evidence failure
  the design exists to prevent. Carry the fence generation as provenance beside
  the policy version.
- **Art. 12 is a commercial lever, not a compliance chore.** Automatic event
  recording for high-risk systems becomes mandatory in 2027–2028. A bot that
  already produces a verifiable action record sells into that requirement and
  does not retrofit for it.
