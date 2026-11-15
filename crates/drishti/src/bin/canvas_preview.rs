//! Explicit read-only local source viewer.
//!
//! Absorbed from `primitives/canvas-native/crates/preview/src/main.rs`
//! (kernel#850 row 15). It cannot save, send, grant authority, or claim test
//! success. `--headless` runs the scripted trace traversal;
//! `--reduced-motion` snaps motion; otherwise the `desktop` feature window
//! opens (or the binary reports that it is not compiled in).
//!
//! Output runs through the shared [`drishti::cli`] convention: the binary
//! only implements `execute`, and every write error propagates as the typed
//! failure. Exit codes keep kernel parity: 0 success, 1 reported failure, 2
//! when the report could not print.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use drishti::canvas_protocol::SessionInvariant;
use drishti::canvas_scene::{Action, Artifact, Canvas, Delta, Limits};
use drishti::cli::{Failure, Streams};

/// Maximum source input bytes (64 KiB).
const INPUT_BYTES: u64 = 65_536;

/// Parsed command-line options.
struct Options {
    /// Required source file.
    source: Option<PathBuf>,
    /// Optional proposal lens file.
    proposal: Option<PathBuf>,
    /// Optional evidence lens file.
    evidence: Option<PathBuf>,
    /// Run the headless trace traversal.
    headless: bool,
    /// Snap motion (reduced).
    reduced: bool,
}

impl Options {
    /// Empty options before parsing.
    fn empty() -> Self {
        Self {
            source: None,
            proposal: None,
            evidence: None,
            headless: false,
            reduced: false,
        }
    }
}

/// Enforce a CLI precondition.
fn require_input(valid: bool, message: &'static str) -> Result<(), Failure> {
    if valid { Ok(()) } else { Err(message.into()) }
}

/// Parse `canvas-preview --source PATH [--proposal PATH] [--evidence PATH]
/// [--headless] [--reduced-motion]`.
fn parse(arguments: &[OsString]) -> Result<Options, Failure> {
    let mut options = Options::empty();
    let mut args = arguments.iter();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--headless") => options.headless = true,
            Some("--reduced-motion") => options.reduced = true,
            Some("--source" | "--proposal" | "--evidence") => {
                let value = args.next().ok_or("file argument is missing")?;
                let slot = match arg.to_str() {
                    Some("--source") => &mut options.source,
                    Some("--proposal") => &mut options.proposal,
                    _ => &mut options.evidence,
                };
                require_input(slot.is_none(), "duplicate file argument")?;
                *slot = Some(value.into());
            }
            _ => require_input(
                false,
                "usage: canvas-preview --source PATH [--proposal PATH] [--evidence PATH] [--headless] [--reduced-motion]",
            )?,
        }
    }
    require_input(
        options.source.is_some(),
        "--source is required; sample data is never substituted",
    )?;
    Ok(options)
}

/// Read a source file: regular files only, at most 64 KiB, valid UTF-8.
///
/// The type is checked before opening so a FIFO never blocks the open.
/// A path swapped between the check and the open could still block; that
/// residual TOCTOU is documented rather than silently assumed away.
fn read_text(path: &Path) -> Result<String, Failure> {
    let file_type = std::fs::symlink_metadata(path)?.file_type();
    require_input(file_type.is_file(), "input must be a regular file")?;
    let file = std::fs::File::open(path)?;
    require_input(file.metadata()?.is_file(), "input must be a regular file")?;
    let mut bytes = Vec::new();
    use std::io::Read as _;
    file.take(INPUT_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)?;
    let byte_count = u64::try_from(bytes.len())?;
    require_input(byte_count <= INPUT_BYTES, "input exceeds 65536 bytes")?;
    String::from_utf8(bytes).map_err(|error| error.into())
}

/// Run the preview against the parsed arguments; returns the exit code.
fn execute(arguments: &[OsString], streams: &mut Streams<'_>) -> Result<u8, Failure> {
    let options = parse(arguments)?;
    let source_path = options.source.as_ref().ok_or("--source is required")?;
    let source = read_text(source_path)?;
    let proposal = options
        .proposal
        .as_ref()
        .map(|input| read_text(input))
        .transpose()?;
    let evidence = options
        .evidence
        .as_ref()
        .map(|input| read_text(input))
        .transpose()?;
    // Existence probe: the source path must resolve before reading.
    source_path.canonicalize()?;
    let limits = Limits::default();
    // Diagnostic context only: a fixed diagnostic work object binds the
    // artifact and the session together for the scope check. These values do
    // not impersonate an authenticated session.
    let work_object = String::from("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa");
    let artifact = Artifact::new(
        &work_object,
        1,
        &source,
        proposal.as_deref(),
        evidence.as_deref(),
        limits,
    )?;
    let session = SessionInvariant::new(
        work_object,
        String::from("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb"),
        String::from("cccccccc-cccc-cccc-cccc-cccccccccccc"),
        String::from("local-preview"),
        String::from("dddddddd-dddd-dddd-dddd-dddddddddddd"),
    )?;
    let mut canvas = Canvas::new(session, 1, artifact, limits)?;
    if options.reduced {
        canvas.enqueue(Delta::new(
            canvas.session().clone(),
            1,
            0,
            Action::ReducedMotion(true),
        ))?;
    }
    if options.headless {
        drishti::preview::run_traversal(canvas, streams.out())?;
        return Ok(0);
    }
    #[cfg(feature = "desktop")]
    {
        drishti::preview::desktop::run_window(canvas)?;
        Ok(0)
    }
    #[cfg(not(feature = "desktop"))]
    {
        streams.warn(
            "desktop backend is not compiled; use --features desktop or explicitly select --headless",
        )?;
        Ok(1)
    }
}

/// Preview entry point: exit code, never `process::exit`.
fn main() -> ExitCode {
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();
    drishti::cli::run("canvas-preview", |streams| execute(&arguments, streams))
}
