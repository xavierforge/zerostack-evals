# matrix-render

## Purpose

The `matrix` subcommand renders a scenario x target comparison table from one or more `report.json` files, side-effect free, with fixed-width, JSON, and markdown output, and heuristics (SPREAD, DRIFT) that flag disagreement and incomparability without adjudicating them.

## Requirements

### Requirement: matrix renders reports without side effects
The `matrix` subcommand SHALL accept one or more `report.json` paths and render a scenario x target table. It SHALL make no API calls and SHALL write nothing to disk.

#### Scenario: Rendering creates no files
- **WHEN** `matrix` is given a set of report files
- **THEN** it prints a table and creates or modifies no files on disk

### Requirement: matrix supports --json and --markdown
`matrix` SHALL emit its output to stdout: the fixed-width table by default, structured JSON under `--json`, and markdown under `--markdown`. This preserves the invariant that every subcommand takes `--json`.

#### Scenario: JSON output on stdout
- **WHEN** `matrix ... --json` is invoked
- **THEN** structured JSON is written to stdout

### Requirement: Cells are the trial pass rate, holes are dashes
Each cell SHALL be the scenario's `trial_pass_rate` for that column. A scenario a column did not run, or ran but could not grade (no graded trials), SHALL render as `-`, kept distinct from a graded `0.000`.

#### Scenario: All graded trials failed shows a real zero
- **WHEN** a column graded a scenario and every graded trial failed
- **THEN** the cell shows `0.000`

#### Scenario: Ungradable shows a dash
- **WHEN** a column has no graded trial for a scenario, because it did not run it or every trial was indeterminate
- **THEN** the cell shows `-`

### Requirement: The footer is recomputed over the common scenario set
The footer (pass@k, pass^k, total cost) SHALL be computed over the scenarios present and gradable in every column, so columns are compared on the same denominator. WHEN the common set is smaller than some column's own set, the table SHALL note which scenarios were excluded from the footer.

The footer SHALL render three metric groups — regression, capability, and overall, in that order — each computed over the common gradable set filtered to that kind (overall over the whole common set, unchanged in definition from before). A kind with no gradable scenario in the common set renders its rates as `n/a`, the same convention as a report's own summary. The grouping adds footer rows only, never width: the fixed-width budget (48 + 12N) is untouched.

#### Scenario: Different scenario sets use the intersection
- **WHEN** two columns ran different scenario sets
- **THEN** the footer is computed over their common gradable scenarios and the excluded scenarios are listed

#### Scenario: Identical suites match each report's own summary
- **WHEN** all columns ran the same suite
- **THEN** the footer equals each column's own report summary and nothing is excluded

#### Scenario: Footer groups by kind
- **WHEN** the common set contains both kinds
- **THEN** the footer shows regression pass@k/pass^k, capability pass@k/pass^k, and overall pass@k/pass^k as separate row groups, regression first, overall last

#### Scenario: A kind absent from the common set is n/a
- **WHEN** the common gradable set contains no capability scenario
- **THEN** the capability footer rows render `n/a` for every column

### Requirement: Rows are grouped by kind
The scenario rows SHALL render in two sections — regression first, then capability — with a section marker between them, reading each scenario's `kind` from the report rows (never from scenario.toml). Within a section, row order follows the existing ordering. JSON output carries `kind` per row and keeps its existing flat array shape; the sectioning is a rendering concern.

#### Scenario: Two sections in kind order
- **WHEN** a matrix renders reports containing both kinds
- **THEN** regression rows appear first under a regression marker, capability rows after under a capability marker

#### Scenario: JSON stays flat
- **WHEN** `matrix --json` renders the same reports
- **THEN** rows form one array, each row carrying its `kind`, with no section nesting

### Requirement: SPREAD marks scenarios where targets genuinely disagree
A row SHALL be marked SPREAD when `max(row) - min(row)` over its gradable cells exceeds `1 / min(n_graded_trials of those cells)`, a threshold derived from the data rather than a fixed constant. Ungradable (`-`) cells SHALL be excluded from the max, the min, and the threshold. SPREAD is a display heuristic, not a statistical claim, and SHALL be labelled as such.

#### Scenario: A gap beyond one trial's resolution is marked
- **WHEN** a scenario's pass rates differ across targets by more than one trial's resolution
- **THEN** the row is marked SPREAD

#### Scenario: A gap within one trial's resolution is not marked
- **WHEN** the difference across targets is within one trial's resolution
- **THEN** the row is not marked SPREAD

### Requirement: DRIFT marks columns measured by a changed ruler
The matrix has no baseline, so DRIFT SHALL mark, never adjudicate. WHEN a scenario's `content_hash` differs across the cells of a row, that row SHALL be marked DRIFT and the differing columns listed. WHEN columns were graded by different rulers (`judge_hash` differs, or a column's judge is unknown while another column's is known), the affected column(s) SHALL be marked DRIFT. WHEN no column carries a known judge (e.g. every column ran `--no-judge`), there is no known ruler that could have moved, and no column SHALL be marked for its judge. DRIFT SHALL be phrased as "may not be comparable, look", not a verdict, and SHALL NOT pick any column as the correct one.

#### Scenario: A scenario redefined between columns is a per-row DRIFT
- **WHEN** one scenario's `content_hash` is not the same in all columns
- **THEN** that row is marked DRIFT and the differing columns are listed

#### Scenario: An unrecorded content hash is not a mismatch
- **WHEN** a column's copy of a scenario carries an empty `content_hash` (a hand-built report; the run path always records one)
- **THEN** that column does not participate in the row's hash comparison: observing no hash is not observing a change

#### Scenario: A column judged by a different ruler is a per-column DRIFT
- **WHEN** a column's `judge_hash` differs from the others, or its judge is unknown
- **THEN** that column is marked DRIFT

#### Scenario: All-unknown judges drift nothing
- **WHEN** no column in the matrix records a known judge identity
- **THEN** no column is marked DRIFT for its judge: with no known ruler anywhere, there is no evidence any ruler moved

### Requirement: Column identity and the legend
A column SHALL be labelled in the header by its target stem. The full provider/model, the target path, the judge tri-state (unknown / nothing-graded / the listed rulers), and the prompt pack identity (its path with a short content hash, or a plain marker when the column used no pack) SHALL appear in the legend rather than the header. A report that carries no target identity SHALL NOT be a column: `matrix` SHALL exit 2 naming it.

#### Scenario: The header is the stem, the legend has the rest
- **WHEN** a report with target `targets/opus.toml` is a column
- **THEN** the header shows `opus` and the legend shows its full provider/model, path, judge tri-state, and pack identity

#### Scenario: A report without a target identity is rejected
- **WHEN** a report has no target identity
- **THEN** `matrix` exits 2 naming that report as one it cannot render as a column

#### Scenario: Two columns of the same pack path with different contents
- **WHEN** two columns record the same pack path but different pack hashes
- **THEN** their legend lines differ in the displayed short hash

### Requirement: A column is marked when the pack moved alongside the target
`matrix`'s own axis is the target, so columns differing only by target are the table working as intended, and columns sharing a target and differing only by pack are equally a clean single-variable comparison. WHEN the columns differ in both target and pack identity, the affected column(s) SHALL be marked, because no cell difference can be attributed to either. This mark SHALL be distinct from DRIFT, which stays reserved for a changed ruler, and SHALL appear in the legend beside the existing per-column marks. Like SPREAD and DRIFT it is a display heuristic that says "look here", never a verdict about which column is correct.

#### Scenario: Same target, different packs is not marked
- **WHEN** two columns share a target and differ only by pack
- **THEN** neither column is marked for multiple variables, and their packs are visible in the legend

#### Scenario: Different targets and different packs is marked
- **WHEN** columns differ in both target and pack identity
- **THEN** the affected columns are marked, and the mark is not DRIFT

#### Scenario: Different targets, one shared pack is not marked
- **WHEN** columns differ by target and all record the same pack identity
- **THEN** no column is marked for multiple variables

### Requirement: Duplicate columns for one target across time are allowed
Two columns with the same stem (the same target evaluated at different times) SHALL be allowed, so a target can be compared against its own past. Same-stem columns SHALL be disambiguated in the header, for example by tag or timestamp.

#### Scenario: Same target twice is disambiguated, not rejected
- **WHEN** two reports for the same target from different runs are given as columns
- **THEN** both render, disambiguated in the header, rather than colliding or erroring

### Requirement: Incomparability is layered
Partial scenario overlap between columns SHALL surface as `-` cells, not an error. A changed ruler SHALL surface as DRIFT, not an error. Only when two reports share no scenario at all SHALL `matrix` hard-error, naming the report that cannot join the table.

#### Scenario: Partial overlap leaves dashes
- **WHEN** one column ran a scenario another column did not
- **THEN** the missing cell is `-` and no error is raised

#### Scenario: Zero overlap is a hard error
- **WHEN** a report shares no scenario id with the rest of the table
- **THEN** `matrix` exits 2 naming that report

### Requirement: matrix exit codes
`matrix` SHALL exit 0 when a table rendered and 2 when it could not render or when any column is fully ungradable. `matrix` SHALL have no exit 1: it is a view, not a gate, so low scores alone never produce a nonzero exit.

#### Scenario: A rendered table exits 0
- **WHEN** the table renders, whatever the scores
- **THEN** the command exits 0

#### Scenario: A fully-ungradable column exits 2
- **WHEN** any column has no gradable scenario at all
- **THEN** the command exits 2

### Requirement: Renderers over one model, shared across subcommands
`matrix` SHALL provide a fixed-width terminal renderer, a markdown renderer (for records, no width limit), and an HTML renderer, all over the same `Matrix` model. The `run` subcommand SHALL reuse the fixed-width renderer function for its stderr table, and the `site` subcommand SHALL reuse the HTML one for its results section.

`matrix`'s own command-line surface is unchanged by the third renderer: `matrix` SHALL still emit fixed-width by default, markdown under `--markdown`, and JSON under `--json`, and SHALL NOT gain an HTML flag. The HTML renderer is reachable only through `site`.

Renderers live together in the same module rather than beside the subcommands that use them, because the cell, hole, footer-figure and row-mark formatting is shared and private. A renderer outside the module would have to restate that formatting, and three independent answers to "how is a hole written" drift apart on the first change.

#### Scenario: Markdown on request, fixed-width by default
- **WHEN** `matrix` is invoked with `--markdown`
- **THEN** it emits markdown; without it, it emits the fixed-width table

#### Scenario: `matrix` gains no HTML flag
- **WHEN** `matrix` is invoked with any combination of its own flags
- **THEN** it never emits HTML; the HTML renderer is reached only by `site`

#### Scenario: Every renderer reads the same model
- **WHEN** the same report is rendered as fixed-width, markdown, and HTML
- **THEN** all three report the same cells, the same holes, the same per-kind grouping, and the same footer figures, because none of them recomputes the model
