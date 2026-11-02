//! Scratch directories for the `lgwks` command tests.
//!
//! One helper serves every family that drives the built binary, so a fix to
//! the scratch layout is a fix to all of them: a directory unique to the
//! process and to the call, created empty, removed on drop.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// How many scratch directories this process has handed out.
static NEXT: AtomicU64 = AtomicU64::new(0);

/// A scratch directory, removed when the test ends.
pub struct Scratch {
    /// The directory.
    dir: PathBuf,
}

impl Scratch {
    /// A fresh directory for `label`, unique to the process and the call.
    ///
    /// The name draws OS entropy through the estate's one source, with the
    /// call counter beside it: a process id is reused by the OS, so two test
    /// processes would name the same directory.
    ///
    /// # Errors
    ///
    /// The directory could not be created, or entropy refused its name.
    pub fn fresh(label: &str) -> std::io::Result<Self> {
        match scratch_dir(label) {
            Ok(dir) => Ok(Self { dir }),
            Err(error) => Err(std::io::Error::other(error)),
        }
    }

    /// The scratch root this test builds and serves under.
    pub fn dir(&self) -> &Path {
        self.dir.as_path()
    }
}

/// The scratch directory path for `label`, created empty.
///
/// The name draws OS entropy through the estate's one source, with the call
/// counter beside it.
fn scratch_dir(label: &str) -> Result<PathBuf, String> {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let nonce = match lgwks_std::random::bytes::<8>() {
        Ok(nonce) => lgwks_std::hex::encode(nonce),
        Err(error) => {
            lgwks_std::trace::debug!(error = %error, "lgwks-test: entropy refused a scratch name");
            return Err(format!("no entropy for a scratch directory: {error}"));
        }
    };
    let dir = std::env::temp_dir().join(format!("lgwks-test-{nonce}-{id}-{label}"));
    std::fs::create_dir_all(&dir).map_err(|error| format!("no scratch: {error}"))?;
    Ok(dir)
}

/// A unique tag for `label`: entropy plus the call counter, spelling only
/// letters, digits and `-`, so it names app directories and idempotency keys.
pub fn unique_tag(label: &str) -> Result<String, String> {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    match lgwks_std::random::bytes::<4>() {
        Ok(nonce) => Ok(format!("{label}-{}-{id}", lgwks_std::hex::encode(nonce))),
        Err(error) => {
            lgwks_std::trace::debug!(error = %error, "lgwks-test: entropy refused a tag");
            Err(format!("no entropy for a test tag: {error}"))
        }
    }
}

/// A loopback port in 18000..19000 drawn from OS entropy, for one server.
pub fn test_port() -> Result<u16, String> {
    loop {
        let nonce = match lgwks_std::random::bytes::<2>() {
            Ok(nonce) => u16::from_le_bytes(nonce),
            Err(error) => {
                lgwks_std::trace::debug!(error = %error, "lgwks-test: entropy refused a port");
                return Err(format!("no entropy for a test port: {error}"));
            }
        };
        if nonce < 1_000 {
            return Ok(18_000_u16.saturating_add(nonce));
        }
    }
}

/// Pace a readiness poll on a condition variable with a deadline.
///
/// The estate bans parking a thread in `thread::sleep` where a bounded wait
/// names what it waits for; this names the readiness, checks it, and reports
/// whether it arrived within `deadline`, pacing tries `step` apart.
pub fn poll_ready(
    deadline: std::time::Duration,
    step: std::time::Duration,
    mut ready: impl FnMut() -> bool,
) -> bool {
    let start = std::time::Instant::now();
    let lock = std::sync::Mutex::new(());
    let pace = std::sync::Condvar::new();
    let mut guard = match lock.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    loop {
        if ready() {
            return true;
        }
        if start.elapsed() > deadline {
            return false;
        }
        guard = match pace.wait_timeout(guard, step) {
            Ok((guard, _)) => guard,
            Err(poisoned) => poisoned.into_inner().0,
        };
    }
}

impl Drop for Scratch {
    /// Best-effort removal: a leftover is a name only, and a failing
    /// removal must not fail the test that observed it.
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.dir) {
            lgwks_std::trace::debug!(error = %error, "lgwks-test: a scratch directory survived");
        }
    }
}

/// One run of the built `lgwks` binary.
pub struct Outcome {
    /// The exit code, or `None` when a signal ended the process.
    pub(crate) code: Option<i32>,
    /// What it wrote to stdout.
    pub(crate) stdout: Vec<u8>,
    /// What it wrote to stderr.
    pub(crate) stderr: String,
}

/// The `lgwks` binary under test: the binary Cargo built for this suite.
pub fn binary() -> Result<PathBuf, String> {
    match std::env::var_os("CARGO_BIN_EXE_lgwks") {
        Some(binary) => Ok(PathBuf::from(binary)),
        None => {
            lgwks_std::trace::debug!("lgwks-test: no test binary was provided");
            Err(String::from("Cargo did not provide the lgwks test binary"))
        }
    }
}

/// Drive the built `lgwks` binary with `args` from `dir`, capturing both
/// streams so the verdict can assert on them.
pub fn run_lgwks(dir: &Path, args: &[&str]) -> Result<Outcome, String> {
    let output = std::process::Command::new(binary()?)
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|error| format!("lgwks could not start: {error}"))?;
    match String::from_utf8(output.stderr) {
        Ok(stderr) => Ok(Outcome {
            code: output.status.code(),
            stdout: output.stdout,
            stderr,
        }),
        Err(_) => {
            lgwks_std::trace::debug!("lgwks-test: the binary wrote non-UTF-8 to stderr");
            Err(String::from("lgwks wrote non-UTF-8 to stderr"))
        }
    }
}

/// The workspace root: two levels above this package's manifest dir.
pub fn workspace_root() -> Result<PathBuf, String> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    match manifest.parent().and_then(Path::parent) {
        Some(root) => Ok(root.to_path_buf()),
        None => Err(String::from(
            "the workspace root is not above the manifest dir",
        )),
    }
}

/// Points every app scaffolded under `dir` at this workspace's `lgwks_bot`
/// and `lgwks_std`, through a `.cargo/config.toml` beside the app rather
/// than inside it: the tree under test is what the app builds against, never
/// the last release on crates.io, and the scaffold itself stays untouched.
fn patch_workspace(dir: &Path) -> Result<(), String> {
    let root = workspace_root()?;
    let config = dir.join(".cargo");
    std::fs::create_dir_all(&config).map_err(|error| format!("no cargo config: {error}"))?;
    let mut text = String::from("[patch.crates-io]\n");
    for (name, path) in [("lgwks_bot", "lgwks-bot"), ("lgwks_std", "lgwks-std")] {
        let crate_dir = root.join("crates").join(path);
        text.push_str(
            format!(
                "{name} = {{ path = {:?} }}\n",
                crate_dir.display().to_string()
            )
            .as_str(),
        );
    }
    std::fs::write(config.join("config.toml"), text)
        .map_err(|error| format!("the cargo config could not be written: {error}"))
}

/// The target directory every app a test serves builds in: under Cargo's
/// per-suite scratch, so the suite's own target cache keeps it warm.
pub fn app_target() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("lgwks-apps")
}

/// Start `lgwks run` (with `--json` when asked) in `dir` on `port`, building
/// in [`app_target`]; stdout is piped for `--json`, inherited otherwise.
pub fn spawn_run(dir: &Path, port: u16, json: bool) -> Result<std::process::Child, String> {
    let mut command = std::process::Command::new(binary()?);
    command.arg("run");
    if json {
        command.arg("--json").stdout(std::process::Stdio::piped());
    } else {
        command.stdout(std::process::Stdio::inherit());
    }
    command
        .current_dir(dir)
        .env("PORT", port.to_string())
        .env("CARGO_TARGET_DIR", app_target())
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .map_err(|error| format!("lgwks run could not start: {error}"))
}

/// Kill `lgwks run` alone, as a supervisor would, then require `port` shut
/// within `deadline`: the app it started must stop with it. Nothing else is
/// signalled, so a port left open is the command's own defect.
pub fn stop_run(
    child: &mut std::process::Child,
    port: u16,
    deadline: std::time::Duration,
) -> Result<(), String> {
    if let Err(error) = child.kill() {
        lgwks_std::trace::debug!(error = %error, "lgwks-test: lgwks run could not be killed");
    }
    if let Err(error) = child.wait() {
        lgwks_std::trace::debug!(error = %error, "lgwks-test: waiting on the killed run failed");
    }
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let step = std::time::Duration::from_millis(200);
    if poll_ready(deadline, step, || {
        std::net::TcpStream::connect(addr).is_err()
    }) {
        Ok(())
    } else {
        lgwks_std::trace::debug!(port, "lgwks-test: the app outlived lgwks run");
        Err(format!(
            "the app still serves on {port} after lgwks run was killed"
        ))
    }
}

/// Scaffold `name` in `dir` with `lgwks new` and read back its app file.
pub fn reference_app(dir: &Path, name: &str) -> Result<String, String> {
    patch_workspace(dir)?;
    let outcome = run_lgwks(dir, &["new", name, "--json"])?;
    if outcome.code != Some(0) {
        lgwks_std::trace::debug!(
            stderr = outcome.stderr.as_str(),
            "lgwks-test: lgwks new refused the scaffold"
        );
        return Err(format!(
            "lgwks new refused the reference app: {}",
            outcome.stderr
        ));
    }
    std::fs::read_to_string(dir.join(name).join("app.rs"))
        .map_err(|error| format!("the reference app could not be read: {error}"))
}
