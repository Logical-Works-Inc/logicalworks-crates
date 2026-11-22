//! The upward imports still open outside the kernel, and the two checks that
//! keep the list honest (#991).
//!
//! The layer rule of `core_boundary` refuses a file that names a module of a
//! higher layer. Some files outside the kernel still do. Each is one named
//! entry in [`LAYER_EXCEPTIONS`]: a new edge is refused until it is listed, and
//! an entry whose edge the tree no longer has is refused until it is dropped,
//! so the list only shrinks. The kernel, the lowest layer, holds no entry.

use std::error::Error;
use std::path::Path;

use super::test_cfg::read;
use super::test_decl::{Sources, is_test_file};
use super::{
    KERNEL, KERNEL_DRIVERS, KERNEL_SUPER, LAYERS, Rng, layer_index, layer_of, names, read_tree,
};

/// Every upward import in the production code of one source file, as the line,
/// the file's layer and the path it names: a `crate::<module>` of a higher
/// layer, and for the kernel a tokio or axum path and the `super::` chains of
/// `KERNEL_SUPER`. A file outside every layer names none; whether the file is
/// test code is the caller's question (`test_decl`), asked of the tree.
pub(super) fn upward_imports(relative: &str, text: &str) -> Vec<(usize, &'static str, String)> {
    let mut found = Vec::new();
    let Some(own) = layer_index(relative) else {
        return found;
    };
    let Some(layer) = LAYERS.get(own) else {
        return found;
    };
    let mut refused: Vec<String> = LAYERS
        .iter()
        .skip(own.saturating_add(1))
        .flat_map(|upper| upper.modules.iter())
        .map(|name| format!("crate::{name}"))
        .collect();
    if layer.name == KERNEL {
        refused.extend(KERNEL_DRIVERS.iter().map(|path| (*path).to_owned()));
        refused.extend(KERNEL_SUPER.iter().map(|path| (*path).to_owned()));
    }
    let shipped = read(text).shipped;
    for (index, code) in shipped.lines().enumerate() {
        for path in refused.iter().filter(|path| names(code, path)) {
            found.push((index.saturating_add(1), layer.name, path.clone()));
        }
    }
    found
}

/// The refusals of [`upward_imports`], as `path:line: <layer> module names <path>`.
pub(super) fn upward_refusals(relative: &str, text: &str) -> Vec<String> {
    upward_imports(relative, text)
        .into_iter()
        .map(|(number, layer, path)| format!("{relative}:{number}: {layer} module names {path}"))
        .collect()
}

/// One draw of `items`, from the seeded stream.
pub(super) fn pick<T: Copy>(rng: &mut Rng, items: &[T]) -> Result<T, Box<dyn Error>> {
    let bound = u32::try_from(items.len())?;
    let at = usize::try_from(rng.below(bound))?;
    items
        .get(at)
        .copied()
        .ok_or_else(|| Box::<dyn Error>::from("the draw fell outside the table"))
}

/// Every upward edge the sources under `src` hold, one per file and upper module.
pub(super) fn in_tree(src: &Path) -> Result<Vec<Upward>, Box<dyn Error>> {
    Ok(upward_in(&read_tree(src)?))
}

/// Every upward edge the files of `tree` hold, one per file and upper module, leaving out each
/// file its parent declares as test code. A file named `test_*` that ships is read (#1101).
pub(super) fn upward_in(tree: &Sources) -> Vec<Upward> {
    let mut found: Vec<Upward> = Vec::new();
    for file in tree {
        if is_test_file(&file.0, tree) {
            continue;
        }
        for (_, layer, path) in upward_imports(&file.0, &file.1) {
            let module = match path.strip_prefix("crate::") {
                Some(rest) => rest.to_owned(),
                None => path,
            };
            if !found
                .iter()
                .any(|edge| edge.file == file.0 && edge.module == module)
            {
                found.push(Upward {
                    file: file.0.clone(),
                    layer,
                    module,
                });
            }
        }
    }
    found
}

/// One upward import found in the tree: the file, its layer, the upper module.
pub(super) struct Upward {
    /// The file, as a `/`-separated path below `src/`.
    pub(super) file: String,
    /// The layer the file belongs to.
    pub(super) layer: &'static str,
    /// The module of a higher layer the file names, without `crate::`.
    pub(super) module: String,
}

/// Every upward edge still open, as (file below `src/`, upper module).
///
/// A file appears once per upper module, however many lines name it. The list
/// holds no kernel file.
pub(super) const LAYER_EXCEPTIONS: &[(&str, &str)] = &[
    // world: 2
    ("ecs.rs", "cell"),
    ("registry/condition.rs", "domain"),
    // rt: 22
    ("rt/apps/routes.rs", "script"),
    ("rt/apps/run.rs", "script"),
    ("rt/apps/store.rs", "proposal"),
    ("rt/apps/store.rs", "script"),
    ("rt/edge.rs", "cell"),
    ("rt/edge.rs", "ecs"),
    ("rt/estate.rs", "script"),
    ("rt/estate.rs", "task"),
    ("rt/runtime.rs", "ecs"),
    ("rt/serve.rs", "task"),
    ("rt/serve/hosts.rs", "task"),
    ("rt/serve/ingress.rs", "ecs"),
    ("rt/serve/ingress.rs", "task"),
    ("rt/serve/table.rs", "script"),
    ("rt/serve/table.rs", "task"),
    ("rt/supervise/admit.rs", "script"),
    ("rt/supervise/lease.rs", "script"),
    ("rt/supervise/run.rs", "script"),
    ("rt/supervise/supervisor.rs", "script"),
    ("rt/supervise/tenancy_support.rs", "script"),
    ("rt/supervise/world.rs", "ecs"),
    ("rt/tenancy.rs", "script"),
];

/// The found edges that no entry of `exceptions` names.
pub(super) fn unlisted(exceptions: &[(&str, &str)], found: &[Upward]) -> Vec<String> {
    found
        .iter()
        .filter(|edge| {
            !exceptions
                .iter()
                .any(|entry| entry.0 == edge.file && entry.1 == edge.module)
        })
        .map(|edge| {
            format!(
                "{}: the {} layer names crate::{}; remove the import or list it in LAYER_EXCEPTIONS",
                edge.file, edge.layer, edge.module
            )
        })
        .collect()
}

/// The entries of `exceptions` whose edge `found` no longer holds.
pub(super) fn stale(exceptions: &[(&str, &str)], found: &[Upward]) -> Vec<String> {
    exceptions
        .iter()
        .filter(|entry| {
            !found
                .iter()
                .any(|edge| edge.file == entry.0 && edge.module == entry.1)
        })
        .map(|entry| {
            format!(
                "{}: no longer names crate::{}; drop it from LAYER_EXCEPTIONS",
                entry.0, entry.1
            )
        })
        .collect()
}

/// The entries of `exceptions` that name a kernel file, which may hold none.
pub(super) fn kernel_entries(exceptions: &[(&str, &str)]) -> Vec<String> {
    exceptions
        .iter()
        .filter(|entry| layer_of(entry.0) == Some(KERNEL))
        .map(|entry| {
            format!(
                "{}: the kernel holds no exception, fix crate::{}",
                entry.0, entry.1
            )
        })
        .collect()
}
