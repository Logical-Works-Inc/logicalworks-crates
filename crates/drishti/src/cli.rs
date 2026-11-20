//! Shared command-line convention for the preview binaries.
//!
//! Both `canvas_preview` and `trace_check` run through [`run`]: locked
//! standard streams become one [`Streams`], the binary's `execute` returns a
//! typed [`Failure`] or an exit code, and the exit mapping reports the
//! failure on stderr. Every write error propagates as the failure — including
//! the final report, whose failure selects a distinct exit code because no
//! channel remains for its text.

use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

/// Application failure for the preview binaries.
///
/// Every variant names the failure and keeps its source, so the exit-code
/// report and any caller can see why: I/O, UTF-8, and size failures carry
/// their source errors, while invocation faults carry their message.
///
/// # Examples
///
/// ```
/// use drishti::cli::Failure;
///
/// let usage = Failure::from("usage: tool FILE");
/// assert!(matches!(usage, Failure::Input(_)));
/// assert_eq!(usage.to_string(), "usage: tool FILE");
///
/// let io = Failure::from(std::io::Error::other("disk gone"));
/// assert!(io.to_string().starts_with("I/O failure: "));
/// ```
#[derive(Debug, lgwks_macros::Error)]
#[non_exhaustive]
pub enum Failure {
    /// Invalid invocation or input; the message names the problem and no
    /// deeper source exists.
    #[error("{0}")]
    Input(String),
    /// An I/O operation failed.
    #[error("I/O failure: {0}")]
    Io(#[from] std::io::Error),
    /// Input bytes were not valid UTF-8.
    #[error("invalid UTF-8: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
    /// A byte count overflowed `u64`.
    #[error("input size overflow")]
    SizeOverflow(#[from] std::num::TryFromIntError),
    /// A trace line was not valid JSON.
    #[error("invalid JSON: {0}")]
    Json(#[from] lgwks_std::json::Error),
    /// A trace failed the analytic check.
    #[error("trace rejected: {0}")]
    Check(#[from] crate::preview::TraceCheckError),
    /// A scene reducer step failed.
    #[error("scene failure: {0}")]
    Scene(#[from] crate::canvas_scene::model::Error),
    /// A session binding was malformed.
    #[error("invalid session: {0}")]
    Session(#[from] crate::canvas_protocol::SessionInvariantError),
    /// The windowing event loop failed (`desktop` feature).
    #[cfg(feature = "desktop")]
    #[error("event loop failure: {0}")]
    EventLoop(#[from] lgwks_deps::winit::error::EventLoopError),
    /// The platform refused to create the window (`desktop` feature).
    #[cfg(feature = "desktop")]
    #[error("window creation failure: {0}")]
    Window(#[from] lgwks_deps::winit::error::OsError),
    /// The CPU presentation surface failed (`desktop` feature).
    #[cfg(feature = "desktop")]
    #[error("surface failure: {0}")]
    Surface(#[from] lgwks_deps::softbuffer::SoftBufferError),
}

impl From<&str> for Failure {
    /// Usage and input faults from a static message.
    fn from(message: &str) -> Self {
        Self::Input(String::from(message))
    }
}

impl From<String> for Failure {
    /// Usage and input faults from a computed message.
    fn from(message: String) -> Self {
        Self::Input(message)
    }
}

/// The two output streams of a run: data on standard output, diagnostics on
/// standard error. Only [`run`] builds one; a binary's `execute` receives it
/// by mutable reference.
///
/// # Examples
///
/// ```
/// use std::process::ExitCode;
///
/// use drishti::cli::run;
///
/// let mut reported = false;
/// let code = run("doc-streams", |streams| {
///     streams.tell("data line")?;
///     streams.warn("diagnostic line")?;
///     reported = true;
///     Ok(0)
/// });
/// assert!(reported);
/// assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
/// ```
pub struct Streams<'sink> {
    /// Standard output.
    out: &'sink mut dyn Write,
    /// Standard error.
    err: &'sink mut dyn Write,
}

impl std::fmt::Debug for Streams<'_> {
    /// The sinks are opaque writers, so only the type is named.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Streams").finish_non_exhaustive()
    }
}

impl Streams<'_> {
    /// Build streams over locked standard output and error. Only [`run`]
    /// calls this; binaries receive the streams.
    fn new<'sink>(out: &'sink mut dyn Write, err: &'sink mut dyn Write) -> Streams<'sink> {
        Streams { out, err }
    }

    /// A line on standard output. Stdout carries data (JSONL traces), so a
    /// failed line is a failed run, not a warning.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::cli::run;
    ///
    /// let mut told = false;
    /// run("doc-tell", |streams| {
    ///     streams.tell("{\"kind\":\"summary\"}")?;
    ///     told = true;
    ///     Ok(0)
    /// });
    /// assert!(told);
    /// ```
    pub fn tell(&mut self, text: &str) -> Result<(), Failure> {
        writeln!(self.out, "{text}")
            .map_err(|error| Failure::from(format!("the report was not written: {error}")))
    }

    /// A line on standard error. A failed write is a failure too: the run
    /// reports it through the distinct exit code rather than dropping it.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::cli::run;
    ///
    /// let mut warned = false;
    /// run("doc-warn", |streams| {
    ///     streams.warn("backend not compiled")?;
    ///     warned = true;
    ///     Ok(1)
    /// });
    /// assert!(warned);
    /// ```
    pub fn warn(&mut self, text: &str) -> Result<(), Failure> {
        writeln!(self.err, "{text}")
            .map_err(|error| Failure::from(format!("the report was not written: {error}")))
    }

    /// Reborrow standard output for data sinks (trace JSONL).
    ///
    /// # Examples
    ///
    /// ```
    /// use std::io::Write as _;
    ///
    /// use drishti::cli::run;
    ///
    /// let mut wrote = false;
    /// run("doc-out", |streams| {
    ///     streams.out().write_all(b"{\"kind\":\"frame\"}\n")?;
    ///     wrote = true;
    ///     Ok(0)
    /// });
    /// assert!(wrote);
    /// ```
    pub fn out(&mut self) -> &mut dyn Write {
        &mut *self.out
    }
}

/// Read a file bounded by `cap` bytes: refuse oversized inputs before
/// reading, then cap the read itself so a file grown between the check and
/// the read cannot exhaust memory.
///
/// # Errors
///
/// Returns `over_message` when the file exceeds `cap`, or the I/O failure.
///
/// # Examples
///
/// ```
/// use drishti::cli::{Failure, read_bounded};
///
/// let path = std::env::temp_dir().join(format!(
///     "drishti-doc-read-bounded-{}.bin",
///     std::process::id()
/// ));
/// std::fs::write(&path, b"12345").map_err(|error| error.to_string())?;
/// let at_cap = read_bounded(&path, 5, "too big");
/// let over_cap = read_bounded(&path, 4, "too big");
/// std::fs::remove_file(&path).map_err(|error| error.to_string())?;
///
/// assert_eq!(at_cap.map_err(|error| error.to_string()), Ok(b"12345".to_vec()));
/// assert!(matches!(over_cap, Err(Failure::Input(ref message)) if message == "too big"));
/// # Ok::<(), String>(())
/// ```
pub fn read_bounded(path: &Path, cap: u64, over_message: &'static str) -> Result<Vec<u8>, Failure> {
    let size = std::fs::metadata(path)?.len();
    (size <= cap)
        .then_some(())
        .ok_or_else(|| Failure::from(over_message))?;
    let file = std::fs::File::open(path)?;
    let mut bytes = Vec::new();
    use std::io::Read as _;
    file.take(cap.saturating_add(1)).read_to_end(&mut bytes)?;
    (u64::try_from(bytes.len())? <= cap)
        .then_some(())
        .ok_or_else(|| Failure::from(over_message))?;
    Ok(bytes)
}

/// Run a binary: lock the standard streams, execute, and map the outcome to
/// an exit code. Exit 0 is success, 1 is a reported failure, and 2 records a
/// failure whose report could not print because stderr itself is broken.
///
/// # Examples
///
/// ```
/// use std::process::ExitCode;
///
/// use drishti::cli::{Failure, run};
///
/// let ok = run("doc-run", |_| Ok(0));
/// let failed = run("doc-run", |_| Err(Failure::from("refused")));
/// assert_eq!(format!("{ok:?}"), format!("{:?}", ExitCode::SUCCESS));
/// assert_eq!(format!("{failed:?}"), format!("{:?}", ExitCode::FAILURE));
/// ```
pub fn run<F>(program: &'static str, execute: F) -> ExitCode
where
    F: FnOnce(&mut Streams<'_>) -> Result<u8, Failure>,
{
    let stdout = std::io::stdout();
    let stderr = std::io::stderr();
    let (mut out, mut err) = (stdout.lock(), stderr.lock());
    let mut streams = Streams::new(&mut out, &mut err);
    exit(execute(&mut streams), &mut streams, program)
}

/// Map a run outcome to its exit code, reporting failures on stderr.
fn exit(result: Result<u8, Failure>, streams: &mut Streams<'_>, program: &str) -> ExitCode {
    match result {
        Ok(code) => ExitCode::from(code),
        Err(failure) => match streams.warn(&format!("{program}: {failure}")) {
            Ok(()) => ExitCode::from(1),
            // No channel remains for the message text, so the distinct exit
            // code records the unreported failure instead of dropping it.
            Err(_) => ExitCode::from(2),
        },
    }
}

/// Shared conformance-driver scaffolding for the absorbing drivers.
///
/// The `api-drive-*` binaries drive the full absorbed surface for the surface
/// gate, so their digest helpers and frozen fixtures live here once instead
/// of copied per binary. Testing aid only, never product behavior.
pub mod drive {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use super::Failure;
    use crate::canvas_protocol::Capability;
    use crate::canvas_protocol::SessionInvariant;
    use crate::native_host::broker::{
        AuditEvent, AuditFn, Broker, ClockFn, GrantFn, GrantVerificationResult, IdFn,
        RegisteredHandle, VerifyGrantFn,
    };
    use crate::native_host::protocol::{WidgetDescriptor, WidgetSurface};

    /// The audit events a frozen broker's sink captured.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::cli::drive::{Events, captured, frozen_broker};
    ///
    /// let (_, events, _) = frozen_broker();
    /// let shared: Events = events;
    /// assert!(captured(&shared).is_empty());
    /// ```
    pub type Events = Arc<Mutex<Vec<AuditEvent>>>;

    /// Frozen broker for the drivers: fixed clock, counting identities,
    /// always-valid ML grants, a grant store the drive opens with the returned
    /// flag (closed at first), and a capturing audit sink.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::atomic::Ordering;
    ///
    /// use drishti::cli::drive::{captured, frozen_broker};
    ///
    /// let (_, events, open) = frozen_broker();
    /// assert!(!open.load(Ordering::SeqCst));
    /// assert!(captured(&events).is_empty());
    /// ```
    #[must_use]
    pub fn frozen_broker() -> (Broker, Events, Arc<AtomicBool>) {
        let ids = Arc::new(Mutex::new(0_u128));
        let clock: ClockFn = Arc::new(|| 1_000);
        let new_id: IdFn = Arc::new(move || {
            let mut guard = match ids.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            *guard = guard.wrapping_add(1);
            lgwks_std::id::Uuid::from(guard.to_le_bytes())
        });
        let verify: VerifyGrantFn = Arc::new(|_| GrantVerificationResult::Valid);
        let open = Arc::new(AtomicBool::new(false));
        let gate = Arc::clone(&open);
        let grants: GrantFn = Arc::new(move |_, _| gate.load(Ordering::SeqCst));
        let events: Events = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        let audit: AuditFn = Arc::new(move |event: &AuditEvent| match sink.lock() {
            Ok(mut guard) => guard.push(event.clone()),
            Err(poisoned) => poisoned.into_inner().push(event.clone()),
        });
        (
            Broker::new(clock, new_id, verify, grants, audit),
            events,
            open,
        )
    }

    /// Fixed work-object identity for every fixture; a canonical UUID so
    /// digests built from it are identical across runs.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::cli::drive::WORK_OBJECT;
    ///
    /// assert!(lgwks_std::id::Uuid::parse(WORK_OBJECT).is_ok());
    /// ```
    pub const WORK_OBJECT: &str = "11111111-1111-1111-1111-111111111111";
    /// Fixed session identity for every fixture; differs from every other
    /// fixed identity so a mixed-up binding cannot pass.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::cli::drive::{SESSION_ID, WORK_OBJECT};
    ///
    /// assert!(lgwks_std::id::Uuid::parse(SESSION_ID).is_ok());
    /// assert_ne!(SESSION_ID, WORK_OBJECT);
    /// ```
    pub const SESSION_ID: &str = "22222222-2222-2222-2222-222222222222";
    /// Fixed policy bundle for every fixture; a canonical UUID distinct from
    /// the session identity.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::cli::drive::{POLICY_BUNDLE, SESSION_ID};
    ///
    /// assert!(lgwks_std::id::Uuid::parse(POLICY_BUNDLE).is_ok());
    /// assert_ne!(POLICY_BUNDLE, SESSION_ID);
    /// ```
    pub const POLICY_BUNDLE: &str = "33333333-3333-3333-3333-333333333333";
    /// Fixed user subject for every fixture; a canonical UUID distinct from
    /// the policy bundle.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::cli::drive::{POLICY_BUNDLE, USER_SUB};
    ///
    /// assert!(lgwks_std::id::Uuid::parse(USER_SUB).is_ok());
    /// assert_ne!(USER_SUB, POLICY_BUNDLE);
    /// ```
    pub const USER_SUB: &str = "44444444-4444-4444-4444-444444444444";

    /// Register a minimal widget on `broker` and return its handle, for
    /// drivers that need an owner to tap as. The widget holds the emit and
    /// subscribe capabilities and lives in `session`.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::cli::drive::{SESSION_ID, frozen_broker, session, widget_owner};
    ///
    /// let (broker, _, _) = frozen_broker();
    /// let mut digest = String::new();
    /// let bound = session(&mut digest, "tenant-a").map_err(|error| error.to_string())?;
    /// let handle = widget_owner(&mut digest, &broker, &bound).map_err(|error| error.to_string())?;
    /// assert!(handle.session_id() == SESSION_ID);
    /// assert!(digest.contains("widget-owner="));
    /// # Ok::<(), String>(())
    /// ```
    pub fn widget_owner(
        digest: &mut String,
        broker: &Broker,
        session: &SessionInvariant,
    ) -> Result<RegisteredHandle, Failure> {
        let descriptor = WidgetDescriptor::new(
            String::from("demo.kind"),
            String::from("widget.owner"),
            String::from("1.0.0"),
            WidgetSurface::Native,
            Vec::new(),
            Vec::new(),
            vec![Capability::SignalEmit, Capability::SignalSubscribe],
        );
        must(
            digest,
            "widget-owner",
            broker.register(descriptor, session.clone()),
        )
    }

    /// A copy of the audit events captured so far, in sink order.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::cli::drive::{captured, frozen_broker, session, widget_owner};
    ///
    /// let (broker, events, _) = frozen_broker();
    /// assert!(captured(&events).is_empty());
    /// let mut digest = String::new();
    /// let bound = session(&mut digest, "tenant-a").map_err(|error| error.to_string())?;
    /// widget_owner(&mut digest, &broker, &bound).map_err(|error| error.to_string())?;
    /// let first = captured(&events);
    /// assert_eq!(format!("{first:?}"), format!("{:?}", captured(&events)));
    /// # Ok::<(), String>(())
    /// ```
    #[must_use]
    pub fn captured(events: &Events) -> Vec<AuditEvent> {
        match events.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// Record a value in the digest as one `what=value` line (Debug form).
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::cli::drive::note;
    ///
    /// let mut digest = String::new();
    /// note(&mut digest, "count", &3_u8).map_err(|error| error.to_string())?;
    /// note(&mut digest, "name", "x").map_err(|error| error.to_string())?;
    /// assert_eq!(digest, "count=3\nname=\"x\"\n");
    /// # Ok::<(), String>(())
    /// ```
    pub fn note<T: std::fmt::Debug + ?Sized>(
        digest: &mut String,
        what: &str,
        value: &T,
    ) -> Result<(), Failure> {
        std::fmt::Write::write_fmt(digest, format_args!("{what}={value:?}\n"))
            .map_err(|error| Failure::from(format!("digest unwritable: {error}")))?;
        Ok(())
    }

    /// Build a value through a fallible constructor, recording it, or fail
    /// the drive with context.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::cli::drive::must;
    ///
    /// let mut digest = String::new();
    /// let seven = must(&mut digest, "value", Ok::<u8, String>(7)).map_err(|error| error.to_string())?;
    /// assert_eq!(seven, 7);
    /// assert_eq!(digest, "value=7\n");
    ///
    /// let refused = must(&mut digest, "step", Err::<u8, String>(String::from("boom")));
    /// assert_eq!(
    ///     refused.map_err(|error| error.to_string()),
    ///     Err(String::from("step refused: boom"))
    /// );
    /// assert_eq!(digest, "value=7\n");
    /// # Ok::<(), String>(())
    /// ```
    pub fn must<T: std::fmt::Debug, E: std::fmt::Display>(
        digest: &mut String,
        what: &str,
        result: Result<T, E>,
    ) -> Result<T, Failure> {
        match result {
            Ok(value) => {
                note(digest, what, &value)?;
                Ok(value)
            }
            Err(error) => Err(Failure::from(format!("{what} refused: {error}"))),
        }
    }

    /// Fail the drive with `message` unless `condition` holds.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::cli::drive::ensure;
    ///
    /// assert!(ensure(true, "held").is_ok());
    /// let failed = ensure(false, "a negative control passed");
    /// assert_eq!(
    ///     failed.map_err(|error| error.to_string()),
    ///     Err(String::from("a negative control passed"))
    /// );
    /// ```
    pub fn ensure(condition: bool, message: &'static str) -> Result<(), Failure> {
        crate::native_host::ensure(condition, || Failure::from(message))
    }

    /// Take the refusal out of a negative control: the error of `outcome`, or
    /// a failed drive with `message` when the control was admitted.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::cli::drive::refused;
    ///
    /// let error = refused(Err::<u8, &str>("no"), "control admitted");
    /// assert_eq!(error.map_err(|failure| failure.to_string()), Ok("no"));
    /// let admitted = refused(Ok::<u8, &str>(1), "control admitted");
    /// assert_eq!(
    ///     admitted.map_err(|failure| failure.to_string()),
    ///     Err(String::from("control admitted"))
    /// );
    /// ```
    pub fn refused<T, E>(outcome: Result<T, E>, message: &'static str) -> Result<E, Failure> {
        outcome.err().ok_or_else(|| Failure::from(message))
    }

    /// Parse a fixed fixture UUID, recording it in the digest; a malformed
    /// text fails the drive and leaves the digest untouched.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::cli::drive::{WORK_OBJECT, uuid};
    ///
    /// let mut digest = String::new();
    /// assert!(uuid(&mut digest, "id", WORK_OBJECT).is_ok());
    /// assert!(digest.starts_with("id="));
    ///
    /// let before = digest.clone();
    /// let refused = uuid(&mut digest, "bad-id", "nope").map_err(|error| error.to_string());
    /// assert!(matches!(refused, Err(ref text) if text.starts_with("bad-id refused: ")));
    /// assert_eq!(digest, before);
    /// ```
    pub fn uuid(
        digest: &mut String,
        what: &str,
        text: &str,
    ) -> Result<lgwks_std::id::Uuid, Failure> {
        must(digest, what, lgwks_std::id::Uuid::parse(text))
    }

    /// A nonce the broker never issued, derived from a live one by inverting
    /// every byte, so a negative control forges its handle from the run and no
    /// literal stands in the shipped drive. The result differs from `live` in
    /// every byte.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::cli::drive::forged_nonce;
    ///
    /// let live = lgwks_std::id::Uuid::from([1_u8; 16]);
    /// let forged = forged_nonce(&live);
    /// assert_eq!(forged, lgwks_std::id::Uuid::from([0xFE_u8; 16]));
    /// assert_eq!(forged_nonce(&forged), live);
    /// ```
    #[must_use]
    pub fn forged_nonce(live: &lgwks_std::id::Uuid) -> lgwks_std::id::Uuid {
        lgwks_std::id::Uuid::from((*live.as_bytes()).map(|byte| !byte))
    }

    /// Bound session fixture for one tenant: the fixed work object, session,
    /// policy bundle and user subject, with `tenant` as the only variable.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::cli::drive::session;
    ///
    /// let mut digest = String::new();
    /// let bound = session(&mut digest, "tenant-a").map_err(|error| error.to_string())?;
    /// assert_eq!(bound.tenant_id(), "tenant-a");
    /// # Ok::<(), String>(())
    /// ```
    pub fn session(digest: &mut String, tenant: &str) -> Result<SessionInvariant, Failure> {
        must(
            digest,
            "session",
            SessionInvariant::new(
                String::from(WORK_OBJECT),
                String::from(SESSION_ID),
                String::from(POLICY_BUNDLE),
                String::from(tenant),
                String::from(USER_SUB),
            ),
        )
    }
}
