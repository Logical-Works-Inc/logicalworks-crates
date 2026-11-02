//! `lgwks run [file]`: build and serve the file, printing build times.
//!
//! The build runs through cargo with one shared target directory, so a warm
//! rebuild is cached: the first `cargo build` is timed as the cold build, a
//! second one as the warm build, and the spawn-to-accept time as the start.
//! All three numbers are printed (stderr for humans) or reported as JSON, so
//! an edit-and-rerun loop is measured, not promised.
//!
//! Every cargo step and the app itself run through the shared [`Children`]
//! registry: each leads a process group of its own and a watchdog stops the
//! group when this command dies, so stopping `lgwks run` (a terminal's Ctrl-C
//! or a supervisor's `TERM` to this process alone) never leaves the app
//! serving behind it.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::children::{Children, Launch, forget_launched_crate, inherited_env};
use super::common::{self, Refusal};

/// The command's own usage.
const RUN_USAGE: &str = "\
lgwks run [file] [--json]

Build and serve the app file, printing the cold build, the warm rebuild and
the spawn-to-accept start times. The build runs through cargo with one shared
target directory, so warm rebuilds are cached. [file] is the app file, its
directory, or app.rs when omitted.
";

/// How long the command waits for the app to accept before stopping it.
const START_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the readiness probe rests between attempts.
const PROBE_REST: Duration = Duration::from_millis(50);

/// The port the reference app binds when `PORT` names nothing usable.
const FALLBACK_PORT: u16 = 9000;

/// The longest resolving a fresh app's dependencies may take.
const LOCK_BOUND: Duration = Duration::from_secs(600);

/// The longest one build may take: a cold build of the whole storefront.
const BUILD_BOUND: Duration = Duration::from_secs(3_600);

/// The longest the app may serve: a server runs until it is stopped, so the
/// bound is a year rather than none, and every wait still names one.
const SERVE_BOUND: Duration = Duration::from_secs(365 * 24 * 3_600);

/// The most of a failed resolution's stderr a refusal quotes.
const QUOTED_TAIL: usize = 2_000;

/// The two build timings.
struct BuildTimings {
    /// The first `cargo build`, in milliseconds.
    cold_ms: u128,
    /// The second `cargo build`, in milliseconds.
    warm_ms: u128,
}

/// The serving facts, printed once the app accepts.
struct ServeFacts {
    /// The port the app accepted on.
    port: u16,
    /// The first `cargo build`, in milliseconds.
    cold_ms: u128,
    /// The second `cargo build`, in milliseconds.
    warm_ms: u128,
    /// The spawn-to-accept time, in milliseconds.
    start_ms: u128,
}

/// What the readiness probe concluded.
enum Readiness {
    /// The app accepted within the deadline.
    Ready,
    /// The app exited before accepting.
    Exited,
    /// Nothing accepted within the deadline.
    TimedOut,
}

/// What every cargo step of one run shares.
struct Cargo<'run> {
    /// The registry every step and the app run under.
    children: &'run Children,
    /// The environment each step sees: this process's, less what a
    /// `cargo run` of `lgwks` itself exported about its own crate.
    env: BTreeMap<String, String>,
    /// The app's directory, where every step runs.
    root: &'run Path,
    /// The app's manifest.
    manifest: &'run Path,
    /// The shared target directory.
    target: &'run Path,
}

impl Cargo<'_> {
    /// `cargo <verb>` against the app, `--locked`, in the shared target.
    fn argv(&self, verb: &str) -> Vec<String> {
        vec![
            String::from("cargo"),
            String::from(verb),
            String::from("--locked"),
            String::from("-j"),
            String::from("4"),
            String::from("--target-dir"),
            self.target.to_string_lossy().into_owned(),
            String::from("--manifest-path"),
            self.manifest.to_string_lossy().into_owned(),
        ]
    }

    /// A launch of `argv` in the app's directory under `bound`.
    fn launch<'launch>(&'launch self, argv: &'launch [String], bound: Duration) -> Launch<'launch> {
        Launch {
            argv,
            root: self.root,
            env: &self.env,
            stdin: None,
            bound,
        }
    }

    /// One `cargo build`, with the terminal's own streams; its exit code.
    fn build(&self) -> i32 {
        let argv = self.argv("build");
        self.children.run(&self.launch(&argv, BUILD_BOUND), None)
    }

    /// Write the app's `Cargo.lock` when it has none: a fresh scaffold has no
    /// lockfile and `--locked` refuses to make one, so the lock is resolved
    /// once here and every build after it is `--locked`.
    fn ensure_lock(&self) -> Result<(), Refusal> {
        if self.manifest.with_file_name("Cargo.lock").is_file() {
            return Ok(());
        }
        let argv = vec![
            String::from("cargo"),
            String::from("generate-lockfile"),
            String::from("--manifest-path"),
            self.manifest.to_string_lossy().into_owned(),
        ];
        let resolved = self.children.capture(&self.launch(&argv, LOCK_BOUND));
        if resolved.code == 0 {
            return Ok(());
        }
        // Cargo explains a failed resolution on stderr; a stderr that says
        // nothing leaves stdout as the only account.
        let said = match resolved.stderr.trim() {
            "" => resolved.stdout.trim(),
            stderr => stderr,
        };
        let tail = match said.char_indices().rev().nth(QUOTED_TAIL) {
            Some((at, _)) => &said[at..],
            None => said,
        };
        lgwks_std::trace::debug!(
            code = resolved.code,
            "lgwks run: the app's dependencies did not resolve"
        );
        Err(Refusal::whole(format!(
            "resolving the app's dependencies failed with exit code {}: {tail}",
            resolved.code
        )))
    }
}

/// The port the app binds: `PORT` when it parses, the reference default
/// otherwise.
fn app_port() -> u16 {
    match std::env::var("PORT") {
        Ok(text) => match text.trim().parse() {
            Ok(port) => port,
            Err(_) => FALLBACK_PORT,
        },
        Err(_) => FALLBACK_PORT,
    }
}

/// Refuse with the failure `what` earned, naming its exit code.
fn refuse_code(what: &str, code: i32) -> Refusal {
    Refusal::whole(format!("{what} failed with exit code {code}"))
}

/// The process exit code for the app's `code`: its own when it fits a byte,
/// 2 otherwise.
fn exit_of(code: i32) -> ExitCode {
    match u8::try_from(code) {
        Ok(0) => ExitCode::SUCCESS,
        Ok(byte) => ExitCode::from(byte),
        Err(_) => ExitCode::from(common::EXIT_REFUSED),
    }
}

/// This process's stderr as a file the app can write both its streams to,
/// so `--json` keeps stdout for the one verdict line.
#[cfg(unix)]
fn stderr_file() -> std::io::Result<File> {
    use std::os::fd::AsFd;
    Ok(File::from(std::io::stderr().as_fd().try_clone_to_owned()?))
}

/// This process's stderr as a file the app can write both its streams to,
/// so `--json` keeps stdout for the one verdict line.
#[cfg(windows)]
fn stderr_file() -> std::io::Result<File> {
    use std::os::windows::io::AsHandle;
    Ok(File::from(
        std::io::stderr().as_handle().try_clone_to_owned()?,
    ))
}

/// A target with neither file descriptors nor handles to share (WASI) cannot
/// hand the app this process's stderr, and cannot start the app either; the
/// refusal says so rather than serving with the verdict line mixed in.
#[cfg(not(any(unix, windows)))]
fn stderr_file() -> std::io::Result<File> {
    lgwks_std::trace::debug!("lgwks run: this target cannot share stderr with a child");
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "this target cannot share stderr with the app",
    ))
}

/// Wait until `addr` accepts, the app has `exited`, or the deadline passes.
///
/// The loop paces itself on a condition variable with a deadline rather
/// than on a sleep: the estate bans parking a thread in `thread::sleep`
/// where a bounded wait names what it waits for, and this wait names the
/// readiness it waits for.
fn await_ready(addr: SocketAddr, exited: &AtomicBool) -> Readiness {
    let start = Instant::now();
    let lock = std::sync::Mutex::new(());
    let pace = std::sync::Condvar::new();
    let mut guard = match lock.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    loop {
        if exited.load(Ordering::SeqCst) {
            return Readiness::Exited;
        }
        if TcpStream::connect(addr).is_ok() {
            return Readiness::Ready;
        }
        if start.elapsed() > START_TIMEOUT {
            return Readiness::TimedOut;
        }
        guard = match pace.wait_timeout(guard, PROBE_REST) {
            Ok((guard, _)) => guard,
            Err(poisoned) => poisoned.into_inner().0,
        };
    }
}

/// Serve the app in `manifest`'s directory: build twice, start, report.
pub(crate) fn run(
    args: &[String],
    json: bool,
    out: &mut impl Write,
    err: &mut impl Write,
) -> std::io::Result<ExitCode> {
    let source = match common::open_app("run", RUN_USAGE, args, json, out, err)? {
        Ok(source) => source,
        Err(code) => return Ok(code),
    };
    let manifest = source.dir().join("Cargo.toml");
    if !manifest.is_file() {
        let why = format!(
            "the directory {} holds no Cargo.toml; run lgwks new first",
            source.dir().display()
        );
        return common::fail(why, json, out, err);
    }
    // The caller's own target directory when they name one, as cargo itself
    // honors it; otherwise one shared directory every app's builds reuse.
    let target = match std::env::var_os("CARGO_TARGET_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => std::env::temp_dir().join("lgwks-target"),
    };
    if let Err(error) = std::fs::create_dir_all(&target) {
        let why = format!("the shared target directory could not be created: {error}");
        return common::fail(why, json, out, err);
    }
    let children = Children::new(true);
    let mut env = inherited_env();
    forget_launched_crate(&mut env);
    let cargo = Cargo {
        children: &children,
        env,
        root: source.dir(),
        manifest: manifest.as_path(),
        target: target.as_path(),
    };
    let verdict = build_and_serve(&cargo, source.file(), json, out, err);
    children.stop_all();
    children.finish();
    verdict
}

/// Resolve, build cold, build warm, then serve and report.
fn build_and_serve(
    cargo: &Cargo<'_>,
    file: &Path,
    json: bool,
    out: &mut impl Write,
    err: &mut impl Write,
) -> std::io::Result<ExitCode> {
    let cold_start = Instant::now();
    if let Err(refusal) = cargo.ensure_lock() {
        return common::report_one(refusal, json, out, err);
    }
    let cold_code = cargo.build();
    let cold_ms = cold_start.elapsed().as_millis();
    if cold_code != 0 {
        return common::report_one(refuse_code("the build", cold_code), json, out, err);
    }
    let warm_start = Instant::now();
    let warm_code = cargo.build();
    let warm_ms = warm_start.elapsed().as_millis();
    if warm_code != 0 {
        return common::report_one(refuse_code("the rebuild", warm_code), json, out, err);
    }
    let builds = BuildTimings { cold_ms, warm_ms };
    serve(cargo, file, &builds, json, out, err)
}

/// Serve the app, then wait on it: the facts print once it accepts.
///
/// The app runs on a scoped thread under the registry while this thread
/// probes for readiness; the scope joins it before this function returns.
fn serve(
    cargo: &Cargo<'_>,
    file: &Path,
    builds: &BuildTimings,
    json: bool,
    out: &mut impl Write,
    err: &mut impl Write,
) -> std::io::Result<ExitCode> {
    let port = app_port();
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let log = if json { Some(stderr_file()?) } else { None };
    let argv = cargo.argv("run");
    let launch = cargo.launch(&argv, SERVE_BOUND);
    let exited = AtomicBool::new(false);
    let boot = Instant::now();
    std::thread::scope(|scope| {
        let runner = scope.spawn(|| {
            let code = cargo.children.run(&launch, log.as_ref());
            exited.store(true, Ordering::SeqCst);
            code
        });
        let readiness = await_ready(addr, &exited);
        let start_ms = boot.elapsed().as_millis();
        // The facts print the moment the app accepts, while it serves; a
        // report that cannot be written stops the app rather than leaving it
        // serving unannounced.
        let announced = match readiness {
            Readiness::Ready => {
                let facts = ServeFacts {
                    port,
                    cold_ms: builds.cold_ms,
                    warm_ms: builds.warm_ms,
                    start_ms,
                };
                report_serving(file, &facts, json, out, err).and_then(|()| out.flush())
            }
            Readiness::Exited | Readiness::TimedOut => Ok(()),
        };
        if announced.is_err() || matches!(readiness, Readiness::TimedOut) {
            cargo.children.stop_all();
        }
        let code = match runner.join() {
            Ok(code) => code,
            Err(_) => {
                lgwks_std::trace::debug!("lgwks run: the app's runner panicked");
                return Err(std::io::Error::other("the app's runner panicked"));
            }
        };
        announced?;
        match readiness {
            Readiness::Ready => Ok(exit_of(code)),
            Readiness::Exited => common::report_one(refuse_code("the app", code), json, out, err),
            Readiness::TimedOut => {
                let why = format!("nothing accepted on 127.0.0.1:{port} within 30s");
                common::fail(why, json, out, err)
            }
        }
    })
}

/// Print the serving facts: the same numbers as text or as JSON.
fn report_serving(
    file: &Path,
    facts: &ServeFacts,
    json: bool,
    out: &mut impl Write,
    err: &mut impl Write,
) -> std::io::Result<()> {
    if json {
        let value = lgwks_std::json::json!({
            "ok": true,
            "cold_ms": facts.cold_ms,
            "warm_ms": facts.warm_ms,
            "start_ms": facts.start_ms,
            "port": facts.port,
        });
        common::json_line(&value, out)
    } else {
        writeln!(err, "cold build {}ms", facts.cold_ms)?;
        writeln!(err, "warm build {}ms", facts.warm_ms)?;
        writeln!(err, "start {}ms", facts.start_ms)?;
        writeln!(err, "serving {} on {}", file.display(), facts.port)
    }
}
