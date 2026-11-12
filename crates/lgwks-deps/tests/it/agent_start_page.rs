//! Holds the pages an agent starts from to the tree they describe (ADR 0022).
//!
//! A hand-kept page drifts: an audit on 2026-11-08 found guides naming 7.0.0 on
//! an 8.1.0 tree, a publish order the manifests refuted, and a bot README
//! quickstart that did not compile. Each test here reads the page and the tree,
//! so the next such drift fails the suite instead of misleading the next agent.
//! The skill-drift feedback loop is from dev.to/gde "Agent Skills Drift".

use std::collections::BTreeSet;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

type TestResult = Result<(), Box<dyn Error>>;

/// The crate directories this repository publishes.
const CRATES: [&str; 6] = [
    "lgwks-std",
    "lgwks-ast",
    "lgwks-deps",
    "lgwks-macros",
    "lgwks-bot",
    "drishti",
];

/// Where a word that names a repository path begins.
const PATH_ROOTS: [&str; 9] = [
    "crates/",
    "docs/",
    "scripts/",
    "skills/",
    "contract/",
    "bench/",
    ".github/",
    ".cargo/",
    ".config/",
];

/// What the tree holds that a page may name.
pub(crate) struct Known {
    /// Every gate lane id.
    pub(crate) lanes: BTreeSet<String>,
    /// Every declared binary.
    pub(crate) binaries: BTreeSet<String>,
    /// Every package name.
    pub(crate) packages: BTreeSet<String>,
}

/// The repository root.
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A repository file as text.
fn page(relative: &str) -> Result<String, Box<dyn Error>> {
    Ok(fs::read_to_string(root().join(relative))?)
}

/// The value of each `<key> = "value"` line of `text`.
fn quoted_values(text: &str, key: &str) -> Vec<String> {
    let opening = format!("{key} = \"");
    text.lines()
        .filter_map(|line| line.strip_prefix(opening.as_str()))
        .filter_map(|rest| rest.split_once('"'))
        .map(|(value, _)| value.to_owned())
        .collect()
}

/// A crate's manifest as text.
fn manifest(directory: &str) -> Result<String, Box<dyn Error>> {
    page(&format!("crates/{directory}/Cargo.toml"))
}

/// A crate's own `name` and `version`, the first of each its manifest holds.
fn identity(directory: &str) -> Result<(String, String), Box<dyn Error>> {
    let text = manifest(directory)?;
    let name = quoted_values(&text, "name").into_iter().next();
    let version = quoted_values(&text, "version").into_iter().next();
    match (name, version) {
        (Some(name), Some(version)) => Ok((name, version)),
        _ => Err(format!("crates/{directory}/Cargo.toml names no package").into()),
    }
}

/// The long form named `scripts/check-gate-parity.py` for weeks after the Python
/// ban deleted it, because at 2798 lines the size law refused any edit (audit of
/// 2026-11-08, INV-DOC-7). Split, each page stays editable, reachable and honest.
#[test]
fn every_long_form_page_is_indexed_editable_and_names_no_deleted_script() -> TestResult {
    let index = page("docs/invariants-long-form.md")?;
    let mut pages = 0_usize;
    for entry in fs::read_dir(root().join("docs/invariants"))? {
        let path = entry?.path();
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        let name = name.ok_or("a long-form page has no file name")?;
        let text = fs::read_to_string(&path)?;
        assert!(
            index.contains(&format!("(invariants/{name})")),
            "{name} is not indexed"
        );
        assert!(
            text.lines().count() <= 120,
            "{name} is over the 120-line law"
        );
        assert!(
            !text.contains(".py"),
            "{name} names a Python script the ban deleted"
        );
        pages += 1;
    }
    assert!(pages > 1, "the long form is one page again");
    assert!(
        index.lines().count() <= 120,
        "the long-form index is over the law"
    );
    Ok(())
}

/// What the live tree holds.
fn known() -> Result<Known, Box<dyn Error>> {
    let mut binaries = BTreeSet::new();
    let mut packages = BTreeSet::new();
    for directory in CRATES {
        let text = manifest(directory)?;
        packages.insert(identity(directory)?.0);
        for section in text.split("[[bin]]").skip(1) {
            binaries.extend(quoted_values(section, "name").into_iter().take(1));
        }
    }
    let lanes = quoted_values(&page("scripts/gate-lanes.toml")?, "id");
    Ok(Known {
        lanes: lanes.into_iter().collect(),
        binaries,
        packages,
    })
}

/// A word with a placeholder or a glob, which names a family and not one thing.
fn is_placeholder(word: &str) -> bool {
    word.contains(['<', '>', '*', '{', '}', '$'])
}

/// Whether `word` is an absolute or home-relative path, which opens on one
/// machine and in no reader's checkout.
fn is_machine_path(word: &str) -> bool {
    let bare = word.trim_start_matches(['`', '(', '"']);
    let rooted = bare
        .strip_prefix('/')
        .is_some_and(|rest| rest.starts_with(char::is_alphanumeric));
    rooted || bare.starts_with("~/")
}

/// `word` as a repository path, when it reads as one.
fn path_word(word: &str) -> Option<&str> {
    let bare = word
        .trim_start_matches("./")
        .trim_end_matches(['.', ',', ';', ':', ')']);
    let bare = match bare.split_once(['#', ':']) {
        Some((before, _)) => before,
        None => bare,
    };
    let rooted = PATH_ROOTS.iter().any(|prefix| bare.starts_with(prefix));
    let top_page = !bare.contains('/')
        && bare.ends_with(".md")
        && bare.chars().next().is_some_and(char::is_uppercase);
    (rooted || top_page).then_some(bare)
}

/// What `line` holds as literal text: all of a fenced line, and the spans between
/// backticks of any other. A flag and its value are read within one span, so
/// `--bin`, followed by a span of its own, names nothing.
fn literal_spans(line: &str, fenced: bool) -> Vec<&str> {
    if fenced {
        return vec![line];
    }
    line.split('`').skip(1).step_by(2).collect()
}

/// The words of `line` a reader takes as literal.
fn literal_words(line: &str, fenced: bool) -> Vec<&str> {
    literal_spans(line, fenced)
        .into_iter()
        .flat_map(str::split_whitespace)
        .collect()
}

/// Every local link target of `line`, with its fragment taken off.
fn link_targets(line: &str) -> Vec<&str> {
    line.split("](")
        .skip(1)
        .filter_map(|piece| piece.split_once(')'))
        .map(|(target, _)| target)
        .filter(|target| !target.contains("://") && !target.starts_with('#'))
        .filter(|target| !target.contains("::") && !target.starts_with("mailto:"))
        .map(|target| match target.split_once('#') {
            Some((before, _)) => before,
            None => target,
        })
        .collect()
}

/// What the word after `flag` must name, and the set that holds it.
fn named_by_flag<'known>(
    flag: &str,
    known: &'known Known,
) -> Option<(&'static str, &'known BTreeSet<String>)> {
    match flag {
        "--lane" => Some(("lane", &known.lanes)),
        "--bin" => Some(("binary", &known.binaries)),
        "-p" => Some(("package", &known.packages)),
        _ => None,
    }
}

/// Everything `line` names that the tree does not hold, added to `found`.
fn line_findings(
    line: &str,
    fenced: bool,
    directory: &str,
    known: &Known,
    exists: &dyn Fn(&str) -> bool,
    found: &mut Vec<String>,
) {
    for word in line.split_whitespace().filter(|word| is_machine_path(word)) {
        found.push(format!("a machine path, {word}"));
    }
    let words = literal_words(line, fenced);
    for word in &words {
        if !is_placeholder(word)
            && let Some(path) = path_word(word)
            && !exists(path)
        {
            found.push(format!("a dead path, {path}"));
        }
    }
    for span in literal_spans(line, fenced) {
        let spoken: Vec<&str> = span.split_whitespace().collect();
        for pair in spoken.windows(2) {
            let (Some(flag), Some(named)) = (pair.first(), pair.get(1)) else {
                continue;
            };
            if let Some((kind, set)) = named_by_flag(flag, known)
                && !is_placeholder(named)
                && !set.contains(*named)
            {
                found.push(format!("an unknown {kind}, {named}"));
            }
        }
    }
    for target in link_targets(line) {
        let joined = if directory.is_empty() {
            target.to_owned()
        } else {
            format!("{directory}/{target}")
        };
        if !exists(&joined) {
            found.push(format!("a dead link, {target}"));
        }
    }
}

/// Everything `text` names that the tree does not hold. `exists` answers for a
/// path relative to the repository root, and `directory` is where the page sits.
pub(crate) fn findings(
    text: &str,
    directory: &str,
    known: &Known,
    exists: &dyn Fn(&str) -> bool,
) -> Vec<String> {
    let mut found = Vec::new();
    let mut fenced = false;
    for line in text.lines() {
        if line.starts_with("```") {
            fenced = !fenced;
        } else {
            line_findings(line, fenced, directory, known, exists, &mut found);
        }
    }
    found
}

/// The skill folders.
fn skill_names() -> Result<Vec<String>, Box<dyn Error>> {
    let mut names = Vec::new();
    for entry in fs::read_dir(root().join("skills"))? {
        names.push(entry?.file_name().to_string_lossy().into_owned());
    }
    names.sort();
    Ok(names)
}

/// Every Markdown file under `directory`, depth bounded, skipping build output.
fn markdown_under(directory: &Path, depth: u8, out: &mut Vec<PathBuf>) -> TestResult {
    assert!(depth <= 8, "{} is nested too deep", directory.display());
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name == "target") {
                continue;
            }
            markdown_under(&path, depth.saturating_add(1), out)?;
        } else if path.extension().is_some_and(|extension| extension == "md") {
            out.push(path);
        }
    }
    Ok(())
}

/// The first fenced Rust block of `text`.
fn first_rust_block(text: &str) -> Option<String> {
    let after = text.split_once("```rust\n")?.1;
    Some(after.split_once("\n```")?.0.to_owned())
}

/// A dead path, lane, binary or package on a start page sends the next agent
/// the wrong way with full confidence. Catches a renamed file or lane.
#[test]
fn the_start_page_names_only_what_the_tree_holds() -> TestResult {
    let known = known()?;
    let base = root();
    let exists = |relative: &str| base.join(relative).exists();
    let mut pages = vec![("SKILLS.md".to_owned(), String::new())];
    for name in skill_names()? {
        pages.push((format!("skills/{name}/SKILL.md"), format!("skills/{name}")));
    }
    for (name, directory) in pages {
        let found = findings(&page(&name)?, &directory, &known, &exists);
        assert!(found.is_empty(), "{name} names {found:?}");
    }
    Ok(())
}

/// A skill an agent cannot select by its description is never loaded, and an
/// oversized one costs every session. Catches a folder missing from the route.
#[test]
fn a_skill_declares_its_name_and_when_to_use_it() -> TestResult {
    let route = page("SKILLS.md")?;
    let names = skill_names()?;
    assert!(names.len() >= 6, "the route holds six skills: {names:?}");
    for name in names {
        let path = format!("skills/{name}/SKILL.md");
        let text = page(&path)?;
        assert!(
            text.starts_with(&format!("---\nname: {name}\ndescription: Use ")),
            "{path} opens with its name and a description that says when"
        );
        assert!(text.lines().count() <= 120, "{path} is past 120 lines");
        assert!(route.contains(&path), "SKILLS.md does not route to {path}");
    }
    Ok(())
}

/// The workspace crates a manifest needs before it can publish: normal and
/// build edges, never dev, which a registry does not resolve.
fn publish_edges(text: &str) -> BTreeSet<String> {
    let names: Vec<String> = CRATES.iter().map(|name| name.replace('-', "_")).collect();
    let mut edges = BTreeSet::new();
    let mut counts = false;
    for line in text.lines() {
        if line.starts_with('[') {
            counts = line.contains("dependencies") && !line.contains("dev-dependencies");
            continue;
        }
        let Some((name, _)) = line.split_once(" = ") else {
            continue;
        };
        if counts && names.iter().any(|known| known == name.trim()) {
            edges.insert(name.trim().to_owned());
        }
    }
    edges
}

/// `docs/releasing.md` once listed `lgwks_deps` before `lgwks_ast` after #787 had
/// reversed that edge, which stops a release at its second upload.
#[test]
fn the_publish_order_is_the_dependency_order() -> TestResult {
    let text = page("docs/releasing.md")?;
    let order: Vec<&str> = text
        .lines()
        .filter_map(|line| line.strip_prefix("cargo publish -p "))
        .filter_map(|rest| rest.split_whitespace().next())
        .collect();
    for directory in CRATES {
        let (name, _) = identity(directory)?;
        let Some(place) = order.iter().position(|listed| *listed == name) else {
            return Err(format!("docs/releasing.md never publishes {name}").into());
        };
        for edge in publish_edges(&manifest(directory)?) {
            let earlier = order.get(..place).ok_or("a position past the order")?;
            assert!(
                earlier.contains(&edge.as_str()),
                "{name} is published before {edge}, which it needs"
            );
        }
    }
    Ok(())
}

/// A feature its guide omits is a capability no reader can find: `ecs`, `trace`,
/// `wire`, `ron`, `legacy`, `testing` and `sealed` were all missing.
#[test]
fn every_bot_feature_is_in_its_guide() -> TestResult {
    let manifest_text = manifest("lgwks-bot")?;
    let guide = page("docs/guides/lgwks-bot/features.md")?;
    let section = manifest_text
        .split_once("[features]\n")
        .ok_or("the bot manifest has no features")?
        .1;
    for line in section.lines().take_while(|line| !line.starts_with('[')) {
        if let Some((feature, _)) = line.split_once(" = [") {
            assert!(
                feature == "default" || guide.contains(&format!("`{feature}`")),
                "docs/guides/lgwks-bot/features.md omits `{feature}`"
            );
        }
    }
    Ok(())
}

/// The front page's version table is the first version a reader believes.
#[test]
fn the_readme_versions_are_the_manifests() -> TestResult {
    let readme = page("README.md")?;
    for directory in CRATES {
        let (name, version) = identity(directory)?;
        assert!(
            readme.contains(&format!("| `{name}` | {version} |")),
            "README.md does not list {name} at {version}"
        );
    }
    Ok(())
}

/// A start page no entry links is a page nobody opens.
#[test]
fn the_start_page_is_linked_from_every_agent_entry() -> TestResult {
    for entry in ["AGENTS.md", "README.md", "docs/README.md", "llms.txt"] {
        assert!(
            page(entry)?.contains("SKILLS.md"),
            "{entry} omits SKILLS.md"
        );
    }
    Ok(())
}

/// A manifest line is copied whole, so a stale major resolves an old release:
/// the `lgwks_std` README's feature map still said `version = "3"` on 4.1.0, and
/// the storefront guide said `lgwks_deps` `version = "6"` on 7.0.0 (#934).
#[test]
fn a_manifest_line_in_a_page_names_the_current_major() -> TestResult {
    let base = root();
    let mut pages = Vec::new();
    markdown_under(&base.join("docs/guides"), 0, &mut pages)?;
    markdown_under(&base.join("skills"), 0, &mut pages)?;
    for directory in CRATES {
        for entry in fs::read_dir(base.join("crates").join(directory))? {
            let path = entry?.path();
            if path.extension().is_some_and(|extension| extension == "md") {
                pages.push(path);
            }
        }
    }
    let mut stale = Vec::new();
    for directory in CRATES {
        let (name, version) = identity(directory)?;
        let major = version.split('.').next().ok_or("a version has no major")?;
        for path in &pages {
            for line in fs::read_to_string(path)?.lines() {
                let body = line.trim_start_matches(['#', ' ']);
                let Some(rest) = body.strip_prefix(&format!("{name} = ")) else {
                    continue;
                };
                let quoted = rest.split_once('"').map(|(_, after)| after);
                let named = quoted.and_then(|after| after.split(['"', '.']).next());
                if named != Some(major) {
                    stale.push(format!("{}: {line}", path.display()));
                }
            }
        }
    }
    assert!(stale.is_empty(), "{}", stale.join("\n"));
    Ok(())
}

/// The binary's own usage text, which decides what a page may call a command.
fn printed_usage() -> Result<String, Box<dyn Error>> {
    let usage = Command::new(env!("CARGO_BIN_EXE_lgwks-deps")).output()?;
    Ok(format!(
        "{}{}",
        String::from_utf8_lossy(&usage.stdout),
        String::from_utf8_lossy(&usage.stderr)
    ))
}

/// Every word of `text` run as a command of `lgwks-deps` that `printed` does not
/// show. `scan` moved to `lgwks-ast` and answers with where it went (ADR 0002).
pub(crate) fn command_findings(text: &str, printed: &str) -> Vec<String> {
    let mut named = Vec::new();
    for rest in text.split("-p lgwks_deps -- ").skip(1) {
        named.extend(rest.split_whitespace().next());
    }
    let mut fenced = false;
    for line in text.lines() {
        if line.starts_with("```") {
            fenced = !fenced;
            continue;
        }
        for span in literal_spans(line, fenced) {
            let words: Vec<&str> = span.split_whitespace().collect();
            for pair in words.windows(2) {
                if let [program, word] = *pair
                    && program == "lgwks-deps"
                {
                    named.push(word);
                }
            }
        }
    }
    named
        .into_iter()
        .map(|word| word.trim_matches(['`', '.', ',', ')', ';', ':']))
        .filter(|word| !word.starts_with(['-', '<', '[', '$', '|', '&', '#']) && !word.is_empty())
        .filter(|word| *word != "scan" && !printed.contains(&format!("lgwks-deps {word}")))
        .map(|word| format!("an unknown command, lgwks-deps {word}"))
        .collect()
}

/// `lgwks-deps catalog` stood in a skill, and an agent who ran the line got "unknown
/// command". Judged against the live binary's usage, what it prints passes and a
/// command it dropped is named, however a page spells the call.
#[test]
fn a_gate_subcommand_in_a_page_is_one_the_binary_prints() -> TestResult {
    let printed = printed_usage()?;
    let live = "`lgwks-deps check .` then `lgwks-deps catalog --json`\n\
                `cargo run -p lgwks_deps -- tiers`\n";
    assert_eq!(command_findings(live, &printed), Vec::<String>::new());
    let dropped = command_findings("`lgwks-deps ledger`\n", &printed);
    assert_eq!(dropped, ["an unknown command, lgwks-deps ledger"]);
    Ok(())
}

/// The pages that describe the tree as it stands. An ADR, a changelog page, a
/// ledger or a recorded run states what was true on its date, so it is not read.
fn living_pages() -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let base = root();
    let mut pages = Vec::new();
    for entry in fs::read_dir(&base)? {
        let path = entry?.path();
        if path.extension().is_some_and(|extension| extension == "md") {
            pages.push(path);
        }
    }
    for tree in ["docs", "skills", "bench", "proofs"] {
        markdown_under(&base.join(tree), 0, &mut pages)?;
    }
    for directory in CRATES {
        markdown_under(&base.join("crates").join(directory), 0, &mut pages)?;
    }
    let dated = [
        "CHANGELOG.md",
        "docs/adr/",
        "docs/changelog/",
        "docs/governance/ledger",
        "bench/ai-authoring/runs/",
    ];
    pages.retain(|path| {
        let Ok(relative) = path.strip_prefix(&base) else {
            return false;
        };
        let relative = relative.to_string_lossy();
        !relative.contains("/tests/") && !dated.iter().any(|prefix| relative.starts_with(prefix))
    });
    pages.sort();
    Ok(pages)
}

/// Whole pages whose subject is another repository, so a path on them is that
/// repository's. A row a page no longer needs fails the test that reads this list.
const FOREIGN_PAGES: [(&str, &str); 2] = [
    (
        "docs/bevy-admission-hazard.md",
        "crates/bevy_time/src/virt.rs",
    ),
    (
        "crates/drishti/docs/research.md",
        "kernel/crates/canvas-syscall/src/authorize.rs",
    ),
];

/// One name a page may hold that the tree does not: the page, the name, and why.
const NAMED_ELSEWHERE: [(&str, &str, &str); 3] = [
    (
        "docs/governance/gates.md",
        "docs/archive/",
        "the first retired document creates it",
    ),
    (
        "bench/async/results.md",
        "bench/async/target/release/lgwks-bench-async",
        "build output, which git does not hold",
    ),
    (
        "docs/prd/README.md",
        "NNNN-short-name.md",
        "the shape a new record's name takes",
    ),
];

/// Whether `found` is a name [`NAMED_ELSEWHERE`] admits on `page`.
fn admitted(page: &str, found: &str) -> bool {
    NAMED_ELSEWHERE.iter().any(|&(listed, name, _)| {
        listed == page
            && found
                .trim_end_matches('/')
                .ends_with(name.trim_end_matches('/'))
    })
}

/// An audit by pattern on 2026-11-09 found a skill's `lgwks-deps catalog`, two
/// pages' `lgwks-deps ledger` and an INV naming a moved test, none of them on a
/// page the start-page test read. So every living page is read, and a path resolves
/// from the page, from its crate or from the root.
#[test]
fn every_living_page_names_only_what_the_tree_holds() -> TestResult {
    let known = known()?;
    let printed = printed_usage()?;
    let base = root();
    let pages = living_pages()?;
    assert!(pages.len() > 200, "the walk found only {}", pages.len());
    let mut wrong = Vec::new();
    for path in &pages {
        let inside = path.strip_prefix(&base)?;
        let relative = inside.to_string_lossy().into_owned();
        let directory = match inside.parent() {
            Some(parent) => parent.to_string_lossy().into_owned(),
            None => String::new(),
        };
        let crate_root: PathBuf = inside.components().take(2).collect();
        let exists = |named: &str| {
            base.join(named).exists()
                || base.join(&directory).join(named).exists()
                || (relative.starts_with("crates/") && base.join(&crate_root).join(named).exists())
        };
        let text = fs::read_to_string(path)?;
        let mut found = command_findings(&text, &printed);
        if !FOREIGN_PAGES.iter().any(|&(page, _)| page == relative) {
            found.extend(
                findings(&text, &directory, &known, &exists)
                    .into_iter()
                    .filter(|finding| !finding.starts_with("a machine path")),
            );
        }
        for finding in found {
            if !admitted(&relative, &finding) {
                wrong.push(format!("{relative}: {finding}"));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    Ok(())
}

/// An allow-list is where an error hides, so a row the page no longer holds is
/// itself a failure, and a foreign page must still be about what it was admitted for.
#[test]
fn every_allowed_name_is_still_on_its_page() -> TestResult {
    for (name, held) in FOREIGN_PAGES {
        assert!(page(name)?.contains(held), "{name} no longer names {held}");
    }
    for (name, held, why) in NAMED_ELSEWHERE {
        assert!(!why.is_empty(), "{name} states no reason for {held}");
        assert!(page(name)?.contains(held), "{name} no longer names {held}");
    }
    Ok(())
}

/// Text handed to a model verbatim: its bytes are the input a recorded run graded.
const RECORDED_INPUTS: &str = "bench/ai-authoring/api/";

/// A page past the law is refused at every commit that stages it, so it cannot be
/// fixed. `docs/frontier.md` held 685 lines and `REQUIREMENTS.md` 397 (ADR 0023).
#[test]
fn every_living_page_keeps_to_the_size_law() -> TestResult {
    let base = root();
    let mut over = Vec::new();
    for path in living_pages()? {
        let relative = path.strip_prefix(&base)?.to_string_lossy().into_owned();
        let lines = fs::read_to_string(&path)?.lines().count();
        if lines > 120 && !relative.starts_with(RECORDED_INPUTS) {
            over.push(format!("{relative}: {lines} lines"));
        }
    }
    assert!(over.is_empty(), "{}", over.join("\n"));
    Ok(())
}

/// PRD 0001, the catalog's own register and the estate's contract all say
/// `lgwks-deps catalog`, and it answered "unknown command". The command and the
/// lane's binary are one implementation, so they answer in the same bytes.
#[test]
fn the_catalog_subcommand_is_the_catalog_binary() -> TestResult {
    let base = root();
    let run = |arguments: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_lgwks-deps"))
            .arg("catalog")
            .args(arguments)
            .output()
    };
    let command = run(&["--json", &base.to_string_lossy()])?;
    let binary = Command::new(env!("CARGO_BIN_EXE_catalog"))
        .arg("--json")
        .arg(&base)
        .output()?;
    assert!(command.status.success(), "the command answers the tree");
    assert!(!command.stdout.is_empty(), "the command printed nothing");
    assert_eq!(command.stdout, binary.stdout, "two answers for one catalog");
    let check = run(&["--check", &base.to_string_lossy()])?;
    assert!(check.status.success(), "the catalog holds on the live tree");
    let typo = run(&["--jsno"])?;
    assert_eq!(typo.status.code(), Some(2), "a typo is a refusal");
    let said = String::from_utf8_lossy(&typo.stderr);
    assert!(
        said.contains("unknown option for `catalog`: --jsno"),
        "{said}"
    );
    assert!(
        printed_usage()?.contains("lgwks-deps catalog"),
        "usage omits it"
    );
    Ok(())
}

/// The bot README's quickstart once named types that exist nowhere. It is now the
/// script page's block, the one `published-consumer` builds from crates.io.
#[test]
fn the_bot_readme_quickstart_is_the_compiled_example() -> TestResult {
    let front = first_rust_block(&page("crates/lgwks-bot/README.md")?);
    let script = first_rust_block(&page("crates/lgwks-bot/README-script.md")?);
    assert!(front.is_some(), "the bot README shows a quickstart");
    assert_eq!(
        front, script,
        "the quickstart differs from the compiled block"
    );
    Ok(())
}
