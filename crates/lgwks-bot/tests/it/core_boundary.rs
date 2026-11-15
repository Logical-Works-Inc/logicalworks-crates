//! ADR 0006 decision 4: the core does not import the driver, and only `ecs/`
//! names the ECS engine.
//!
//! The ECS world is the core; tokio and axum are the one driver below it. ADR
//! 0005 held the same boundary as `review only`, and two pull requests (#734,
//! #735) built a tokio stack beside the world without a test refusing them.
//! This is that test: it reads every source file of the crate and refuses
//!
//! - a file outside `src/ecs.rs` and `src/ecs/` that names `bevy`, and
//! - a core module (`ecs`, `verb`, `cap`, `gate`, `effect`, `journal`,
//!   `error`, `clock`, `spec`, `world`) that names tokio, axum, or the crate's `rt`,
//!   `task`, `script` or `domain` modules.
//!
//! It also holds ADR 0053 decision 5, "no `catch_unwind`": the files that still
//! catch a panic are [`UNWIND_SITES`], a new site is refused, and a listed file
//! that stops catching must leave the list, so the set only shrinks.
//!
//! It holds the layer rule of #990, "a lower layer's error never names an upper
//! one": every top-level module sits in one of the [`LAYERS`], and a kernel
//! module (the lowest layer) that names the world, the session or the stability
//! monitor is refused, so an error type cannot drift upward by an import.
//!
//! Comments are not code: a line is read up to its first `//`. Tests are not
//! the core either: a test drives the core from above, the way the driver
//! does, so a test file (`tests.rs`, a `tests/` directory, `sim_*`) and the
//! `#[cfg(test)] mod` that closes a file are not read.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use crate::sim::Rng;

/// How a failure keeps its class and names its layer as it is wrapped upward.
#[cfg(feature = "script")]
mod layer_errors;

/// Hostile seeded sweeps over the same chain, one fool per family.
#[cfg(feature = "script")]
mod sim_error_layers;

/// The modules ADR 0006 names as the core.
const CORE: [&str; 10] = [
    "ecs", "verb", "cap", "gate", "effect", "journal", "error", "clock", "spec", "world",
];

/// What a core module may not name: the driver and the stacks that sit on it.
const DRIVER: [&str; 6] = [
    "tokio",
    "axum",
    "crate::rt",
    "crate::task",
    "crate::script",
    "crate::domain",
];

/// One layer of the crate: its name and the top-level modules that belong to it.
struct Layer {
    /// What the layer is called in a refusal.
    name: &'static str,
    /// The top-level modules (a `<name>.rs` file or a `<name>/` directory).
    modules: &'static [&'static str],
}

/// The layers, lowest first. A module is in exactly one; an error type lives in
/// the layer that raises it and an upper layer wraps it with `From`.
const LAYERS: [Layer; 7] = [
    Layer {
        name: KERNEL,
        modules: &[
            "broker",
            "cap",
            "cap_token",
            "cas",
            "clock",
            "dist_lease",
            "durable",
            "effect",
            "error",
            "gate",
            "journal",
            "json",
            "kernel",
            "retry",
            "sealed",
            "skew",
            "tenant_store",
        ],
    },
    Layer {
        name: "rt",
        modules: &["rt"],
    },
    Layer {
        name: "world",
        modules: &["ecs", "registry", "spec", "verb", "world"],
    },
    Layer {
        name: "session",
        modules: &["interface", "language", "semantic", "session"],
    },
    Layer {
        name: "script",
        modules: &["proposal", "script", "task"],
    },
    Layer {
        name: "domains",
        modules: &[
            "domain",
            "frontier",
            "idempotent",
            "inspect",
            "keyed",
            "review",
            "stability",
        ],
    },
    Layer {
        name: "app",
        modules: &["app", "app_error", "cell"],
    },
];

/// The name of the lowest layer in [`LAYERS`].
const KERNEL: &str = "kernel";

/// What a kernel module may not name: the world, the session and the stability
/// monitor, by absolute or relative path.
const KERNEL_REFUSES: [&str; 7] = [
    "crate::session",
    "crate::ecs",
    "crate::spec",
    "crate::stability",
    "crate::world",
    "super::session",
    "super::ecs",
];

/// The files that still catch a panic (ADR 0053 decision 5). The list only
/// shrinks; the cell host's site leaves with the supervisor (W6, #1006).
const UNWIND_SITES: [&str; 3] = ["app/check.rs", "app/command.rs", "cell/host.rs"];

/// The refusal a new panic catch site gets.
const UNWIND_REFUSAL: &str = "catches a panic outside UNWIND_SITES (ADR 0053 decision 5)";

/// Whether `relative` (a `/`-separated path below `src/`) is the module `name`
/// or a file inside it.
fn in_module(relative: &str, name: &str) -> bool {
    relative == format!("{name}.rs") || relative.starts_with(&format!("{name}/"))
}

/// The layer `relative` belongs to, by its top-level module, if it has one.
fn layer_of(relative: &str) -> Option<&'static str> {
    LAYERS
        .iter()
        .find(|layer| layer.modules.iter().any(|name| in_module(relative, name)))
        .map(|layer| layer.name)
}

/// Whether `code` names `path` as a whole path: the character after it does not
/// continue an identifier, so `crate::spec` is not `crate::specimen`.
fn names(code: &str, path: &str) -> bool {
    code.match_indices(path).any(|(at, matched)| {
        let Some(rest) = code.get(at.saturating_add(matched.len())..) else {
            return false;
        };
        match rest.chars().next() {
            Some(next) => !(next.is_alphanumeric() || next == '_'),
            None => true,
        }
    })
}

/// The code of one line: everything before its first `//`.
fn code_of(line: &str) -> &str {
    match line.split_once("//") {
        Some((code, _)) => code,
        None => line,
    }
}

/// Whether `relative` is a test file rather than shipped code.
fn is_test_file(relative: &str) -> bool {
    relative.split('/').any(|part| {
        part == "tests"
            || part == "tests.rs"
            || part.starts_with("sim_")
            || part.starts_with("test_")
    })
}

/// Whether line `index` of `lines` opens a `#[cfg(test)]` module.
fn opens_test_module(lines: &[&str], index: usize) -> bool {
    let Some(line) = lines.get(index) else {
        return false;
    };
    line.trim() == "#[cfg(test)]"
        && lines
            .iter()
            .skip(index.saturating_add(1))
            .find(|next| !next.trim().is_empty())
            .is_some_and(|next| next.trim_start().starts_with("mod "))
}

/// Every boundary edge in one source file, as `path:line: reason`.
fn edges(relative: &str, text: &str) -> Vec<String> {
    let core = CORE.iter().any(|name| in_module(relative, name));
    let ecs = in_module(relative, "ecs");
    let kernel = layer_of(relative) == Some(KERNEL);
    let mut found = Vec::new();
    if is_test_file(relative) {
        return found;
    }
    let lines: Vec<&str> = text.lines().collect();
    for (index, line) in lines.iter().enumerate() {
        if opens_test_module(&lines, index) {
            break;
        }
        let number = index.saturating_add(1);
        let code = code_of(line);
        if !ecs && code.contains("bevy") {
            found.push(format!("{relative}:{number}: names bevy outside ecs/"));
        }
        if core {
            for driver in DRIVER.iter().filter(|driver| code.contains(**driver)) {
                found.push(format!("{relative}:{number}: core module names {driver}"));
            }
        }
        if kernel {
            for upper in KERNEL_REFUSES.iter().filter(|upper| names(code, upper)) {
                found.push(format!("{relative}:{number}: kernel module names {upper}"));
            }
        }
        if code.contains("catch_unwind") && !UNWIND_SITES.contains(&relative) {
            found.push(format!("{relative}:{number}: {UNWIND_REFUSAL}"));
        }
    }
    found
}

/// Whether the production part of `text`, above any closing test module,
/// names `catch_unwind` in code.
fn catches_a_panic(text: &str) -> bool {
    let lines: Vec<&str> = text.lines().collect();
    for (index, line) in lines.iter().enumerate() {
        if opens_test_module(&lines, index) {
            return false;
        }
        if code_of(line).contains("catch_unwind") {
            return true;
        }
    }
    false
}

/// Every `.rs` file under `dir`, recursively, in a stable order.
fn sources(dir: &Path, into: &mut Vec<PathBuf>) -> Result<(), Box<dyn Error>> {
    let mut entries = fs::read_dir(dir)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(fs::DirEntry::path);
    for entry in entries {
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            sources(&path, into)?;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            into.push(path);
        }
    }
    Ok(())
}

#[test]
/// The shipped tree holds the boundary: no edge in any source file.
fn the_core_names_no_driver_and_only_ecs_names_bevy() -> Result<(), Box<dyn Error>> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    sources(&src, &mut files)?;
    assert!(files.len() > 100, "the walk found the crate's sources");
    let mut found = Vec::new();
    for file in &files {
        let relative = file
            .strip_prefix(&src)?
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        found.extend(edges(&relative, &fs::read_to_string(file)?));
    }
    for site in UNWIND_SITES {
        if !catches_a_panic(&fs::read_to_string(src.join(site))?) {
            found.push(format!(
                "{site}: no longer catches a panic; drop it from UNWIND_SITES"
            ));
        }
    }
    assert!(
        found.is_empty(),
        "ADR 0006 boundary edges:\n{}",
        found.join("\n")
    );
    Ok(())
}

#[test]
/// ADR 0006's falsifier: a planted `use crate::rt` in `gate.rs` is refused.
fn a_planted_driver_import_in_the_core_is_refused() {
    assert_eq!(
        edges("gate.rs", "use crate::rt::runtime::Runtime;"),
        vec![String::from("gate.rs:1: core module names crate::rt")],
    );
    assert_eq!(
        edges("journal/file/write.rs", "use lgwks_deps::tokio::fs;").len(),
        1
    );
    assert_eq!(
        edges("ecs/bot.rs", "use lgwks_deps::axum::Router;").len(),
        1
    );
    assert_eq!(
        edges("world/state.rs", "use crate::rt::runtime::Runtime;"),
        vec![String::from(
            "world/state.rs:1: core module names crate::rt"
        )],
        "the owned world (ADR 0055) is a core module"
    );
}

#[test]
/// ADR 0053 decision 5: a planted fourth `catch_unwind` is refused, a listed
/// site stands until it leaves the list, and comments and tests are not sites.
fn a_new_catch_unwind_site_is_refused() {
    assert_eq!(
        edges("gate.rs", "let caught = std::panic::catch_unwind(work);"),
        vec![format!("gate.rs:1: {UNWIND_REFUSAL}")],
    );
    assert_eq!(
        edges("cell/worker.rs", "use std::panic::catch_unwind;").len(),
        1,
        "a neighbour of a listed site is not listed"
    );
    assert!(
        edges(
            "cell/host.rs",
            "use std::panic::{AssertUnwindSafe, catch_unwind};"
        )
        .is_empty(),
        "a listed site stands until W6 removes it"
    );
    assert!(edges("rt/supervise.rs", "// never catch_unwind here").is_empty());
    assert!(edges("app/sim_app.rs", "let _caught = catch_unwind(work);").is_empty());
    assert!(catches_a_panic("use std::panic::catch_unwind;\n"));
    let tested = "fn run() {}\n#[cfg(test)]\nmod tests {\n    use std::panic::catch_unwind;\n}\n";
    assert!(
        !catches_a_panic(tested),
        "a catch inside the closing test module is not a site"
    );
}

#[test]
/// bevy is refused everywhere but `ecs/`, and a comment is not an edge.
fn bevy_outside_ecs_is_refused_and_comments_are_not_code() {
    assert_eq!(
        edges("task/host.rs", "use lgwks_deps::bevy_ecs::World;").len(),
        1
    );
    assert!(edges("ecs/fold.rs", "use lgwks_deps::bevy_ecs::World;").is_empty());
    assert!(edges("ecs.rs", "use lgwks_deps::bevy_ecs::World;").is_empty());
    assert!(edges("gate.rs", "/// built on tokio // and crate::rt").is_empty());
    assert!(edges("rt/runtime.rs", "use lgwks_deps::tokio;").is_empty());
    assert!(
        edges("verbose.rs", "use crate::rt;").is_empty(),
        "verb is a module, not a prefix"
    );
}

#[test]
/// A test drives the core from above and is not read; the code before a test
/// module still is.
fn test_code_is_a_caller_and_production_code_above_it_is_still_read() {
    assert!(edges("journal/file/tests.rs", "use crate::rt::runtime::block_on;").is_empty());
    assert!(edges("clock/tests/wait.rs", "use crate::rt::time;").is_empty());
    assert!(edges("sim_gate.rs", "use crate::task::Host;").is_empty());
    let owner = "fn flush() {}\n\n#[cfg(test)]\nmod tests {\n    use crate::task::RunStore;\n}\n";
    assert!(
        edges("journal/owner.rs", owner).is_empty(),
        "a closing test module is a caller"
    );
    let planted = "use crate::script::Scope;\n#[cfg(test)]\nmod tests {}\n";
    assert_eq!(
        edges("journal/owner.rs", planted),
        vec![String::from(
            "journal/owner.rs:1: core module names crate::script"
        )],
        "an edge above the test module is still refused"
    );
    let gated_item = "#[cfg(test)]\nfn helper() {}\nuse crate::rt::time;\n";
    assert_eq!(
        edges("clock.rs", gated_item).len(),
        1,
        "only a test module closes the read"
    );
}

#[test]
/// #990's falsifier: a kernel error file that imports the session or the world,
/// absolutely or relatively, is refused with the exact path it named.
fn a_kernel_error_naming_an_upper_layer_is_refused() {
    assert_eq!(
        edges("error.rs", "use crate::session::Terminal;"),
        vec![String::from(
            "error.rs:1: kernel module names crate::session"
        )],
    );
    assert_eq!(
        edges("error/class.rs", "use super::super::ecs::WorkId;"),
        vec![String::from(
            "error/class.rs:1: kernel module names super::ecs"
        )],
    );
    assert_eq!(
        edges("cas.rs", "use crate::world::WorldError;"),
        vec![String::from("cas.rs:1: kernel module names crate::world")],
        "every kernel module is held, not only the error"
    );
    assert_eq!(
        edges("journal/file/open.rs", "use crate::stability::Monitor;").len(),
        1
    );
    assert_eq!(edges("gate.rs", "use crate::spec::BotSpec;").len(), 1);
}

/// The kernel module drawn by `draw`, as a path below `src/`.
fn kernel_file(draw: u32) -> &'static str {
    match draw {
        0 => "error.rs",
        1 => "error/class.rs",
        2 => "cap.rs",
        3 => "gate.rs",
        4 => "clock/epoch.rs",
        5 => "effect/id.rs",
        6 => "journal/file/open.rs",
        7 => "broker/fence.rs",
        8 => "retry.rs",
        9 => "tenant_store/store.rs",
        10 => "cas.rs",
        _ => "dist_lease/tcp.rs",
    }
}

/// The upper-layer path drawn by `draw`, which no kernel module may name.
fn upper_path(draw: u32) -> &'static str {
    match draw {
        0 => "crate::session",
        1 => "crate::ecs",
        2 => "crate::spec",
        3 => "crate::stability",
        4 => "crate::world",
        5 => "super::session",
        _ => "super::ecs",
    }
}

/// A file outside the kernel, drawn by `draw`, which may name the world.
fn upper_file(draw: u32) -> &'static str {
    match draw {
        0 => "session/runner.rs",
        1 => "task/host.rs",
        2 => "domain/gh/verbs.rs",
        3 => "script/error.rs",
        _ => "app/serve.rs",
    }
}

#[test]
/// Seeded: whichever kernel module and whichever upper path are drawn, the
/// planted import is refused exactly once with the exact message, and the same
/// import in an upper layer's file is not a kernel refusal.
fn sim_a_planted_upper_import_is_refused_in_every_kernel_module() {
    let seeds = [
        0x9900_B000_u64,
        0x9900_B001,
        0xFFFF_9900_B002,
        0x1990_9900_B003,
    ];
    for seed in seeds {
        let mut rng = Rng::new(seed);
        for draw in 0..96_u32 {
            let file = kernel_file(rng.below(12));
            let path = upper_path(rng.below(7));
            let planted = format!("use {path}::Thing;");
            assert_eq!(
                edges(file, &planted),
                vec![format!("{file}:1: kernel module names {path}")],
                "seed {seed:#x} draw {draw}"
            );
            let above = upper_file(rng.below(5));
            let refused = edges(above, &planted);
            assert!(
                refused.iter().all(|edge| !edge.contains("kernel module")),
                "seed {seed:#x} draw {draw}: {above} is not a kernel module: {refused:?}"
            );
        }
    }
}

#[test]
/// A kernel module may name its own layer, a comment is not an edge, a word that
/// only begins like a forbidden module is not it, and an upper layer may name the
/// kernel.
fn a_kernel_module_naming_the_kernel_or_a_neighbour_word_is_not_refused() {
    assert!(edges("error.rs", "use crate::cap::Cap;").is_empty());
    assert!(edges("error/display.rs", "use super::class::RetryClass;").is_empty());
    assert!(edges("error.rs", "/// see crate::session // and crate::ecs").is_empty());
    assert!(edges("gate.rs", "use crate::specimen::Page;").is_empty());
    assert!(edges("gate.rs", "use super::ecs_like::Page;").is_empty());
    assert!(
        edges("session/error.rs", "use crate::error::KernelError;").is_empty(),
        "the session layer wraps the kernel's error"
    );
    assert!(edges("ecs/error.rs", "use crate::error::KernelError;").is_empty());
    assert!(
        edges("session/runner.rs", "use crate::ecs::EcsError;").is_empty(),
        "an upper layer names the layer below it"
    );
}

#[test]
/// Every top-level module is in exactly one layer and every listed module is a
/// file or directory of the crate, so the table cannot drift from the tree.
fn every_top_level_module_is_in_exactly_one_layer() -> Result<(), Box<dyn Error>> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut present = Vec::new();
    for entry in fs::read_dir(&src)? {
        let path = entry?.path();
        let Some(stem) = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
        else {
            continue;
        };
        let is_module =
            path.is_dir() || path.extension().is_some_and(|extension| extension == "rs");
        if is_module && !is_test_file(&stem) && stem != "lib" && stem != "bin" {
            present.push(stem);
        }
    }
    present.sort();
    present.dedup();
    let mut problems = Vec::new();
    for stem in &present {
        let homes = LAYERS
            .iter()
            .filter(|layer| layer.modules.contains(&stem.as_str()))
            .count();
        if homes != 1 {
            problems.push(format!("{stem}: in {homes} layers, expected 1"));
        }
    }
    for layer in &LAYERS {
        for name in layer.modules {
            if !present.iter().any(|stem| stem == name) {
                problems.push(format!(
                    "{} lists {name}, which is not a module",
                    layer.name
                ));
            }
        }
    }
    assert!(
        problems.is_empty(),
        "the layer table disagrees with src/:\n{}",
        problems.join("\n")
    );
    Ok(())
}
