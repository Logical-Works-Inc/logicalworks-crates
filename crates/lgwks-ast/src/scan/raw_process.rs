//! Detector raw-process: a `std` or `tokio` process `Command` built by hand, in
//! production and in test code alike (#1160 G3).
//!
//! A raw `Command` starts a child nothing owns once the caller stops running:
//! a SIGKILL of the caller leaves the child and its group alive. The tests of
//! two consumer repositories started their servers this way, and the children
//! held a 24 GB Mac for hours. The owners are `lgwks_std::process::Command`,
//! whose start holds the group on the lifeline (ADR 0092), and a `ProcessSpec`
//! handed to the `lgwks_bot` `Supervisor`; a disk a tool attaches is held with
//! the lifeline's `hold_device` too.
//!
//! A call is raw when it is `new` on a path that ends `process::Command`
//! (`std::process::Command::new`, `tokio::process::Command::new`), or on the
//! bare name or alias a `use` of such a path bound in this file. A path that
//! starts `crate`, `self` or `super` names the file's own module, and one that
//! starts `lgwks_std` names the owner; neither is raw. Calls written in a
//! macro's arguments are read from its tokens. Test code is not exempt: the
//! leak this rule exists for came from tests.

use tree_sitter::Node;

use super::Hit;
use super::syntax::{self, Fielded, Path};

/// The rule name every finding carries.
const RULE: &str = "raw-process";

/// The programs that attach or mount a disk, so a finding names the device
/// hold as well.
const DISK_TOOLS: [&str; 4] = ["hdiutil", "mount", "diskutil", "umount"];

/// Runs the raw-process detector over a whole file.
pub(super) fn detect(source: &str, root: Node<'_>, hits: &mut Vec<Hit>) {
    let mut names = Names::default();
    collect_imports(source, root, &mut names);
    walk(source, root, &names, hits);
}

/// The names this file binds to a process `Command`.
#[derive(Default)]
struct Names {
    /// Whether a bare `Command` is the process one.
    bare: bool,
    /// The aliases a `use .. as ..` bound to it.
    aliases: Vec<String>,
}

/// The crate whose `process::Command` is the owner every finding names: its
/// start holds the child's group on the lifeline (#1164, ADR 0092).
const OWNER_CRATE: &str = "lgwks_std";

/// Whether `segments` name a raw process `Command` by path: they end
/// `process::Command`, and start neither in this crate nor in the owner's.
fn is_process_command(segments: &[String]) -> bool {
    let ends =
        matches!(*segments, [.., ref before, ref last] if before == "process" && last == "Command");
    let local = segments
        .first()
        .is_some_and(|first| matches!(first.as_str(), "crate" | "self" | "super" | OWNER_CRATE));
    ends && !local
}

/// Records every `use` that binds a process `Command`, at any depth.
fn collect_imports(source: &str, node: Node<'_>, names: &mut Names) {
    if node.kind() == "use_declaration" {
        if let Some(argument) = node.by_field("argument") {
            import(source, argument, &[], names);
        }
        return;
    }
    for child in syntax::named_kids(node) {
        collect_imports(source, child, names);
    }
}

/// Reads one use tree under `prefix`, the path its enclosing groups wrote.
fn import(source: &str, node: Node<'_>, prefix: &[String], names: &mut Names) {
    let joined = |path: Option<Path>| -> Vec<String> {
        let mut full = prefix.to_vec();
        if let Some(path) = path {
            full.extend(path.segments);
        }
        full
    };
    match node.kind() {
        "identifier" | "scoped_identifier" => {
            if is_process_command(&joined(Path::of(source, node))) {
                names.bare = true;
            }
        }
        "use_as_clause" => {
            let path = node
                .by_field("path")
                .and_then(|path| Path::of(source, path));
            if is_process_command(&joined(path))
                && let Some(alias) = node.by_field("alias")
            {
                names
                    .aliases
                    .push(syntax::unraw(syntax::text(source, alias)).to_owned());
            }
        }
        "use_wildcard" => {
            let inner = syntax::named_kids(node).into_iter().next();
            let mut full = joined(inner.and_then(|path| Path::of(source, path)));
            full.push("Command".to_owned());
            if is_process_command(&full) {
                names.bare = true;
            }
        }
        "scoped_use_list" => {
            let full = joined(
                node.by_field("path")
                    .and_then(|path| Path::of(source, path)),
            );
            if let Some(list) = node.by_field("list") {
                import(source, list, &full, names);
            }
        }
        "use_list" => {
            for child in syntax::named_kids(node) {
                import(source, child, prefix, names);
            }
        }
        _ => {}
    }
}

/// Whether a call of `new` on `owner` starts a raw process in this file.
fn is_raw(owner: &[String], leading: bool, names: &Names) -> bool {
    if is_process_command(owner) {
        return true;
    }
    match *owner {
        [ref only] if !leading => {
            (only == "Command" && names.bare) || names.aliases.iter().any(|alias| alias == only)
        }
        _ => false,
    }
}

/// The finding for a raw start of `program` at `line`, spelled `spelled`.
fn finding(line: usize, spelled: &str, program: Option<&str>) -> Hit {
    let disk = program
        .and_then(|name| name.rsplit('/').next())
        .is_some_and(|base| DISK_TOOLS.contains(&base));
    let device = if disk {
        "; a disk it attaches is held with `lgwks_std::process::hold_device` the moment it is named"
    } else {
        ""
    };
    Hit::new(
        RULE,
        line,
        format!(
            "`{spelled}` starts a process nothing owns once this code stops running, so a SIGKILL \
             leaves it alive (#1160); build it with `lgwks_std::process::Command`, whose start \
             holds its group on the lifeline and kills it at every end, or hand a `ProcessSpec` \
             to the `lgwks_bot` `Supervisor` for a deadline and a temp dir{device}"
        ),
    )
}

/// Visits every node, reporting each raw start.
fn walk(source: &str, node: Node<'_>, names: &Names, hits: &mut Vec<Hit>) {
    match node.kind() {
        "call_expression" => call(source, node, names, hits),
        "macro_invocation" => {
            if let Some(tree) = syntax::macro_tokens(node) {
                macro_calls(source, tree, names, hits);
            }
        }
        _ => {}
    }
    for child in syntax::named_kids(node) {
        walk(source, child, names, hits);
    }
}

/// Reports `node` when it is a raw `new` call.
fn call(source: &str, node: Node<'_>, names: &Names, hits: &mut Vec<Hit>) {
    let Some(function) = node.by_field("function") else {
        return;
    };
    let Some(path) = Path::of(source, function) else {
        return;
    };
    let Some((last, owner)) = path.segments.split_last() else {
        return;
    };
    if last != "new" || !is_raw(owner, path.leading, names) {
        return;
    }
    let program = node
        .by_field("arguments")
        .and_then(|arguments| syntax::named_kids(arguments).into_iter().next())
        .and_then(|first| syntax::string_value(source, first));
    hits.push(finding(
        syntax::line(node),
        &path.render(),
        program.as_deref(),
    ));
}

/// Reports each raw `new` call written in the token tree `tree`, nested groups
/// included: a path of identifiers joined by `::`, ending `new`, then a `(..)`.
fn macro_calls(source: &str, tree: Node<'_>, names: &Names, hits: &mut Vec<Hit>) {
    let children: Vec<Node<'_>> = syntax::kids(tree)
        .into_iter()
        .filter(|child| !child.is_extra())
        .collect();
    for (index, child) in children.iter().enumerate() {
        if child.kind() == "token_tree" {
            macro_calls(source, *child, names, hits);
            continue;
        }
        if syntax::text(source, *child) != "new" {
            continue;
        }
        let Some(arguments) = children.get(index.saturating_add(1)) else {
            continue;
        };
        if arguments.kind() != "token_tree" || !syntax::text(source, *arguments).starts_with('(') {
            continue;
        }
        let mut segments = Vec::new();
        let mut leading = false;
        let mut first = *child;
        let mut at = index;
        while let Some(before) = at.checked_sub(1).and_then(|back| children.get(back)) {
            if syntax::text(source, *before) != "::" {
                break;
            }
            let ident = at.checked_sub(2).and_then(|back| children.get(back));
            match ident {
                Some(name) if name.kind() == "identifier" || name.kind() == "self" => {
                    segments.push(syntax::unraw(syntax::text(source, *name)).to_owned());
                    first = *name;
                    at = at.saturating_sub(2);
                }
                _ => {
                    leading = true;
                    break;
                }
            }
        }
        segments.reverse();
        if segments.is_empty() || !is_raw(&segments, leading, names) {
            continue;
        }
        let program = syntax::named_kids(*arguments)
            .into_iter()
            .next()
            .and_then(|literal| syntax::string_value(source, literal));
        let spelled = format!("{}::new", segments.join("::"));
        hits.push(finding(syntax::line(first), &spelled, program.as_deref()));
    }
}
