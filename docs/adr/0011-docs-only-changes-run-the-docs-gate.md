# ADR 0011: a docs-only change runs the docs gate, not the suites

Status: **accepted**, 2026-11-04. The Director's orders of 2026-11-04 were "WHY IS THE FULL
FUCKING SUITE BEING RUN FOR 3 MARKDOWNS?" and "ADD DOCS ONLY CHECKS TO ENSURE EXCELLENCE IN
TOKIO LEVEL DOCUMENTATION AND SEMANTICALLY A CLAUDE HAIKU FROM 2 YEARS AGO CAN UNDERSTAND".
They added "NO XTRA LLM CALLS".

Stakeholders: the Director; a contributor landing a docs change; every reader of a page, a person or a small model.
Concerns: a docs change that waits on every suite and fails on a slow runner; a page its reader cannot follow.
Requirements: R13
Viewpoint: the gate viewpoint, one row per CI job and the check that decides it.
Elements and relations: the `changes` job, the `gate` job, the `Build and workspace tests` row, and `check-docs` with its `prose` module. The job classifies the change, the row reads the class, and the gate runs `check-docs --base`.

## Context

PR #861 changed three governance pages. CI ran every suite for them. A macOS shard passed all
162 of its tests, then ran past its 5-minute limit on a slow runner and was cancelled. The
required row failed, so a docs change could not merge.

The docs had no reader-facing check. `check-docs` held ADRs to 42010 and PRDs to 29148, and
nothing judged a guide. On `2a601be2` the rules below found 883 findings across the docs. 762
were sentences or paragraphs past the bounds, and 121 were shape findings. Most shape findings
were skipped heading levels in changelog and ledger pages. The rest were code blocks with no
language, dead relative links, and pages opening with no title or no sentence.

## Decision

1. **The `changes` job classifies every pull request.** A change is docs-only when every file is
   a `docs/**/*.md` page, a root `*.md` page other than `README.md`, or `llms.txt`. The two
   generated families, `docs/script-lexicon*.md` and `docs/catalog.md`, count as code. So does a
   push to `main`, a manual run, and a listing that fails or comes back empty.
2. **A docs-only change runs the gate and skips the suites.** The `Gate checks` job still runs,
   so `llms.txt`, the citations, the example ceilings, the catalog and `check-docs` all judge it.
   `Clippy and scan` passes on skipped legs. `Build and workspace tests` passes when the
   classification and the gate passed, and it still holds the run to 300 seconds.
3. **`check-docs` reads every page as its reader does.** The `prose` module holds each page to
   seven rules, with no model call, so one tree always gets one verdict:
   - it opens with a `# ` title, after any front matter;
   - the first block after the title is a sentence that says what the page is for;
   - every code block names its language;
   - no heading skips a level;
   - every relative link lands on a file;
   - no sentence holds more than 45 words;
   - no paragraph or list item holds more than 8 sentences.
4. **The gate judges what a change touches.** `check-docs --base REV` holds every page changed
   since `REV` to the five shape rules. It holds the prose the change added to the two bounds.
   Bare, `check-docs` audits every page. CI passes `HEAD^1`, the base of the merge commit.

## Alternatives rejected

- **`paths-ignore` on the workflow.** The ruleset requires three named rows. A workflow that never
  starts never reports them, so every docs pull request would wait forever.
- **A model as the judge.** The Director refused extra model calls. A model's verdict also
  changes between runs, so a page could pass today and fail tomorrow unchanged.
- **A readability formula such as Flesch-Kincaid.** Its syllable count is a guess for technical
  English. A score of 41 tells the writer nothing to do, while "this sentence is 63 words" does.
- **markdownlint or Vale.** Each is a second docs checker beside `check-docs`, in another
  language, with its own install step in CI.
- **The length bounds over every page now.** The 762 length findings sit mostly in changelog and
  ledger pages, which are records of what happened. Many pages also pass the 120-line law only
  when split. A page meets the bounds the next time a change touches it.

## Consequences

- A docs-only pull request runs the classification, the gate, the advisories and the required
  rows. It no longer runs the 90 suite, matrix and evidence jobs.
- It runs no Rust doc test. That holds because no crate includes a `docs/` page in its rustdoc
  but the lexicon pages, which count as code. `drishti` includes its own `crates/drishti/docs`.
- The bare audit still reports 767 findings, almost all length findings in pages no change has
  touched since. Oversized pages such as `docs/bot-on-ecs.md` (569 lines) keep their shape
  findings until a change splits them.
- This change raised 88 changelog and ledger pages from `###` to `##`. It tagged eight fences,
  fixed four dead links, and gave seven pages a title or an opening sentence.

## Correspondence

Refines ADR 0004: `check-docs` keeps its 42010 and 29148 rules and gains the reader rules.
Relates to `docs/workflow/ci-layout.md`, which describes the jobs, and to PR #861.

## Verification

- The `doc-governance` lane runs `check-docs -- . --base ${DOCS_BASE:-origin/main}`.
- The `prose` tests: `a_plain_page_passes`, `the_shape_rules_refuse`,
  `front_matter_precedes_the_title`, `the_sentence_bound_is_exact`,
  `sentences_split_where_a_reader_would`, `a_dead_relative_link_is_refused` and
  `the_length_rules_judge_only_added_prose`.
- The `changes` and `docs-only-verdict` lanes name the two new CI steps, and the `gate-parity`
  lane holds them to `ci.yml`.
