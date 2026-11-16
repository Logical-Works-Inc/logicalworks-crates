//! Library doors with no front of their own, read as a downstream caller reads them.
//!
//! A versioned record write, a guidance session's answer, an upstream credential refusal and a torn
//! read are each a public door of the library, and the smoke is the shipped artifact calling each
//! one. Every outcome is asserted, so a door that stops answering, or answers something else,
//! fails the evening rather than a coordinator. Each door is its own function, and each returns the
//! line it proved or the deviation it found.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use lgwks_bot::cap::{Cap, is_credential_status};
use lgwks_bot::cas::{CasInput, NO_VERSION, RecordStore};
use lgwks_bot::domain::cas::CasWrite;
use lgwks_bot::ecs::EcsError;
use lgwks_bot::error::{KernelError, RetryClass};
use lgwks_bot::gate::GrantSet;
use lgwks_bot::session::{
    FlowBounds, FlowSpec, NodeKind, Session, SessionError, Terminal, VarType,
};
use lgwks_bot::spec::{Need, upstream_credential_rejection};
use lgwks_bot::stability::{MAX_STABILITY_READS, Reading, read_stable};
use lgwks_bot::verb::{Execute, Unstable};

/// What one door proved, or the deviation it found.
type Door = Result<String, Deviation>;

/// A library door that answered something other than its contract, with what it answered.
#[derive(Debug)]
pub(super) struct Deviation {
    /// What the door did instead of its contract.
    cause: String,
}

impl std::fmt::Display for Deviation {
    /// The cause as the door observed it.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.cause)
    }
}

impl std::error::Error for Deviation {}

/// Every door, in order, with its scratch files under `dir`.
pub(super) fn run(dir: &Path) -> Vec<Door> {
    vec![
        record_door(dir),
        session_door(),
        credential_door(),
        stability_door(),
    ]
}

/// A deviation, emitted before it is returned so the refusal is a signal and not only a value.
fn deviation(cause: String) -> Deviation {
    lgwks_std::trace::warn!(
        deviation = cause.as_str(),
        "estate-smoke: a library door deviated"
    );
    Deviation { cause }
}

/// A door's refusal: the deviation `cause`, emitted, returned as the door's own result.
fn refuse<T>(cause: impl Into<String>) -> Result<T, Deviation> {
    Err(deviation(cause.into()))
}

/// The versioned record write: a first write installs, a stale one is a typed conflict that is
/// never retried blindly, and a proof that does not cover the file writes nothing.
fn record_door(dir: &Path) -> Door {
    let store = RecordStore::open(dir.join("doors.cas"))
        .map_err(|error| deviation(format!("the record store did not open: {error}")))?;
    let adapter = CasWrite::new(Arc::new(store));
    let auth = GrantSet::empty()
        .grant(Cap::fs())
        .issue(&[Cap::fs()])
        .map_err(|error| deviation(format!("the fs proof was refused: {error}")))?;

    let first = CasInput::new("draft", NO_VERSION, "one");
    let installed = lgwks_std::task::block_on(adapter.execute_action((auth.clone(), &first)))
        .map_err(|error| deviation(format!("a first write was refused: {error}")))?;
    if installed.version() != 1 {
        return refuse(format!(
            "a first write installed version {}, not 1",
            installed.version()
        ));
    }
    if installed.record() != "draft" {
        return refuse(format!(
            "a first write to draft installed {}",
            installed.record()
        ));
    }

    let stale = CasInput::new("draft", NO_VERSION, "two");
    let Err(error) = lgwks_std::task::block_on(adapter.execute_action((auth, &stale))) else {
        return refuse("a stale write installed through the adapter");
    };
    let EcsError::Kernel(KernelError::Conflict {
        expected, found, ..
    }) = error
    else {
        return refuse(format!(
            "a stale write failed as {error}, not as a conflict"
        ));
    };
    if expected != NO_VERSION {
        return refuse(format!(
            "the conflict expected {expected}, not {NO_VERSION}"
        ));
    }
    if found != 1 {
        return refuse(format!("the conflict found version {found}, not 1"));
    }
    if error.retry_class() != RetryClass::Never {
        return refuse("a stale write's conflict allowed a retry");
    }

    let bare = GrantSet::empty()
        .issue(&[])
        .map_err(|error| deviation(format!("the empty proof was refused: {error}")))?;
    let uncovered = CasInput::new("draft", 1, "three");
    if !matches!(
        lgwks_std::task::block_on(adapter.execute_action((bare, &uncovered))),
        Err(EcsError::Kernel(KernelError::CapabilityDenied { .. }))
    ) {
        return refuse("a proof without the fs capability was not denied");
    }
    Ok(format!(
        "record door: installed draft at version {}, a stale write was a conflict (expected \
         {expected}, found {found}), an uncovered proof was denied",
        installed.version()
    ))
}

/// The guidance session: an unrecognized answer leaves the ask current, a recognized one routes
/// and ends the flow, and an answer after the end is a typed refusal.
fn session_door() -> Door {
    let options: Vec<String> = ["small", "large"].map(String::from).to_vec();
    let vars = BTreeMap::from([("size".to_owned(), VarType::Choice(options.clone()))]);
    let routes = BTreeMap::from([
        ("small".to_owned(), "small_end".to_owned()),
        ("large".to_owned(), "large_end".to_owned()),
    ]);
    let ask = NodeKind::Ask {
        var: "size".to_owned(),
        options,
        routes,
    };
    let nodes = BTreeMap::from([
        ("ask_size".to_owned(), ask),
        ("small_end".to_owned(), NodeKind::End),
        ("large_end".to_owned(), NodeKind::End),
    ]);
    let flow = FlowSpec::from_nodes(vars, "ask_size", nodes, FlowBounds::new(8))
        .map_err(|error| deviation(format!("the flow was refused: {error}")))?;
    let mut session = Session::new("estate", flow)
        .map_err(|error| deviation(format!("the session was refused: {error}")))?;

    session
        .answer("something else entirely")
        .map_err(|error| deviation(format!("an unrecognized answer was an error: {error}")))?;
    if session.current() != Some("ask_size") {
        return refuse("an unrecognized answer moved the cursor off the ask");
    }
    session
        .answer("small")
        .map_err(|error| deviation(format!("a recognized answer was refused: {error}")))?;
    if let Some(cursor) = session.current() {
        return refuse(format!("a recognized answer left the cursor on {cursor}"));
    }
    if session.terminal() != Some(&Terminal::Completed) {
        return refuse("a recognized answer did not complete the flow");
    }
    if !matches!(
        session.answer("large"),
        Err(SessionError::SessionTerminated)
    ) {
        return refuse("an answer after the end was not SessionTerminated");
    }
    Ok(format!(
        "session door: one re-ask, one routed answer, and an answer after the end refused in {} steps",
        session.steps()
    ))
}

/// The upstream credential refusal: a credential status carries the repair naming exactly what the
/// call required, a transport status is no credential verdict, and neither is retried.
fn credential_door() -> Door {
    let required = [Cap::net(), Cap::fs()];
    if !is_credential_status(401) {
        return refuse("401 must be a credential status");
    }
    if is_credential_status(500) {
        return refuse("500 is a transport status, not a credential verdict");
    }
    let rejection = upstream_credential_rejection("net::probe", 401, &required);
    if rejection.retry_class() != RetryClass::Never {
        return refuse("a rejected credential must never be retried");
    }
    let EcsError::CredentialRejected { ref needs, .. } = rejection else {
        return refuse(format!(
            "an upstream refusal is CredentialRejected, not {rejection}"
        ));
    };
    let mut sorted = required.to_vec();
    sorted.sort_unstable();
    if !matches!(
        needs.needs(),
        [Need::CredentialExpired { capabilities, .. }] if *capabilities == sorted
    ) {
        return refuse(
            "the refusal must carry one repair re-granting exactly the required capabilities",
        );
    }
    Ok("credential door: a 401 carries its re-grant repair and is never retried".to_owned())
}

/// The stable-read protocol: a subject nobody rewrites settles whole, and one that never pauses is
/// refused at the declared bound with the axis that moved.
fn stability_door() -> Door {
    let whole = b"{\"revision\":9}".to_vec();
    let reported = u64::try_from(whole.len())
        .map_err(|error| deviation(format!("a document longer than u64 bytes: {error}")))?;
    let at = SystemTime::UNIX_EPOCH
        .checked_add(Duration::from_secs(7))
        .ok_or_else(|| deviation("seven seconds after the epoch is not a SystemTime".to_owned()))?;
    let settled = read_stable(|| Ok(Reading::of_subject(whole.clone(), reported, at))).map_err(
        |failure| {
            deviation(format!(
                "a subject nobody rewrites did not settle: {failure}"
            ))
        },
    )?;
    if settled.bytes() != whole.as_slice() {
        return refuse("a settled subject read other bytes than it holds");
    }
    if !settled.is_whole() {
        return refuse("a settled subject was not read whole");
    }

    let mut tick = 0_u64;
    let torn = read_stable(|| {
        tick = tick.saturating_add(1);
        let bytes = format!("[{tick};]").into_bytes();
        let intended = u64::try_from(bytes.len()).map_err(std::io::Error::other)?;
        let stamp = SystemTime::UNIX_EPOCH
            .checked_add(Duration::from_secs(tick))
            .ok_or_else(|| std::io::Error::other("a tick past SystemTime"))?;
        Ok(Reading::of_subject(bytes, intended, stamp))
    });
    let Err(failure) = torn else {
        return refuse("a writer that never pauses was admitted");
    };
    let Some(unstable): Option<&Unstable> = failure.unstable() else {
        return refuse("a modelled subject never becomes unreadable");
    };
    if unstable.reads() != MAX_STABILITY_READS {
        return refuse(format!(
            "a refused attempt stopped after {} reads, not at the declared {MAX_STABILITY_READS}",
            unstable.reads()
        ));
    }
    Ok(format!(
        "stability door: a steady subject settled whole, a writer that never pauses was refused \
         after {} reads ({})",
        unstable.reads(),
        unstable.drift()
    ))
}
