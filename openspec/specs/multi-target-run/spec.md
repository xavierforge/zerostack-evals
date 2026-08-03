# multi-target-run

## Purpose

`run` accepts `--target` more than once to evaluate a suite against several targets sequentially under one shared budget, nesting each target's results under its stem and reporting the most severe exit code across columns.

## Requirements

### Requirement: Repeated --target evaluates each target sequentially
The `run` subcommand SHALL accept `--target` more than once; each occurrence names one target the suite is evaluated against. Targets SHALL run sequentially, one fully completing before the next begins. A single `--target` SHALL behave exactly as before this change.

#### Scenario: Two targets produce two reports
- **WHEN** `run` is given `--target a.toml --target b.toml`
- **THEN** the suite runs once per target and one report is produced per target

#### Scenario: N=1 is unchanged
- **WHEN** `run` is given a single `--target`
- **THEN** the results layout and report location are identical to before this change

### Requirement: One shared budget across all targets
`--max-total-usd` SHALL bound the entire invocation, not each target. Spend SHALL accumulate across targets in the order they run.

#### Scenario: The cap is a total, not per target
- **WHEN** `--max-total-usd` is set and the targets run in order
- **THEN** spend accumulates across targets and a later target sees the budget already partly consumed by earlier ones

### Requirement: A budget-truncated column is marked incomplete
WHEN the shared budget cap stops a target before it has run every scenario, the table SHALL mark that column incomplete rather than letting its missing rows read as an ordinary absence. The mark SHALL be keyed off the truncation fact the run records (`Report.budget_truncated`), NOT off a scenario count, so that a column which simply ran a smaller suite in full is never mistaken for a truncated one.

#### Scenario: A cut-short column is flagged
- **WHEN** a run recorded `budget_truncated` because the shared cap was reached before a declared scenario
- **THEN** that column is marked incomplete in the rendered table

#### Scenario: A complete smaller suite is not flagged
- **WHEN** a column ran every scenario its own suite defines but that suite is smaller than another column's (e.g. a shorter committed baseline), and it was not budget-truncated
- **THEN** that column is NOT marked incomplete, and its missing scenarios render as ordinary `-` holes

### Requirement: --target is mandatory for the zs backend
The `zs` backend SHALL require `--target`. WHEN it is selected with no `--target`, the command SHALL exit 2 before any trial runs, and `"provider-default"` SHALL NOT be recorded as a model.

#### Scenario: Missing target on zs is a usage error
- **WHEN** `run --backend zs` is invoked with no `--target`
- **THEN** the command exits 2 with a usage error and runs no trial

### Requirement: --target is rejected for the mock backend
The `mock` backend SHALL reject `--target`. WHEN it is selected with `--target` given, the command SHALL exit 2. A mock run SHALL record its model as `"mock"`.

#### Scenario: Target on mock is a usage error
- **WHEN** `run --backend mock=<path>` is given `--target`
- **THEN** the command exits 2

#### Scenario: A mock run records the mock model
- **WHEN** a mock run completes
- **THEN** its report's `model` is `"mock"`

### Requirement: N>1 nests results under the target stem
WHEN more than one target runs, each target's results, its `report.json`, a copy of its `target.toml`, and its per-scenario trial directories, SHALL live under `results/<tag>/<stem>/`, where `<stem>` is the target file's basename without extension. The N=1 layout SHALL stay flat at `results/<tag>/`. Two targets whose files share a stem within one run SHALL be a hard error.

#### Scenario: Two targets nest under their stems
- **WHEN** targets `a/opus.toml` and `b/sonnet.toml` run together
- **THEN** their reports and trial directories are written under `results/<tag>/opus/` and `results/<tag>/sonnet/`

#### Scenario: A clean target copy is kept at the run level
- **WHEN** a target runs
- **THEN** a clean copy of its `target.toml` is written at the run level, distinct from the per-trial isolated config dir which a scenario `config:` seed can overwrite

#### Scenario: Stem collision within one run errors
- **WHEN** two `--target` files share a basename
- **THEN** the run exits with an error rather than silently overwriting one target's directory

### Requirement: An N>1 run has no single JSON form
WHEN `run --json` is combined with more than one `--target`, the command SHALL exit 2 and direct the caller to `zseval matrix --json`. `run --json` with a single target SHALL still emit the report JSON on stdout.

#### Scenario: JSON with multiple targets is a usage error
- **WHEN** `run --json` is given two `--target` flags
- **THEN** the command exits 2 and names `zseval matrix --json` as the way to get machine output

### Requirement: The run table is written to stderr
WHEN a run completes, its human-readable table and summary SHALL be written to stderr, keeping stdout reserved for the machine (JSON) contract.

#### Scenario: Redirecting stdout does not capture the table
- **WHEN** `run --target a --target b` completes with stdout redirected to a file
- **THEN** the table appears on stderr and the redirected file is not polluted by it

### Requirement: The run exit code is the most severe across columns
WHEN N targets run, the process exit code SHALL be the most severe of the per-target codes: 2 if any column is fully ungradable, else 1 if any trial graded Fail, else 0. A column holding no scenarios at all (reachable only when the budget cap shuts a target out before its first scenario) is a recorded fact (`budget_truncated`, marked in the table), not a harness error: it contributes 0, deliberately diverging from `matrix`, which exits 2 when handed a zero-scenario report (the 2026-07-21 target-matrix design ruling: the two realise "fully ungradable" differently on purpose).

#### Scenario: A failing column makes the run fail
- **WHEN** one column has a trial graded Fail and another column is clean
- **THEN** the run exits 1

#### Scenario: A fully-ungradable column is a harness error
- **WHEN** any column has no gradable scenario at all
- **THEN** the run exits 2

#### Scenario: A budget-emptied column does not redden the run
- **WHEN** the budget cap stops a target before its first scenario, so its column holds zero scenarios
- **THEN** the column is marked budget-truncated and contributes exit 0, while `matrix`, handed that same zero-scenario report, exits 2
