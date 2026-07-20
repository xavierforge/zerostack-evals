## Why

Answering "model A or model B?" today means running the suite against each target
separately and eyeballing two reports whose numbers were produced under different
conditions. There is no single table that lays N targets side by side, and no way
to reuse a prior run as one column without re-paying to run it. The result is that
the one question the harness exists to answer, "which model is better on this
suite", has no durable, comparable, re-renderable artifact.

## What Changes

- `zseval run` accepts **repeated `--target`**: one invocation evaluates the suite
  against N targets sequentially, under one shared `--max-total-usd` budget, and
  prints a scenario x target table to stderr on completion.
- **BREAKING** `--target` becomes mandatory for the `zs` backend (missing => exit
  2) and is rejected for the `mock` backend (given => exit 2). The
  `"provider-default"` model label is removed: it is now an unreachable state. A
  `mock` run records its model as `"mock"`.
- **BREAKING** `run --json` with N>1 targets is a usage error (N reports have no
  single JSON form); it points the caller at `zseval matrix --json`.
- New subcommand **`zseval matrix <report.json>... [--json] [--markdown]`**: a pure
  renderer over existing reports. It makes no API calls and writes nothing to disk.
  This is the primary entry point for composing a table across runs and across
  time (reusing a committed baseline as one column).
- The total table marks what cannot be silently trusted: **SPREAD** (targets
  genuinely disagree on a scenario), **DRIFT** (the measuring stick, judge or
  scenario definition, changed between columns), and columns cut short by the
  budget cap are marked **incomplete**. Cells that were not run or could not be
  graded render as `-`, kept distinct from a real `0.000`.
- `Report` gains a **`target`** field (the target file, path-normalized) so a
  report carries its own column identity even when detached from its run
  directory. `REPORT_SCHEMA_VERSION` is frozen at `1` (nothing reads it; the field
  is decorative until a real consumer needs it).
- New committed **`experiments/`** directory: dated markdown snapshots of a matrix
  run, written once by hand via redirect and never regenerated.
- Documentation wording that currently justifies cross-target `compare` as the
  "A/B use case" is corrected: A/B moves to `matrix`; `compare`'s remaining
  cross-target purpose is the migration gate (a real baseline to decide a switch).

## Capabilities

### New Capabilities
- `multi-target-run`: `run` accepting repeated `--target`, sequential execution
  under a shared budget, the N>1 nested results layout with a run-level target
  copy, mandatory/rejected `--target` per backend, the `mock` model label, and the
  N>1 `--json` usage error.
- `matrix-render`: the `matrix` subcommand and the total-table specification, cells
  and the `-` vs `0.000` distinction, the footer recomputed over the common
  scenario set, SPREAD and DRIFT marking, incomplete-column marking, column
  identity and legend, duplicate-column handling, the incomparability layers, exit
  codes, and the two renderers (fixed-width and markdown).
- `report-target`: the `Report.target` field, its path normalization (reusing the
  `report-paths` rule), the identity-vs-content split against the run-level
  `target.toml`, and freezing `REPORT_SCHEMA_VERSION`.
- `experiments-record`: the committed `experiments/` directory, its never-regenerate
  discipline, and the provenance a snapshot must embed.

### Modified Capabilities
<!-- No existing spec's requirements change. `report-target` reuses report-paths'
     normalization rule but adds a new field rather than changing report-paths. -->

## Impact

- Code: `main.rs` (flag parsing, `--target` arity and backend rules, new `matrix`
  subcommand, N>1 dispatch), `runner.rs` (shared-budget loop over targets, nested
  run_root and trial-dir layout, run-level target copy), `backend.rs` (mock target
  rejection), `verdict.rs` (`Report.target`, schema freeze), a new renderer module,
  `compare.rs` and `README.md` (wording only).
- Behavior: three new exit-2 usage errors; the results directory layout gains a
  stem level at N>1 (N=1 unchanged).
- Data: existing schema <= 3 baselines are retired and regenerated rather than
  migrated; `matrix` requires the `target` field and does not accept legacy
  reports.

## Non-Goals

- **Pairwise compare.** The total table generalises it; pairwise would narrow a
  matrix into head-to-head bouts and smuggle in "regression", a notion that needs a
  baseline, among N equal peers.
- **Changing the target file format.** It stays an ordinary zerostack
  `config.toml`, copied verbatim.
- **Changing `compare`'s behavior.** Wording only.
- **The truncation fix** for a budget-capped run silently reporting zeros: this
  change marks a truncated column rather than fixing the underlying silent zero.
- **Parallel execution across targets**, which would race the shared budget and
  interleave per-trial output.
- **Backward compatibility for legacy reports.** Schema <= 3 baselines are retired
  and regenerated, not migrated.
