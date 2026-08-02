# Tasks: zseval-site

Conventions: a section is the execution unit (one section per session, landing
exactly one commit) and every task names the execution evidence that lets its
box be ticked. Sections are vertical slices: each carries its own failing tests
and the implementation that turns them green. `depends` names the sections that
must be green first and is the real ordering constraint; `parallel: no` holds
throughout here, because every later section writes through the page buffer
section 1 defines.

Vocabulary, fixed for this document. Each term below means this and nothing
else, and the bare word it disambiguates is never used alone:

- **the target model** is the LLM under test, the report's `model` field. The
  other two senses are **the `Matrix` model** (`matrix::build`'s output) and
  **the page model** (`site::PageModel`, which `--json` emits).
- **scenario** is an eval case under `scenarios/` carrying an id. A `#### Scenario`
  block in a spec file is a **spec scenario**; a numbered heading in this file
  is a **section**.
- **the page buffer** is the private `String` inside `site::Page`, reachable
  only through `Page::raw(&'static str)` and `Page::text(&str)`.
- **the headline figure** is the count of ledger areas holding no `covered`
  claim, rendered as `Areas with no scenario at all: N of M.` The loader
  refuses a `covered` claim with no scenarios, so "no `covered` claim" and "no
  scenario at all" name one set; this document uses the rendered wording.
- **not provided**, **(none)**, **unknown** and **nothing was graded** are
  literal rendered strings, not descriptions of concepts. 2.2 holds the table.

## 1. Escaping and the page skeleton [dispatch: sai-hu, depends: none, parallel: no, reason: D9's "one escape function, no exemptions" is the only security-relevant decision here, and whether later sections are safe by construction or safe by vigilance is decided by the shape chosen now]

- [x] 1.1 Write the failing unit tests in `crates/zseval/src/site.rs` first: the escape function maps `&`→`&amp;`, `<`→`&lt;`, `>`→`&gt;`, `"`→`&quot;`, `'`→`&#39;` and leaves every other character alone; a report whose `zs_version` is `<script>alert('zs')</script>` puts `&lt;script&gt;alert(&#39;zs&#39;)&lt;/script&gt;` on the page and the raw banner nowhere on it; a claim `reason` carrying all five characters renders each one escaped, and the raw prose is likewise absent. Cite red `cargo test` output.
- [x] 1.2 Implement the writer so an unescaped runtime value is unwritable rather than merely discouraged: `pub(crate) struct Page` holds a private `buf: String` with exactly two ways in, `raw(&'static str)` for markup (a runtime `String` cannot be passed, so only a source literal enters unescaped) and `text(&str)` which escapes, plus `finish()`. `pub(crate)`, not private: section 4's renderer lives in `matrix.rs` and writes through this same buffer, which is what keeps D9's "no exemptions" true of the whole page. A design that asks the caller to remember to escape does not satisfy this task.
- [x] 1.3 Build the page skeleton, written through `raw` as `&'static str` throughout: `<!doctype html>`, `<html lang="en">`, a head carrying `charset`, `viewport`, `<title>`, and `<style>` holding one `const CSS: &str` inlined verbatim, then the three sections in design D11's order, `<section id="header">`, `<section id="coverage">`, `<section id="results">`. The CSS is yours to write, under one constraint: it names no font, image or sheet to fetch, so 1.4 passes.
- [x] 1.4 Write the self-containment test, asserting on absent patterns rather than on a visual check: the whole page, CSS included, contains none of `http://`, `https://`, `<script`, `<link`, `<img`, `<iframe`, ` src=`, `@import`, `url(`. Assert `<style>` is present as the counterweight, since a page with no styling at all would satisfy every absence.
- [x] 1.5 Add `pub mod site;` to `lib.rs`. Evidence: 1.1 and 1.4 green, `cargo test --workspace` green.

## 2. Header section: read back, derive nothing [dispatch: too-te, depends: 1, parallel: no, reason: every field, its label and its rendered spelling are pinned in 2.2 below, and the three cases that could go wrong (null vs empty, the two judge halves, judge_model's three states) are each a spec scenario; no design choice is left]

- [x] 2.1 Write the failing tests first, one per spec scenario, asserting on the literal strings 2.2 pins: `git_sha: null` and `features: null` render exactly `<dt>git sha</dt><dd>not provided</dd>` and `<dt>features</dt><dd>not provided</dd>`, never an empty `<dd>`; `judge_file` with `judge_hash` renders under `configured` and `judge_model` under `graded by`, so neither reads as the other; `judge_model: Some([])` renders `nothing was graded` and the page does not contain the configured judge file's name as the answer; `budget_truncated: true` renders `<dt>budget truncated</dt><dd>yes</dd>`. Cite red output.
- [x] 2.2 Render the header as a `<dl>`, every value read verbatim with no inference, no defaulting and no computed substitute. The label and spelling of each row, which the tests in 2.1 assert on:
  - `zerostack` ← `zs_version`; `binary sha256` ← `zs_bin_sha256`; `binary path` ← `zs_bin_path`
  - `ledger audited against` ← the ledger's `audited_against`, and `audit vs this run` ← `Ledger::audit_matches` (see 5.1); these two are the only rows not read from the report, and they sit beside `zerostack` so the two version strings are read together
  - `git sha` ← `git_sha`, `features` ← `features`: `None` renders `not provided`, and for the list `Some([])` renders `(none)` — a visibly different string, because an absent fact and an empty one are different claims (D5)
  - `model` ← the target model; `backend`; `target`; `timestamp`; `trials`
  - `total cost` ← `summary.total_cost_usd`, formatted `${:.4}`
  - `budget truncated` ← `yes` or `no`
  - `configured` ← `judge_file` followed by ` (hash: ` and `judge_hash` (`not provided` when `None`); `graded by` ← `judge_model`'s three states, `None`→`unknown`, `Some([])`→`nothing was graded`, `Some(items)`→the items joined by `, `. Never collapse the two rows into one: that would report an intention as a fact.
- [x] 2.3 Evidence: 2.1 green, plus a grep over the header rendering functions showing zero hits for `unwrap_or`, `unwrap_or_default`, `unwrap_or_else` and for the arithmetic operators `+ - * /` — the header derives nothing, and these are the shapes a derivation would arrive in. `format!("${:.4}")` on `total_cost_usd` is formatting, not arithmetic, and is the one permitted transformation.

## 3. Coverage section [dispatch: sai-hu, depends: 1, parallel: no, reason: the covered-but-not-exercised derivation is new, and laying out four statuses whose evidence fields differ per status is a presentation judgment the spec constrains but does not settle]

- [x] 3.1 Write the failing tests first: over three areas, one holding only an `uncovered` claim and one only a `product-blocked` claim, both are listed by title and the headline reads `2 of 3`; reordering the ledger's areas reorders the section with no sorting applied; below `</style>` the page contains no `%` and no case-insensitive `percent`; a claim citing three ids where the report holds two marks only the third and the claim still renders under `covered`. Cite red output.
- [x] 3.2 Render every area in ledger file order (nothing sorts), each claim under its status label spelled in the ledger's own vocabulary (`covered`, `uncovered`, `product-blocked`, `excluded`), carrying the evidence that status owes and no other: cited ids as `<code>` list items for `covered`; `blocked by: ` plus the sentence when an `uncovered` claim carries `blocked_by`, and nothing at all when it does not, because the field's presence is itself the fact; the bare `reason` for `product-blocked` and `excluded`; `tracked as: ` plus the `zs` pointer where one exists; and `note: ` plus any `note`, rendered outside the status match since any status may carry one.
- [x] 3.3 Implement the covered-but-not-exercised derivation per cited id, not per claim: an id a `covered` claim cites that is absent from this report's `scenarios` renders the mark `not exercised by this run` beside it, and the claim itself stays `covered`. Derive it at render time; write nothing back to the ledger.
- [x] 3.4 Render the headline figure as `Areas with no scenario at all: N of M.` — a count and the total it is counted out of, never a ratio and never a percentage, here or in any other section. Mark each counted area where it sits, with `no scenario at all` beside its title, so a reader can find the areas the headline counted.
- [x] 3.5 Evidence: 3.1 green, plus a manual `site` run against the committed ledger showing the real headline (`7 of 15` as of the 2026-07-30 ledger). Cite the run's output; do not encode `7` as a unit-test assertion, which would make a passing suite depend on ledger content that is expected to move.

## 4. Results section: a third renderer in `matrix.rs` [dispatch: sai-hu, depends: 1, parallel: no, reason: it must reuse the private cell, hole, footer and mark formatting rather than restate it, and it must not recompute any figure; this is also the section that makes `matrix-render`'s spec change true]

- [x] 4.1 Write the failing tests first in `crates/zseval/src/matrix.rs`: a scenario whose only trial is `Indeterminate` renders `-` and the page contains no `0.000` for it; rows are grouped by kind, regression before capability; for a single-report `Matrix` (one column), `render_fixed_width`, `render_markdown` and `render_html` return identical cells for every scenario row and for the seven footer rows (`regression pass@k`, `regression pass^k`, `capability pass@k`, `capability pass^k`, `pass@k`, `pass^k`, `cost usd`) — the three-renderer agreement `matrix-render` requires, not a pairwise check; a non-empty `Matrix::footer_excluded` renders its ids on the page rather than silently narrowing the denominator. Cite red output.
- [x] 4.2 Implement `pub(crate) fn render_html(page: &mut Page, m: &Matrix)` beside `render_fixed_width` and `render_markdown`, reusing the existing private formatting helpers. It writes into section 1's page buffer rather than returning a `String`: every runtime value goes through `page.text`, every piece of markup through `page.raw`, and no escape function is defined in `matrix.rs` — D9's rule is one function for the whole page, and a second one here is the failure it exists to prevent. Do not duplicate any formatting rule, and do not add a public helper solely to let another module restate one.
- [x] 4.3 Assert `matrix`'s own surface is unchanged, in `tests/harness.rs` where `matrix`'s command-line surface is already tested: `matrix <report> --html` exits 2 with `unknown flag '--html'` on stderr, and `matrix` under no flags, under `--markdown` and under `--json` emits no HTML. Leave the existing `--markdown` and `--json` tests untouched and still green.
- [x] 4.4 Evidence: 4.1 and 4.3 green, `cargo test --workspace` green.

## 5. The `site` subcommand: gates, exit codes, `--json` [dispatch: sai-hu, depends: 1, 2, 3, 4, parallel: no, reason: the two failure modes are deliberately unlike each other (drift aborts, a version mismatch discloses) and are the easiest part of the spec to implement backwards; the ordering guarantee that nothing is written before the gate passes is also load-bearing]

- [x] 5.1 Write the failing tests first, one per spec scenario: a ledger citing a nonexistent id exits 2 naming the id with no file at `--out`; a tree holding an unclaimed scenario exits 2 the same way; a `--backend mock` report whose `zs_version` is `mock` against a ledger recording `1.7.2` exits 0 with both strings on the page, labelled `ledger audited against` and `audit vs this run`; a matching version renders the agreeing sentence scoped to the version (containment supports "the two name one version" and nothing about whether the ledger's prose is current); a missing ledger exits 2 naming the path; a missing report exits 2 naming the path; an `--out` whose parent directory does not exist exits 2; a missing `--out` and an unknown flag are usage errors, exit 2. Two exit-0 cases carry the no-exit-1 rule, which cannot be tested by quantifying over all inputs: a report whose every trial failed, and a report whose every scenario is ungradable, both exit 0 with a page written. Do not copy `cmd_matrix`'s fully-ungradable exit 2 — `matrix` gates on it deliberately and `site` deliberately does not. Cite red output.
- [x] 5.2 Add the subcommand: positional `<report.json>`, required `--out <file>`, `--json`, and `--ledger <path>`. Copy the argument loop's shape from `cmd_matrix` in `main.rs`; usage errors exit 2 the way its neighbours do. Document `--ledger` as a test override in all three places the CLI documents itself: `main.rs`'s usage block, the doc comment on the subcommand, and the README paragraph 6.1 adds.
- [x] 5.3 Implement the order and hold it, as a contract rather than an implementation detail: load the report, load the ledger, run `Ledger::check_drift`, build the page model, render, write, and only then emit `--json`. Nothing reaches `--out` before the drift gate passes, and a failed write leaves `--out` holding whatever it held before — write to a sibling `.tmp` path and rename, so the path never holds a truncated page, and remove the temp file on failure.
- [x] 5.4 Implement `--json` to emit `PageModel` (the header read-back, the coverage rows with their marks, and the `Matrix`) to stdout, with `--out` still required and still written. Under `--json` the command owes two deliverables, so the page is written first and a stdout failure after the page landed is exit 2 naming the already-written page: a machine handed a truncated model must not read exit 0 as "this model is complete".
- [x] 5.5 Evidence: 5.1 green, plus a manual run of the two commands `zseval run scenarios/prompts/ask-readonly --backend mock=crates/zseval/tests/fixtures/session-ask-readonly.json --trials 1 --no-judge --results <dir> --tag <tag>` then `zseval site <dir>/<tag>/report.json --out <page>`, citing the page's `audit vs this run` row showing the `mock` mismatch.

## 6. Docs, integration test, and verification [dispatch: too-te, depends: 5, parallel: no, reason: mechanical work against criteria this document already states]

- [x] 6.1 Add `zseval site` to `main.rs`'s usage block and to the README's subcommand documentation, in the shape the `matrix` paragraph beside it uses. State that it makes no API call, that `--out` is required, and that `--ledger` is a test override rather than a general-purpose option.
- [x] 6.2 Add the integration test to `crates/zseval/tests/harness.rs` (not a second test binary, per the 2026-07-23 single-binary decision): drive the real `site` subcommand as a subprocess over a real mock-backend report, against the committed ledger and scenario tree rather than a fixture, then assert on the written file with exactly one assertion per rendered section — header (section 2), coverage (section 3), results (section 4).
- [x] 6.3 Confirm no dependency was added: `git diff` shows `Cargo.toml` and `Cargo.lock` unchanged.
- [x] 6.4 Confirm the three counts the page states, against the committed ledger and one real mock run, and cite all three: the headline (`Areas with no scenario at all: 7 of 15`), the cited ids marked `not exercised by this run` (41 of the ledger's 42, this run exercising 1), and the results table's scenario rows (1, matching the report's scenario count, above the seven footer rows).
- [x] 6.5 Evidence: `cargo test --workspace` green, `cargo fmt --all --check` clean, `cargo clippy --workspace --all-targets` clean.
