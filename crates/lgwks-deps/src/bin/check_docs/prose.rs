//! The reader-facing rules: every page a person or a small model reads is held
//! to the shape the Tokio guides use, so the first paragraph says what the page
//! is for, every example names its language, every link lands, and no sentence
//! or paragraph is longer than a reader holds at once.
//!
//! Every rule is mechanical, with no model call, so the same tree always gives
//! the same verdict. The pages are the root `*.md` files, every `*.md` under
//! `docs/` and each crate's `README.md`. Generated pages are left to their
//! generator: `docs/script-lexicon*.md` (`lgwks_macros::lexicon_docs`) and
//! `docs/catalog.md` (`catalog --check`).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The most words one sentence may hold. Plain-language guidance (the US
/// Federal Plain Language Guidelines, the GOV.UK style guide) puts the readable
/// average near 20 and treats 25 as long, so a sentence past this is two.
pub(crate) const MAX_SENTENCE_WORDS: usize = 45;

/// The most sentences one paragraph or list item may hold.
pub(crate) const MAX_PARAGRAPH_SENTENCES: usize = 8;

/// How deep the walk under `docs/` goes; a tree deeper than this is refused.
const MAX_DEPTH: usize = 8;

/// One page: its path relative to the root, and its whole text.
pub(crate) struct Page {
    /// The path relative to the root, with `/` separators.
    pub(crate) name: String,
    /// The whole file.
    pub(crate) text: String,
}

/// Whether a relative path names a page the generator owns.
fn generated(name: &str) -> bool {
    name.starts_with("docs/script-lexicon") || name == "docs/catalog.md"
}

/// Every `*.md` under `directory`, at most `depth` directories down.
fn markdown_under(directory: &Path, depth: usize, out: &mut Vec<PathBuf>) -> io::Result<()> {
    if depth > MAX_DEPTH {
        lgwks_std::trace::warn!(directory = %directory.display(), depth, "docs tree past its depth bound");
        return Err(io::Error::other(format!(
            "{} is more than {MAX_DEPTH} directories deep",
            directory.display()
        )));
    }
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            markdown_under(&path, depth.saturating_add(1), out)?;
        } else if path.extension().is_some_and(|ext| ext == "md") {
            out.push(path);
        }
    }
    Ok(())
}

/// The reader-facing pages under `root`, sorted by name.
pub(crate) fn pages(root: &Path) -> io::Result<Vec<Page>> {
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if path.is_file() && path.extension().is_some_and(|ext| ext == "md") {
            paths.push(path);
        }
    }
    let docs = root.join("docs");
    if docs.is_dir() {
        markdown_under(&docs, 0, &mut paths)?;
    }
    let crates = root.join("crates");
    if crates.is_dir() {
        for entry in fs::read_dir(&crates)? {
            let readme = entry?.path().join("README.md");
            if readme.is_file() {
                paths.push(readme);
            }
        }
    }
    let mut found = Vec::new();
    for path in paths {
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let name = relative.to_string_lossy().replace('\\', "/");
        if generated(&name) {
            continue;
        }
        let text = fs::read_to_string(&path)?;
        found.push(Page { name, text });
    }
    found.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(found)
}

/// Which lines of a page the length rules judge. The shape rules judge every
/// line of a judged page; the length rules judge prose a change wrote, so a
/// page's history is not rewritten to land a new paragraph beside it.
#[derive(Clone, Copy)]
pub(crate) enum Judged<'a> {
    /// Every line: the bare `check-docs` audit.
    Whole,
    /// Only these inclusive 1-based line ranges, the lines added since a base.
    Added(&'a [(usize, usize)]),
}

impl Judged<'_> {
    /// Whether any line from `first` to `last` is judged.
    fn covers(self, first: usize, last: usize) -> bool {
        match self {
            Self::Whole => true,
            Self::Added(ranges) => ranges
                .iter()
                .any(|&(start, end)| start <= last && first <= end),
        }
    }
}

/// A line's text with inline code, link targets and emphasis taken out, so a
/// path or a URL is not read as words. Inline code counts as one word.
fn plain(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut in_code = false;
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '`' {
            in_code = !in_code;
            if in_code {
                out.push_str("code");
            }
            continue;
        }
        if in_code {
            continue;
        }
        if ch == ']' && chars.peek() == Some(&'(') {
            let mut depth = 0_usize;
            for next in chars.by_ref() {
                if next == '(' {
                    depth = depth.saturating_add(1);
                } else if next == ')' {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        break;
                    }
                }
            }
            continue;
        }
        if matches!(ch, '[' | '*' | '_') {
            continue;
        }
        out.push(ch);
    }
    out
}

/// Whether a word ending in `.` is an abbreviation rather than a sentence end.
fn abbreviation(word: &str) -> bool {
    let lowered = word.to_lowercase();
    ["e.g.", "i.e.", "etc.", "vs.", "cf.", "no.", "approx."]
        .iter()
        .any(|short| lowered.ends_with(short))
}

/// The sentences of one paragraph: split after `.`, `?` or `!` where the next
/// word opens with a capital, a digit or a quote.
fn sentences(paragraph: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut current = String::new();
    let words: Vec<&str> = paragraph.split_whitespace().collect();
    for (index, word) in words.iter().enumerate() {
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
        let ends = word.ends_with(['.', '?', '!', ':']) && !abbreviation(word);
        let next_opens = words.get(index.saturating_add(1)).is_none_or(|next| {
            next.chars()
                .next()
                .is_some_and(|ch| ch.is_uppercase() || ch.is_ascii_digit() || ch == '"')
        });
        if ends && next_opens {
            found.push(std::mem::take(&mut current));
        }
    }
    if !current.trim().is_empty() {
        found.push(current);
    }
    found
}

/// What a line of Markdown is, for the rules below.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A line inside or opening a fenced block.
    Fence,
    /// A heading.
    Heading,
    /// A table row.
    Table,
    /// A quoted line.
    Quote,
    /// A list item's first line.
    Item,
    /// A blank line.
    Blank,
    /// Any other line: prose.
    Prose,
}

/// Whether a trimmed line opens a list item.
fn item(trimmed: &str) -> bool {
    trimmed.starts_with("- ")
        || trimmed.starts_with("* ")
        || trimmed.split_once(". ").is_some_and(|(number, _)| {
            !number.is_empty() && number.chars().all(|ch| ch.is_ascii_digit())
        })
}

/// Whether a trimmed line is an ATX heading: one to six `#`, then a space, a tab
/// or the end of the line (CommonMark 0.31 §4.2). A wrapped `#108) was closed`
/// is prose.
fn heading(trimmed: &str) -> bool {
    let body = trimmed.trim_start_matches('#');
    let depth = trimmed.len().saturating_sub(body.len());
    (1..=6).contains(&depth) && (body.is_empty() || body.starts_with([' ', '\t']))
}

/// Classifies `line`, given whether a fence is open.
fn kind(line: &str, fenced: bool) -> Kind {
    let trimmed = line.trim_start();
    if fenced || trimmed.starts_with("```") {
        Kind::Fence
    } else if trimmed.is_empty() {
        Kind::Blank
    } else if heading(trimmed) {
        Kind::Heading
    } else if trimmed.starts_with('|') {
        Kind::Table
    } else if trimmed.starts_with('>') {
        Kind::Quote
    } else if item(trimmed) {
        Kind::Item
    } else {
        Kind::Prose
    }
}

/// One finding: the line it is on and what is wrong.
pub(crate) struct Finding {
    /// The 1-based line.
    pub(crate) line: usize,
    /// What a reader would trip on.
    pub(crate) what: String,
}

/// Judges one paragraph that runs from line `start` to line `last`, when the
/// length rules cover any of it.
fn judge_paragraph(
    text: &str,
    (start, last): (usize, usize),
    judged: Judged<'_>,
    out: &mut Vec<Finding>,
) {
    if !judged.covers(start, last) {
        return;
    }
    let sentences = sentences(&plain(text));
    if sentences.len() > MAX_PARAGRAPH_SENTENCES {
        out.push(Finding {
            line: start,
            what: format!(
                "a paragraph of {} sentences, past {MAX_PARAGRAPH_SENTENCES}: split it where the subject changes",
                sentences.len()
            ),
        });
    }
    for sentence in &sentences {
        let words = sentence.split_whitespace().count();
        if words > MAX_SENTENCE_WORDS {
            let opening = sentence
                .split_whitespace()
                .take(8)
                .collect::<Vec<_>>()
                .join(" ");
            out.push(Finding {
                line: start,
                what: format!(
                    "a sentence of {words} words, past {MAX_SENTENCE_WORDS}: \"{opening} ...\" is two sentences"
                ),
            });
        }
    }
}

/// The repository path a link target names, or `None` for a URL, an anchor or
/// a rustdoc intra-doc path such as `crate::verb::Query`.
fn local_target(target: &str) -> Option<&str> {
    if target.contains("://")
        || target.contains("::")
        || target.starts_with("mailto:")
        || target.starts_with('#')
    {
        return None;
    }
    let path = target.split('#').next()?.split_whitespace().next()?;
    if path.is_empty() { None } else { Some(path) }
}

/// The `](target)` link targets on one line.
fn link_targets(line: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut rest = line;
    while let Some(at) = rest.find("](") {
        let Some(after) = rest.get(at.saturating_add(2)..) else {
            break;
        };
        let Some(close) = after.find(')') else {
            break;
        };
        let (Some(target), Some(next)) = (after.get(..close), after.get(close..)) else {
            break;
        };
        found.push(target);
        rest = next;
    }
    found
}

/// The number of lines a YAML front-matter block (`---` to `---`) takes at the
/// top of the page, or zero when the page has none.
fn front_matter(lines: &[&str]) -> usize {
    if lines.first().is_none_or(|first| first.trim_end() != "---") {
        return 0;
    }
    match lines
        .iter()
        .skip(1)
        .position(|line| line.trim_end() == "---")
    {
        Some(close) => close.saturating_add(2),
        None => 0,
    }
}

/// The findings on the page's opening, after any front matter: a `# ` title,
/// then a sentence.
fn judge_opening(lines: &[&str], out: &mut Vec<Finding>) {
    let skipped = front_matter(lines);
    let mut body = lines
        .iter()
        .enumerate()
        .skip(skipped)
        .filter(|&(_, line)| !line.trim().is_empty());
    if !body.next().is_some_and(|(_, line)| line.starts_with("# ")) {
        out.push(Finding {
            line: skipped.saturating_add(1),
            what: "the page opens with no `# ` title".to_owned(),
        });
    }
    if let Some((index, line)) = body.next()
        && !matches!(kind(line, false), Kind::Prose | Kind::Quote)
    {
        out.push(Finding {
            line: index.saturating_add(1),
            what: "the first block after the title is not a sentence: open with what the page is for, as Tokio's guides do".to_owned(),
        });
    }
}

/// The findings for one page under `root`, the length rules over `judged`.
pub(crate) fn check_page(root: &Path, page: &Page, judged: Judged<'_>) -> Vec<Finding> {
    let mut out = Vec::new();
    let lines: Vec<&str> = page.text.lines().collect();
    judge_opening(&lines, &mut out);
    let Some(directory) = Path::new(&page.name).parent() else {
        out.push(Finding {
            line: 1,
            what: "the page has no path, so its links cannot be resolved".to_owned(),
        });
        return out;
    };
    let mut fenced = false;
    let mut level = 1_usize;
    let mut paragraph = String::new();
    let mut opened = 0_usize;
    for (index, line) in lines.iter().enumerate() {
        let number = index.saturating_add(1);
        let found = kind(line, fenced);
        let fence = line.trim_start();
        if fence.starts_with("```") {
            if !fenced && fence.trim_start_matches('`').trim().is_empty() {
                out.push(Finding {
                    line: number,
                    what:
                        "a code block names no language: write ```rust, ```sh, ```toml or ```text"
                            .to_owned(),
                });
            }
            fenced = !fenced;
        }
        let continues = found == Kind::Prose && !paragraph.is_empty();
        if !continues && !paragraph.is_empty() {
            judge_paragraph(&paragraph, (opened, index), judged, &mut out);
            paragraph.clear();
        }
        if matches!(found, Kind::Prose | Kind::Item) {
            if paragraph.is_empty() {
                opened = number;
            }
            paragraph.push(' ');
            paragraph.push_str(line.trim());
        }
        if found == Kind::Heading {
            let depth = fence.chars().take_while(|ch| *ch == '#').count();
            if depth > level.saturating_add(1) {
                out.push(Finding {
                    line: number,
                    what: format!(
                        "a level-{depth} heading under a level-{level} one skips a level"
                    ),
                });
            }
            level = depth;
        }
        if found != Kind::Fence {
            for target in link_targets(line) {
                let Some(path) = local_target(target) else {
                    continue;
                };
                if !root.join(directory).join(path).exists() {
                    out.push(Finding {
                        line: number,
                        what: format!("the link `{path}` lands on nothing"),
                    });
                }
            }
        }
    }
    if !paragraph.is_empty() {
        judge_paragraph(&paragraph, (opened, lines.len()), judged, &mut out);
    }
    out
}

/// The rules' own tests.
#[cfg(test)]
mod tests {
    use super::{Judged, MAX_SENTENCE_WORDS, Page, check_page, plain, sentences};
    use std::path::Path;

    /// Judges one page, under a root nothing is read from, over `judged`.
    fn judge_over(text: &str, judged: Judged<'_>) -> Vec<String> {
        let page = Page {
            name: "docs/x.md".to_owned(),
            text: text.to_owned(),
        };
        check_page(Path::new("/nonexistent"), &page, judged)
            .into_iter()
            .map(|finding| finding.what)
            .collect()
    }

    /// Judges every line of one page.
    fn judge(text: &str) -> Vec<String> {
        judge_over(text, Judged::Whole)
    }

    /// The length rules judge only a paragraph a change added to, while the
    /// shape rules still judge the whole page; a hunk that only deletes adds
    /// nothing.
    #[test]
    fn the_length_rules_judge_only_added_prose() {
        let past = vec!["word"; MAX_SENTENCE_WORDS.saturating_add(1)].join(" ");
        let text = format!("# T\n\n{past}.\n\nA new line.\n\n```\nx\n```\n");
        let elsewhere = judge_over(&text, Judged::Added(&[(5, 5)]));
        assert!(
            !elsewhere
                .iter()
                .any(|finding| finding.contains("words, past")),
            "{elsewhere:?}"
        );
        assert!(
            elsewhere
                .iter()
                .any(|finding| finding.contains("names no language")),
            "{elsewhere:?}"
        );
        let touched = judge_over(&text, Judged::Added(&[(3, 3)]));
        assert!(
            touched
                .iter()
                .any(|finding| finding.contains("words, past")),
            "{touched:?}"
        );
    }

    /// A page shaped the way the rules ask passes, so each refusal below is the
    /// rule under test.
    #[test]
    fn a_plain_page_passes() {
        let text = "# Title\n\nThis page says what the crate is for.\n\n## Use\n\n```rust\nfn main() {}\n```\n\n- One item. Another sentence.\n";
        assert_eq!(judge(text), Vec::<String>::new());
    }

    /// A code block with no language, a page with no title, a page opening on a
    /// table and a skipped heading level are each refused.
    #[test]
    fn the_shape_rules_refuse() {
        let untagged = judge("# T\n\nSays.\n\n```\nx\n```\n");
        assert!(
            untagged
                .iter()
                .any(|finding| finding.contains("names no language")),
            "{untagged:?}"
        );
        let untitled = judge("Says.\n");
        assert!(
            untitled
                .iter()
                .any(|finding| finding.contains("no `# ` title")),
            "{untitled:?}"
        );
        let tabled = judge("# T\n\n| a |\n");
        assert!(
            tabled
                .iter()
                .any(|finding| finding.contains("not a sentence")),
            "{tabled:?}"
        );
        let skipped = judge("# T\n\nSays.\n\n### Deep\n");
        assert!(
            skipped
                .iter()
                .any(|finding| finding.contains("skips a level")),
            "{skipped:?}"
        );
    }

    /// CommonMark 0.31 §4.2: a heading is one to six `#` and then a space, so a
    /// sentence wrapped onto a line opening `#108)` stays in its paragraph, and a
    /// real heading after it is judged against the real level above.
    #[test]
    fn a_wrapped_issue_number_is_prose_and_not_a_heading() {
        let wrapped = judge(
            "# T\n\nSays.\n\n## A\n\nSays that the rest,\n#108) stayed open.\n\n### B\n\nSays.\n",
        );
        assert!(wrapped.is_empty(), "{wrapped:?}");
        let seven = judge("# T\n\nSays.\n\n####### Not a heading\n\n## B\n\nSays.\n");
        assert!(seven.is_empty(), "{seven:?}");
        let skipped =
            judge("# T\n\nSays.\n\n## A\n\nSays that the rest,\n#108) stayed open.\n\n#### Deep\n");
        assert!(
            skipped
                .iter()
                .any(|finding| finding.contains("a level-4 heading under a level-2 one")),
            "{skipped:?}"
        );
    }

    /// A YAML front-matter block is not the page's opening: the title and the
    /// sentence after it are read below it.
    #[test]
    fn front_matter_precedes_the_title() {
        assert!(judge("---\ntype: Decision\n---\n\n# T\n\nSays.\n").is_empty());
        let untitled = judge("---\ntype: Decision\n---\n\nSays.\n");
        assert!(
            untitled
                .iter()
                .any(|finding| finding.contains("no `# ` title")),
            "{untitled:?}"
        );
    }

    /// A sentence one word past the bound is refused and one at it passes;
    /// inline code and link targets are not counted as words.
    #[test]
    fn the_sentence_bound_is_exact() {
        let at = vec!["word"; MAX_SENTENCE_WORDS].join(" ");
        let past = vec!["word"; MAX_SENTENCE_WORDS.saturating_add(1)].join(" ");
        assert!(judge(&format!("# T\n\n{at}.\n")).is_empty());
        let refused = judge(&format!("# T\n\n{past}.\n"));
        assert!(
            refused
                .iter()
                .any(|finding| finding.contains("words, past")),
            "{refused:?}"
        );
        assert_eq!(
            plain("see [the guide](docs/a/b.md) and `a::b::c`"),
            "see the guide and code"
        );
    }

    /// Sentences split at a capital after a full stop, not inside `e.g.` or a
    /// version number.
    #[test]
    fn sentences_split_where_a_reader_would() {
        assert_eq!(sentences("One two. Three four.").len(), 2);
        assert_eq!(sentences("Use e.g. this one. Then stop.").len(), 2);
        assert_eq!(sentences("Version 4.0.0 is out. It ships.").len(), 2);
    }

    /// A relative link to a missing file is refused; a URL and an anchor are not
    /// checked here.
    #[test]
    fn a_dead_relative_link_is_refused() {
        let found = judge("# T\n\nSee [x](missing.md), [y](https://a.b) and [z](#top).\n");
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(
            found.iter().any(|finding| finding.contains("missing.md")),
            "{found:?}"
        );
    }
}
