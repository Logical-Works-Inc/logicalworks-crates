//! The consumer gate, driven as a stranger runs it (#656).
//!
//! Every test builds a scratch consumer repository in the temporary root — a
//! manifest, a lockfile, a register and a source file, nothing else — plants
//! one defect, and runs the shipped `lgwks-deps gate --json` against it. The
//! assertions read the JSON legs, so each case proves its leg failed rather
//! than that the gate failed somewhere: an unregistered edge fails
//! `admission`, a refused flow fails `script-check`, and a widening change
//! fails `arch-diff`. A fourth case plants nothing and passes, so the gate is
//! shown to pass as well as to refuse.
//!
//! The script plants are the tool's own fixtures: the refusal is the
//! `std::process::exit` flow `script check` locates at its line, and the
//! widening is an added flow, which the architecture diff classifies as work
//! the base revision did not run. The flows live outside `src/` so the
//! clippy leg never sees them: each case isolates exactly one failing leg.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

/// What every fallible helper returns.
type HelperResult<T> = Result<T, Box<dyn std::error::Error>>;

/// What every test here returns.
type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Logs `message` where the test refused it, then hands it back: the scan
/// holds test code to the same logging rule as production code, and a bare
/// `Err` names no value the failure can be diagnosed from.
fn fail<T>(message: String) -> HelperResult<T> {
    lgwks_std::trace::debug!(message = %message, "gate_consumer: refusing");
    Err(message.into())
}

/// A script the language accepts.
const ACCEPTED: &str =
    "lgwks_bot::script! {\n    flow add(x: u32) -> u32:\n        within 20ms:\n            x\n}\n";

/// A second script the language accepts, under another name.
const ADDED: &str = "lgwks_bot::script! {\n    flow double(x: u32) -> u32:\n        within 20ms:\n            x\n}\n";

/// A script the language refuses: ending the process from a flow.
const REFUSED: &str = "lgwks_bot::script! {\n    flow stop():\n        std::process::exit(1)\n}\n";

/// Scratch case numbers, so two tests on one machine never share one.
static CASES: AtomicU64 = AtomicU64::new(0);

/// The shipped gate binary under test.
fn gate_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_lgwks-deps"))
}

/// The workspace target directory, read from the gate binary's own path, so
/// the `lgwks-ast` build below lands beside the binaries this test run uses
/// whatever target layout the runner configured.
fn target_dir() -> HelperResult<PathBuf> {
    let bin = gate_binary();
    let profile = bin
        .parent()
        .ok_or("the gate binary has no parent directory")?;
    profile
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "the gate binary has no target directory".into())
}

/// The `lgwks-ast` binary the script legs run. A workspace build makes it
/// beside the gate binary, as `tool` is a default feature of `lgwks_ast`, and
/// that one is used. Run 45510390970 rebuilt the whole tree in the dev profile
/// instead: 80 s a case against 37 s. Otherwise it is built on demand with
/// `tool`. A present binary is reused, so the cases share one build; two
/// threads racing to build it both run the same command cargo serializes on
/// its own lock. A build that fails is the test's failure, with the
/// compiler's own words attached.
fn ast_binary() -> HelperResult<PathBuf> {
    let beside = gate_binary().with_file_name("lgwks-ast");
    if beside.is_file() {
        return Ok(beside);
    }
    let built = target_dir()?.join("debug/lgwks-ast");
    if built.is_file() {
        return Ok(built);
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = Command::new("cargo")
        .args(["build", "--locked", "--target-dir"])
        .arg(target_dir()?)
        .args([
            "-p",
            "lgwks_ast",
            "--bin",
            "lgwks-ast",
            "--features",
            "tool",
        ])
        .current_dir(&root)
        .output()?;
    if !output.status.success() {
        return fail(format!(
            "building lgwks-ast for the script legs failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    if !built.is_file() {
        return fail(format!(
            "the lgwks-ast build left no binary at {}",
            built.display()
        ));
    }
    Ok(built)
}

/// A scratch consumer repository: manifest, register, source and flows.
struct Scratch {
    /// The repository root.
    root: PathBuf,
    /// The target directory every nested cargo shares, so one cold
    /// dependency build serves every case in the run.
    target: PathBuf,
}

impl Scratch {
    /// Makes the case directory with a clean library root, a manifest with
    /// `dependencies`, and the fail-closed register with no approvals.
    fn make(dependencies: &str) -> HelperResult<Self> {
        let id = CASES.fetch_add(1, Ordering::Relaxed);
        // Random bits, not the process id: pids are reused by the OS, so two
        // runs share no identity through them (the `id` row of the catalog).
        let bits = lgwks_std::random::bytes::<8>().map_err(|error| error.to_string())?;
        let name = u64::from_le_bytes(bits);
        let root = std::env::temp_dir().join(format!("lgwks-gate-consumer-{name:016x}-{id}"));
        // Under Cargo's per-suite scratch, beside the suite's own build: one
        // name every case shares, so the dependency build is warm after the
        // first, and `cargo clean` owns it. A target named per case in the temp
        // root was never shared and never removed (ADR 0078).
        let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("gate-consumer-target");
        std::fs::create_dir_all(root.join("src"))?;
        std::fs::create_dir_all(root.join("contract"))?;
        std::fs::create_dir_all(&target)?;
        std::fs::write(
            root.join("Cargo.toml"),
            format!(
                "[package]\nname = \"scratch\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[dependencies]\n{dependencies}\n"
            ),
        )?;
        std::fs::write(
            root.join("contract/APPROVED.toml"),
            "[policy]\nschema = 3\nenforce = true\naccepted_licenses = \"MIT, Apache-2.0\"\n",
        )?;
        std::fs::write(root.join("src/lib.rs"), "pub fn f() -> u32 {\n    1\n}\n")?;
        let scratch = Self { root, target };
        scratch.lock()?;
        Ok(scratch)
    }

    /// Resolves a subject directory's lockfile from the local caches.
    fn lock_at(&self, dir: &Path) -> HelperResult<()> {
        let output = Command::new("cargo")
            .args(["generate-lockfile"])
            .current_dir(dir)
            .env("CARGO_TARGET_DIR", &self.target)
            .output()?;
        if !output.status.success() {
            return fail(format!(
                "resolving {} failed: {}",
                dir.display(),
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(())
    }

    /// Resolves the scratch lockfile from the local caches.
    fn lock(&self) -> HelperResult<()> {
        self.lock_at(&self.root)
    }

    /// Writes `text` to the scratch-relative `path`, creating parents.
    fn plant(&self, path: &str, text: &str) -> HelperResult<()> {
        let file = self.root.join(path);
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&file, text)?;
        Ok(())
    }

    /// Runs the shipped gate over the scratch-relative `subject` with
    /// `--json`, through the `lgwks-ast` the setup built. Returns the exit
    /// code and the document.
    fn gate(&self, subject: &str, extra: &[String]) -> HelperResult<(i32, String)> {
        let ast = ast_binary()?;
        let mut argv = vec![
            gate_binary().to_string_lossy().into_owned(),
            "gate".to_owned(),
            self.root.join(subject).to_string_lossy().into_owned(),
            "--json".to_owned(),
            "--ast".to_owned(),
            ast.to_string_lossy().into_owned(),
        ];
        argv.extend(extra.iter().cloned());
        let mut env: BTreeMap<String, String> = std::env::vars().collect();
        env.insert(
            "CARGO_TARGET_DIR".to_owned(),
            self.target.to_string_lossy().into_owned(),
        );
        let (program, arguments) = argv.split_first().ok_or("the gate command is empty")?;
        let output = Command::new(program).args(arguments).envs(&env).output()?;
        let code = output.status.code().ok_or("the gate has no exit code")?;
        let document = String::from_utf8_lossy(&output.stdout).into_owned();
        if document.trim().is_empty() {
            return fail(format!(
                "the gate printed no JSON (exit {code}): {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok((code, document))
    }
}

impl Drop for Scratch {
    /// Removes the case directory; the shared target directory is Cargo's.
    /// A case directory that stays is named by the residue census, which
    /// fails the test (ADR 0078), so the error is traced, not swallowed.
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.root) {
            lgwks_std::trace::debug!(%error, "gate consumer: a case directory stayed");
        }
    }
}

/// Asserts the named leg came to the named verdict in the gate's document.
fn assert_leg(document: &str, name: &str, verdict: &str, case: &str) -> TestResult {
    let needle = format!("\"name\":\"{name}\",\"verdict\":\"{verdict}\"");
    if document.contains(&needle) {
        return Ok(());
    }
    Err(format!("{case}: expected {needle} in the gate document:\n{document}").into())
}

/// Plants `text` at `path`, runs the gate over the whole scratch root and
/// returns its document, refusing any exit but 1: the `case` must fail the
/// gate, not err it and not pass it.
fn refused_gate(scratch: &Scratch, path: &str, text: &str, case: &str) -> HelperResult<String> {
    scratch.plant(path, text)?;
    let (code, document) = scratch.gate("", &[])?;
    if code != 1 {
        return fail(format!(
            "{case} must fail the gate (exit 1), got {code}:\n{document}"
        ));
    }
    Ok(document)
}

#[test]
/// An unregistered edge fails the admission leg, and the gate with it.
fn sim_an_unregistered_edge_fails_the_gate() -> TestResult {
    let scratch = Scratch::make("itoa = \"1\"\n")?;
    // The crate names the edge, so the nightly's unused_dependencies lint stays
    // quiet and the clippy leg judges code, not the manifest (ADR 0066).
    let source = "use itoa as _;\n\npub fn f() -> u32 {\n    1\n}\n";
    let document = refused_gate(&scratch, "src/lib.rs", source, "an unregistered edge")?;
    assert_leg(&document, "admission", "fail", "an unregistered edge")?;
    assert_leg(&document, "script-check", "pass", "an unregistered edge")?;
    assert_leg(&document, "clippy", "pass", "an unregistered edge")?;
    Ok(())
}

#[test]
/// A refused flow fails the script-check leg, and the gate with it.
fn sim_a_refused_flow_fails_the_gate() -> TestResult {
    let scratch = Scratch::make("")?;
    // Outside `src/`, so the clippy leg never sees it: the file is a
    // refusal, not a program, and no build reads it.
    let document = refused_gate(&scratch, "flows/bad.rs", REFUSED, "a refused flow")?;
    assert_leg(&document, "script-check", "fail", "a refused flow")?;
    assert_leg(&document, "admission", "pass", "a refused flow")?;
    assert_leg(&document, "clippy", "pass", "a refused flow")?;
    Ok(())
}

#[test]
/// A widening change fails the architecture leg, and the gate with it.
fn sim_a_widening_change_fails_the_gate() -> TestResult {
    let scratch = Scratch::make("")?;
    // The head is a repository of its own beside the base: the diff matches
    // files by relative path, so the unchanged flow sits at the same path on
    // both sides and only the added flow is new.
    scratch.plant("base/flows.rs", ACCEPTED)?;
    scratch.plant(
        "head/Cargo.toml",
        "[package]\nname = \"scratch\"\nversion = \"0.0.0\"\nedition = \"2021\"\n",
    )?;
    scratch.plant(
        "head/contract/APPROVED.toml",
        "[policy]\nschema = 3\nenforce = true\naccepted_licenses = \"MIT, Apache-2.0\"\n",
    )?;
    scratch.plant("head/src/lib.rs", "pub fn f() -> u32 {\n    1\n}\n")?;
    scratch.plant("head/flows.rs", ACCEPTED)?;
    scratch.plant("head/extra.rs", ADDED)?;
    scratch.lock_at(&scratch.root.join("head"))?;
    let base = scratch.root.join("base").to_string_lossy().into_owned();
    let (code, document) = scratch.gate("head", &["--base".to_owned(), base])?;
    assert_ne!(code, 0, "a widening change must fail the gate:\n{document}");
    assert_leg(&document, "arch-diff", "fail", "a widening change")?;
    assert_leg(&document, "admission", "pass", "a widening change")?;
    assert_leg(&document, "script-check", "pass", "a widening change")?;
    Ok(())
}

#[test]
/// A consumer whose `target` is a regular file leaves the clippy leg no
/// directory for its log. The leg errs and says why, naming the directory and
/// the operating system's refusal, while the other legs still judge the code:
/// a dropped error here would read as a bare "no log file" with no cause.
fn sim_a_target_file_names_why_the_clippy_leg_errs() -> TestResult {
    let scratch = Scratch::make("")?;
    scratch.plant("target", "not a directory\n")?;
    let (code, document) = scratch.gate("", &[])?;
    assert_ne!(code, 0, "no clippy log must fail the gate:\n{document}");
    assert_leg(&document, "clippy", "error", "a target file")?;
    assert_leg(&document, "admission", "pass", "a target file")?;
    assert!(
        document.contains("could not be created"),
        "the clippy leg must name why its log directory failed:\n{document}"
    );
    Ok(())
}

#[test]
/// A clean consumer passes the gate: every applicable leg passes and the
/// inapplicable ones are named skips, not evidence.
fn sim_a_clean_consumer_passes_the_gate() -> TestResult {
    let scratch = Scratch::make("")?;
    scratch.plant("flows/good.rs", ACCEPTED)?;
    let (code, document) = scratch.gate("", &[])?;
    assert_eq!(code, 0, "a clean consumer must pass the gate:\n{document}");
    assert_leg(&document, "admission", "pass", "a clean consumer")?;
    assert_leg(&document, "script-check", "pass", "a clean consumer")?;
    assert_leg(&document, "raw-process", "pass", "a clean consumer")?;
    assert_leg(&document, "clippy", "pass", "a clean consumer")?;
    assert_leg(&document, "arch-diff", "skip", "a clean consumer")?;
    Ok(())
}

#[test]
/// A test that starts its server with a raw `Command` fails the raw-process
/// leg, and the gate with it (#1160: the tests of two consumers started
/// `logical_ci serve` and `larry acp` this way, and a SIGKILL left them alive).
/// The finding names the file, the line and the `Supervisor` to use instead.
fn sim_a_raw_process_in_test_code_fails_the_gate() -> TestResult {
    let scratch = Scratch::make("")?;
    let serve = concat!(
        "#[test]\n",
        "fn serves() {\n",
        "    let _server = std::process::Command::new(\"sleep\").arg(\"5\").spawn();\n",
        "}\n",
    );
    let document = refused_gate(&scratch, "tests/serve.rs", serve, "a raw process in a test")?;
    assert_leg(&document, "raw-process", "fail", "a raw process in a test")?;
    assert_leg(&document, "admission", "pass", "a raw process in a test")?;
    assert_leg(&document, "script-check", "pass", "a raw process in a test")?;
    assert!(
        document.contains("serve.rs") && document.contains("Supervisor"),
        "the leg must name the file and the owner to use:\n{document}"
    );
    Ok(())
}
