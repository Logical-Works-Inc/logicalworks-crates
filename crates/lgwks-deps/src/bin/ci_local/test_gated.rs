//! Which lines of a Rust source are compiled only under `cfg(test)`, and so
//! which lines of it the unwrap lane may exempt.
//!
//! The lane once read "the first line that contains `mod tests`" as the start of
//! the test code, so a comment or a string that spelled those words exempted
//! every line below it, and a real test module placed first exempted the
//! shipped code after it (#911 item 9b). A line is test code here only when an
//! item gated on `cfg(test)` spans it, read from the code with its comments and
//! literals blanked by the one lexer (`shared/rust_code.rs`).

use super::rust_code::{attributes, code_only, gates_on_test, item_end};

/// The call the lane bans in shipped code, spelled in two pieces so this file
/// does not contain the text its own scan looks for.
const FORBIDDEN_CALL: &str = concat!(".unw", "rap()");

/// The inclusive, 1-based line ranges of the items `source` gates on
/// `cfg(test)`. A `#![cfg(test)]` outside every block gates the whole file,
/// `(1, usize::MAX)`; one inside a block gates that block only, which is not
/// read here, so it exempts nothing.
fn test_gated_lines(source: &str) -> Vec<(usize, usize)> {
    let code: Vec<char> = code_only(source).chars().collect();
    let line_at = |index: usize| {
        code.iter()
            .take(index)
            .filter(|&&ch| ch == '\n')
            .count()
            .saturating_add(1)
    };
    let block_depth_at = |index: usize| {
        let opened = code.iter().take(index).filter(|&&ch| ch == '{').count();
        let closed = code.iter().take(index).filter(|&&ch| ch == '}').count();
        opened.saturating_sub(closed)
    };
    let mut spans = Vec::new();
    for attribute in attributes(&code) {
        if !gates_on_test(&attribute.body) {
            continue;
        }
        if attribute.inner {
            if block_depth_at(attribute.start) == 0 {
                spans.push((1, usize::MAX));
            }
            continue;
        }
        let last = item_end(&code, attribute.end).saturating_sub(1);
        spans.push((line_at(attribute.start), line_at(last)));
    }
    spans
}

/// The 1-based numbers of the lines of `source` that hold the banned call and
/// that no `cfg(test)` item spans. Doc comments count: a doc example is shipped
/// surface.
pub(super) fn unwrap_lines(source: &str) -> Vec<usize> {
    let spans = test_gated_lines(source);
    source
        .lines()
        .zip(1_usize..)
        .filter(|&(line, number)| {
            line.contains(FORBIDDEN_CALL)
                && !spans
                    .iter()
                    .any(|&(first, last)| (first..=last).contains(&number))
        })
        .map(|(_, number)| number)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::unwrap_lines;

    /// The lines of the source made of `lines` that the lane refuses; every `@`
    /// stands for the banned call, so this file never spells it.
    fn refused(lines: &[&str]) -> Vec<usize> {
        unwrap_lines(&lines.join("\n").replace('@', super::FORBIDDEN_CALL))
    }

    /// #911 item 9b, the control: a real `#[cfg(test)] mod tests` exempts what
    /// it holds, and shipped code before it is still refused.
    #[test]
    fn a_real_test_module_exempts_its_own_lines() {
        let found = refused(&[
            "fn shipped() { x@; }",
            "#[cfg(test)]",
            "mod tests {",
            "    fn t() { y@; }",
            "}",
        ]);
        assert_eq!(found, vec![1]);
    }

    /// #911 item 9b: the scan stopped at the first line containing `mod tests`,
    /// a comment included, so every call below it went unseen.
    #[test]
    fn a_comment_that_says_mod_tests_exempts_nothing() {
        let found = refused(&["// mod tests", "fn shipped() { x@; }"]);
        assert_eq!(found, vec![2]);
        let block = refused(&["/* #[cfg(test)] mod tests { */", "fn shipped() { x@; }"]);
        assert_eq!(block, vec![2]);
    }

    /// #911 item 9b: nor does a string that spells the module.
    #[test]
    fn a_string_that_spells_a_test_module_exempts_nothing() {
        let found = refused(&[
            "const NOTE: &str = \"#[cfg(test)] mod tests {\";",
            "fn shipped() { x@; }",
            "const RAW: &str = r#\"mod tests\"#;",
            "fn more() { y@; }",
        ]);
        assert_eq!(found, vec![2, 4]);
    }

    /// #911 item 9b: the exemption is the module's body, not the rest of the
    /// file, so a test module first does not hide the code after it.
    #[test]
    fn shipped_code_after_a_test_module_is_still_refused() {
        let found = refused(&[
            "#[cfg(test)]",
            "mod tests {",
            "    fn t() { let s = \"}\"; y@; }",
            "}",
            "fn shipped() { x@; }",
        ]);
        assert_eq!(found, vec![5]);
    }

    /// #911 item 9b: a module gated on `test` among other conditions is test
    /// code; one gated on `not(test)` is shipped, whatever it is called.
    #[test]
    fn only_a_gate_on_test_exempts() {
        let found = refused(&[
            "#[cfg(all(test, unix))]",
            "mod unix_tests {",
            "    fn t() { y@; }",
            "}",
            "#[cfg(not(test))]",
            "mod tests {",
            "    fn shipped() { x@; }",
            "}",
            "#[cfg(any(test, unix))]",
            "mod either {",
            "    fn shipped() { z@; }",
            "}",
        ]);
        assert_eq!(found, vec![7, 11]);
    }

    /// #911 item 9b: a test-gated function, impl or `use` group is test code
    /// too, with its attributes and brackets read through.
    #[test]
    fn a_test_gated_item_of_any_kind_is_exempt() {
        let found = refused(&[
            "#[cfg(test)]",
            "#[allow(dead_code)]",
            "fn helper() -> [u8; 2] { [1; 2] @; }",
            "#[cfg(test)]",
            "impl Thing {",
            "    fn t(&self) { y@; }",
            "}",
            "#[cfg(test)]",
            "use a::{b, c};",
            "fn shipped() { x@; }",
        ]);
        assert_eq!(found, vec![10]);
    }

    /// #911 item 9b: `#![cfg(test)]` at the top gates the file; the same line
    /// inside a block gates that block, so it must not exempt the rest.
    #[test]
    fn an_inner_attribute_exempts_a_file_only_outside_every_block() {
        let whole = refused(&["#![cfg(test)]", "fn t() { y@; }"]);
        assert_eq!(whole, Vec::<usize>::new());
        let nested = refused(&[
            "mod empty {",
            "    #![cfg(test)]",
            "}",
            "fn shipped() { x@; }",
        ]);
        assert_eq!(nested, vec![4]);
    }

    /// #911 item 9b: a doc example is shipped surface, so a call in a doc
    /// comment is refused outside test code.
    #[test]
    fn a_call_in_a_doc_comment_is_refused() {
        let found = refused(&["/// let v = x@;", "pub fn shipped() {}"]);
        assert_eq!(found, vec![1]);
    }
}
