## 1. Escaping and the page skeleton [dispatch: sai-hu, parallel: no, reason: D9's "one escape function, no exemptions" is the only security-relevant decision here, and whether later sections are safe by construction or safe by vigilance is decided by the shape chosen now]

- [x] 1.1 Write the failing unit tests in `crates/zseval/src/site.rs` first: the escape function over `<`, `>`, `&`, `"`, `'`; a report whose `zs_version` carries markup renders escaped and the raw form is absent from the page; a claim `reason` carrying markup-significant characters renders escaped. Cite red `cargo test` output.
- [x] 1.2 Implement escaping so an unescaped runtime value cannot reach the buffer by accident: runtime values go through one function, and raw markup enters only as literal `&'static str`. A design that merely asks the caller to remember to escape does not satisfy this task.
- [x] 1.3 Build the page skeleton: doctype, `<style>` with the page's CSS inlined, and the three section containers in the order header, coverage, results (design D11).
- [x] 1.4 Write the self-containment test: the rendered page contains no external URL, stylesheet, font, image, or script reference. Assert on the absence of the patterns, not on a visual check.
- [x] 1.5 Add `pub mod site;` to `lib.rs`. Evidence: 1.1 and 1.4 green, `cargo test --workspace` green.

## 2. Header section: read back, derive nothing [dispatch: too-te, parallel: no, reason: every field and its rendering rule is named in the spec, and the three cases that could go wrong (null vs empty, the two judge halves, judge_model's three states) are each stated as a scenario; no design choice is left]

- [x] 2.1 Write the failing tests first, one per spec scenario: `git_sha` and `features` being `null` render as not provided rather than as an empty string or empty list; `judge_file` with `judge_hash` and `judge_model` both appear, labelled so neither reads as the other; `judge_model` of `Some([])` says nothing was graded and does not echo the configured model; `budget_truncated` true is visible. Cite red output.
- [x] 2.2 Render the header from `zs_version`, `zs_bin_sha256`, `zs_bin_path`, `git_sha`, `features`, `model`, `backend`, `target`, `timestamp`, `trials`, `summary.total_cost_usd`, `budget_truncated`. Read each field verbatim: no inference, no defaulting, no computed substitute.
- [x] 2.3 Evidence: 2.1 green, and a grep over the header code showing no arithmetic or fallback on any report field.

## 3. Coverage section [dispatch: sai-hu, parallel: no, reason: the covered-but-not-exercised derivation is new, and laying out four statuses whose evidence fields differ per status is a presentation judgment the spec constrains but does not settle]

- [x] 3.1 Write the failing tests first: an area with no `covered` claim is listed and counted in the headline figure; reordering the ledger's areas reorders the section with no sorting applied; no ratio or percentage appears anywhere in the page; a claim citing three ids where the report holds two marks only the third and is not reported as uncovered. Cite red output.
- [x] 3.2 Render every area in ledger file order, and every claim under its status carrying the evidence that status owes: cited ids for `covered`, the `blocked_by` sentence when an `uncovered` claim has one, `reason` for `product-blocked` and `excluded`, the `zs` pointer where present, and any `note`.
- [x] 3.3 Implement the covered-but-not-exercised derivation: per `covered` claim, the cited ids absent from this report's results. Derive it at render time; write nothing back to the ledger.
- [x] 3.4 Render the headline figure as a count of areas with no scenario at all. No percentage, in this section or any other.
- [x] 3.5 Evidence: 3.1 green, and the headline count matches the ledger's real content (7 of 15 as of the 2026-07-30 ledger).

## 4. Results section: a third renderer in `matrix.rs` [dispatch: sai-hu, parallel: no, reason: it must reuse the private cell, hole, footer and mark formatting rather than restate it, and it must not recompute any figure; this is also the section that makes `matrix-render`'s spec change true]

- [x] 4.1 Write the failing tests first in `crates/zseval/src/matrix.rs`: a scenario with no gradable trial renders as a hole and not as a zero; rows are grouped by kind; for one model, the HTML renderer's cells and footer figures agree with `render_markdown`'s for the same model; `footer_excluded` is disclosed rather than silently narrowing the denominator. Cite red output.
- [x] 4.2 Implement `render_html` beside `render_fixed_width` and `render_markdown`, reusing the existing private formatting helpers. Do not duplicate any formatting rule, and do not add a public helper solely to let another module restate one.
- [x] 4.3 Assert `matrix`'s own surface is unchanged: no HTML flag, `--markdown` and `--json` behave exactly as before.
- [x] 4.4 Evidence: 4.1 and 4.3 green, `cargo test --workspace` green.

## 5. The `site` subcommand: gates, exit codes, `--json` [dispatch: sai-hu, parallel: no, reason: the two failure modes are deliberately unlike each other (drift aborts, a version mismatch discloses) and are the easiest part of the spec to implement backwards; the ordering guarantee that nothing is written before the gate passes is also load-bearing]

- [x] 5.1 Write the failing tests first, one per spec scenario: a ledger citing a nonexistent id exits 2 naming the id with no file written at `--out`; a tree holding an unclaimed scenario exits 2 the same way; a `--backend mock` report whose `zs_version` is `mock` exits 0 with both version strings shown and labelled; a matching version is shown as agreeing without claiming more than containment supports; a missing ledger exits 2; no input produces exit 1. Cite red output.
- [x] 5.2 Add the subcommand: positional `<report.json>`, required `--out <file>`, `--json`, and `--ledger <path>` documented as a test override rather than a general option. Usage errors exit 2, matching the CLI's existing flag-parsing conventions.
- [x] 5.3 Implement the order and hold it: load the report, load the ledger, run `Ledger::check_drift`, build the model, render, then write. Nothing reaches `--out` before the drift gate passes.
- [x] 5.4 Implement `--json` to emit the page model (header read-back, coverage rows with their marks, the `Matrix`) to stdout, with `--out` still required and still written.
- [x] 5.5 Evidence: 5.1 green, and a manual run showing the mock-report page written with the mismatch visible.

## 6. Docs, integration test, and verification [dispatch: too-te, parallel: no, reason: mechanical work against criteria this document already states]

- [ ] 6.1 Add `zseval site` to `main.rs`'s usage block and to the README's subcommand documentation, in the shape the neighbouring subcommands use. State that it makes no API call and that `--ledger` is a test override.
- [ ] 6.2 Add the integration test to `crates/zseval/tests/harness.rs` (not a second test binary, per the 2026-07-23 single-binary decision): drive the real `site` subcommand over a mock-backend report, then assert on the written file's content for one scenario from each of sections 2, 3, and 4.
- [ ] 6.3 Confirm no dependency was added: `Cargo.toml` and `Cargo.lock` unchanged.
- [ ] 6.4 Confirm the three counts the page headlines against the real ledger and a real mock run, and cite them.
- [ ] 6.5 Evidence: `cargo test --workspace` green, `cargo fmt --all --check` clean, `cargo clippy --workspace --all-targets` clean.
