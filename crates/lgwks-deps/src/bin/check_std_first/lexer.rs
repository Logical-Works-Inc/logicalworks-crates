//! Reading Rust source as code: the lexer and `use`-tree reader behind the
//! std-first audit.
//!
//! This module owns *reading* Rust; the audit owns *judging* what it reads.
//! Nothing here knows about the register, the workspace, or a finding.

use std::collections::BTreeSet;

/// One `use` or `extern crate` statement found in code.
pub(super) struct Statement {
    /// The byte offset where the statement starts.
    pub(super) start: usize,
    /// What the statement says.
    pub(super) kind: Kind,
}

/// The two statement forms the audit reads.
pub(super) enum Kind {
    /// `use <body>;`, with the whole path tree as the body.
    Use {
        /// The text between `use` and `;`.
        body: String,
    },
    /// `extern crate <root>;`.
    Extern {
        /// The crate named.
        root: String,
    },
}

/// The one lexer the audit and the unwrap lane share (`shared/rust_code.rs`).
pub(super) use super::rust_code::{code_only, is_word};

/// The text, or empty when there is none.
fn or_empty(text: Option<&str>) -> &str {
    match text {
        Some(found) if !found.is_empty() => found,
        Some(_) | None => "",
    }
}

/// Splits a `use` group body on the commas outside nested braces.
fn split_top_level(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0_i64;
    let mut current = String::new();
    for ch in text.chars() {
        if ch == '{' {
            depth = depth.saturating_add(1);
        } else if ch == '}' {
            depth = depth.saturating_sub(1);
        }
        if ch == ',' && depth == 0 {
            parts.push(std::mem::take(&mut current));
        } else {
            current.push(ch);
        }
    }
    parts.push(current);
    parts
}

/// The crate roots a `use` specification reaches, and the names it binds.
///
/// `use lgwks_std::json::serde;` reaches `lgwks_std` and binds `serde`.
/// `use std::collections::{BTreeMap, BTreeSet};` reaches `std` and binds both.
/// `use a::b::{self, c};` binds `b` as well as `c`, which is the arm a
/// first-segment-only reading gets wrong.
pub(super) fn use_tree(body: &str, prefix: &str) -> (BTreeSet<String>, BTreeSet<String>) {
    let body = body.trim();
    let mut roots = BTreeSet::new();
    let mut names = BTreeSet::new();
    if body.is_empty() {
        return (roots, names);
    }
    if let Some(brace) = body.find('{')
        && body.ends_with('}')
    {
        let head_text = or_empty(body.get(..brace)).trim_end_matches(':').trim();
        // The prefix is not decoration: in `use lgwks_deps::bevy_ecs::{
        // prelude::{…} }` the inner group's head is `prelude` only until the
        // outer path is put back in front of it.
        let head = if prefix.is_empty() {
            head_text.to_owned()
        } else if head_text.is_empty() {
            prefix.to_owned()
        } else {
            format!("{prefix}::{head_text}")
        };
        let inner = or_empty(body.get(brace.saturating_add(1)..body.len().saturating_sub(1)));
        for part in split_top_level(inner) {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            if part == "self" {
                if !head.is_empty() {
                    roots.insert(or_empty(head.split("::").next()).to_owned());
                    names.insert(or_empty(head.rsplit("::").next()).to_owned());
                }
                continue;
            }
            let (sub_roots, sub_names) = use_tree(part, &head);
            roots.extend(sub_roots);
            names.extend(sub_names);
        }
        return (roots, names);
    }
    let joined = if prefix.is_empty() {
        body.to_owned()
    } else {
        format!("{prefix}::{body}")
    };
    let path = joined.trim_start_matches(':');
    if path.ends_with("::*") || path == "*" {
        let root = or_empty(path.split("::").next());
        if !root.is_empty() {
            roots.insert(root.to_owned());
        }
        return (roots, names);
    }
    let (path, name) = match path.rsplit_once(" as ") {
        Some((target, alias)) => (target.trim().to_owned(), alias.trim().to_owned()),
        None => (
            path.to_owned(),
            or_empty(path.rsplit("::").next()).trim().to_owned(),
        ),
    };
    let root = or_empty(path.split("::").next()).trim();
    if !root.is_empty() {
        roots.insert(root.to_owned());
    }
    if !name.is_empty() {
        names.insert(name);
    }
    (roots, names)
}

/// The byte offsets at which a line starts, including the first.
fn line_starts(code: &str) -> Vec<usize> {
    let mut starts = vec![0_usize];
    for (index, ch) in code.char_indices() {
        if ch == '\n' {
            starts.push(index.saturating_add(1));
        }
    }
    starts
}

/// The `use` statement beginning at `start`, as its body and the offset after its `;`.
fn use_at(code: &str, start: usize) -> Option<(String, usize)> {
    let rest = code.get(start..)?;
    let mut rest = rest.trim_start_matches([' ', '\t']);
    if let Some(after_pub) = rest.strip_prefix("pub") {
        let after_paren = match after_pub.strip_prefix('(') {
            Some(inside) => inside.split_once(')')?.1,
            None => after_pub,
        };
        let spaced = after_paren.trim_start_matches([' ', '\t']);
        if spaced.len() == after_paren.len() {
            return None;
        }
        rest = spaced;
    }
    let rest = rest.strip_prefix("use")?;
    let spaced = rest.trim_start_matches([' ', '\t']);
    if spaced.len() == rest.len() {
        return None;
    }
    let (body, _) = spaced.split_once(';')?;
    let consumed = code
        .len()
        .saturating_sub(spaced.len())
        .saturating_add(body.len());
    Some((body.to_owned(), consumed.saturating_add(1)))
}

/// The `extern crate <root>` statement beginning at `start`.
fn extern_at(code: &str, start: usize) -> Option<String> {
    let rest = code.get(start..)?.trim_start_matches([' ', '\t']);
    let rest = rest.strip_prefix("extern")?;
    let spaced = rest.trim_start_matches([' ', '\t']);
    if spaced.len() == rest.len() {
        return None;
    }
    let rest = spaced.strip_prefix("crate")?;
    let spaced = rest.trim_start_matches([' ', '\t']);
    if spaced.len() == rest.len() {
        return None;
    }
    let first = spaced.chars().next()?;
    if !(first.is_ascii_alphabetic() || first == '_') {
        return None;
    }
    let root: String = spaced
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
        .collect();
    Some(root)
}

/// Every `use` statement, then every `extern crate`, in `code`.
///
/// The whole statement is captured rather than only its first segment: what a
/// statement binds is what a later statement may resolve through.
pub(super) fn statements(code: &str) -> Vec<Statement> {
    let starts = line_starts(code);
    let mut found = Vec::new();
    let mut floor = 0_usize;
    for &start in &starts {
        if start < floor {
            continue;
        }
        if let Some((body, end)) = use_at(code, start) {
            found.push(Statement {
                start,
                kind: Kind::Use { body },
            });
            floor = end;
        }
    }
    for &start in &starts {
        if let Some(root) = extern_at(code, start) {
            found.push(Statement {
                start,
                kind: Kind::Extern { root },
            });
        }
    }
    found
}

/// Every module name declared in `code`, in either form: `mod cap;` and `mod cap { … }`.
pub(super) fn module_names(code: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for (index, _) in code.match_indices("mod") {
        let before = code.get(..index).and_then(|head| head.chars().next_back());
        if before.is_some_and(is_word) {
            continue;
        }
        let Some(rest) = code.get(index.saturating_add(3)..) else {
            continue;
        };
        let spaced = rest.trim_start();
        if spaced.len() == rest.len() {
            continue;
        }
        let Some(first) = spaced.chars().next() else {
            continue;
        };
        if !(first.is_ascii_alphabetic() || first == '_') {
            continue;
        }
        let name: String = spaced
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
            .collect();
        let tail = or_empty(spaced.get(name.len()..)).trim_start();
        if tail.starts_with([';', '{']) {
            names.insert(name);
        }
    }
    names
}

/// The 1-based line number an offset falls on.
pub(super) fn line_of(text: &str, offset: usize) -> usize {
    let before = match text.get(..offset) {
        Some(head) => head.matches('\n').count(),
        None => 0,
    };
    before.saturating_add(1)
}

/// The offsets of `name::` in `code` not preceded by a path or word character.
pub(super) fn path_reaches(code: &str, name: &str) -> Vec<usize> {
    let needle = format!("{name}::");
    code.match_indices(&needle)
        .filter(|&(index, _)| {
            let before = code.get(..index).and_then(|head| head.chars().next_back());
            !before.is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == ':')
        })
        .map(|(index, _)| index)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{Kind, code_only, module_names, path_reaches, statements, use_tree};

    /// The bodies of the `use` statements in `source`, as the audit sees them.
    fn scan_uses(source: &str) -> Vec<String> {
        statements(&code_only(source))
            .into_iter()
            .filter_map(|statement| match statement.kind {
                Kind::Use { body } => Some(body),
                Kind::Extern { .. } => None,
            })
            .collect()
    }

    #[test]
    fn a_use_line_inside_a_multiline_string_is_not_a_statement() {
        // The refuse.rs shape that put prose into the crate list (#195).
        let source = "let m = \"flows stop here: \\\n    use `?` or `.or_fail()?` (permanent)\";\nuse std::fmt;\n";
        assert_eq!(
            scan_uses(source),
            vec!["std::fmt"],
            "only the real statement"
        );
    }

    #[test]
    fn raw_strings_comments_and_char_literals_hide_code_like_text() {
        let source = "let a = r#\"use serde::Value; \"quoted\" tokio::spawn\"#;\n\
                      /* use regex::Regex; /* nested */ still comment */\n\
                      let q = '\"'; let e = '\\''; // tokio::spawn\n\
                      use lgwks_std::json;\n";
        let code = code_only(source);
        assert_eq!(
            scan_uses(source),
            vec!["lgwks_std::json"],
            "only the real statement"
        );
        assert!(
            !code.contains("tokio::"),
            "the comment and the raw string are blanked"
        );
        assert!(
            !code.contains("regex::"),
            "the nested block comment is blanked"
        );
    }

    #[test]
    fn lifetimes_stay_code_and_offsets_are_preserved() {
        let source = "fn f<'a>(x: &'a str) -> &'a str { serde_json::from_str(x) }\n\"\n\"";
        let code = code_only(source);
        assert_eq!(code.len(), source.len(), "offsets are preserved");
        assert_eq!(
            code.matches('\n').count(),
            source.matches('\n').count(),
            "newlines are kept"
        );
        assert!(code.contains("serde_json::"), "code stays code");
        assert!(code.contains("'a"), "a lifetime is not a literal");
    }

    #[test]
    fn a_use_tree_reaches_its_roots_and_binds_its_names() {
        let (roots, names) = use_tree("std::collections::{BTreeMap, BTreeSet}", "");
        assert_eq!(
            roots.into_iter().collect::<Vec<_>>(),
            vec!["std"],
            "one root"
        );
        assert_eq!(
            names.into_iter().collect::<Vec<_>>(),
            vec!["BTreeMap", "BTreeSet"],
            "both names bound"
        );
        let (roots, names) = use_tree("a::b::{self, c as d}", "");
        assert_eq!(
            roots.into_iter().collect::<Vec<_>>(),
            vec!["a"],
            "self keeps the root"
        );
        assert_eq!(
            names.into_iter().collect::<Vec<_>>(),
            vec!["b", "d"],
            "self binds its parent"
        );
    }

    #[test]
    fn modules_and_path_reaches_are_found_at_word_boundaries() {
        let code =
            "mod cap;\npub mod wire { }\nlet x = amod y;\nserde::a(); lgwks_deps::tokio::b();";
        assert_eq!(
            module_names(code).into_iter().collect::<Vec<_>>(),
            vec!["cap", "wire"],
            "declared modules only"
        );
        assert_eq!(
            path_reaches(code, "serde").len(),
            1,
            "a bare path is a reach"
        );
        assert!(
            path_reaches(code, "tokio").is_empty(),
            "a path through the storefront is not"
        );
    }
}
