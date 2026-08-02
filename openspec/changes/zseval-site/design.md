## Context

`zseval` already holds every number the page needs and no way to look at them together. `report.json` carries the run's identity and per-scenario results; `scenarios/coverage.toml` carries the denominator. Joining them is a manual, error-prone reading exercise today.

Two existing pieces set most of this design before it starts. `matrix.rs` is already built as one model (`Matrix`) with renderers over it (`render_fixed_width`, `render_markdown`) and a pure, infallible `build(reports: &[&Report])` that accepts a single report. `coverage.rs` was written as a contract for exactly this renderer and deliberately shipped without one: `Ledger` exposes `areas()`, `audited_against()`, `scenario_roots()`, `audit_matches()` and `check_drift()`, and nothing outside `tests/harness.rs` calls any of them.

Three repo-wide constraints apply. `main.rs`'s header states the exit contract (0 pass, 1 fail, 2 usage or harness error) and that every subcommand supports `--json`. `matrix-render` establishes that a view has no exit 1. `coverage-ledger` establishes that the `audited_against` comparison must never block a publish.

## Goals / Non-Goals

**Goals:**

- One command turns a report plus the ledger into a single file a human can read and a maintainer can hand to someone else.
- The page cannot state something the data does not support. Every header field is a readback, the coverage section is the ledger, and the results table keeps the semantics `matrix` already defines.
- The ledger's contract gets its first real reader, which is the only way to find out whether it was written correctly.
- Machines never have to parse the rendered page.

**Non-Goals:**

- Publishing. No Pages deployment, no Actions, no committed output directory.
- Multi-report pages, coverage percentages, trends, history, and any re-running or re-grading. See proposal.md's Non-Goals for why each is out.
- A shared design system. This is one page; a second page is when styling becomes a shared concern.

## Decisions

### D1: `site` is a view, so it has no exit 1

Exit 0 when a page was written, whatever the scores; exit 2 when it could not be written. Low pass rates, a fully ungradable report, and an `audited_against` mismatch are all things the page reports, not things that fail the command. This copies `matrix`'s rule verbatim (`matrix-render`: "it is a view, not a gate") rather than inventing a second convention for the same category of subcommand.

Alternative considered: exit 1 on a version mismatch, so CI could gate on staleness. Rejected: `coverage-ledger` requires the mismatch never block a publish, and a gate on it would do exactly that.

### D2: `--out` is required; `--json` emits the page model

`main.rs` states the invariant that every subcommand supports `--json`, and `matrix --json` sets the meaning: emit the model, not the rendered form. So `site --json` prints the page model (header fields, coverage rows with their marks, and the `Matrix`) to stdout, and `--out <file>` writes the HTML. Both may be given; `--out` is required either way, because writing the page is what the subcommand is for.

This is not a convenience. It is what makes the derivations testable without parsing HTML, and it means a future consumer (the Pages change, a trend page) reads structured data rather than scraping a rendered artifact.

Alternative considered: HTML to stdout, no `--out`. Rejected: it collides with `--json` on the same channel, and the product here is a file on disk rather than something to pipe.

### D3: Drift aborts, a version mismatch discloses

These are two different failures and they are deliberately not treated alike.

`Ledger::check_drift` failing means the ledger and the scenario tree disagree, so the coverage section would describe scenarios that do not exist or omit ones that do. The page would be false. Abort before writing anything, exit 2. `cargo test --workspace` already enforces this, so a failure here means the page was generated from a tree that never passed its own tests, which is exactly the case worth refusing.

`audited_against` not matching the report's `zs_version` means the ledger's judgments were made against a different build than the one measured. That is a fact worth showing, not a reason to refuse: `coverage-ledger` requires the worst outcome of the containment comparison to be a spurious mismatch notice and never a blocked publish, and a `--backend mock` report records `mock`, so it mismatches by construction. The page states both strings and which is which.

### D4: The HTML renderer joins `matrix.rs`, and `matrix-render` changes to say so

`matrix.rs` holds private helpers for how a cell, a hole, a footer figure and a row mark are written. A renderer in another module cannot reach them and would have to restate that formatting, giving three independent answers to "how is a hole written" that drift apart on the first change. So `render_html` goes beside its two siblings, and `matrix-render`'s "Two renderers" requirement becomes three.

The cost is honest and bounded: `matrix`'s own command-line surface does not change, and the spec says so explicitly, so nobody reads the third renderer as a new `matrix` flag.

Alternative considered: `render_html` in `site.rs`, consuming `Matrix` as a public type, leaving `matrix-render` untouched. Rejected for the duplication above. The spec churn is one requirement; the duplication would be permanent.

### D5: The header reads back, and `null` is not empty

Every header field comes from a report field with no derivation, no inference, and no defaulting. `git_sha` and `features` are `null` against today's binary because it embeds neither, and the page says they are not provided rather than rendering an empty string or omitting the row. An absent fact and an empty fact are different claims, which is the same reasoning that made `judge_model` a three-state field (`None` unknown, `Some([])` nothing graded, `Some([m])` these graded) rather than a string.

The judge is shown as two things, not one: `judge_file` with `judge_hash` for what the run was told to grade with, and `judge_model` for what actually answered. Collapsing them would report an intention as a fact.

One exemption, and it is temporary. A field whose schema documents `""` as "none was named" is rendered as that absence rather than verbatim: today `judge_file` (`no judge configured`) and `target` (`no target file`). The reason is that the schema spells one absence two ways, so verbatim readback is not neutral here, it makes a claim: a `--no-judge` run renders `configured: (hash: not provided)`, which states a judge file was configured and then withholds its name, and the canonical `--backend mock` flow renders a blank `target` row the same way. Reading a documented sentinel is not inference, because the schema itself says what `""` means; inference would be guessing at an undocumented blank, which is why `target`'s sentinel is written into its doc comment as part of this rule rather than special-cased. `--json` keeps emitting the raw field: machines read what the report holds, and the wording is the page's reading for a human. The exemption expires when `judge_file` and `target` become `Option` in the report schema, which is its own change; at that point `render_absent_or` is deleted and `render_opt_str` covers both.

### D6: "Covered but not in this run" is derived here, never stored

`coverage-ledger` makes `covered` an existence claim and explicitly refuses to record run membership, because that would make the ledger stale on every run. So the mark is computed at render time: for each `covered` claim, the scenario ids it cites that do not appear in this report's `scenarios`. D3 guarantees those ids exist in the tree, so the only reading left is "exists, not exercised here", which is a fact about the report and belongs to the consumer.

### D7: One self-contained file, CSS inline, no JavaScript

A page that fetches anything at view time is not a deliverable: it breaks when opened from a file path, when the network is absent, and when whatever it fetched moves. Inline the CSS, embed nothing external, ship no script. Nothing on this page needs behaviour; sorting and filtering are what `--json` is for.

### D8: String assembly, no template engine and no new dependency

`render_markdown` assembles its output by pushing onto a `String`, and the HTML renderer does the same. A template crate would buy separation of markup from logic that a single page does not need, at the cost of a dependency and a second place to look. This follows the repo's standing preference for a verified hand-rolled utility over a crate when there is no correctness or security upside, and there is not one here as long as D9 holds.

### D9: One escape function, applied where values enter the buffer

This is the only place the page handles input it does not control. `zs_version` is captured verbatim from `ZS_BIN --version`'s first line, deliberately unvalidated so upstream's banner shape never becomes a compatibility contract. `zs_bin_path`, `target`, `judge_file` and `prompts_names` come from a filesystem. A banner or path containing `<`, `>`, `&`, `"` or `'` would otherwise land in the markup and, at worst, execute in whoever opens the page.

So: a single escaping function, applied at every point a runtime value is written into the buffer, with no exceptions for values that "cannot" contain markup. The test for this is a report whose `zs_version` is hostile, asserting the page contains the escaped form and not the raw one. Ledger prose and scenario ids are author-controlled and are escaped anyway, because a rule with an exemption list is a rule nobody can check.

### D10: The ledger path is fixed, with an override for tests only

`site` reads `scenarios/coverage.toml` relative to the repo root, the same path `check_drift` walks `scenario_roots` from. A missing ledger is exit 2, not a page with the coverage section omitted: a page missing its denominator is the thing this change exists to stop shipping. A `--ledger <path>` override exists so tests can point at a fixture without building a whole tree, and it is documented as such rather than as a general-purpose knob.

### D11: Section order is header, coverage, results

Identity first, because every number below it is only meaningful once you know what produced it. Coverage before results, because a reader who sees scores first anchors on them and then reads coverage as a caveat, when the intended reading is the reverse: these are the scores on the part of the surface we test at all. This is the same argument the ledger's own existence rests on.

### D12: The headline figure is a count, not a ratio

The coverage section states how many areas have no scenario at all (7 of 15 today). No percentage appears anywhere, per `coverage-ledger`: fine-grained `covered` claims sit beside coarse `uncovered` ones, so any ratio over them is manipulable by re-slicing, while the count of empty areas is not.

## Risks / Trade-offs

- **A missed interpolation site is an injection into a published page.** → D9's single escape function, applied without exemptions, plus a hostile-`zs_version` test. The rule is deliberately "escape everything" rather than "escape the untrusted fields", because the second version requires every future editor to re-derive which fields are trusted.
- **The page becomes a third consumer of the report schema.** → Accepted; it is the cost the ledger and the report identity fields were built to have. The mitigation is that the page derives nothing: a schema change breaks a readback loudly rather than silently changing a computed number.
- **`check_drift` needs the scenario tree, so `site` cannot render a report copied to a machine without this repo.** → Deliberate. The coverage section is a claim about this repo's suite, and rendering it from a tree that is not there would be a claim about nothing.
- **Inline CSS means the page's look is shared with nothing.** → Accepted while there is one page. The moment a second page exists, this is the first thing to factor out, and doing it now would be designing for a consumer that does not exist.
- **The ledger's prose is stale within a week of being written** (`coverage-ledger` D12 assumed this rate, and three claims went stale two days after the audit). The page makes that prose more visible without making it more true. → `audited_against` is surfaced next to the report's `zs_version` precisely so a reader sees how old the judgments are, which is the only honest answer available until the audit moves.

## Open Questions

- Should the coverage section show `scenario_roots`? It is what the drift check walked, so it bounds what "every scenario is claimed" means, but it is also plumbing that most readers do not need. Leaning toward showing it in a small print footer of that section rather than in the header.
- Does the later Pages change want the output at a fixed path rather than an arbitrary `--out`? Not decided here on purpose: `--out` is strictly more general, and a fixed convention is the deploying change's call to make once it knows how it publishes.
