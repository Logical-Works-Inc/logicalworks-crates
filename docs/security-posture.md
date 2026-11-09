# Security posture

Continued in: [What the design forbids](security-posture-forbidden.md), [The evidence packet](security-posture-evidence.md).

Status: reference, and the external case. What a security reviewer can check
without trusting the vendor, where each claim stops, and the artifacts this
project must be able to produce. Companion to
[`guidance-runner-spec.md`](guidance-runner-spec.md).

## What this page is

The commercial problem is not performance. It is that a component which drives
a customer's application has to be approved by a security team whose default
answer is no, and whose reason is almost always the same: *automation that runs
inside our page is a script on our page.*

So this page does two things. It states the architectural claim that
distinguishes this component from that class, and it states — in the same place,
at the same weight — what the claim does **not** cover. A vendor document that
only does the first is worth less than nothing to a competent reviewer, because
the reviewer finds the missing ceiling themselves and then discounts everything
else.

## The claim

**`lgwks_bot` executes no code in the consumer's browser.** It is a Rust library
with no browser driver of its own: `crates/lgwks-bot` holds no CDP session, no
injected script and no `Runtime.evaluate`. A browser-driving consumer supplies the
driver behind the `Observe` and `Execute` verbs. The design this page holds that
driver to is an operator-controlled browser, resolved from the structural DOM and
accessibility tree, with no extension in the critical path.

The first sentence is checkable, not a policy statement: search the crate's source
for a JavaScript evaluation surface. The in-page driver lives in a separate
repository and is not a dependency of the published crate.

### Its ceiling, stated first

This is a **textual argument** from the requirement's own wording. No regulator,
acquirer, or QSA has published a determination applying the scope test to an
out-of-process driver. So:

- We do **not** claim PCI DSS 6.4.3 compliance on a customer's behalf.
- We **do** claim the product supplies the mechanical evidence an assessor's
  evidence list asks for, and never executes in the consumer's browser.
- Presented to a security team, the argument goes with the Customized Approach
  Objective quoted, never as a settled interpretation.

## Why the locus is the load-bearing fact

PCI DSS v4.0.1 **6.4.3** and **11.6.1** became effective **31 March 2025** (the
rest of v4.0 was 31 March 2024). Note the numbering: 11.6 is the heading and
11.6.1 is its only sub-requirement — bare "11.6" is not a citable requirement.

Both are scoped to the consumer's browser, in three separate strings, in the
standard and its glossary:

| Source | Text |
|---|---|
| 6.4.3 | "All payment page scripts that are **loaded and executed in the consumer browser** must be managed as follows: a method is implemented to confirm that each script is authorized; a method is implemented to assure the integrity of each script; an inventory of all scripts is maintained with written business or technical justification for why each is necessary." |
| Glossary | A script is "any programming language commands or instructions on a payment page that are **processed and/or interpreted by a consumer's browser**, including commands or instructions that interact with a page's document object model (DOM)". |
| 11.6.1 | change- and tamper-detection over "the security-impacting HTTP headers and the script contents of payment pages **as received by the consumer browser**", alerting on unauthorized modification including changes, additions and deletions, at least weekly or at a TRA-derived cadence. |

The Customized Approach Objectives are tighter still — 6.4.3's is that
"unauthorized code cannot be executed on the payment page **as it is rendered in
the consumer's browser**."

The scope is the artifact the consumer's browser receives and runs. It is not
the act of driving a page.

### The PCI SSC documents the injection problem itself

The Council's Information Supplement on these two requirements names two
monitoring models, and one of them is this architecture:

> **Agentless:** a process or service (for example a headless browser) that
> regularly navigates through checkout flows, observing loaded scripts, headers,
> and behaviors without introducing additional scripts in real users' sessions.

It lists agent-based monitoring's disadvantages in its own words — "require
integration into each page; incorrectly implemented solutions could affect
performance or conflict with other scripts" — and it records the gaps in the
browser-native controls a site would use against an injected agent:

| Control | The supplement's own stated limitation |
|---|---|
| SRI | "fails silently — there is no native alert mechanism"; "does not support reporting"; "not practical for rapidly changing scripts (for example, dynamic third-party scripts)" |
| CSP | "does not maintain a baseline of normal activity"; "static and cannot track historical or expected states across sessions"; "not inherently able to detect deletions of security-impacting headers" |

An injected automation agent is therefore simultaneously **in scope** for 6.4.3
— it is a new inventory line needing written justification, and a new 11.6.1
alerting subject — and **in conflict** with the controls the merchant is using
to satisfy 6.4.3, since CSP hash-pinning and nonces break injected agents
("using nonces with third-party hosted scripts is impractical").

Two scope facts matter commercially and should be checked with the customer's
acquirer rather than assumed: 3DS scripts are carved out of 6.4.3, and SAQ A had
6.4.3, 11.6.1 and 12.3.1 **removed**, replaced by an eligibility criterion the
merchant satisfies "by using techniques such as, but not limited to, those
spelled out in PCI DSS Requirements 6.4.3 and 11.6.1."
