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
//! #991 widens it to every layer: a file names no module of a higher layer, and
//! the kernel names no tokio or axum either. The edges still open outside the
//! kernel are the named entries of `layer_edges::LAYER_EXCEPTIONS`, a new edge
//! is refused, a stale entry is refused, and the kernel holds none.
//!
//! Comments are not code: a line is read up to its first `//`. Tests are not
//! the core either: a test drives the core from above, the way the driver
//! does, so a test file (`tests.rs`, a `tests/` directory, `sim_*`) and an item
//! under `#[test]` or a `cfg` no build without `test` satisfies are not read
//! (#1096, `test_cfg`, through lgwks_ast's Rust grammar). The code after such an
//! item is read, and a file the grammar refuses is named and read whole.

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

/// The upward imports still open outside the kernel, and the checks that keep
/// that list shrinking.
mod layer_edges;

/// Which lines of a file are test code: an item under a `cfg` that needs `test`.
mod test_cfg;

/// Hostile seeded sweeps over that reader, against a reference model.
mod sim_test_cfg;

/// Hostile seeded sweeps over the shipped view: weird bytes, cut files, and
/// timed-out parses on every thread.
mod sim_shipped_view;

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
        // `kernel` is the supported facade over the kernel modules and the
        // task front door, so it sits above everything it re-exports (#991).
        modules: &["app", "app_error", "cell", "kernel"],
    },
];

/// The name of the lowest layer in [`LAYERS`].
const KERNEL: &str = "kernel";

/// What a kernel module may not name by a relative path, which a `super::` chain
/// reaches without `crate::`: the session and the ECS engine.
const KERNEL_SUPER: [&str; 2] = ["super::session", "super::ecs"];

/// The drivers no kernel module may name.
const KERNEL_DRIVERS: [&str; 2] = ["tokio", "axum"];

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

/// The position in [`LAYERS`] of the layer `relative` belongs to, if any.
fn layer_index(relative: &str) -> Option<usize> {
    LAYERS
        .iter()
        .position(|layer| layer.modules.iter().any(|name| in_module(relative, name)))
}

/// The layer `relative` belongs to, by its top-level module, if it has one.
fn layer_of(relative: &str) -> Option<&'static str> {
    layer_index(relative)
        .and_then(|index| LAYERS.get(index))
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

/// Whether `relative` is a test file rather than shipped code.
fn is_test_file(relative: &str) -> bool {
    relative.split('/').any(|part| {
        part == "tests"
            || part == "tests.rs"
            || part.starts_with("sim_")
            || part.starts_with("test_")
    })
}

/// Every boundary edge in one source file, as `path:line: reason`.
fn edges(relative: &str, text: &str) -> Vec<String> {
    let core = CORE.iter().any(|name| in_module(relative, name));
    let ecs = in_module(relative, "ecs");
    let mut found = Vec::new();
    if is_test_file(relative) {
        return found;
    }
    let reading = test_cfg::read(text);
    if let Some(ref refusal) = reading.refused {
        found.push(test_cfg::refusal_edge(relative, refusal));
    }
    for (index, code) in reading.shipped.lines().enumerate() {
        let number = index.saturating_add(1);
        if !ecs && code.contains("bevy") {
            found.push(format!("{relative}:{number}: names bevy outside ecs/"));
        }
        if core {
            for driver in DRIVER.iter().filter(|driver| code.contains(**driver)) {
                found.push(format!("{relative}:{number}: core module names {driver}"));
            }
        }
        if code.contains("catch_unwind") && !UNWIND_SITES.contains(&relative) {
            found.push(format!("{relative}:{number}: {UNWIND_REFUSAL}"));
        }
    }
    for (number, layer, path) in layer_edges::upward_imports(relative, text) {
        // A core module that names the driver is already refused above.
        if layer == KERNEL && !(core && DRIVER.contains(&path.as_str())) {
            found.push(format!("{relative}:{number}: {layer} module names {path}"));
        }
    }
    found
}

/// Whether the production code of `text`, outside every test module, names
/// `catch_unwind` in code.
fn catches_a_panic(text: &str) -> bool {
    test_cfg::read(text).shipped.contains("catch_unwind")
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

/// Every source file of the crate as its `/`-separated path below `src/` and
/// its text, in a stable order.
fn read_tree(src: &Path) -> Result<Vec<(String, String)>, Box<dyn Error>> {
    let mut files = Vec::new();
    sources(src, &mut files)?;
    assert!(files.len() > 100, "the walk found the crate's sources");
    let mut tree = Vec::with_capacity(files.len());
    for file in &files {
        let relative = file
            .strip_prefix(src)?
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        tree.push((relative, fs::read_to_string(file)?));
    }
    Ok(tree)
}

#[test]
/// The shipped tree holds the boundary: no edge in any source file.
fn the_core_names_no_driver_and_only_ecs_names_bevy() -> Result<(), Box<dyn Error>> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = Vec::new();
    for (relative, text) in read_tree(&src)? {
        found.extend(edges(&relative, &text));
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

#[test]
/// #991: whichever layer is drawn, a planted import of a module of a higher
/// layer is refused with the exact message, a module of its own or a lower layer
/// is not, and the kernel refuses the drivers as well.
fn a_planted_upward_import_is_refused_in_every_layer() -> Result<(), Box<dyn Error>> {
    use layer_edges::{pick, upward_refusals};
    // One file list per layer below the last, kernel first. The kernel's list
    // opens with the sweeper file whose upward edges #991 cut.
    let files: [&[&str]; 6] = [
        &[
            "sealed/sweep.rs",
            "sealed/keys.rs",
            "journal/seam.rs",
            "tenant_store/store.rs",
            "broker/fence.rs",
            "dist_lease/tcp.rs",
        ],
        &["rt/runtime.rs", "rt/sync.rs", "rt/task.rs"],
        &["ecs/bot.rs", "registry/table.rs", "world/state.rs"],
        &[
            "session/runner.rs",
            "interface/model.rs",
            "language/lexicon.rs",
        ],
        &["script/error.rs", "task/host.rs", "proposal/plan.rs"],
        &["domain/gh/verbs.rs", "review/pr.rs", "stability/monitor.rs"],
    ];
    let seeds = [
        0x9910_A000_u64,
        0x9910_A001,
        0xFFFF_9910_A002,
        0x1991_9910_A003,
    ];
    for seed in seeds {
        let mut rng = Rng::new(seed);
        for draw in 0..128_u32 {
            let at = pick(&mut rng, &[0_usize, 1, 2, 3, 4, 5])?;
            let layer = LAYERS
                .get(at)
                .ok_or_else(|| Box::<dyn Error>::from("the layer is not in the table"))?;
            let owned = files
                .get(at)
                .ok_or_else(|| Box::<dyn Error>::from("the layer has no files"))?;
            let file = pick(&mut rng, owned)?;
            let higher: Vec<&str> = LAYERS
                .iter()
                .skip(at.saturating_add(1))
                .flat_map(|upper| upper.modules.iter().copied())
                .collect();
            let upper = pick(&mut rng, &higher)?;
            assert_eq!(
                upward_refusals(file, &format!("use crate::{upper}::Thing;")),
                vec![format!(
                    "{file}:1: {} module names crate::{upper}",
                    layer.name
                )],
                "seed {seed:#x} draw {draw}"
            );
            let own: Vec<&str> = LAYERS
                .iter()
                .take(at.saturating_add(1))
                .flat_map(|same| same.modules.iter().copied())
                .collect();
            let lower = pick(&mut rng, &own)?;
            assert!(
                upward_refusals(file, &format!("use crate::{lower}::Thing;")).is_empty(),
                "seed {seed:#x} draw {draw}: {file} may name crate::{lower}"
            );
        }
    }
    for driver in ["tokio", "axum"] {
        let planted = format!("use lgwks_deps::{driver}::Thing;");
        assert_eq!(
            upward_refusals("sealed/sweep.rs", &planted),
            vec![format!("sealed/sweep.rs:1: kernel module names {driver}")],
        );
        assert!(
            upward_refusals("rt/runtime.rs", &planted).is_empty(),
            "the driver layer names the driver"
        );
    }
    Ok(())
}

#[test]
/// #991: every upward import the tree holds is a named exception, no exception
/// is a kernel file, and the kernel names no upper layer at all.
fn every_upward_import_is_a_named_exception_and_the_kernel_has_none() -> Result<(), Box<dyn Error>>
{
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let found = layer_edges::in_tree(&src)?;
    let kernel: Vec<String> = found
        .iter()
        .filter(|edge| edge.layer == KERNEL)
        .map(|edge| format!("{}: the kernel names {}", edge.file, edge.module))
        .collect();
    assert!(
        kernel.is_empty(),
        "the kernel layer names an upper layer:\n{}",
        kernel.join("\n")
    );
    let mut problems = layer_edges::kernel_entries(layer_edges::LAYER_EXCEPTIONS);
    problems.extend(layer_edges::unlisted(layer_edges::LAYER_EXCEPTIONS, &found));
    problems.extend(layer_edges::stale(layer_edges::LAYER_EXCEPTIONS, &found));
    assert!(
        problems.is_empty(),
        "upward imports against LAYER_EXCEPTIONS:\n{}",
        problems.join("\n")
    );
    Ok(())
}

#[test]
/// #991: an exception whose edge the tree no longer has is refused by name, so
/// the list only shrinks; a live entry and an unlisted edge answer correctly.
fn a_stale_layer_exception_is_refused() {
    let held = [layer_edges::Upward {
        file: String::from("rt/sweep.rs"),
        layer: "rt",
        module: String::from("script"),
    }];
    let stale = layer_edges::stale(&[("rt/sweep.rs", "session")], &held);
    assert_eq!(stale.len(), 1);
    assert!(
        stale
            .iter()
            .all(|message| message.contains("drop it from LAYER_EXCEPTIONS")),
        "{stale:?}"
    );
    assert!(layer_edges::stale(&[("rt/sweep.rs", "script")], &held).is_empty());
    let unlisted = layer_edges::unlisted(&[("rt/sweep.rs", "session")], &held);
    assert_eq!(unlisted.len(), 1, "an edge no entry names is refused");
    assert!(layer_edges::unlisted(&[("rt/sweep.rs", "script")], &held).is_empty());
    assert_eq!(
        layer_edges::kernel_entries(&[("sealed/sweep.rs", "rt"), ("rt/sweep.rs", "script")]).len(),
        1,
        "only the kernel file is refused as an exception"
    );
}
