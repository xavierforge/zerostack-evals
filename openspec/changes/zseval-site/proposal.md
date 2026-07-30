## Why

Day 1 made the numbers trustworthy and nothing renders them. A run's answers live in `results/<tag>/report.json` and the denominator lives in `scenarios/coverage.toml`, so "how is zerostack doing, and on what fraction of its surface" is answerable today only by opening two files and doing the join by hand. This is ROADMAP Day 2 item 5, the step that turns Day 1's honesty into something deliverable.

It is also the ledger's first consumer. `coverage-ledger` shipped a written contract with no caller: `coverage::` appears nowhere outside `crates/zseval/tests/harness.rs`, so the module's cost is already paid and its benefit is not. A contract with one renderer and no renderer written is a contract nobody has tested against a real reader.

## What Changes

- New subcommand `zseval site <report.json> --out <file.html>`. Pure rendering: it reads a report and the ledger, writes one file, and makes no API call, so it costs nothing and runs offline.
- Three sections, in this order:
  - **Header**, the identity of the run. Every field is read from the report verbatim and none is derived: `zs_version` with `zs_bin_sha256` and `zs_bin_path`, `git_sha` and `features` (both `null` against today's binary, shown as not provided rather than as empty), `model`, `backend`, `target`, `timestamp`, `trials`, `summary.total_cost_usd`, `budget_truncated`, and the judge's three-state identity (`judge_file` and `judge_hash` for what was asked for, `judge_model` for what actually graded).
  - **Coverage**, the ledger. All 15 areas in file order, each claim under one of the four statuses, areas with no `covered` claim listed rather than omitted. No percentage anywhere. Each `covered` claim whose scenario ids do not all appear in this report is marked as covered but not exercised by this run.
  - **Results**, the scenario table. Built by `matrix::build` over the single report and rendered by a new HTML renderer over the same `Matrix` model, so cells, holes, per-kind grouping, the recomputed footer, the SPREAD and DRIFT marks, and the footer-excluded disclosure all keep the meaning they already have.
- Generation runs `Ledger::check_drift` against the repo tree first and aborts before writing anything if it fails. A ledger that disagrees with the tree makes the coverage section false, and a page that cannot be trusted is worse than no page.
- A mismatch between `audited_against` and the report's `zs_version` does **not** abort. The page states both and says which is which. `coverage-ledger` already requires that the worst outcome of the containment comparison is a spurious mismatch notice and never a blocked publish, and a `--backend mock` report records `mock`, so it mismatches by construction.
- One self-contained file: CSS inline, no JavaScript, no external requests. A page that needs the network to render is not a deliverable artifact.
- No new dependency. The HTML is assembled the way `matrix::render_markdown` assembles markdown.

## Capabilities

### New Capabilities

- `site-render`: the `site` subcommand's contract. Its inputs and exit codes, the three sections and what each may and may not say, the two different failure modes (drift aborts, version mismatch discloses), the "covered but not in this run" derivation, and the self-contained single-file output rule.

### Modified Capabilities

- `matrix-render`: the requirement titled "Two renderers, shared with run" becomes three. The `Matrix` model gains an HTML renderer beside the fixed-width and markdown ones. `matrix`'s own command-line surface is unchanged: it still emits fixed-width by default, markdown under `--markdown`, and JSON under `--json`, and the HTML renderer is reachable only through `site`.

## Impact

- Code: new `crates/zseval/src/site.rs` holding the page's assembly and the two derivations it owns (the header's field readback and the covered-but-not-run set); `render_html` added to `crates/zseval/src/matrix.rs` beside its two siblings; one subcommand arm in `main.rs`; one `pub mod site;` line in `lib.rs`.
- The HTML renderer joins the other two rather than living in `site.rs` because the cell, footer, and mark formatting helpers in `matrix.rs` are private. A renderer outside the module would have to duplicate that formatting, and three copies of "how a hole is written" is how they drift apart.
- Data and schema: none. No report field is added, moved, or reinterpreted, and the ledger's schema is untouched. The change is a reader.
- Dependencies: none added.
- Tests: unit tests in `site.rs` for each derivation and each failure mode, plus an integration test in `tests/harness.rs` that drives the real subcommand over a mock-backend report and asserts on the written file. Zero API cost, so it runs in CI like everything else.
- Ongoing cost: the page is now a consumer of both the report schema and the ledger schema, so a change to either has a third place to update. That is the cost the ledger was written to have.

## Non-Goals

- GitHub Pages deployment, whether by Actions or by a committed output directory. Whether the page renders correctly and whether it publishes correctly are two questions, and a failure in one should not be diagnosed through the other. Separate change.
- A multi-report comparison page. `matrix::build` takes a slice and would allow it, but a header section describing one identity per column, and a coverage section answering "which column did not exercise this", are a different page with a different layout. `matrix --markdown` already serves cross-report comparison for now.
- Coverage percentages. Forbidden by `coverage-ledger` for a reason that has not changed: fine-grained `covered` claims beside coarse `uncovered` ones make any ratio manipulable by re-slicing.
- Trend or history pages, and any storage of past pages. The subcommand renders the report it is given.
- Re-running or re-grading anything. `site` never invokes a backend or a judge.
