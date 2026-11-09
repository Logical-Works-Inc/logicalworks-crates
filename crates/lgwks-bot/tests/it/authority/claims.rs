//! First-party text revocation claims scanner.
//!
//! Scans repository markdown, code, and config files to guarantee no first-party
//! text claims an in-process revocation operation that `lgwks_bot` does not provide (#41).

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

type TestResult = Result<(), Box<dyn Error>>;

// ── The prose ───────────────────────────────────────────────────────────────

/// The stem the scan looks for: `revo`.
///
/// The word family splits its spelling — `revoke`, `revoked`, `revoking` carry a
/// `k`; `revocation`, `revocable`, `RevocationFence` carry a `c` — and the two
/// earlier versions of this scan each used one of them, so each silently missed
/// half the family. `revok` could not see `RevocationFence`; `revoc` could not
/// see `revoked`, which is the word the original defect was written with. The
/// prefix is what they share, and nothing else in this repository's prose
/// contains it.
const REVOCATION_STEM: &str = "revo";

/// How much flattened text either side of a mention counts as its context.
const WINDOW: usize = 200;

/// Phrases that turn a mention of revocation into a denial that the operation
/// exists. A claim needs only one of them nearby: the point is that a reader
/// who lands on the mention is told the truth there.
const DENIALS: [&str; 7] = [
    // `GrantSet` has no `revoke`, and no method that removes a capability.
    "no revoke",
    // `the no revocation boundary` — the guide's name for the same fact.
    "no revocation",
    // `nothing revokes it` — the ECS `fire` comment.
    "nothing revokes",
    // `No revocable authority.` — the guide's non-goal.
    "no revocable",
    // The crate contract's phrase for what an `Auth` is not.
    "not a live lease",
    // `there is no in process revocation and nothing here should be described
    // as one` — the README's own sentence, the one #59 added.
    "no in process revocation",
    // `the name is wrong for what exists` — the security-posture row.
    "wrong for what exists",
];

/// Phrases that tie a mention to the operation that now exists.
///
/// `EcsBot::replace_grants` (#716) swaps the retained set between ticks and
/// reports what it withdrew, so a mention written about that operation is the
/// operation's own documentation rather than a claim with nothing behind it.
/// Flattened text turns `replace_grants` into `replace grants`, which every
/// spelling of the call carries.
const OPERATION: [&str; 5] = [
    // The operation, in any spelling: `replace_grants`, `EcsBot::replace_grants`.
    "replace grants",
    // Its report: `GrantReplacement::withdrawn` / `added`.
    "grant replacement",
    // The property it implements, named as the guide names it.
    "live revocation",
    // The broker's one-way withdrawal of an operation, as its invariant names it.
    "inv fence 1",
    "sim broker fence",
];

/// Phrases that show the mention is about a different subject.
///
/// Neither subject is authority lifetime: a lexicon **alias row**
/// (`language.rs`, `general-bot-fold.md`), which is deleted rather than revoked,
/// and a **sibling repository's** revocation machinery — `Rocco`'s
/// `rocco-runtime::RevocationFence`, which `docs/estate-asset-inventory.md`
/// inventories as an asset to port and `docs/guidance-runner-spec.md` cites as
/// one of the shapes a future runner would draw on. Neither is a claim about
/// `lgwks_bot`.
const OTHER_SUBJECTS: [&str; 5] = [
    // The alias table: "revoking one is deleting a row".
    "deleting a row",
    // The `learn` method's doc: "a row is readable and revocable which a
    // fitted weight is not".
    "readable and revocable",
    // The test that pins the alias behaviour.
    "learned alias",
    // `Rocco`'s type, named as an asset rather than as this crate's.
    "revocationfence",
    // The runner spec's sentence about where the estate's assets stop: "the
    // estate assets inform the shape ... while the bot keeps its own seams".
    "estate assets inform the shape",
];

/// Files the scan does not read, each with the reason it is not read.
///
/// One file, and the cost is stated: nothing in it is checked. It is the
/// state-of-the-art survey — macaroons, leases, revocation fences, published
/// work and design targets — so every mention in it is a claim about the field
/// rather than about this crate's shipped surface, and the phrases that would
/// qualify them are not denials of a shipped operation.
///
/// The four `dist_lease` entries are the same bargain for a different reason:
/// coordination-lease fencing (`LeaseAuthority::acquire`/`revoke` move the
/// epoch past outstanding leases; proved by
/// `dist_lease::a_revoked_lease_grants_nothing`) is a different subject from
/// `GrantSet` capability lifetime, which has no `revoke` and is what this scan
/// protects. A mention there claims supersession of a coordination lease, not
/// revocation of a capability.
///
/// The five `crates/drishti` entries are another crate's design record (ADR 0006
/// R6): each mention is a requirement Drishti's blocked proof gates G01–G15 must
/// meet, about Drishti's own sessions, so none states what `lgwks_bot` ships.
///
/// The three `cap_token` entries are the same bargain for bearer tokens (#779):
/// an offline token-id ledger with an explicit tested verify, attenuate and
/// decide surface is a different subject from `GrantSet` capability lifetime,
/// which has no `revoke` and is what this scan protects. A mention there claims
/// denial of a bearer token by id, not revocation of a capability.
///
/// The six broker entries are the same bargain for effect attempts (#876):
/// `Broker::revoke_operation` is a shipped, tested one-way withdrawal of one
/// operation's dispatch authority, and it says nothing about `GrantSet`.
const NOT_AUTHORITY_CLAIMS: [(&str, &str); 22] = [
    (
        "crates/lgwks-bot/src/broker.rs",
        "the broker's refusals name its shipped withdrawal of an operation, not GrantSet",
    ),
    (
        "crates/lgwks-bot/src/broker/fence.rs",
        "the shipped Broker::revoke_operation itself, not GrantSet",
    ),
    (
        "crates/lgwks-bot/src/bin/broker_fence.rs",
        "the binary that drives Broker::revoke_operation, not GrantSet",
    ),
    (
        "crates/lgwks-bot/tests/it/broker_fence_cli.rs",
        "the journey proof of Broker::revoke_operation, not a capability claim",
    ),
    (
        "crates/lgwks-bot/tests/it/sim_broker_fence.rs",
        "the simulation proof of Broker::revoke_operation, not a capability claim",
    ),
    (
        "docs/adr/0017-a-broker-fence-a-generation-does-not-answer.md",
        "the decision record of Broker::revoke_operation, not GrantSet",
    ),
    (
        "docs/frontier.md",
        "the state-of-the-art survey: its mentions are published research and design \
         targets, not statements about this crate's shipped authority",
    ),
    (
        "docs/frontier-capability-authority.md",
        "the same survey's capability section (ADR 0023 split the page), its mentions \
         published research and design targets",
    ),
    (
        "docs/frontier-synthesis.md",
        "the same survey's synthesis (ADR 0023 split the page), a design target",
    ),
    (
        "docs/frontier-tamper-evident-logs.md",
        "the same survey's log section (ADR 0023 split the page), a design target",
    ),
    (
        "crates/lgwks-bot/src/dist_lease.rs",
        "coordination-lease fencing with an explicit tested supersede operation, \
         not GrantSet capability revocation",
    ),
    (
        "crates/lgwks-bot/src/dist_lease/coordinator.rs",
        "the coordinator that fence moved out of dist_lease.rs (#319): its stale-lease \
         refusals are the same coordination-lease supersession, not GrantSet revocation",
    ),
    (
        "crates/lgwks-bot/tests/it/dist_lease.rs",
        "the behavioral proof of the lease fencing above, not a capability claim",
    ),
    (
        "crates/lgwks-bot/tests/it/sim_dist_lease.rs",
        "the simulation proof of the lease fencing above, not a capability claim",
    ),
    (
        "crates/drishti/VISION.md",
        "Drishti's vision: the human's power to inspect and revoke is a goal for its \
         scheduler, not an lgwks_bot operation",
    ),
    (
        "crates/drishti/docs/architecture.md",
        "Drishti's architecture: a cancellation that cannot revoke a dispatched effect \
         is a rule for its own adoption, not a claim about GrantSet",
    ),
    (
        "crates/drishti/docs/assurance.md",
        "Drishti's assurance plan: revoked authority is a case its blocked gates must \
         exercise, not a shipped operation",
    ),
    (
        "crates/drishti/docs/contract.md",
        "Drishti's requirement contract: capability loss and revocation handling is \
         requirement r34, unmet while G01-G15 are blocked",
    ),
    (
        "crates/drishti/docs/decisions.md",
        "Drishti's decisions: gate G06, blocked, names authority revocation as work its \
         lifecycle must model",
    ),
    (
        "crates/lgwks-bot/src/cap_token.rs",
        "the offline bearer-token id ledger with its explicit tested verify, attenuate \
         and decide surface, not GrantSet capability revocation",
    ),
    (
        "crates/lgwks-bot/tests/it/cap_tokens.rs",
        "the behavioral proof of the bearer-token ledger above, not a capability claim",
    ),
    (
        "crates/lgwks-bot/src/bin/cap_token_cost.rs",
        "the measurement harness driving the bearer-token ledger above, not a capability claim",
    ),
];

/// File extensions the scan reads. A revocation claim is prose or code; a
/// lockfile or an image is neither.
const TEXT_EXTENSIONS: [&str; 8] = ["md", "rs", "toml", "txt", "json", "yml", "yaml", "sh"];

/// Directory names the walk never descends into.
///
/// `vendor` is third-party, `target` is derived, `.git` is history, and
/// `graphify-out`, `.codegraph` and `.lgwks` are derived artifacts — the same
/// set the repository's artifact law refuses to commit. Without them the
/// scan's verdict would depend on whether the machine happened to hold a
/// knowledge-graph export whose node labels quote this file's own test names,
/// which is how a guard green in CI fails on a developer's machine.
const SKIPPED_DIRECTORIES: [&str; 6] = [
    "target",
    "vendor",
    ".git",
    "graphify-out",
    ".codegraph",
    ".lgwks",
];

/// How far below the repository root the walk descends.
const MAX_DEPTH: u8 = 6;

/// `text` with every run of non-alphanumeric characters collapsed to a single
/// space and ASCII letters lowercased.
///
/// The window rule has to survive a line break inside the phrase it is looking
/// for — `GrantSet` has no\n`revoke` is a denial whose marker a wrap splits —
/// so the scan runs on flattened text rather than on lines. The output is pure
/// ASCII, which is what makes byte offsets into it safe to slice.
fn flatten(text: &str) -> String {
    let mut flat = String::with_capacity(text.len());
    for character in text.chars() {
        if character.is_ascii_alphanumeric() {
            flat.push(character.to_ascii_lowercase());
        } else if !flat.ends_with(' ') {
            flat.push(' ');
        }
    }
    flat
}

/// Every mention of revocation in `text` whose context carries no denial and
/// puts it in no other subject.
///
/// Returns the context of each unqualified mention, for the failure message.
/// An empty result is the property the issue asks for: nothing in this file
/// claims an operation that does not exist.
fn unqualified_mentions(file: &str, text: &str) -> Vec<String> {
    let flat = flatten(text);
    let mut flagged = Vec::new();
    let mut cursor = 0usize;

    while let Some(found) = flat
        .get(cursor..)
        .and_then(|rest| rest.find(REVOCATION_STEM))
    {
        let at = cursor.saturating_add(found);
        // The context is taken in whole characters from boundaries `char_indices`
        // reports, and both ends are clamped to the text, so the slice is total:
        // a mention at either edge still gets its window rather than a
        // substitute for one.
        // A mention nearer an edge than the window starts or ends the context
        // at that edge: the text itself is the bound, not a substituted value.
        let start = match flat[..at].char_indices().nth_back(WINDOW) {
            Some((offset, _)) => offset,
            None => 0,
        };
        let end = match flat[at..].char_indices().nth(WINDOW) {
            Some((offset, character)) => at
                .saturating_add(offset)
                .saturating_add(character.len_utf8()),
            None => flat.len(),
        };
        let window = &flat[start..end];
        let qualified = DENIALS
            .iter()
            .chain(OTHER_SUBJECTS.iter())
            .chain(OPERATION.iter())
            .any(|phrase| window.contains(phrase));
        if !qualified {
            flagged.push(format!("{file}: {window}"));
        }
        cursor = at.saturating_add(REVOCATION_STEM.len());
    }

    flagged
}

/// Whether `path` is this file, `tests/it/authority/claims.rs`, or `tests/it/authority.rs`.
///
/// The scan skips them, and they are the only files that are skipped. These files are
/// *about* revocation, so they name the operation on nearly every line and the
/// scan would report the guard itself as a claim. Everything else in the
/// repository is read.
fn is_this_file(path: &Path) -> bool {
    let file = path.file_name().and_then(|name| name.to_str());
    let directory = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str());
    (file == Some("authority.rs") || file == Some("claims.rs"))
        && (directory == Some("it") || directory == Some("authority"))
}

/// Every first-party text file at or under `root`, sorted, with the walk's
/// skips applied.
fn first_party_files(root: &Path) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let mut found = Vec::new();
    let mut pending = vec![(root.to_path_buf(), 0u8)];

    while let Some((directory, depth)) = pending.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                let name = entry.file_name();
                let skip = SKIPPED_DIRECTORIES
                    .iter()
                    .any(|skipped| name.to_string_lossy().eq_ignore_ascii_case(skipped));
                if !skip && depth < MAX_DEPTH {
                    pending.push((path, depth.saturating_add(1)));
                }
                continue;
            }
            let readable = path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| TEXT_EXTENSIONS.contains(&extension));
            if readable && path.file_name().is_some_and(|name| name != "Cargo.lock") {
                found.push(path);
            }
        }
    }

    found.sort();
    Ok(found)
}

#[test]
fn the_claim_guard_flags_the_claim_it_was_written_for() -> TestResult {
    // The sentence this guard exists for, quoted from `docs/general-bot-fold.md`
    // as it stood when #41 was filed: a live-revocation guarantee with no
    // operation behind it. A guard that does not flag this one is a no-op, and
    // the run of [`no_first_party_text_claims_revocation_this_crate_lacks`]
    // before the fix is what it looked like in the repository.
    let claim = "| REST gateway with JWT and scopes | `Cap` / `GrantSet`, proof-carrying | \
                 Strictly stronger: a scope checked once at a gateway is ambient for the rest \
                 of the request; `Auth` is re-proved on every call, so a grant revoked after \
                 build cannot fire (`lib.rs`) |";

    let flagged = unqualified_mentions("synthetic.md", claim);
    assert_eq!(
        flagged.len(),
        1,
        "the guard must flag the claim it was written for, and flag it once: {flagged:#?}"
    );

    // The control for the control: the same claim with the denial the fix added
    // is not flagged, so the guard is not simply reporting every mention.
    let denied = format!("{claim} `GrantSet` has no revoke operation");
    assert!(
        unqualified_mentions("synthetic.md", &denied).is_empty(),
        "a claim that carries the denial must pass: {:#?}",
        unqualified_mentions("synthetic.md", &denied)
    );

    // The operation's own documentation is not a claim: a mention that names
    // `replace_grants` passes, so the guard keeps refusing unqualified claims
    // without forbidding the operation's manual.
    let operated = "revocation is EcsBot::replace_grants, which swaps the retained set and reports grant replacement";
    assert!(
        unqualified_mentions("synthetic.md", operated).is_empty(),
        "a mention tied to the shipped operation must pass: {:#?}",
        unqualified_mentions("synthetic.md", operated)
    );

    // Both spellings of the family, because each earlier version of this scan
    // used one stem and could not see the other: a claim written with the verb
    // (`revoke`, the word the original defect used) and one written with the
    // noun (`revocable`, which is what `RevocationFence` is built from).
    for probe in [
        "the host can revoke a capability at runtime and the next call is denied",
        "authority is revocable through whatever grant set the caller holds",
    ] {
        assert_eq!(
            unqualified_mentions("synthetic.md", probe).len(),
            1,
            "the guard must report a claim written with either spelling: {probe}"
        );
    }
    Ok(())
}

#[test]
fn no_first_party_text_claims_revocation_this_crate_lacks() -> TestResult {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let files = first_party_files(&root)?;

    // The walk is the evidence, so its reach is asserted rather than assumed:
    // a walk that reaches nothing would report every file as claim-free.
    assert!(
        files.len() > 50,
        "the walk found only {} text files under {}; too few to be the repository",
        files.len(),
        root.display()
    );
    for expected in [
        "docs/security-posture.md",
        "docs/general-bot-fold.md",
        "crates/lgwks-bot/src/lib.rs",
        "crates/lgwks-bot/README.md",
    ] {
        assert!(
            root.join(expected).is_file(),
            "the walk's roots do not reach {expected}: the corpus is not the repository"
        );
    }

    let mut flagged = Vec::new();
    let mut mentions = 0usize;
    let mut checked_files = 0usize;
    let mut exempt_seen = vec![false; NOT_AUTHORITY_CLAIMS.len()];
    for path in &files {
        if is_this_file(path) {
            continue;
        }
        let relative = path
            .strip_prefix(&root)
            .map_err(|error| format!("{} is outside the repository root: {error}", path.display()))?
            .to_string_lossy()
            .into_owned();
        if let Some(index) = NOT_AUTHORITY_CLAIMS
            .iter()
            .position(|entry| entry.0 == relative)
        {
            if let Some(seen) = exempt_seen.get_mut(index) {
                *seen = true;
            }
            continue;
        }
        let text = fs::read_to_string(path)
            .map_err(|error| format!("{} could not be read: {error}", path.display()))?;
        checked_files = checked_files.saturating_add(1);
        mentions = mentions.saturating_add(text.to_lowercase().matches(REVOCATION_STEM).count());
        flagged.extend(unqualified_mentions(&relative, &text));
    }

    // A stale exemption is an unchecked file nobody noticed: every name in the
    // list has to be a file the walk actually reaches.
    for (entry, seen) in NOT_AUTHORITY_CLAIMS.iter().zip(exempt_seen.iter()) {
        assert!(
            *seen,
            "{} is exempt from this scan ({}) and the walk did not reach it, \
             so the exemption is protecting nothing",
            entry.0, entry.1
        );
    }

    // The scan's own control: at least one mention must have been found, or
    // the assertion below is passing on a corpus with nothing in it.
    assert!(
        mentions > 0,
        "no mention of `{REVOCATION_STEM}` was found in {checked_files} files: the scan is not looking where the claims are"
    );
    assert!(
        flagged.is_empty(),
        "these mentions read as revocation claims, and no operation backs them:\n{flagged:#?}\n\
         Each mention must sit in text that denies the operation (`{DENIALS:?}`), or in one that \
         shows it is about another subject (`{OTHER_SUBJECTS:?}`), or in one that ties it to the \
         shipped operation (`{OPERATION:?}`), or the file must be listed in \
         `NOT_AUTHORITY_CLAIMS` with the reason it is not a claim about this crate"
    );
    Ok(())
}
