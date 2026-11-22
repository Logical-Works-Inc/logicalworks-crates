//! Whether a source file is test code because the module that declares it says so (#1101).
//!
//! The layer scan used to skip a file by its name (`tests.rs`, `sim_*`, `test_*`). A name is not what
//! makes a file test code; the item that declares it does. A `src/rt/test_gate.rs` holding a
//! `crate::script` import would ship if `rt.rs` declared it with a plain `mod test_gate;`, and the
//! scan would never read it. A file is test code here only when every place that declares it, by
//! its module path or by a `#[path = ".."]` attribute (a bin that includes `../world.rs` is such a
//! place), sits on a line the #1096 reader (`test_cfg::test_lines`) marks as test, or in a file that
//! is itself test code. No declaration found, a declaration that ships anywhere, and a declaring
//! file the grammar refuses all leave the file read: the scan reads more, never less.

use std::error::Error;

use super::edges;
use super::layer_edges::{pick, upward_in};
use super::test_cfg::test_lines;
use crate::sim::{Rng, Trace};

/// Every source file of the crate: its `/`-separated path below `src/` and its text.
pub(super) type Sources = [(String, String)];

/// The module path of source file `relative`, without `mod` for a `mod.rs`: `clock/tests/wait.rs` is
/// `clock`, `tests`, `wait`. Empty for a path that is not a Rust file.
fn module_path(relative: &str) -> Vec<&str> {
    let Some(bare) = relative.strip_suffix(".rs") else {
        return Vec::new();
    };
    let mut parts: Vec<&str> = bare.split('/').collect();
    if parts.last() == Some(&"mod") {
        parts.pop();
    }
    parts
}

/// The files that may declare a module whose parent module is `parent`.
fn parent_files(parent: &[&str]) -> Vec<String> {
    if parent.is_empty() {
        return vec![String::from("lib.rs"), String::from("main.rs")];
    }
    let joined = parent.join("/");
    vec![format!("{joined}.rs"), format!("{joined}/mod.rs")]
}

/// The deepest chain of declaring files followed, so a `#[path]` loop ends.
const MAX_DEPTH: usize = 16;

/// How many lines after a `#[path]` attribute its `mod` may sit, past other attributes and comments.
const PATH_REACH: usize = 6;

/// One place a module is declared: the declaring file, its text and the 0-based line of the `mod`.
struct Declaration<'text> {
    /// The declaring file, below `src/`.
    file: &'text str,
    /// Its text.
    text: &'text str,
    /// The line holding `mod <name>;`.
    line: usize,
}

/// Whether `line` is a comment line, which declares nothing.
fn is_comment(line: &str) -> bool {
    let code = line.trim_start();
    code.starts_with("//") || code.starts_with("/*") || code.starts_with('*')
}

/// `path` read against the directory of `file`, `..` and `.` folded: how a `#[path]` is resolved.
fn resolve(file: &str, path: &str) -> String {
    let mut parts: Vec<&str> = match file.rsplit_once('/') {
        Some(split) => split.0.split('/').collect(),
        None => Vec::new(),
    };
    for piece in path.split('/') {
        match piece {
            ".." => {
                parts.pop();
            }
            "." | "" => {}
            other => parts.push(other),
        }
    }
    parts.join("/")
}

/// Every `#[path = ".."]` module of `text`, as the line of its `mod` and the file it names.
fn path_declarations(file: &str, text: &str) -> Vec<(usize, String)> {
    if !text.contains("#[path") {
        return Vec::new();
    }
    let lines: Vec<&str> = text.lines().collect();
    let mut found = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let Some(named) = line
            .split_once("#[path = \"")
            .and_then(|split| split.1.split_once('"'))
        else {
            continue;
        };
        let module = lines
            .iter()
            .enumerate()
            .skip(index)
            .take(PATH_REACH)
            .find(|candidate| {
                !is_comment(candidate.1)
                    && candidate.1.contains("mod ")
                    && candidate.1.trim_end().ends_with(';')
            });
        if let Some(declared) = module {
            found.push((declared.0, resolve(file, named.0)));
        }
    }
    found
}

/// Every place `relative` is declared in `tree`: by its module path in the parent module's file,
/// and by a `#[path]` attribute in any file.
fn declarations<'tree>(relative: &str, tree: &'tree Sources) -> Vec<Declaration<'tree>> {
    let mut found = Vec::new();
    let parts = module_path(relative);
    let above = parts.len().saturating_sub(1);
    let (Some(name), Some(parent)) = (parts.last(), parts.get(..above)) else {
        return found;
    };
    let wanted = parent_files(parent);
    let declaration = format!("mod {name};");
    for file in tree {
        let pathed = path_declarations(&file.0, &file.1);
        for entry in &pathed {
            if entry.1 == relative {
                found.push(Declaration {
                    file: &file.0,
                    text: &file.1,
                    line: entry.0,
                });
            }
        }
        if !wanted.contains(&file.0) {
            continue;
        }
        for (line, code) in file.1.lines().enumerate() {
            let moved = pathed.iter().any(|entry| entry.0 == line);
            if !moved && !is_comment(code) && code.contains(&declaration) {
                found.push(Declaration {
                    file: &file.0,
                    text: &file.1,
                    line,
                });
            }
        }
    }
    found
}

/// Whether the line `place` names is a test item of its file. A file the grammar refuses is read.
fn on_test_line(place: &Declaration<'_>) -> bool {
    match test_lines(place.text) {
        Ok(lines) => lines.get(place.line) == Some(&true),
        Err(refusal) => {
            lgwks_std::trace::debug!(
                file = place.file,
                error = ?refusal,
                "test_decl: the declaring file is refused, so its declaration is read as shipped"
            );
            false
        }
    }
}

/// Whether every declaration of `relative` sits on a test line or in a file that is test code.
fn under_test(relative: &str, tree: &Sources, depth: usize) -> bool {
    if depth > MAX_DEPTH {
        return false;
    }
    let declared = declarations(relative, tree);
    !declared.is_empty()
        && declared.iter().all(|place| {
            on_test_line(place) || under_test(place.file, tree, depth.saturating_add(1))
        })
}

/// Whether `relative` is test code: every place that declares it is a test item, or is itself in a
/// test file. The name of the file says nothing.
pub(super) fn is_test_file(relative: &str, tree: &Sources) -> bool {
    under_test(relative, tree, 0)
}

/// [`edges`] of one file of `tree`: none when its parent declares it as test code.
pub(super) fn edges_in(relative: &str, text: &str, tree: &Sources) -> Vec<String> {
    if is_test_file(relative, tree) {
        Vec::new()
    } else {
        edges(relative, text)
    }
}

/// A crate tree of the files named, for a planted case.
pub(super) fn planted(files: &[(&str, &str)]) -> Vec<(String, String)> {
    files
        .iter()
        .map(|file| (file.0.to_owned(), file.1.to_owned()))
        .collect()
}

/// The (file, module) pairs the layer scan reports over `tree`.
fn found(tree: &Sources) -> Vec<(String, String)> {
    upward_in(tree)
        .into_iter()
        .map(|edge| (edge.file, edge.module))
        .collect()
}

#[test]
/// Source: #1101. `src/rt/test_gate.rs` imports `crate::script` and ships, because `rt.rs` declares
/// it with a plain `mod test_gate;`; the name exempted it and the layer scan reported no edge.
/// Requirement: a planted shipped file of that name is refused, and the same file declared under
/// `#[cfg(test)]` is not.
fn a_test_named_file_is_read_unless_its_declaration_is_under_cfg_test() {
    let import = "use crate::script::Scope;\n";
    let shipped = planted(&[("rt.rs", "mod test_gate;\n"), ("rt/test_gate.rs", import)]);
    assert!(
        !is_test_file("rt/test_gate.rs", &shipped),
        "a plain declaration ships the file whatever its name"
    );
    assert_eq!(
        found(&shipped),
        vec![(String::from("rt/test_gate.rs"), String::from("script"))],
        "the shipped file's upward import is refused"
    );
    let gated = planted(&[
        ("rt.rs", "#[cfg(test)]\nmod test_gate;\n"),
        ("rt/test_gate.rs", import),
    ]);
    assert!(is_test_file("rt/test_gate.rs", &gated));
    assert!(
        found(&gated).is_empty(),
        "the same file under cfg(test) is a caller, not the layer"
    );
}

#[test]
/// Source: `clock/tests/wait.rs`, a helper below a test module that `clock.rs` gates. Requirement: a
/// file below a test-declared module is test code, and a `tests.rs` that ships is read.
fn a_module_below_a_test_module_is_test_code_and_a_shipped_tests_file_is_not() {
    let nested = planted(&[
        ("clock.rs", "#[cfg(test)]\nmod tests;\n"),
        ("clock/tests.rs", "mod wait;\n"),
        ("clock/tests/wait.rs", "use crate::rt::time;\n"),
    ]);
    assert!(is_test_file("clock/tests/wait.rs", &nested));
    assert!(is_test_file("clock/tests.rs", &nested));
    let plain = planted(&[("clock.rs", "mod tests;\n"), ("clock/tests.rs", "")]);
    assert!(
        !is_test_file("clock/tests.rs", &plain),
        "a shipped tests.rs is read"
    );
    assert!(
        !is_test_file("clock/tests.rs", &planted(&[("clock/tests.rs", "")])),
        "a file with no declaration is read"
    );
}

#[test]
/// Source: `src/sim_cap.rs` and `src/rt/sim_one_driver.rs` are declared by `#[path]` from `cap.rs`
/// and `rt/runtime.rs`, and the `world-bench` bin includes `../world.rs` that `lib.rs` gates with
/// `cfg(test)`. Requirement: a `#[path]` declaration is found by the file it names, whatever the
/// module is called; and a file one place gates and another ships, with the files below it, is read.
fn a_path_declaration_decides_and_a_bin_that_includes_a_module_ships_it() {
    let gated = "#[cfg(test)]\n#[path = \"sim_cap.rs\"]\nmod sim_cap;\n";
    let sim = planted(&[("cap.rs", gated), ("sim_cap.rs", "")]);
    assert!(is_test_file("sim_cap.rs", &sim));
    let driver = "#[cfg(all(test, feature = \"script\"))]\n#[path = \"sim_one_driver.rs\"]\nmod one_driver_tests;\n";
    let renamed = planted(&[("rt/runtime.rs", driver), ("rt/sim_one_driver.rs", "")]);
    assert!(
        is_test_file("rt/sim_one_driver.rs", &renamed),
        "a module is found by the file its path names, not by its own name"
    );
    let plain = "#[path = \"sim_cap.rs\"]\nmod sim_cap;\n";
    let shipped = planted(&[("cap.rs", plain), ("sim_cap.rs", "")]);
    assert!(!is_test_file("sim_cap.rs", &shipped));
    let world = [
        (
            "lib.rs",
            "#[cfg(all(test, feature = \"wire\"))]\nmod world;\n",
        ),
        ("world.rs", "mod state;\n"),
        ("world/state.rs", ""),
    ];
    let test_only = planted(&world);
    assert!(is_test_file("world.rs", &test_only));
    assert!(is_test_file("world/state.rs", &test_only));
    let mut files = world.to_vec();
    files.push((
        "bin/world_bench.rs",
        "#[path = \"../world.rs\"]\nmod world;\n",
    ));
    let included = planted(&files);
    assert!(
        !is_test_file("world.rs", &included),
        "a bin that includes the module ships it"
    );
    assert!(
        !is_test_file("world/state.rs", &included),
        "so the files below it ship too"
    );
}

/// The declarations a parent may hold for module `{n}`, and whether each puts the file under test.
/// The verdict is written here from the Reference's rules, not read from the reader.
const FORMS: [(&str, bool); 11] = [
    ("mod {n};\n", false),
    ("pub(crate) mod {n};\n", false),
    ("#[cfg(test)]\nmod {n};\n", true),
    ("#[cfg(test)] mod {n};\n", true),
    ("#[cfg(all(test, unix))]\npub mod {n};\n", true),
    ("#[cfg(any(test, feature = \"x\"))]\nmod {n};\n", false),
    ("#[cfg(not(test))]\nmod {n};\n", false),
    ("// mod {n};\n", false),
    ("#[cfg(test)]\nmod other;\nmod {n};\n", false),
    ("#[cfg(test)]\nmod {n};\nfn (\n", false),
    ("#[test]\nfn mod_{n}() {}\n", false),
];

/// A parent module of a layer below the script layer, and the upper module its import names.
const PARENTS: [(&str, &str); 3] = [("rt", "script"), ("ecs", "task"), ("session", "script")];

/// Names a file may carry, which say nothing about it.
const NAMES: [&str; 5] = ["test_gate", "sim_gate", "tests", "test_x", "gate_helper"];

#[test]
/// Weird-bytes, wrong-order and saboteur: whichever test-sounding or plain name is drawn, and
/// whichever declaration form (plain, gated, `not(test)`, `any(test, ..)`, commented out, beside a
/// gated neighbour, in a file the grammar refuses), the file is test code exactly when the form says
/// so, on either parent file layout, and the layer scan reports its import exactly then. The same
/// seed replays.
fn sim_the_declaration_and_not_the_name_decides() -> Result<(), Box<dyn Error>> {
    let mut hashes = Vec::new();
    for seed in [0x1101_A000_u64, 0x1101_A001, 0x1101_A000] {
        let mut rng = Rng::new(seed);
        let mut trace = Trace::new();
        for draw in 0..300_u32 {
            let (module, upper) = pick(&mut rng, &PARENTS)?;
            let name = pick(&mut rng, &NAMES)?;
            let (form, test) = pick(&mut rng, &FORMS)?;
            let layout = if rng.below(2) == 0 {
                format!("{module}.rs")
            } else {
                format!("{module}/mod.rs")
            };
            let file = format!("{module}/{name}.rs");
            let declaration = form.replace("{n}", name);
            let import = format!("use crate::{upper}::Thing;\n");
            let tree = planted(&[
                (layout.as_str(), declaration.as_str()),
                (file.as_str(), import.as_str()),
            ]);
            assert_eq!(
                is_test_file(&file, &tree),
                test,
                "seed {seed:#x} draw {draw}: {form:?} declares {file}"
            );
            let want: Vec<(String, String)> = if test {
                Vec::new()
            } else {
                vec![(file.clone(), upper.to_owned())]
            };
            assert_eq!(found(&tree), want, "seed {seed:#x} draw {draw}: {form:?}");
            trace.record(&format!("{file} {test}"));
        }
        hashes.push(trace.hash());
    }
    assert_eq!(hashes.first(), hashes.get(2), "the first seed replays");
    assert_ne!(hashes.first(), hashes.get(1), "two seeds draw two traces");
    Ok(())
}
