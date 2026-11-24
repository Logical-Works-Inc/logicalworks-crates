//! Steps of `ci.yml`, each run as CI runs it: its own body under bash, with
//! fake tools on `PATH` in a scratch directory.
//!
//! Source: PR 1142, run 45417609208. GitHub answered the pull request's file
//! listing with HTTP 500 ("this diff is temporarily unavailable due to heavy
//! server load"). Under `set -euo pipefail` the step exited 1, so the required
//! `Build and workspace tests` row failed with every test unrun. Requirement
//! proved (INV-CI-DOCS-ONLY-1): a listing that fails is code, so the suites
//! run, and a listing of only reader-facing pages is not.
//!
//! Source: Gemini's CI review for the Director, 2026-11-24, "Crate-graph and
//! AST blast-radius routing", and PR 1148, which changed one lgwks_ast test
//! and ran all 19 lgwks_bot rows. Requirement proved (INV-CI-ROUTE-1, ADR
//! 0086): a change no lgwks_bot suite builds from routes the bot rows off; a
//! file lgwks_bot includes, its dependencies' code and every unknown path
//! route them on; the route table is the crate graph `cargo metadata` reads;
//! and the listings key moves exactly when a bot input moves.
//!
//! Source: PR 1146, run 45418938734. The wait step saw the bot-rest listing in
//! the REST API at 14:55:09.5, and `download-artifact` read the artifact
//! service 0.3 s later and found 33 artifacts without it, so the simulation
//! lane failed on a missing file. Requirement proved: a listing the download
//! missed is fetched by name, and one that never comes fails by name.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output, Stdio};

use lgwks_std::json::{self, Value};

type TestResult = Result<(), Box<dyn Error>>;

/// A workspace member: its directory and its workspace dependencies.
struct Member {
    dir: String,
    deps: Vec<Dep>,
}

/// One workspace dependency, and whether only tests build it.
struct Dep {
    name: String,
    dev: bool,
}

/// What the step wrote to `GITHUB_OUTPUT`, by name.
type Outputs = BTreeMap<String, String>;

/// The repository root.
fn repo() -> Result<PathBuf, Box<dyn Error>> {
    Ok(Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?)
}

/// The body of step `name`'s `run: |` block, its indent removed.
fn step_body(name: &str) -> Result<String, Box<dyn Error>> {
    let text = fs::read_to_string(repo()?.join(".github/workflows/ci.yml"))?;
    let wanted = format!("- name: {name}");
    let mut lines = text
        .lines()
        .skip_while(|line| line.trim() != wanted)
        .skip_while(|line| line.trim() != "run: |")
        .skip(1);
    let first = lines.next().ok_or("the step has no run body")?;
    let pad = " ".repeat(first.len().saturating_sub(first.trim_start().len()));
    let mut body = vec![first.trim_start()];
    for line in lines {
        match line.strip_prefix(&pad) {
            Some(rest) => body.push(rest),
            None if line.trim().is_empty() => body.push(""),
            None => break,
        }
    }
    Ok(body.join("\n"))
}

/// A scratch git index holding files, with no commit, so no hook runs.
struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(case: &str, files: &[(&str, &str)]) -> Result<Self, Box<dyn Error>> {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("ci-changes-{case}"));
        if root.exists() {
            fs::remove_dir_all(&root)?;
        }
        fs::create_dir_all(&root)?;
        let fixture = Self { root };
        fixture.git(&["init", "--quiet"])?;
        fixture.write(files)?;
        Ok(fixture)
    }

    /// Removes the fixture; a failed test leaves it for the next run's `new`.
    fn finish(self) -> TestResult {
        fs::remove_dir_all(&self.root)?;
        Ok(())
    }

    /// Runs git in the fixture, with no system config and no maintenance.
    fn git(&self, args: &[&str]) -> TestResult {
        let ran = Command::new("git")
            .args(["-c", "maintenance.auto=false"])
            .args(args)
            .current_dir(&self.root)
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/local/bin")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(Stdio::null())
            .output()?;
        assert!(
            ran.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&ran.stderr)
        );
        Ok(())
    }

    /// Writes `files` and stages them.
    fn write(&self, files: &[(&str, &str)]) -> TestResult {
        for &(path, text) in files {
            let full = self.root.join(path);
            if let Some(parent) = full.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(full, text)?;
        }
        self.git(&["add", "--all"])
    }

    /// Runs the step with a `gh` that prints `listing` and exits `exit`, and
    /// returns the step's exit code and each output it wrote.
    fn classify(&self, listing: &str, exit: i32) -> Result<(Option<i32>, Outputs), Box<dyn Error>> {
        let bin = self.root.join(".bin");
        fs::create_dir_all(&bin)?;
        let gh = bin.join("gh");
        fs::write(
            &gh,
            format!("#!/bin/sh\nprintf '%s' '{listing}'\nexit {exit}\n"),
        )?;
        fs::set_permissions(&gh, fs::Permissions::from_mode(0o755))?;
        // The step runs on an Ubuntu runner, which has GNU `sha256sum`. macOS
        // ships only `shasum`, whose `-a 256` prints the same `<hash>  -` line,
        // so the Mac's gate copy gets that one shim and Linux keeps the real tool.
        let host_has_it = ["/usr/bin/sha256sum", "/bin/sha256sum"]
            .iter()
            .any(|path| Path::new(path).exists());
        if !host_has_it {
            let shim = bin.join("sha256sum");
            fs::write(&shim, "#!/bin/sh\nexec /usr/bin/shasum -a 256 \"$@\"\n")?;
            fs::set_permissions(&shim, fs::Permissions::from_mode(0o755))?;
        }
        let output = bin.join("output");
        fs::write(&output, "")?;
        let status = Command::new("/bin/bash")
            .arg("-c")
            .arg(step_body("Classify the change")?)
            .current_dir(&self.root)
            .env_clear()
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin:/usr/local/bin", bin.display()),
            )
            .env("PR", "1142")
            .env("GITHUB_REPOSITORY", "owner/repo")
            .env("GITHUB_OUTPUT", &output)
            .status()?;
        let written = fs::read_to_string(&output)?
            .lines()
            .filter_map(|line| line.split_once('='))
            .map(|(name, value)| (name.to_owned(), value.to_owned()))
            .collect();
        fs::remove_dir_all(&bin)?;
        Ok((status.code(), written))
    }

    /// The step's listings key for the fixture's index.
    fn key(&self) -> Result<String, Box<dyn Error>> {
        let (_, written) = self.classify("", 0)?;
        written.get("key").cloned().ok_or_else(|| "no key".into())
    }
}

#[test]
fn a_failed_listing_is_code_and_a_docs_listing_is_not() -> TestResult {
    let fixture = Fixture::new("classify", &[("Cargo.toml", "[workspace]\n")])?;
    let cases = [
        ("server-error", "", 1, "true", "true"),
        (
            "docs-only",
            "docs/guide.md\nllms.txt\nGOVERNANCE.md",
            0,
            "false",
            "false",
        ),
        (
            "rust",
            "docs/guide.md\ncrates/lgwks-deps/src/lib.rs",
            0,
            "true",
            "true",
        ),
        ("generated", "docs/catalog.md", 0, "true", "true"),
        ("empty", "", 0, "true", "true"),
        (
            "lower-tests",
            "crates/lgwks-ast/tests/it/sim_scan_examples.rs\ncrates/lgwks-deps/examples/a.rs",
            0,
            "true",
            "false",
        ),
        ("drishti", "crates/drishti/src/main.rs", 0, "true", "false"),
        (
            "support",
            "crates/lgwks-std/tests/support/prop.rs",
            0,
            "true",
            "true",
        ),
        (
            "churn-slot",
            "crates/lgwks-deps/src/bin/churn_slot/main.rs",
            0,
            "true",
            "true",
        ),
        ("workflow", ".github/workflows/ci.yml", 0, "true", "true"),
    ];
    for (case, listing, exit, code, bot) in cases {
        let (status, written) = fixture.classify(listing, exit)?;
        assert_eq!(status, Some(0), "{case}: the step must not fail the run");
        assert_eq!(
            written.get("code").map(String::as_str),
            Some(code),
            "{case}"
        );
        assert_eq!(written.get("bot").map(String::as_str), Some(bot), "{case}");
    }
    fixture.finish()
}

#[test]
fn the_listings_key_moves_only_with_a_bot_input() -> TestResult {
    let base = [
        ("crates/lgwks-bot/src/lib.rs", "pub fn bot() {}\n"),
        ("crates/lgwks-deps/tests/it/main.rs", "mod a;\n"),
        (
            "crates/lgwks-deps/tests/support/fixture.rs",
            "pub fn f() {}\n",
        ),
        ("crates/drishti/src/main.rs", "fn main() {}\n"),
        ("docs/guide.md", "# Guide\n"),
        ("Cargo.lock", "version = 4\n"),
    ];
    let fixture = Fixture::new("key", &base)?;
    let first = fixture.key()?;
    let digest = first.strip_prefix("bot-listings-").ok_or("no key prefix")?;
    assert_eq!(digest.len(), 64, "{first} is one SHA-256");
    fixture.write(&[
        ("crates/lgwks-deps/tests/it/main.rs", "mod b;\n"),
        ("crates/drishti/src/main.rs", "fn main() { drishti() }\n"),
        ("docs/guide.md", "# Guide, changed\n"),
    ])?;
    assert_eq!(
        fixture.key()?,
        first,
        "tests, drishti and pages leave the key"
    );
    for (path, text) in [
        (
            "crates/lgwks-deps/tests/support/fixture.rs",
            "pub fn g() {}\n",
        ),
        ("crates/lgwks-bot/src/lib.rs", "pub fn bot2() {}\n"),
        ("Cargo.lock", "version = 5\n"),
    ] {
        let before = fixture.key()?;
        fixture.write(&[(path, text)])?;
        assert_ne!(fixture.key()?, before, "{path} moves the key");
    }
    fixture.finish()
}

/// The text field `key` of `value`.
fn text<'value>(value: &'value Value, key: &str) -> Result<&'value str, Box<dyn Error>> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("no `{key}` in {value}").into())
}

/// The array field `key` of `value`.
fn array<'value>(value: &'value Value, key: &str) -> Result<&'value [Value], Box<dyn Error>> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| format!("no `{key}` array").into())
}

/// Each workspace member by package name, from `cargo metadata`.
fn members() -> Result<BTreeMap<String, Member>, Box<dyn Error>> {
    let root = repo()?;
    let ran = Command::new(env!("CARGO"))
        .args(["metadata", "--format-version", "1", "--no-deps", "--locked"])
        .current_dir(&root)
        .stdin(Stdio::null())
        .output()?;
    assert!(
        ran.status.success(),
        "cargo metadata: {}",
        String::from_utf8_lossy(&ran.stderr)
    );
    let payload: Value = json::from_str(&String::from_utf8(ran.stdout)?)?;
    let mut found = BTreeMap::new();
    for package in array(&payload, "packages")? {
        let manifest = Path::new(text(package, "manifest_path")?);
        let dir = manifest
            .parent()
            .ok_or("no dir")?
            .strip_prefix(&root)?
            .to_string_lossy()
            .into_owned();
        let mut deps = Vec::new();
        for dep in array(package, "dependencies")? {
            if dep.get("path").is_none_or(Value::is_null) {
                continue;
            }
            // `kind` is null for a normal dependency, else "dev" or "build".
            let kind = dep.get("kind");
            let named = kind.and_then(Value::as_str);
            assert!(
                named.is_some() || kind.is_none_or(Value::is_null),
                "a dependency kind in {dep}"
            );
            let dev = named == Some("dev");
            deps.push(Dep {
                name: text(dep, "name")?.to_owned(),
                dev,
            });
        }
        found.insert(text(package, "name")?.to_owned(), Member { dir, deps });
    }
    Ok(found)
}

#[test]
fn the_route_table_is_the_crate_graph() -> TestResult {
    let members = members()?;
    let bot = members.get("lgwks_bot").ok_or("no lgwks_bot")?;
    // lgwks_bot's tests build every kind of its dependency, and each of those
    // builds its own normal and build dependencies.
    let mut closure: BTreeSet<String> = bot.deps.iter().map(|dep| dep.name.clone()).collect();
    let mut queue: Vec<String> = closure.iter().cloned().collect();
    while let Some(name) = queue.pop() {
        let Some(member) = members.get(&name) else {
            continue;
        };
        for dep in &member.deps {
            if !dep.dev && closure.insert(dep.name.clone()) {
                queue.push(dep.name.clone());
            }
        }
    }
    closure.remove("lgwks_bot");
    let body = step_body("Classify the change")?;
    let mut expected = 0_usize;
    for (name, member) in &members {
        if name == "lgwks_bot" {
            continue;
        }
        let dir = &member.dir;
        let line = if closure.contains(name) {
            format!("{dir}/tests/*|{dir}/examples/*|{dir}/benches/*) kind=other ;;")
        } else {
            format!("{dir}/*) kind=other ;;")
        };
        assert!(body.contains(&line), "{name}: the route needs `{line}`");
        expected = expected.saturating_add(1);
    }
    assert_eq!(
        body.matches("kind=other ;;").count(),
        expected,
        "every route to `other` is one workspace member's"
    );
    Ok(())
}

/// Every literal path a `#[path]` or `include*!` in `text` names.
fn included(text: &str) -> Vec<&str> {
    let mut found = Vec::new();
    for marker in [
        "#[path = \"",
        "include_str!(\"",
        "include_bytes!(\"",
        "include!(\"",
    ] {
        for (start, _) in text.match_indices(marker) {
            let Some(rest) = text.get(start.saturating_add(marker.len())..) else {
                continue;
            };
            if let Some((path, _)) = rest.split_once('"') {
                found.push(path);
            }
        }
    }
    found
}

/// `path` with every `.` and `..` resolved, without touching the disk.
fn normal(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// Every `.rs` file under `dir`.
fn sources(dir: &Path, found: &mut Vec<PathBuf>) -> TestResult {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            sources(&path, found)?;
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(path);
        }
    }
    Ok(())
}

#[test]
fn every_file_lgwks_bot_includes_routes_the_bot_rows_on() -> TestResult {
    let root = repo()?;
    let bot = root.join("crates/lgwks-bot");
    let mut files = Vec::new();
    sources(&bot, &mut files)?;
    let mut outside = BTreeSet::new();
    for file in &files {
        let text = fs::read_to_string(file)?;
        let dir = file.parent().ok_or("no dir")?;
        for path in included(&text) {
            let target = normal(&dir.join(path));
            if !target.starts_with(&bot) {
                let relative = target.strip_prefix(&root)?;
                outside.insert(relative.to_string_lossy().into_owned());
            }
        }
    }
    assert!(
        outside.contains("crates/lgwks-std/tests/support/prop.rs"),
        "the scan finds the lgwks_std fixture lgwks_bot includes: {outside:?}"
    );
    // The nextest setup script builds churn-slot for every lgwks_bot test.
    outside.insert("crates/lgwks-deps/src/bin/churn_slot.rs".to_owned());
    let fixture = Fixture::new("includes", &[("Cargo.toml", "[workspace]\n")])?;
    for path in &outside {
        let (_, written) = fixture.classify(path, 0)?;
        assert_eq!(
            written.get("bot").map(String::as_str),
            Some("true"),
            "lgwks_bot includes {path}, so a change to it runs the bot rows"
        );
    }
    fixture.finish()
}

/// A fresh scratch directory for one case.
fn scratch(case: &str) -> Result<PathBuf, Box<dyn Error>> {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("ci-step-{case}"));
    if root.exists() {
        fs::remove_dir_all(&root)?;
    }
    fs::create_dir_all(root.join("bin"))?;
    Ok(root)
}

/// Writes an executable `/bin/sh` script `name` into `root/bin`.
fn tool(root: &Path, name: &str, script: &str) -> TestResult {
    let path = root.join("bin").join(name);
    fs::write(&path, format!("#!/bin/sh\n{script}"))?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
    Ok(())
}

/// Runs step `name` in `root` with `root/bin` first on `PATH`.
fn run_step(root: &Path, name: &str, env: &[(&str, &str)]) -> Result<Output, Box<dyn Error>> {
    let path = format!("{}:/usr/bin:/bin", root.join("bin").display());
    let mut command = Command::new("/bin/bash");
    command
        .arg("-c")
        .arg(step_body(name)?)
        .current_dir(root)
        .env_clear()
        .env("PATH", path)
        .env("GITHUB_REPOSITORY", "owner/repo")
        .env("GITHUB_API_URL", "https://api.invalid")
        .env("GITHUB_RUN_ID", "7");
    for &(key, value) in env {
        command.env(key, value);
    }
    Ok(command.output()?)
}

/// Source: PR 1146, run 45426215266. The bench workspace is its own Cargo
/// root, and rust-cache saved only the root `target`, so every run compiled
/// it cold twice: 64 s for the tests and 127 s for the LTO release build on
/// run 45419917546. On a slower runner it was still compiling at the 5
/// minute cap. Requirement proved: a job that builds a nested workspace
/// passes that workspace to the cache, and the action hands it to
/// rust-cache.
#[test]
fn a_nested_workspace_job_caches_its_own_target() -> TestResult {
    let repo = repo()?;
    let action = fs::read_to_string(repo.join(".github/actions/hosted-rust/action.yml"))?;
    assert!(
        action.contains("workspaces: ${{ inputs.workspace }} -> target"),
        "hosted-rust hands its workspace to rust-cache"
    );
    let workflow = fs::read_to_string(repo.join(".github/workflows/ci.yml"))?;
    let mut nested = Vec::new();
    for (id, body) in crate::ci_compile_cache::jobs(&workflow) {
        let pieces: Vec<&str> = body.split("--manifest-path ").collect();
        for pair in pieces.windows(2) {
            let &[before, after] = pair else { continue };
            let Some(dir) = after
                .split_whitespace()
                .next()
                .and_then(|path| path.strip_suffix("/Cargo.toml"))
            else {
                continue;
            };
            let manifest = fs::read_to_string(repo.join(dir).join("Cargo.toml"))?;
            if !manifest.lines().any(|line| line == "[workspace]") {
                continue;
            }
            // The command the flag belongs to: from its `cargo` to the next `&&` or step.
            let Some((_, head)) = before.rsplit_once("cargo ") else {
                continue;
            };
            let tail = match after.split(" && ").next() {
                Some(command) => match command.split("\n      - ").next() {
                    Some(step) => step,
                    None => command,
                },
                None => after,
            };
            if head.starts_with("fmt")
                || head.contains("--target-dir")
                || tail.contains("--target-dir")
            {
                continue;
            }
            nested.push(id);
            assert!(
                body.contains(&format!("workspace: {dir}\n")),
                "job {id} builds {dir} but caches only the root target"
            );
        }
    }
    assert!(
        nested.contains(&"timing_async"),
        "the bench job builds a nested workspace: {nested:?}"
    );
    Ok(())
}

/// The fetch step, given a download that left out bot-rest. `serves` says
/// whether the REST API answers for it.
fn fetch_missing(case: &str, serves: bool) -> Result<(Output, bool), Box<dyn Error>> {
    let root = scratch(case)?;
    let listings = root.join("nextest-listing");
    let suites = [
        "workspace",
        "bot-it-scale",
        "bot-it-surface",
        "bot-it-refusals",
        "bot-it-g1",
        "bot-it-g2",
        "bot-it-g3",
        "bot-it-g4",
        "bot-it-g5",
        "bot-it-g6",
        "bot-it-g7",
        "bot-it-g8",
    ];
    for suite in suites {
        let dir = listings.join(format!("nextest-listing-{suite}-Linux"));
        fs::create_dir_all(&dir)?;
        fs::write(dir.join("listing.json"), "{}")?;
    }
    let curl = if serves {
        "out=''; url=''\nwhile [ $# -gt 0 ]; do\n  case \"$1\" in -o) out=\"$2\"; shift ;; https*) url=\"$1\" ;; esac\n  shift\ndone\n\
         case \"$url\" in\n  *'artifacts?name=nextest-listing-bot-rest-Linux') printf '{\"total_count\": 1, \"artifacts\": [{\"id\": 42, \"name\": \"x\"}]}' ;;\n  \
         */artifacts/42/zip) printf 'zip' > \"$out\" ;;\n  *) exit 22 ;;\nesac\n"
    } else {
        "exit 22\n"
    };
    tool(&root, "curl", curl)?;
    tool(
        &root,
        "unzip",
        "dir=''\nwhile [ $# -gt 0 ]; do\n  case \"$1\" in -d) dir=\"$2\"; shift ;; esac\n  shift\ndone\n\
         mkdir -p \"$dir\" && printf '{\"rust-suites\": {}}' > \"$dir/listing.json\"\n",
    )?;
    let ran = run_step(
        &root,
        "Download any listing the first download missed",
        &[("TOKEN", "t"), ("LISTING_RETRY_SECS", "0")],
    )?;
    let fetched = listings
        .join("nextest-listing-bot-rest-Linux/listing.json")
        .exists();
    fs::remove_dir_all(&root)?;
    Ok((ran, fetched))
}

#[test]
fn a_listing_the_download_missed_is_fetched_by_name() -> TestResult {
    let (ran, fetched) = fetch_missing("fetch-served", true)?;
    let said = String::from_utf8_lossy(&ran.stdout).into_owned();
    assert_eq!(ran.status.code(), Some(0), "{said}");
    assert!(fetched, "the bot-rest listing is fetched: {said}");
    let (ran, fetched) = fetch_missing("fetch-never", false)?;
    let said = String::from_utf8_lossy(&ran.stdout).into_owned();
    assert_eq!(
        ran.status.code(),
        Some(1),
        "a listing that never comes fails: {said}"
    );
    assert!(!fetched);
    assert!(
        said.contains("bot-rest"),
        "the missing suite is named: {said}"
    );
    Ok(())
}
