## ADDED Requirements

### Requirement: `site` renders one report and the ledger into one file
`zseval site <report.json> --out <file.html>` SHALL read exactly one report and the coverage ledger and write one HTML file. It SHALL make no API call, invoke no backend, and grade nothing, so it costs nothing to run and runs offline. `--out` SHALL be required: writing the page is what the subcommand is for.

The ledger SHALL be read from `scenarios/coverage.toml` relative to the repository root. A `--ledger <path>` override SHALL exist for tests to point at a fixture, and SHALL be documented as such rather than as a general-purpose option. A missing or unloadable ledger SHALL be an error, never a page with the coverage section omitted: a page missing its denominator is what this capability exists to stop shipping.

#### Scenario: A report and a ledger produce a page
- **WHEN** `site` is given a valid report and `--out`
- **THEN** it writes one HTML file at that path and exits 0

#### Scenario: Rendering spends nothing
- **WHEN** `site` runs with no API key present and no network
- **THEN** it still writes the page, because it calls no model and no backend

#### Scenario: A missing ledger is an error
- **WHEN** `scenarios/coverage.toml` cannot be read
- **THEN** the command fails naming the path, and writes no partial page

### Requirement: `site` supports `--json`, which emits the page model
`site` SHALL support `--json`, emitting the page model to stdout: the header's read-back fields, the coverage rows with their marks, and the matrix model. This preserves the repo-wide invariant that every subcommand takes `--json`, and it carries the same meaning it carries for `matrix`: machines read the model, never the rendered form. `--json` SHALL NOT replace `--out`; both may be given, and `--out` is required either way.

#### Scenario: The model is available without parsing HTML
- **WHEN** `site` is invoked with `--json`
- **THEN** it prints the page model as JSON to stdout, and still writes the HTML to `--out`

### Requirement: `site` exit codes
`site` SHALL exit 0 when a page was written and 2 when it could not be written. `site` SHALL have no exit 1: it is a view, not a gate, so low pass rates, a fully ungradable report, and a stale ledger never produce a nonzero exit on their own. This is the same rule `matrix` follows, deliberately, rather than a second convention for the same category of subcommand.

#### Scenario: A rendered page exits 0
- **WHEN** the page is written, whatever the scores it shows
- **THEN** the command exits 0

#### Scenario: A page that could not be written exits 2
- **WHEN** the report cannot be loaded, the ledger cannot be loaded, or the output path cannot be written
- **THEN** the command exits 2

### Requirement: The drift check gates generation
`site` SHALL run the ledger's bidirectional drift check against the repository's scenario tree before writing anything, and SHALL abort with exit 2 if it does not pass. A ledger that disagrees with the tree makes the coverage section describe scenarios that do not exist, or omit ones that do; the page would be false, and a false page is worse than no page.

The check running here SHALL NOT replace its enforcement in `cargo test --workspace`. A failure at generation time means the page was being generated from a tree that never passed its own tests, which is exactly the case worth refusing.

#### Scenario: A dead reference aborts before any output
- **WHEN** the ledger cites a scenario id that exists under no root
- **THEN** `site` exits 2 naming that id, and no file is written at `--out`

#### Scenario: An unclaimed scenario aborts before any output
- **WHEN** the tree holds a scenario no covered claim cites
- **THEN** `site` exits 2 naming that id, and no file is written at `--out`

### Requirement: An `audited_against` mismatch is disclosed, never fatal
When the ledger's `audited_against` does not appear in the report's `zs_version`, `site` SHALL render the page anyway and state both strings and which is which: the version the ledger's judgments were made against, and the version this run measured. It SHALL NOT abort, and SHALL NOT exit nonzero.

This is required rather than merely permitted. `coverage-ledger` guarantees that the worst outcome of the containment comparison is a spurious mismatch notice and never a wrong version claim and never a blocked publish, and a `--backend mock` report records `mock`, so it mismatches by construction. A gate here would break that guarantee.

#### Scenario: A mock report renders with the mismatch shown
- **WHEN** the report's `zs_version` is `mock` and the ledger records `1.7.2`
- **THEN** the page is written, exits 0, and shows both values labelled

#### Scenario: A matching version is shown as agreeing
- **WHEN** `audited_against` appears in `zs_version` as a whole version
- **THEN** the page states that the audit and the run agree, without inventing a stronger claim than containment supports

### Requirement: The header reads report fields back without deriving them
The header section SHALL present the run's identity by reading report fields verbatim, with no inference, no defaulting, and no computed substitutes: `zs_version`, `zs_bin_sha256`, `zs_bin_path`, `git_sha`, `features`, `model`, `backend`, `target`, `timestamp`, `trials`, `summary.total_cost_usd`, and `budget_truncated`.

A field that is `null` SHALL be shown as not provided, distinctly from a field that is present and empty. An absent fact and an empty fact are different claims, which is why `git_sha` and `features` are `null` against today's binary rather than empty lists.

The judge SHALL be shown as two facts, never collapsed into one: `judge_file` with `judge_hash` for what the run was configured to grade with, and `judge_model` for what actually graded. `judge_model` SHALL keep its three readings distinct: unknown, nothing graded, and these rulers graded.

#### Scenario: An unavailable build fact is not shown as empty
- **WHEN** the report's `git_sha` and `features` are `null`
- **THEN** the header shows them as not provided, not as an empty string or an empty list

#### Scenario: Configured and actual rulers are both shown
- **WHEN** a report names a judge file and records the models that answered
- **THEN** the header shows both, labelled so neither reads as the other

#### Scenario: Nothing graded is not the same as unknown
- **WHEN** `judge_model` is an empty list
- **THEN** the header says nothing was graded, and does not echo the configured model as though it had answered

### Requirement: The coverage section shows every area and no percentage
The coverage section SHALL list all areas the ledger declares, in ledger file order, which is presentation order. An area with no `covered` claim SHALL be listed rather than omitted: it is the row a suite-derived count structurally cannot state.

Each claim SHALL be shown under its status, carrying the evidence that status owes: cited scenario ids for `covered`, the `blocked_by` sentence when an `uncovered` claim has one, the `reason` for `product-blocked` and `excluded`, the `zs` pointer where one exists, and any `note`.

No coverage percentage or ratio SHALL appear anywhere on the page. The headline figure SHALL be a count of areas with no scenario at all. Fine-grained `covered` claims sit beside coarse `uncovered` ones, so a ratio over them is manipulable by re-slicing while the count is not.

#### Scenario: A zero-coverage area is listed
- **WHEN** an area has no `covered` claim
- **THEN** it appears in the coverage section, and is counted in the headline figure

#### Scenario: No ratio appears
- **WHEN** the page is rendered
- **THEN** no coverage percentage or ratio appears in it, in any section

#### Scenario: Presentation follows file order
- **WHEN** the ledger's areas are reordered and the page is regenerated
- **THEN** the coverage section's order changes to match, with no sorting applied

### Requirement: Covered claims this run did not exercise are marked
For each `covered` claim, `site` SHALL mark the cited scenario ids that do not appear in this report's results, as covered but not exercised by this run. The mark SHALL be derived at render time from the report, and SHALL NOT be recorded in the ledger.

`covered` is an existence claim by `coverage-ledger`'s definition, and run membership is a property of a report; recording it in the ledger would make the ledger stale on every run. Because the drift check has already passed, a cited id that is missing from the report exists in the tree, so "exists, was not exercised here" is the only available reading.

#### Scenario: A scenario absent from this report is marked
- **WHEN** a covered claim cites an id the report's results do not contain
- **THEN** the coverage section marks it as covered but not exercised by this run

#### Scenario: A partially exercised claim marks only the missing ids
- **WHEN** a covered claim cites three ids and the report contains two of them
- **THEN** only the third is marked, and the claim is not reported as uncovered

### Requirement: The results section reuses the matrix model and its meanings
The results section SHALL be built by `matrix`'s model builder over the single report and rendered by the HTML renderer over that same model. Cells, holes, per-kind grouping, the footer recomputed over the common gradable set, the SPREAD and DRIFT marks, and the footer-excluded disclosure SHALL keep the meanings `matrix-render` already defines for them. The page SHALL NOT compute a second, differently-defined pass rate.

#### Scenario: A hole renders as a hole
- **WHEN** a scenario has no gradable trial in the report
- **THEN** the results section shows the same hole the other renderers show, not a zero

#### Scenario: Rows are grouped by kind
- **WHEN** the report holds both regression and capability scenarios
- **THEN** the results section groups them by kind, as `matrix` does

### Requirement: Every runtime value written into the page is escaped
`site` SHALL escape every runtime value at the point it is written into the page, with no exemption for values judged unlikely to contain markup. `zs_version` is captured verbatim from an external binary's output and is deliberately not format-validated, and paths come from a filesystem; a value carrying markup would otherwise land in the page and, at worst, execute for whoever opens it.

The rule SHALL be uniform rather than a list of untrusted fields. Author-controlled ledger prose and scenario ids are escaped too, because a rule with exemptions requires every future editor to re-derive which fields are trusted.

#### Scenario: A hostile version banner is escaped
- **WHEN** a report's `zs_version` contains HTML markup
- **THEN** the page contains the escaped form of that text and not the raw markup

#### Scenario: Ledger prose is escaped as well
- **WHEN** a claim's `reason` contains characters that are significant in markup
- **THEN** they appear escaped in the page

### Requirement: The page is self-contained
The output SHALL be a single file that renders with no network access and no other file: CSS inline, no JavaScript, no external stylesheet, font, image, or script reference. A page that fetches anything at view time breaks when opened from a file path, when the network is absent, and when what it fetched moves, so it is not a deliverable artifact.

#### Scenario: The page renders from a bare file path
- **WHEN** the written file is opened with no network available
- **THEN** it renders completely, with its styling intact

#### Scenario: No external reference is emitted
- **WHEN** the page is generated
- **THEN** it contains no reference to any external URL, stylesheet, font, image, or script
