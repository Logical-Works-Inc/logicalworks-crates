//! Detector missing-example: a public library item whose doc holds no code
//! block, so a reader, or a model writing the next line, has no call to copy.
//!
//! The bar is the one Tokio's API docs hold: every public function, type,
//! trait, constant and static shows itself in use. A doc with prose and no
//! example tells a reader what an item is for and leaves them to guess how it is
//! called, and a guess is where a wrong argument order or a forgotten `?` comes
//! from. The detector reads only the item's own outer doc, `///`, `/** */` and
//! `#[doc = ".."]`, and asks one question: does it open a fenced code block?
//!
//! It is opt-in ([`Rule::OPT_IN`]): the gate holds it to the lines a change
//! adds (`lgwks-ast scan --rule missing-example --base REV`), so every new item
//! lands with an example while the items written before the rule are counted,
//! not blocked.
//!
//! Not judged, each for a stated reason:
//!
//! - an item that is not plain `pub`: `pub(crate)` is no published reader's
//!   concern, and an item of a trait or a trait `impl` has no visibility of its
//!   own, so the trait's doc is where its example belongs;
//! - test code and `#[doc(hidden)]` items: neither is on a rendered page;
//! - a file under a `bin`, `tests`, `examples` or `benches` directory, or a
//!   `main.rs` or `build.rs`: rustdoc never runs an example written there, so
//!   the rule would ask for text no build checks.
//!
//! [`Rule::OPT_IN`]: crate::scan::Rule::OPT_IN

use std::path::{Component, Path};

use tree_sitter::Node;

use super::cfg;
use super::syntax::Fielded;
use super::{Hit, doc, each_node, syntax};

/// The rule name every finding carries.
const RULE: &str = "missing-example";

/// The item kinds a published API is made of, with the word a finding names
/// each by.
const ITEM_KINDS: [(&str, &str); 8] = [
    ("function_item", "fn"),
    ("struct_item", "struct"),
    ("enum_item", "enum"),
    ("union_item", "union"),
    ("trait_item", "trait"),
    ("type_item", "type"),
    ("const_item", "const"),
    ("static_item", "static"),
];

/// Directories whose Rust rustdoc never documents or tests.
const UNDOCUMENTED_DIRS: [&str; 4] = ["bin", "tests", "examples", "benches"];

/// Files that are a target's root rather than a library module.
const UNDOCUMENTED_FILES: [&str; 2] = ["main.rs", "build.rs"];

/// Runs the missing-example detector over a whole file at `path`.
pub(super) fn detect(source: &str, path: &str, root: Node<'_>, hits: &mut Vec<Hit>) {
    if !is_library_source(Path::new(path)) {
        return;
    }
    each_node(root, |node| {
        if let Some(&(_, word)) = ITEM_KINDS.iter().find(|&&(kind, _)| kind == node.kind()) {
            check(source, node, word, hits);
        }
    });
}

/// Records a finding when a public item's doc opens no code block.
fn check(source: &str, item: Node<'_>, word: &str, hits: &mut Vec<Hit>) {
    let Some(name) = item.by_field("name") else {
        return;
    };
    if !doc::is_public(source, item) || is_test_scope(source, item) {
        return;
    }
    if is_doc_hidden(source, item) || doc::doc_text(source, item).contains("```") {
        return;
    }
    let name_text = syntax::text(source, name);
    hits.push(Hit::new(
        RULE,
        syntax::line(name),
        format!(
            "pub {word} {name_text} — its doc shows no example. Add an `# Examples` section with a fenced block that calls it, so a reader can copy a working use."
        ),
    ));
}

/// True when `item` or any item it sits inside is test code: marked as a test
/// or test helper, or under a `cfg` that holds only in `test`. The exemption is
/// a scope, so a `pub` helper inside a `#[cfg(test)] mod tests` is test code.
fn is_test_scope(source: &str, item: Node<'_>) -> bool {
    let mut cursor = Some(item);
    while let Some(node) = cursor {
        if cfg::is_test_or_helper(source, node) || cfg::is_test_item(source, node) {
            return true;
        }
        cursor = node.parent();
    }
    false
}

/// True when an outer attribute above `item` is `#[doc(hidden)]`.
fn is_doc_hidden(source: &str, item: Node<'_>) -> bool {
    syntax::attributes(item).into_iter().any(|attribute| {
        syntax::text(source, attribute)
            .chars()
            .filter(|character| !character.is_whitespace())
            .eq("#[doc(hidden)]".chars())
    })
}

/// True when `path` is a library module rustdoc renders and tests: under no
/// directory in [`UNDOCUMENTED_DIRS`] and not a file in [`UNDOCUMENTED_FILES`].
fn is_library_source(path: &Path) -> bool {
    let named = |component: Component<'_>, names: &[&str]| match component {
        Component::Normal(part) => part.to_str().is_some_and(|part| names.contains(&part)),
        Component::Prefix(_) | Component::RootDir | Component::CurDir | Component::ParentDir => {
            false
        }
    };
    let mut components: Vec<Component<'_>> = path.components().collect();
    let Some(file) = components.pop() else {
        return false;
    };
    !named(file, &UNDOCUMENTED_FILES)
        && !components
            .into_iter()
            .any(|component| named(component, &UNDOCUMENTED_DIRS))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::is_library_source;

    #[test]
    fn only_library_modules_are_judged() {
        assert!(is_library_source(Path::new("crates/a/src/lib.rs")));
        assert!(is_library_source(Path::new("./crates/a/src/scan/doc.rs")));
        assert!(is_library_source(Path::new("sim.rs")));
        assert!(!is_library_source(Path::new(
            "crates/a/src/bin/tool/scan.rs"
        )));
        assert!(!is_library_source(Path::new("crates/a/tests/it/main.rs")));
        assert!(!is_library_source(Path::new("crates/a/examples/demo.rs")));
        assert!(!is_library_source(Path::new("crates/a/benches/b.rs")));
        assert!(!is_library_source(Path::new("crates/a/src/main.rs")));
        assert!(!is_library_source(Path::new("crates/a/build.rs")));
        assert!(!is_library_source(Path::new("")));
    }
}
