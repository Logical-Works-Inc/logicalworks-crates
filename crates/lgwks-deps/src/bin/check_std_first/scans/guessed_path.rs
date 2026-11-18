//! A binary path guessed from the running binary's own (ADR 0066,
//! INV-CI-BRITTLE-1). The path `std::env::current_exe` returns, navigated with
//! `parent`, `with_file_name`, `pop` or `join` in its statement or through the
//! name it is bound to, is a guess at cargo's target layout, and the build-dir
//! layout moves test binaries away from the bins. A test that re-runs its own
//! binary navigates nothing and is not a finding.

use super::{after_space, is_word, tail, word_before};

/// Why a binary path navigated from the running binary is a finding.
pub(super) const WHY_GUESSED_PATH: &str = "guesses another binary's path from the running one, \
                                           and cargo's build-dir layout moves it; read \
                                           `NEXTEST_BIN_EXE_<name>` or `CARGO_BIN_EXE_<name>`, \
                                           see ADR 0066 and INV-CI-BRITTLE-1";

/// The running binary's path call, spelled in two pieces so this file does
/// not contain the pattern it looks for.
const EXE_CALL: &str = concat!("current", "_exe");

/// The navigations that turn the running binary's path into a guess at another's.
const NAVIGATIONS: [&str; 4] = [".parent(", ".with_file_name(", ".pop(", ".join("];

/// Whether `text`, from its first non-space character, is a navigation.
fn navigates(text: &str) -> bool {
    let rest = after_space(text);
    NAVIGATIONS.iter().any(|step| rest.starts_with(step))
}

/// `text` up to the first `marker`, or all of it when there is none.
fn up_to<'text>(text: &'text str, marker: &str) -> &'text str {
    match text.split_once(marker) {
        Some((head, _)) => head,
        None => text,
    }
}

/// Whether the value the call yields is navigated in its own chain: `rest`
/// opens with the call's arguments, and the chain ends at a closer the chain
/// did not open (the call sat inside another one's arguments) or at a `;` or
/// `,` outside every bracket. A navigation inside a chained method's closure,
/// `.map(|exe| exe.parent())`, is still the value's.
fn chained(rest: &str) -> bool {
    let mut depth = 0_usize;
    for (at, ch) in rest.char_indices() {
        match ch {
            '(' | '[' | '{' => depth = depth.saturating_add(1),
            ')' | ']' | '}' if depth == 0 => return false,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            ';' | ',' if depth == 0 => return false,
            '.' if navigates(tail(rest, at)) => return true,
            _ => {}
        }
    }
    false
}

/// The statement holding byte `index`, from its start up to `index`.
fn statement_head(text: &str, index: usize) -> &str {
    let Some(head) = text.get(..index) else {
        return "";
    };
    match head.rfind([';', '{', '}']) {
        Some(at) => tail(head, at.saturating_add(1)),
        None => head,
    }
}

/// The name a `let` binds when the statement holding byte `index` is one.
fn bound_name(text: &str, index: usize) -> Option<&str> {
    let statement = after_space(after_space(statement_head(text, index)).strip_prefix("let ")?);
    let statement = match statement.strip_prefix("mut ") {
        Some(rest) => after_space(rest),
        None => statement,
    };
    let end = match statement.find(|ch: char| !is_word(ch)) {
        Some(end) => end,
        None => statement.len(),
    };
    statement.get(..end).filter(|name| !name.is_empty())
}

/// The offsets of the running binary's path navigated to guess another's: a
/// navigation in the value's own chain, or one on the name the call is bound
/// to before the item ends at the next line-leading `}`.
pub(super) fn guessed_path_offsets(text: &str) -> Vec<usize> {
    let mut found = Vec::new();
    for (index, _) in text.match_indices(EXE_CALL) {
        let rest = tail(text, index.saturating_add(EXE_CALL.len()));
        if word_before(text, index) || !after_space(rest).starts_with('(') {
            continue;
        }
        let item = up_to(rest, "\n}");
        let bound = bound_name(text, index).is_some_and(|name| {
            item.match_indices(name).any(|(at, _)| {
                !word_before(item, at) && navigates(tail(item, at.saturating_add(name.len())))
            })
        });
        if chained(rest) || bound {
            found.push(index);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::{bound_name, chained, guessed_path_offsets};

    /// ADR 0066: each navigation, chained or on the binding, is found at the
    /// call; a chain that ends, a binding never navigated and a longer word are
    /// not. Each form splits the call across a `concat!` so this file does not
    /// contain what the audit looks for.
    #[test]
    fn each_navigation_is_found_at_the_call_and_nothing_else_is() {
        let call = concat!("current", "_exe");
        for step in ["parent()", "with_file_name(\"x\")", "pop()", "join(\"x\")"] {
            let text = format!("let p = {call}()?.{step};");
            assert_eq!(guessed_path_offsets(&text), vec![8], "{text:?}");
            let text = format!("fn f() {{\n    let p = {call}()?;\n    p\n        .{step};\n}}\n");
            assert_eq!(guessed_path_offsets(&text).len(), 1, "{text:?}");
        }
        for text in [
            format!("Command::new({call}()?).arg(dir.join(\"x\"));"),
            format!(
                "fn f() {{\n    let p = {call}()?;\n    run(&p);\n}}\nfn g() {{ p.parent(); }}"
            ),
            format!("let q = my_{call}().parent();"),
            format!("let r = {call};"),
        ] {
            assert_eq!(guessed_path_offsets(&text), Vec::<usize>::new(), "{text:?}");
        }
    }

    /// The chain ends at a closer it did not open and at a top-level `;` or
    /// `,`; inside its own brackets it goes on.
    #[test]
    fn the_chain_ends_where_the_value_does() {
        assert!(chained("()?.map(|exe| exe.parent())"));
        assert!(!chained("()?).parent()"));
        assert!(!chained("()?; p.parent()"));
        assert!(!chained("()?, other.parent()"));
        assert!(!chained("()?.as_os_str()"));
    }

    /// The binding is the `let` of the call's own statement, with or without
    /// `mut`, and nothing when the statement binds no name.
    #[test]
    fn the_bound_name_is_the_statements_let() {
        let text = "x; let mut path_1 = CALL";
        assert_eq!(bound_name(text, text.len()), Some("path_1"));
        let text = "{ let p = CALL";
        assert_eq!(bound_name(text, text.len()), Some("p"));
        let text = "run(CALL";
        assert_eq!(bound_name(text, text.len()), None);
        let text = "let = CALL";
        assert_eq!(bound_name(text, text.len()), None);
    }
}
