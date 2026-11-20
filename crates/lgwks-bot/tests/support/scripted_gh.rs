//! A scripted `gh` on PATH, and the recordings the tests assert on.
//!
//! The adapter resolves its program through `PATH`, so a test binds it to a
//! script it controls by putting a directory first on `PATH` and naming `gh`
//! inside it. Nothing here replaces a Rust function: the code under test forks,
//! captures stdout and stderr, applies a deadline and reaps a process group
//! exactly as it would for the real client.
//!
//! The script records one line of argv per invocation, appending to a log file
//! whose path arrives in an environment variable. A JSON file beside it carries
//! the behaviour: which head a read reports, whether a create lands, and
//! whether the create's response is dropped after the effect was applied.
//!
//! One copy, included by path from every target that needs a scripted `gh`, so
//! two targets cannot assert different things about the same script. The script
//! itself is the string in `scripted_gh/body.rs`.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use lgwks_bot::domain::gh::{Gh, GhError, Repository};

/// Monotone per-process sequence, so two fixtures in one binary never collide.
static DIR_SEQ: AtomicU64 = AtomicU64::new(0);

/// How long a call to the scripted client may run before it is called a hang.
///
/// A bound on a hung child, not on the work: a read that asks the script for a
/// ceiling's worth of records takes 0.05 s alone and 4.6 s at its worst with 24
/// copies running beside 45 busy processes, and on CI run 38933831444 the same
/// reads were stopped by a 20 s bound while `journal::owner::tests` swept for
/// 69 s on the same host. Two of nextest's 60 s periods keeps a real hang a
/// typed `Deadline` that names what was being read, well inside the five
/// periods after which nextest ends the test itself.
pub(crate) const HANG_BOUND: std::time::Duration = std::time::Duration::from_secs(120);

/// Why a [`ScriptedGh::binding`] could not be built.
#[derive(Debug)]
pub(crate) enum BindingError {
    /// The repository reference was refused.
    Repository(GhError),
    /// The capture ceiling was zero, which an unbounded capture would be.
    ZeroCapture,
    /// The search path could not be joined.
    SearchPath(std::env::JoinPathsError),
}

impl fmt::Display for BindingError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Repository(ref refused) => write!(out, "the repository was refused: {refused}"),
            Self::ZeroCapture => out.write_str("a non-zero capture ceiling"),
            Self::SearchPath(ref cause) => write!(out, "the search path did not join: {cause}"),
        }
    }
}

impl std::error::Error for BindingError {}

impl From<GhError> for BindingError {
    fn from(refused: GhError) -> Self {
        Self::Repository(refused)
    }
}

impl From<std::env::JoinPathsError> for BindingError {
    fn from(cause: std::env::JoinPathsError) -> Self {
        Self::SearchPath(cause)
    }
}

/// A scripted `gh` installed on `PATH` for one scenario.
///
/// The directory is created under the system temp directory, removed on drop,
/// and never depends on a path the repository owns — the fixture is portable
/// and a test leaves nothing behind.
pub(crate) struct ScriptedGh {
    /// The directory to place first on `PATH`.
    dir: PathBuf,
    /// The file the script appends one argv line to per invocation.
    log: PathBuf,
}

impl ScriptedGh {
    /// Install a scripted `gh` and return its handle.
    ///
    /// The behaviour file starts as a record of every read reporting `head_sha`
    /// and no create landing, so a scenario that changes nothing gets a
    /// coherent "GitHub has this pull request and no review" world rather than
    /// an empty one the script has to interpret.
    pub(crate) fn install(name: &str, head_sha: &str) -> std::io::Result<Self> {
        // Random rather than a process id: the OS reuses process ids, and two
        // fixtures sharing a directory would read each other's argv log — which
        // is the one thing these tests measure.
        // An entropy source that cannot answer refuses the fixture rather than
        // naming it zero: a zero every failed draw shares is the collision the
        // random tag exists to rule out.
        let tag = lgwks_std::random::bytes::<8>()
            .map(u64::from_le_bytes)
            .map_err(std::io::Error::other)?;
        let seq = DIR_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("lgwks-scripted-gh-{tag:016x}-{seq}-{name}"));
        std::fs::create_dir_all(&dir)?;
        let log = dir.join("argv.log");
        std::fs::write(&log, b"")?;

        let behaviour = dir.join("behaviour.json");
        std::fs::write(&behaviour, Scenario::new(head_sha).into_behaviour())?;

        let script = dir.join("gh");
        std::fs::write(&script, format!("#!{}\n{SCRIPT_BODY}", interpreter()))?;
        make_executable(&script)?;
        Ok(Self { dir, log })
    }

    /// Replace the behaviour file with the scenario's own receiver state.
    ///
    /// One writer, because every scenario writes the same fields and a
    /// per-test JSON literal is a copy that drifts: a key the script stopped
    /// reading would still be present in one test's literal and absent in
    /// another's, and the difference would look like a behaviour difference.
    /// An unspecified field takes the value the script already defaults to.
    pub(crate) fn configure(&self, scenario: Scenario) -> std::io::Result<()> {
        std::fs::write(self.behaviour(), scenario.into_behaviour())
    }

    /// The behaviour file the script reads on each invocation.
    pub(crate) fn behaviour(&self) -> PathBuf {
        self.dir.join("behaviour.json")
    }

    /// The directory to place first on `PATH`.
    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    /// The `PATH` this script is found through: its directory in front of the
    /// ambient one.
    ///
    /// Handed to a binding as a per-child variable rather than set on this
    /// process, which keeps tests independent and needs no `unsafe`.
    pub(crate) fn search_path(&self) -> Result<std::ffi::OsString, std::env::JoinPathsError> {
        let ambient = match std::env::var_os("PATH") {
            Some(path) => path,
            None => std::ffi::OsString::new(),
        };
        let mut entries = vec![self.dir.clone()];
        entries.extend(std::env::split_paths(&ambient));
        std::env::join_paths(&entries)
    }

    /// The program name the adapter resolves through `PATH`.
    ///
    /// A bare `gh`, so the test exercises real `PATH` resolution rather than
    /// pinning an absolute path — which is exactly what a deployment does.
    pub(crate) const fn program(&self) -> &'static str {
        "gh"
    }

    /// A [`Gh`] binding pointed at this script, with the named capture ceiling.
    ///
    /// One definition rather than a per-test builder, because the two lines of
    /// binding are what make a test "run through the adapter", and a copy is a
    /// place for one test to quietly stop doing it.
    pub(crate) fn binding(&self, repository: &str, capture: usize) -> Result<Gh, BindingError> {
        let ceiling = std::num::NonZeroUsize::new(capture).ok_or(BindingError::ZeroCapture)?;
        Ok(Gh::new(Repository::new(repository)?)
            .program(self.program())
            .capture_limit(ceiling)
            .deadline(Some(HANG_BOUND))
            .env("PATH", self.search_path()?))
    }

    /// Every invocation's argv, in order, one per line with tab separators.
    pub(crate) fn calls(&self) -> std::io::Result<Vec<Vec<String>>> {
        let text = std::fs::read_to_string(&self.log)?;
        Ok(text
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| line.split('\t').map(str::to_owned).collect())
            .collect())
    }

    /// How many invocations used `--method POST` — that is, how many reviews
    /// were *created*, which is the number a duplicate-post defect doubles.
    pub(crate) fn creates(&self) -> std::io::Result<usize> {
        Ok(self
            .calls()?
            .iter()
            .filter(|argv| {
                argv.windows(2).any(
                    |pair| matches!(pair, [flag, verb] if flag == "--method" && verb == "POST"),
                )
            })
            .count())
    }

    /// How many invocations used `--method GET` against `suffix`.
    pub(crate) fn reads_of(&self, suffix: &str) -> std::io::Result<usize> {
        Ok(self
            .calls()?
            .iter()
            .filter(|argv| argv.iter().any(|arg| arg.ends_with(suffix)))
            .count())
    }

    /// Every payload the script received, in order.
    ///
    /// Read from the receiver's own record rather than from the staged file,
    /// because the adapter removes that file as soon as the child exits — a
    /// test that read it afterwards would be asserting on nothing.
    pub(crate) fn received(&self) -> std::io::Result<Vec<String>> {
        let path = self.dir.join("received.jsonl");
        if !path.exists() {
            return Ok(Vec::new());
        }
        Ok(std::fs::read_to_string(path)?
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(str::to_owned)
            .collect())
    }
}

impl Drop for ScriptedGh {
    /// Remove the fixture, best effort: a directory that is already gone is
    /// the state wanted, and any other refusal is emitted rather than dropped.
    fn drop(&mut self) {
        match std::fs::remove_dir_all(&self.dir) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                lgwks_std::trace::debug!(error = ?error, "scripted gh: the fixture was not removed");
            }
        }
    }
}

/// Make `path` executable by whoever owns it.
#[cfg(unix)]
fn make_executable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions)
}

/// Nothing to do off Unix: the script is a `#!/bin/sh` file.
#[cfg(not(unix))]
fn make_executable(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

/// The interpreter the script's `#!` line names: `bash` where the platform
/// ships it, else `sh`.
///
/// The script is POSIX, so either runs it, and what differs is what one costs. A
/// saturation family starts the script fifty thousand times at its 10,000 tier,
/// and on the macOS runners that is kernel time the thirteen jobs of a CI run
/// share (#272). Measured on that host, a script of this style ran 268 times a
/// second under `/bin/sh` and 406 under `/bin/bash` from one loop, and 1,566 and
/// 2,702 a second from twenty-four loops at once; a 100-record read-back took
/// 7.6 ms under `sh` and 6.1 ms under `bash` from eight loops. `dash` starts
/// faster still but reads a file one byte per `read(2)`, so the same read-back
/// took 110 ms from eight loops: it is not a choice. Debian's `bash` is present
/// beside its `sh`.
fn interpreter() -> &'static str {
    if Path::new("/bin/bash").is_file() {
        "/bin/bash"
    } else {
        "/bin/sh"
    }
}

/// The scripted client's shell text, in its own file for the size law.
#[path = "scripted_gh/body.rs"]
mod body;
use body::SCRIPT_BODY;

/// What a scenario tells the scripted receiver to do.
///
/// A builder rather than a JSON literal per test: the fields are named once,
/// the defaults are stated once, and a scenario reads as the difference from a
/// working world rather than as a wall of escaped braces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Scenario {
    /// The head the first pull-request read reports.
    head: String,
    /// The head every read after the first reports, modelling a push landing
    /// between two reads. `None` leaves the head fixed.
    pub(crate) head_after_first: Option<String>,
    /// What a create does: `accept`, `accept_then_drop`, or anything else,
    /// which is treated as a refusal.
    pub(crate) create: &'static str,
    /// The body a created review carries, which is what the read-back returns
    /// and therefore what the verification compares against.
    created_body: String,
    /// The state a created review reports.
    pub(crate) created_state: &'static str,
    /// How many pull-request reads are refused before one succeeds.
    pub(crate) fail_reads: u32,
    /// How long a call hangs, for the deadline probe. `0` hangs not at all.
    pub(crate) hang_seconds: u32,
    /// How many padding lines a call writes to stdout before its answer, for
    /// the bounded-capture probe.
    pub(crate) flood_bytes: u32,
    /// How many filler reviews the receiver reports on a review read, over and
    /// above whatever it actually applied.
    ///
    /// This is how the review-ceiling probe builds a pull request whose review
    /// history is longer than the adapter will decode. A real one arrives from
    /// `--paginate` following page after page; the script emits the same *shape*
    /// in one document, because what is under test is the adapter's refusal to
    /// treat a clean decode as proof of completeness.
    pub(crate) filler_reviews: u32,
    /// The page size the review endpoint serves, `0` for one page however long.
    ///
    /// With a size set, a read without `--paginate` sees only the first page,
    /// so a review on a later page is visible only to a client that follows
    /// every page. That is the falsifier for pagination: an adapter that read
    /// one page would decode a clean, short, wrong list.
    pub(crate) review_page: u32,
    /// What the review read emits instead of a JSON list: `""` for a real list,
    /// `garbage` for something that is not JSON, or `truncated` for a JSON
    /// document whose closing bracket was lost.
    pub(crate) reviews_shape: &'static str,
    /// What the pull-request read emits instead of a pull request: `""` for the
    /// real object, `moved` for a renamed-repository redirect naming the
    /// canonical location, or `denied-<status>` for a receiver refusing the
    /// credential with that status.
    pub(crate) snapshot_shape: &'static str,
    /// What the changed-file read emits: `""` for a real list, or `unavailable`
    /// for a server that declines to render the diff (a `406`).
    pub(crate) files_shape: &'static str,
    /// How many filler files the changed-file read reports, for the file-count
    /// ceiling.
    pub(crate) filler_files: u32,
    /// The bytes of patch text one reported file carries, for the byte ceiling.
    pub(crate) diff_bytes: u32,
    /// How many inline comments a `partial` create lands, when it lands fewer
    /// than the payload intended.
    pub(crate) applied_comments: u32,
    /// Whether the review read is denied with a `403` permission answer.
    pub(crate) deny_reads: bool,
    /// Whether the changed-file inventory names a `build.rs` whose patch text
    /// would perform an effect if it were ever executed.
    pub(crate) build_script: bool,
}

impl Scenario {
    /// A world where the head is fixed, creates are accepted and no read fails.
    #[must_use]
    pub(crate) fn new(head: &str) -> Self {
        Self {
            head: head.to_owned(),
            head_after_first: None,
            create: "accept",
            created_body: String::new(),
            created_state: "COMMENTED",
            fail_reads: 0,
            hang_seconds: 0,
            flood_bytes: 0,
            filler_reviews: 0,
            review_page: 0,
            reviews_shape: "",
            snapshot_shape: "",
            files_shape: "",
            filler_files: 0,
            diff_bytes: 0,
            applied_comments: 0,
            deny_reads: false,
            build_script: false,
        }
    }

    /// Accept a create request and then drop the response, which leaves the
    /// caller unsure whether the review exists.
    #[must_use]
    pub(crate) fn accept_then_drop(mut self) -> Self {
        self.create = "accept_then_drop";
        self
    }

    /// Refuse every create request outright, so the caller must report a
    /// refusal with nothing published.
    #[must_use]
    pub(crate) fn refuse_creates(mut self) -> Self {
        self.create = "refuse";
        self
    }

    /// Every read before the `n`th is refused.
    #[must_use]
    pub(crate) fn fail_reads(mut self, count: u32) -> Self {
        self.fail_reads = count;
        self
    }

    /// Set the body the script reports for a review it created, so a test can
    /// compare it with the body that was sent.
    #[must_use]
    pub(crate) fn created_body(mut self, body: &str) -> Self {
        body.clone_into(&mut self.created_body);
        self
    }

    /// Make the pull request's head commit become `head` after the first read,
    /// which simulates a push landing mid-review.
    #[must_use]
    pub(crate) fn head_moves_to(mut self, head: &str) -> Self {
        self.head_after_first = Some(head.to_owned());
        self
    }

    /// A call hangs for `seconds` before it answers.
    #[must_use]
    pub(crate) fn hangs_for(mut self, seconds: u32) -> Self {
        self.hang_seconds = seconds;
        self
    }

    /// A call writes `bytes` padding lines before its answer.
    #[must_use]
    pub(crate) fn floods(mut self, bytes: u32) -> Self {
        self.flood_bytes = bytes;
        self
    }

    /// The receiver reports `count` extra reviews beyond the ones it applied.
    ///
    /// Used to push a pull request's review history past the adapter's declared
    /// ceiling. The point of the probe is the *refusal*, so the filler needs no
    /// particular body: it only has to be a review the adapter would have had to
    /// read to call the list complete.
    #[must_use]
    pub(crate) fn with_filler_reviews(mut self, count: u32) -> Self {
        self.filler_reviews = count;
        self
    }

    /// The review endpoint serves its list in pages of `size` records.
    #[must_use]
    pub(crate) fn with_review_pages(mut self, size: u32) -> Self {
        self.review_page = size;
        self
    }

    /// The review read answers with something other than a JSON list.
    ///
    /// `garbage` is not JSON at all; `truncated` is a JSON document whose
    /// closing bracket was lost, which is the shape a truncated stream takes
    /// when the client managed to emit valid-looking JSON first.
    #[must_use]
    pub(crate) fn answers_reviews_with(mut self, shape: &'static str) -> Self {
        self.reviews_shape = shape;
        self
    }

    /// The receiver records the create as an unsubmitted draft and drops the
    /// response.
    ///
    /// GitHub creates a review in `PENDING` when the request names no event. The
    /// effect lands (the review exists) and the response is lost, which is the
    /// state a reconciliation must report as a draft, never as a publication and
    /// never as no effect.
    #[must_use]
    pub(crate) fn records_pending_draft(mut self) -> Self {
        self.create = "pending";
        self
    }

    /// The receiver records the create with only `applied` of its inline
    /// comments and drops the response.
    ///
    /// The submitted review is real and about the right commit, but fewer
    /// comments landed than intended — the partial submission PR-08 names.
    #[must_use]
    pub(crate) fn records_partial_comments(mut self, applied: u32) -> Self {
        self.create = "partial";
        self.applied_comments = applied;
        self
    }

    /// The repository answers with a renamed-repository redirect.
    #[must_use]
    pub(crate) fn repository_moved(mut self) -> Self {
        self.snapshot_shape = "moved";
        self
    }

    /// The changed-file read answers with a server that declines to render the
    /// diff (a `406`).
    #[must_use]
    pub(crate) fn diff_unavailable(mut self) -> Self {
        self.files_shape = "unavailable";
        self
    }

    /// The changed-file read reports `count` filler files.
    #[must_use]
    pub(crate) fn with_filler_files(mut self, count: u32) -> Self {
        self.filler_files = count;
        self
    }

    /// One reported file carries `bytes` of patch text.
    #[must_use]
    pub(crate) fn with_diff_bytes(mut self, bytes: u32) -> Self {
        self.diff_bytes = bytes;
        self
    }

    /// The pull-request read refuses the credential `gh` holds with `status`:
    /// `401` for an expired or cancelled token, `403` for one that lost the
    /// scope, `404` for a resource it can no longer see. Any other status is
    /// not a credential refusal and leaves the read answering normally.
    #[must_use]
    pub(crate) fn deny_credential(mut self, status: u16) -> Self {
        self.snapshot_shape = match status {
            401 => "denied-401",
            403 => "denied-403",
            404 => "denied-404",
            _ => self.snapshot_shape,
        };
        self
    }

    /// The review read answers with a `403` permission refusal.
    ///
    /// The shape `gh` prints when a credential cannot reach the resource, which
    /// the adapter reports as its own typed permission failure rather than as a
    /// transport outage.
    #[must_use]
    pub(crate) fn deny_reads(mut self) -> Self {
        self.deny_reads = true;
        self
    }

    /// The changed-file inventory names a `build.rs` that would perform an
    /// effect if anything ever executed its patch text.
    ///
    /// The effect is a shell command that removes the file named by
    /// `LGWKS_TEST_MARKER`, so a non-execution oracle can observe whether the
    /// review path ran it. Nothing here runs it: the inventory is read as data.
    #[must_use]
    pub(crate) fn hostile_build_script(mut self) -> Self {
        self.build_script = true;
        self
    }

    /// The behaviour file this scenario is, as the script reads it.
    ///
    /// Written here rather than assembled by each test so the keys the script
    /// looks up are the keys that exist, which is the failure mode a literal
    /// per test invites.
    #[must_use]
    pub(crate) fn into_behaviour(self) -> String {
        let base = "b".repeat(40);
        let after = match self.head_after_first.as_ref() {
            Some(head) => format!("\"head_after_first\":\"{head}\","),
            None => String::new(),
        };
        format!(
            "{{{after}\"head_sha\":\"{head}\",\"base_sha\":\"{base}\",\
             \"create\":\"{create}\",\"next_review_id\":9001,\
             \"created_body\":\"{body}\",\"created_state\":\"{state}\",\
             \"fail_reads\":{fail_reads},\"hang_seconds\":{hang},\"flood_bytes\":{flood},\
             \"filler_reviews\":{filler},\"review_page\":{page},\"reviews_shape\":\"{shape}\",\
             \"snapshot_shape\":\"{snapshot}\",\"files_shape\":\"{files}\",\
             \"filler_files\":{filler_files},\"diff_bytes\":{diff_bytes},\
             \"applied_comments\":{applied},\"deny_reads\":{deny},\
             \"build_script\":{build_script}}}\n",
            head = self.head,
            create = self.create,
            body = self.created_body,
            state = self.created_state,
            fail_reads = self.fail_reads,
            hang = self.hang_seconds,
            flood = self.flood_bytes,
            filler = self.filler_reviews,
            page = self.review_page,
            shape = self.reviews_shape,
            snapshot = self.snapshot_shape,
            files = self.files_shape,
            filler_files = self.filler_files,
            diff_bytes = self.diff_bytes,
            applied = self.applied_comments,
            deny = u8::from(self.deny_reads),
            build_script = u8::from(self.build_script),
        )
    }
}
