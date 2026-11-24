//! The `skill-hygiene` lane: every agent skill is one a harness can load, an
//! agent can choose, and a reader can trust (ADR 0084, INV-DOC-10).
//!
//! Claude Code, OpenCode and Command Code read a skill's YAML frontmatter at
//! session start and choose it from `name` and `description` alone; the body is
//! read only once chosen (the Agent Skills specification, agentskills.io). So a
//! name that is not its folder, a description past 1,024 characters, a plain
//! value YAML reads as a map, or a page no route names is a skill never loaded.
//! Each `rust` block is a doctest of `lgwks_bot` (`crates/lgwks-bot/src/lib.rs`),
//! so a block that names a gone item fails a build, and an untagged fence, which
//! rustdoc compiles as Rust, is refused here first. What a page names in the tree
//! is INV-DOC-7's (`tests/it/agent_start_page.rs`); this lane holds the shape.

use std::fs;
use std::path::{Path, PathBuf};

use super::builtins::{pass, refuse, relative};
use super::{Context, GateError, Verdict, fail, read};

/// The longest name the specification allows.
const NAME_MAX: usize = 64;

/// The longest description the specification allows, in characters.
const DESCRIPTION_MAX: usize = 1024;

/// The size law for a Markdown page.
const LINES_MAX: usize = 120;

/// The frontmatter keys the specification defines. Any other is a typo.
const KEYS: [&str; 6] = [
    "name",
    "description",
    "license",
    "compatibility",
    "metadata",
    "allowed-tools",
];

/// Words a name may not hold: the harness reserves them.
const RESERVED: [&str; 2] = ["anthropic", "claude"];

/// The file whose `include_str!` lines make a skill's blocks doctests.
const DOCTEST_HOME: &str = "crates/lgwks-bot/src/lib.rs";

/// Where each harness looks for user skills, relative to the home directory.
const HARNESS_DIRS: [&str; 5] = [
    ".claude/skills",
    ".config/opencode/skills",
    ".commandcode/skills",
    ".gemini/config/skills",
    ".agents/skills",
];

/// How many links a harness entry may pass through before it is refused.
const HOPS_MAX: usize = 8;

/// What the tree around one skill holds.
struct Around<'tree> {
    /// `SKILLS.md`, the route table.
    route: &'tree str,
    /// [`DOCTEST_HOME`] as text.
    doctests: &'tree str,
}

/// What is wrong with `name` as the specification and the harness read it.
fn name_fault(name: &str) -> Option<String> {
    let lawful = name
        .chars()
        .all(|glyph| glyph.is_ascii_lowercase() || glyph.is_ascii_digit() || glyph == '-');
    if name.is_empty() || name.len() > NAME_MAX || !lawful {
        return Some(format!(
            "name {name:?} is not 1 to {NAME_MAX} of a-z, 0-9 and -"
        ));
    }
    if name.starts_with('-') || name.ends_with('-') || name.contains("--") {
        return Some(format!("name {name:?} has a stray hyphen"));
    }
    RESERVED
        .iter()
        .find(|word| name.contains(*word))
        .map(|word| format!("name {name:?} holds the reserved word {word:?}"))
}

/// Why a plain YAML value would not read as the one string it shows.
fn yaml_fault(value: &str) -> Option<&'static str> {
    if value.contains(": ") || value.ends_with(':') {
        Some("a plain value holds `: `, which YAML reads as a map; quote it or reword it")
    } else if value.contains(" #") {
        Some("a plain value holds ` #`, which YAML reads as a comment")
    } else if value.starts_with(['>', '|', '"', '\'', '[', '{', '&', '*', '!', '%', '@', '`']) {
        Some("the value opens with a YAML indicator; write it as one plain line")
    } else {
        None
    }
}

/// What the `description` value gets wrong.
fn description_findings(description: &str, found: &mut Vec<String>) {
    if !description.starts_with("Use ") {
        found.push("the description does not open with `Use `, saying when to load it".to_owned());
    }
    let length = description.chars().count();
    if length > DESCRIPTION_MAX {
        found.push(format!(
            "the description is {length} characters, past {DESCRIPTION_MAX}"
        ));
    }
    if description.contains(['<', '>']) {
        found.push("the description holds `<` or `>`, which a harness reads as a tag".to_owned());
    }
    if let Some(why) = yaml_fault(description) {
        found.push(format!("description: {why}"));
    }
}

/// What the frontmatter `header` of skill `folder` gets wrong.
fn header_findings(folder: &str, header: &str, found: &mut Vec<String>) {
    let mut names = Vec::new();
    let mut descriptions = Vec::new();
    for line in header.lines().filter(|line| !line.starts_with([' ', '\t'])) {
        let Some((key, value)) = line.split_once(':') else {
            found.push(format!("frontmatter line {line:?} is not `key: value`"));
            continue;
        };
        let value = value.trim();
        match key {
            "name" => names.push(value),
            "description" => descriptions.push(value),
            other if KEYS.contains(&other) => {}
            other => found.push(format!(
                "frontmatter key {other:?} is not in the specification"
            )),
        }
    }
    match *names.as_slice() {
        [name] if name == folder => found.extend(name_fault(name)),
        [name] => found.push(format!("name {name:?} is not its folder {folder:?}")),
        _ => found.push(format!("{} `name` lines, not one", names.len())),
    }
    match *descriptions.as_slice() {
        [description] => description_findings(description, found),
        _ => found.push(format!(
            "{} `description` lines, not one",
            descriptions.len()
        )),
    }
}

/// What the fences of `body` get wrong, and whether it holds a `rust` block.
fn fence_findings(body: &str, found: &mut Vec<String>) -> bool {
    let mut open = false;
    let mut rust = false;
    for (index, line) in body.lines().enumerate() {
        let Some(info) = line.trim_start().strip_prefix("```") else {
            continue;
        };
        if open {
            open = false;
            continue;
        }
        open = true;
        let info = info.trim();
        let language = info.split([',', ' ']).next();
        if language.is_none_or(str::is_empty) {
            found.push(format!(
                "body line {}: a fence with no language, which rustdoc compiles as Rust",
                index.saturating_add(1)
            ));
        }
        if language == Some("rust") {
            rust = true;
            if info.contains("ignore") {
                found.push("a rust block is `ignore`d, so nothing compiles it".to_owned());
            }
        }
    }
    if open {
        found.push("a fence is never closed".to_owned());
    }
    rust
}

/// Everything the page `text` of the skill in folder `folder` gets wrong.
fn page_findings(folder: &str, text: &str, around: &Around<'_>) -> Vec<String> {
    let mut found = Vec::new();
    let lines = text.lines().count();
    if lines > LINES_MAX {
        found.push(format!("{lines} lines, past {LINES_MAX}"));
    }
    let parts = text
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n"));
    let Some((header, body)) = parts else {
        found.push(
            "no frontmatter: the page opens with `---`, then `name:` and `description:`".to_owned(),
        );
        return found;
    };
    header_findings(folder, header, &mut found);
    if !body.lines().any(|line| line.starts_with("# ")) {
        found.push("the body has no `# ` title".to_owned());
    }
    let page = format!("skills/{folder}/SKILL.md");
    let compiled = around.doctests.contains(&format!("../../../{page}"));
    if fence_findings(body, &mut found) && !compiled {
        found.push(format!(
            "a rust block {DOCTEST_HOME} does not include, so it is never compiled"
        ));
    }
    if !around.route.contains(&page) {
        found.push("SKILLS.md does not route to it".to_owned());
    }
    found
}

/// The entries of `directory`, sorted, or none when it does not exist.
fn entries(directory: &Path) -> Result<Vec<PathBuf>, GateError> {
    match fs::read_dir(directory) {
        Ok(reader) => {
            let mut paths: Vec<PathBuf> = reader.flatten().map(|entry| entry.path()).collect();
            paths.sort();
            Ok(paths)
        }
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(cause) => fail(
            GateError::Io {
                path: directory.to_path_buf(),
                cause,
            },
            "skill_hygiene::entries",
        ),
    }
}

/// Where the link chain from `entry` ends, whether it passed through one of
/// `ours`, and how many links it followed.
fn chain_end(entry: &Path, ours: &[PathBuf]) -> (PathBuf, bool, usize) {
    let within = |path: &Path| ours.iter().any(|root| path.starts_with(root));
    let mut current = entry.to_path_buf();
    let mut passed = false;
    for hop in 0..HOPS_MAX {
        let Ok(target) = fs::read_link(&current) else {
            return (current, passed, hop);
        };
        current = match current.parent() {
            Some(parent) => parent.join(target),
            None => target,
        };
        passed = passed || within(&current);
    }
    (current, passed, HOPS_MAX)
}

/// Every harness entry under `home` that links into one of `ours` and does not
/// land on a skill of the same name.
fn link_findings(home: &Path, ours: &[PathBuf]) -> Result<Vec<String>, GateError> {
    let mut found = Vec::new();
    for harness in HARNESS_DIRS {
        for entry in entries(&home.join(harness))? {
            let (end, passed, hops) = chain_end(&entry, ours);
            let Some(name) = entry.file_name() else {
                continue;
            };
            if hops == 0 || !passed {
                continue;
            }
            let shown = format!("HOME {harness}/{}", name.to_string_lossy());
            if hops >= HOPS_MAX {
                found.push(format!("{shown}: more than {HOPS_MAX} links deep"));
            } else if !end.join("SKILL.md").is_file() {
                found.push(format!(
                    "{shown}: links to {}, which holds no SKILL.md",
                    end.display()
                ));
            } else if end.file_name() != Some(name) {
                found.push(format!(
                    "{shown}: links to the skill {}, another name",
                    end.display()
                ));
            }
        }
    }
    Ok(found)
}

/// Every skill of the tree at `root` that fails its shape, one line each.
fn findings(root: &Path) -> Result<Vec<String>, GateError> {
    let route = read(&root.join("SKILLS.md"))?;
    let doctests = read(&root.join(DOCTEST_HOME))?;
    let around = Around {
        route: &route,
        doctests: &doctests,
    };
    let mut found = Vec::new();
    for folder in entries(&root.join("skills"))? {
        let shown = relative(root, &folder);
        let Some(name) = folder.file_name() else {
            continue;
        };
        let page = folder.join("SKILL.md");
        if !page.is_file() {
            found.push(format!("{shown}: no SKILL.md"));
            continue;
        }
        for finding in page_findings(&name.to_string_lossy(), &read(&page)?, &around) {
            found.push(format!("{shown}/SKILL.md: {finding}"));
        }
    }
    Ok(found)
}

/// The lane: the tree's skills, then the harness links into them.
pub(super) fn skill_hygiene(context: &Context<'_>) -> Result<Verdict, GateError> {
    let mut found = findings(context.root)?;
    let skills = context.root.join("skills");
    let mut ours = vec![skills.clone()];
    match fs::canonicalize(&skills) {
        Ok(real) => ours.push(real),
        Err(error) => lgwks_std::trace::debug!(%error, "the skills directory has no real path"),
    }
    match std::env::var_os("HOME") {
        Some(home) => found.extend(link_findings(Path::new(&home), &ours)?),
        None => lgwks_std::trace::debug!("no HOME, so no harness link is read"),
    }
    if found.is_empty() {
        Ok(pass(
            "every skill loads, routes and compiles; every harness link resolves",
        ))
    } else {
        Ok(refuse(format!(
            "{} skill finding(s): {}",
            found.len(),
            found.join("; ")
        )))
    }
}

/// Each rule on its own input, so a broken rule fails in milliseconds and in
/// this module's scope. Source: the Agent Skills specification (`name` 1 to 64
/// of a-z, 0-9 and `-`, `description` at most 1,024 characters) and YAML 1.2
/// section 7.3.3 (a plain scalar holds no `: ` and no ` #`).
#[cfg(test)]
mod tests {
    use super::{
        Around, DESCRIPTION_MAX, LINES_MAX, NAME_MAX, chain_end, description_findings,
        fence_findings, header_findings, name_fault, page_findings, yaml_fault,
    };
    use std::path::Path;

    /// Whether one of `found` holds `words`.
    fn has(found: &[String], words: &str) -> bool {
        found.iter().any(|finding| finding.contains(words))
    }

    /// Whether `why` is a fault that holds `words`.
    fn says(why: Option<impl AsRef<str>>, words: &str) -> bool {
        why.is_some_and(|text| text.as_ref().contains(words))
    }

    /// The findings of one header for folder `folder`.
    fn header(folder: &str, text: &str) -> Vec<String> {
        let mut found = Vec::new();
        header_findings(folder, text, &mut found);
        found
    }

    /// The findings of one description.
    fn described(text: &str) -> Vec<String> {
        let mut found = Vec::new();
        description_findings(text, &mut found);
        found
    }

    /// The findings of one body, and whether it holds a rust block.
    fn fenced(body: &str) -> (Vec<String>, bool) {
        let mut found = Vec::new();
        let rust = fence_findings(body, &mut found);
        (found, rust)
    }

    #[test]
    fn skill_hygiene_names_hold_the_specification_at_its_bounds() {
        assert_eq!(name_fault("lgwks-a1"), None);
        assert_eq!(name_fault(&"a".repeat(NAME_MAX)), None, "64 is lawful");
        let long = "a".repeat(NAME_MAX + 1);
        for bad in ["", "Lgwks", "lgwks_a", "lgwks a", long.as_str()] {
            assert!(
                says(name_fault(bad), "is not 1 to 64"),
                "{bad:?} is refused"
            );
        }
        for stray in ["-lgwks", "lgwks-", "lgwks--a"] {
            assert!(
                says(name_fault(stray), "stray hyphen"),
                "{stray:?} is refused"
            );
        }
        for reserved in ["lgwks-claude", "anthropic-x"] {
            assert!(
                says(name_fault(reserved), "reserved"),
                "{reserved:?} is refused"
            );
        }
    }

    #[test]
    fn skill_hygiene_yaml_reads_a_plain_value_as_one_string_only() {
        for plain in ["Use when a flow runs, then stop.", "Use when", "Use#when"] {
            assert_eq!(yaml_fault(plain), None, "{plain:?} is one string");
        }
        for map in ["Use when: always", "Use when:"] {
            assert!(says(yaml_fault(map), "map"), "{map:?} reads as a map");
        }
        assert!(says(yaml_fault("Use when #1"), "comment"));
        for opener in [
            ">x", "|x", "\"x", "'x", "[x", "{x", "&x", "*x", "!x", "%x", "@x", "`x",
        ] {
            assert!(
                says(yaml_fault(opener), "indicator"),
                "{opener:?} opens with one"
            );
        }
    }

    #[test]
    fn skill_hygiene_descriptions_say_when_and_fit() {
        assert_eq!(described("Use when a flow runs."), Vec::<String>::new());
        assert!(has(
            &described("Load when a flow runs."),
            "does not open with `Use `"
        ));
        let longest = format!("Use {}", "x".repeat(DESCRIPTION_MAX - 4));
        assert_eq!(described(&longest), Vec::<String>::new(), "1,024 fits");
        assert!(has(&described(&format!("{longest}x")), "past 1024"));
        for tagged in ["Use <b> here", "Use a > b"] {
            assert!(
                has(&described(tagged), "`<` or `>`"),
                "{tagged:?} holds a tag mark"
            );
        }
        assert!(has(&described("Use when: x"), "description: a plain value"));
    }

    #[test]
    fn skill_hygiene_headers_hold_one_name_one_description_and_known_keys() {
        let good = "name: lgwks-a\ndescription: Use when a.\nlicense: MIT";
        assert_eq!(header("lgwks-a", good), Vec::<String>::new());
        let nested = "name: lgwks-a\ndescription: Use when a.\nmetadata:\n  owner: x";
        assert_eq!(
            header("lgwks-a", nested),
            Vec::<String>::new(),
            "indented lines belong to their key"
        );
        let cases = [
            (
                "name: lgwks-b\ndescription: Use when a.",
                "is not its folder",
            ),
            ("description: Use when a.", "0 `name` lines"),
            (
                "name: lgwks-a\nname: lgwks-a\ndescription: Use when a.",
                "2 `name` lines",
            ),
            ("name: lgwks-a", "0 `description` lines"),
            (
                "name: lgwks-a\ndescription: Use when a.\ndescripton: x",
                "not in the specification",
            ),
            (
                "name: lgwks-a\ndescription: Use when a.\nloose words",
                "is not `key: value`",
            ),
            (
                "name: lgwks-a\ndescription: Load when a.",
                "does not open with `Use `",
            ),
        ];
        for (text, words) in cases {
            assert!(
                has(&header("lgwks-a", text), words),
                "{text:?} names {words:?}"
            );
        }
        assert!(has(
            &header("Lgwks-A", "name: Lgwks-A\ndescription: Use when a."),
            "is not 1 to 64"
        ));
    }

    #[test]
    fn skill_hygiene_fences_are_tagged_closed_and_compiled() {
        assert_eq!(fenced("```text\nx\n```\n"), (Vec::new(), false));
        assert_eq!(fenced("```rust\nx\n```\n"), (Vec::new(), true));
        assert_eq!(fenced("```rust,no_run\nx\n```\n"), (Vec::new(), true));
        assert_eq!(fenced("  ```sh\nx\n  ```\n"), (Vec::new(), false));
        let (found, _) = fenced("text\n```\nx\n```\n");
        assert!(has(&found, "body line 2: a fence with no language"));
        let (found, rust) = fenced("```rust,ignore\nx\n```\n");
        assert!(rust && has(&found, "`ignore`d"));
        let (found, _) = fenced("```text\nx\n```\n```sh\ny\n");
        assert_eq!(found, vec!["a fence is never closed".to_owned()]);
        let (found, rust) = fenced("```text\nfn x()\n```\n```rust\ny\n```\n");
        assert!(found.is_empty() && rust, "the second fence opens anew");
    }

    #[test]
    fn skill_hygiene_pages_need_frontmatter_a_title_a_route_and_a_compile() {
        let page = "---\nname: lgwks-a\ndescription: Use when a.\n---\n\n# A\n\n```rust\nx\n```\n";
        let routed = "[a](skills/lgwks-a/SKILL.md)";
        let compiled = "#[doc = include_str!(\"../../../skills/lgwks-a/SKILL.md\")]";
        let around = Around {
            route: routed,
            doctests: compiled,
        };
        let raw = Around {
            route: routed,
            doctests: "",
        };
        let lost = Around {
            route: "",
            doctests: compiled,
        };
        let only = |text: &str, near: &Around<'_>| page_findings("lgwks-a", text, near);
        assert_eq!(only(page, &around), Vec::<String>::new());
        assert_eq!(
            only(page, &lost),
            vec!["SKILLS.md does not route to it".to_owned()]
        );
        assert!(has(&only(page, &raw), "never compiled"));
        assert_eq!(
            only(&page.replace("```rust", "```text"), &raw),
            Vec::<String>::new()
        );
        assert_eq!(
            only(&page.replace("# A", "A"), &around),
            vec!["the body has no `# ` title".to_owned()]
        );
        assert!(has(
            &only(&page.replacen("---\n", "", 1), &around),
            "no frontmatter"
        ));
        let full = format!("{page}{}", "x\n".repeat(LINES_MAX - page.lines().count()));
        assert_eq!(only(&full, &around), Vec::<String>::new(), "120 lines fit");
        assert_eq!(
            only(&format!("{full}x\n"), &around),
            vec![format!("{} lines, past {LINES_MAX}", LINES_MAX + 1)]
        );
    }

    #[test]
    fn skill_hygiene_a_plain_entry_is_its_own_end() {
        let entry = Path::new("no-such-skill-hygiene-entry");
        assert_eq!(chain_end(entry, &[]), (entry.to_path_buf(), false, 0));
    }

    #[cfg(unix)]
    #[test]
    fn skill_hygiene_links_land_on_a_skill_of_their_own_name()
    -> Result<(), Box<dyn std::error::Error>> {
        use super::{HOPS_MAX, link_findings};
        use std::os::unix::fs::symlink;
        let fixture = crate::fixture::Fixture::claim(&std::env::temp_dir(), "skill-hygiene")?;
        let root = fixture.path();
        let skills = root.join("tree/skills");
        let own = skills.join("lgwks-a");
        let home = root.join("home");
        let (agents, claude, gemini) = (
            home.join(".agents/skills"),
            home.join(".claude/skills"),
            home.join(".gemini/config/skills"),
        );
        for dir in [
            &own,
            &agents,
            &claude,
            &gemini,
            &home.join(".commandcode/skills/somebody"),
        ] {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(own.join("SKILL.md"), "x")?;
        symlink(&own, agents.join("lgwks-a"))?;
        symlink(agents.join("lgwks-a"), claude.join("lgwks-a"))?;
        let ours = [skills.clone()];
        assert_eq!(
            chain_end(&claude.join("lgwks-a"), &ours),
            (own.clone(), true, 2)
        );
        assert_eq!(link_findings(&home, &ours)?, Vec::<String>::new());
        symlink(&own, agents.join("lgwks-b"))?;
        symlink(skills.join("lgwks-gone"), claude.join("lgwks-gone"))?;
        let found = link_findings(&home, &ours)?;
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(has(
            &found,
            "HOME .agents/skills/lgwks-b: links to the skill"
        ));
        assert!(has(&found, "HOME .claude/skills/lgwks-gone: links to"));
        symlink(skills.join("loop-b"), skills.join("loop-a"))?;
        symlink(skills.join("loop-a"), skills.join("loop-b"))?;
        symlink(skills.join("loop-a"), gemini.join("loop-a"))?;
        let (_, passed, hops) = chain_end(&gemini.join("loop-a"), &ours);
        assert_eq!(
            (passed, hops),
            (true, HOPS_MAX),
            "a loop stops at the bound"
        );
        assert!(has(
            &link_findings(&home, &ours)?,
            "loop-a: more than 8 links deep"
        ));
        Ok(())
    }
}
