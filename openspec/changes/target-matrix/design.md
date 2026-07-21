## Context

The harness today runs one suite against one target and writes a single report to
`results/<tag>/report.json`, with per-scenario trial dirs beside it. Several
foundations for this change already exist on `main`:

- `--model` is gone; `describe(Option<&Path>)`, `AgentBackend::run(sc, run_dir)`,
  and `auto_tag(path, target)` take no model argument.
- The judge is a rig-core provider card. `Report` already carries `judge_file`,
  `judge_hash`, and `judge_model: Option<Vec<String>>` (the three-state list).
- `Flags.kv` is a `Vec<(String, String)>` that already collects every `--target`
  occurrence; only `get()`'s last-wins lookup hides the extras. `count()` exists.
- `describe(None)` returns `"provider-default"`, used as `Report.model` when no
  target is set.
- `REPORT_SCHEMA_VERSION` is `3` and is written but read by no code path (verified:
  only set at build and asserted in tests).
- The budget cap in `run_suite` breaks the scenario loop when `spent >= cap`, on a
  `spent` local to that one `run_suite` call.

The one question the harness exists to answer, "A or B on this suite", has no
single comparable, re-renderable table. This change adds it.

## Goals / Non-Goals

**Goals:**
- One invocation evaluates N targets and yields a scenario x target table.
- The table is re-renderable from stored reports (no re-spend), composable across
  runs and across time (reuse a baseline as a column), and retainable as a record.
- When the measuring stick moved between columns, the table says so; it never
  presents non-comparable numbers as comparable.

**Non-Goals:**
- Pairwise `compare` (the table generalises it; `compare` keeps its migration-gate
  role, wording only).
- Changing the target file format or `compare`'s behaviour.
- The truncation fix for a budget-capped run's silent zeros; this change marks
  truncated columns rather than fixing the underlying silent-zero.
- Parallel execution across targets.

## Decisions

**Repeatable `--target`, not `--matrix` or `--targets a,b`.** Add `Flags::get_all`
over the existing `kv`; the collection is already there. `--matrix` invents a noun
absent from the domain and promises a 2-D scope beyond this change; `--targets`
comma-splitting is lossy on paths and overlaps `--target` semantically. Repeated
`--target` composes cleanly with shell loops and is unambiguous.

**Targets run sequentially; `--jobs` stays trial-level.** Parallel targets would
race the shared `spent`, interleave `print_trial_line` (which has no target
column) into unreadable output, and fire N x jobs requests at one provider. The
runner already made the same call for scenarios (the cost cap is scenario-granular).

**Layout B: N=1 flat, N>1 nested under the stem.** N=1 keeps today's shape
(`results/<tag>/`), so nothing existing breaks. N>1 adds one level
(`results/<tag>/<stem>/`). Alternative A (always nested) touches ~8 sites including
a committed test and the baselines ritual, and still needs a special case because
the target-less mock run has no stem. Layout B is zero-breakage.

> Implementation trap (verified): the results prefix is computed in **two** places,
> `run_root` at `runner.rs:144` (report.json + the run-level target copy) and the
> trial dir at `runner.rs:169` (`results_root.join(tag).join(sc.id).join(trial-N)`).
> Both must gain the stem at N>1, or the report and its trial dirs split across two
> directories and two targets' trial dirs collide on `sc.id`. Compute `run_root`
> once and thread it into `run_trials_for_scenario` rather than re-deriving.

**Stem = target filename without extension, not provider-model.** Provider-model
reads only two of a config's ~30 fields; two targets differing only in temperature
would collide and silently overwrite. A filename is unique by construction. A stem
collision within one run is a hard error.

**`matrix` is a pure renderer; `run` reuses it.** `matrix <report.json>...` makes no
API calls and writes nothing; it is the primary entry for cross-time composition.
`run` calls the same renderer for its end-of-run table. The `run` table goes to
stderr (stdout stays the machine/JSON channel, as the existing summary line already
does); the `matrix` table goes to stdout (it is that command's product). `run --json`
at N>1 is a usage error: N reports have no singular JSON form.

**`Report.target` is new; normalise via the existing `record_path`.** The field is
column identity and must survive a report being copied into `baselines/`, so it
follows the `report-paths` rule (relative, forward-slashed, never absolute), reusing
`record_path`. `target.toml` (content) lives only in the run dir; `Report.target`
(identity) travels with the report. `REPORT_SCHEMA_VERSION` is frozen at `1`: nothing
reads it, so bumping it is decorative; `#[serde(default)]` does the real
backward-load work. Consumers check for a non-empty target on its own merits.

**Cells vs holes.** A cell is `trial_pass_rate`. `trial_pass_rate()` returns `0.0`
both for "ran, all failed" and "nothing gradable", so the renderer MUST consult
`n_graded_trials()` / scenario presence to choose `-` (hole) vs `0.000` (real zero).

**Footer over the intersection.** pass@k / pass^k / total cost are recomputed over
the scenarios gradable in every column, so the overall numbers are comparable; the
excluded scenarios are listed. Alternative (each report's own summary) places two
different denominators side by side, the exact silent mislead this design rejects.
When all columns ran the same suite, the two agree.

**SPREAD is data-derived and hole-safe.** Threshold `1 / min(n_graded_trials over
the row's gradable cells)`; `-` cells are excluded from max, min, and the threshold
(also avoiding a divide-by-zero). It is a display heuristic, labelled as such; a
confidence-interval treatment (Wilson) supersedes it later.

**DRIFT marks, never adjudicates.** `content_hash` mismatch across a row marks the
row (list the differing columns); `judge_hash` mismatch (or unknown judge) marks the
column. The matrix has no baseline, so it never picks a "correct" column. Phrasing is
"may not be comparable, look", because `content_hash` is over-sensitive (it flips on a
comment edit).

**Columns are identified by stem, and a repeated stem is allowed.** The header
carries the stem because it is short; the full provider/model, path, and judge
tri-state go to the legend, which is what keeps `48 + 12N` workable. Two columns
with the same stem are *not* an error at render time, unlike the same-stem
collision inside one run's directory layout: the same target evaluated at two
times is a legitimate "did this target regress" view, so same-stem columns are
disambiguated by tag or timestamp instead of rejected.

**Incomparability is layered, and content-based.** Partial overlap -> `-` cells;
changed ruler -> DRIFT; only zero shared scenarios -> hard error naming the report. A
report with no `target` identity is likewise rejected, on its own merits (no column
identity), never by gating on `schema_version`. Legacy schema <= 3 baselines are
retired and regenerated, not migrated.

**Budget is one shared total; truncation is marked.** The orchestrator feeds each
target a shrinking cap (`total - spent_so_far`). `run_suite`'s per-scenario break needs
no change to its stopping logic, but it now records the truncation as a fact on the
report (`Report.budget_truncated`) when the cap stops it before a declared scenario.
The renderer marks the column incomplete off that recorded fact, NOT off a scenario
count: a bare count cannot tell a budget-cut column apart from one that simply ran a
smaller suite in full (a shorter committed baseline reached completely), and marking the
latter incomplete would be a false alarm. A column's missing scenarios render as `-`
holes regardless; the incomplete `*` is reserved for a real truncation.

**`run` and `matrix` diverge on a zero-scenario column, deliberately.** Both
honour "a fully ungradable column exits 2", but they realise it differently and
the difference is intended, not an oversight to reconcile later. `run`'s
aggregate is `max` over each report's `Report::exit_code`, which scores an
*empty* (zero-scenario) report `0`: a budget cut that shuts a target out before
its first scenario is already a recorded fact (`budget_truncated`, marked in the
table), not a harness error, so it must not turn the whole invocation's exit code
red. `matrix` instead reads its exit code off the rendered table and treats an
all-holes column as ungradable (`2`), because at render time an empty column is
indistinguishable from a genuinely ungradable one, and a viewer asking "can these
compare?" should hear "no". So `run --target a --target b` with `b` budget-starved
can exit `0` while `matrix` over those same two reports exits `2`. This is not
collapsed to one rule on purpose: `run` answers "did the suite pass", `matrix`
answers "is this table trustworthy", and those are different questions.

**mock records `"mock"`.** With `--target` mandatory for zs and rejected for mock,
`describe(None)` / `"provider-default"` is unreachable and is deleted. A mock run's
model is `"mock"`, set on the mock path rather than derived from an absent target.

**`experiments/` is a committed, never-regenerated record.** Markdown only (small),
so unlike gitignored `results/` it belongs in git. Written by hand via redirect,
same ritual as `baselines/`.

## Risks / Trade-offs

- **Budget truncation still returns silent zeros underneath.** → The
  incomplete-column marking converts the silent short-column into a visible hole at
  the table layer, which is all this change owns; the underlying fix is out of scope.
- **DRIFT noise on cross-time tables.** `content_hash` over-triggers (comment edits),
  and composing across time is the headline use, so tables may carry many marks. →
  Soft "look" phrasing, not a verdict; Wilson CI later.
- **Two prefix edit sites (above).** → Thread a single `run_root`; do not re-derive.
- **Terminal width at many columns.** `48 + 12N` wraps after ~4 columns. → Accept
  wide for v1; the markdown renderer has no width limit for records.

## Migration Plan

- No code migration for the schema: `target` is `#[serde(default)]`, the version is
  frozen. Existing baselines (schema 2/3, no `target`) are retired; a fresh
  full-coverage baseline is re-run to serve as a column when needed.
- Additive rollout: `matrix` is new and `run` N=1 is byte-for-byte unchanged, so the
  change is safe to land incrementally and trivially reverted (drop `matrix`, drop the
  repeat-`--target` path).

## Open Questions

- Exact content of `experiments/README.md`.
- The markdown renderer's precise layout.
- How the N>1 stderr table coexists with live per-trial lines during a run.

## Decision reconciliation

One line per decision locked during discussion, with the section of this document
that records it.

- Composing reports across time is a first-class use, so DRIFT marking ships in v1
  rather than being deferred -> Goals; Decisions, "DRIFT marks, never adjudicates"
- A budget-truncated column is marked incomplete rather than left silently short ->
  Decisions, "Budget is one shared total; truncation is marked"
- `--max-total-usd` is one shared total across targets, not a per-target cap ->
  Decisions, "Budget is one shared total; truncation is marked"
- Targets run sequentially; `--jobs` stays trial-level -> Decisions, "Targets run
  sequentially; `--jobs` stays trial-level"
- A mock run records `"mock"`; `"provider-default"` is deleted as unreachable ->
  Decisions, "mock records `\"mock\"`"
- `run` and `matrix` intentionally differ on a zero-scenario column (`run` scores
  it 0, `matrix` scores it 2), and this is left un-reconciled on purpose ->
  Decisions, "`run` and `matrix` diverge on a zero-scenario column, deliberately"
- No legacy-schema roadblock: never gate on `schema_version`, freeze it at 1, and
  regenerate baselines rather than migrate them -> Decisions, "`Report.target` is
  new; normalise via the existing `record_path`"; Migration Plan
- "Cannot compare" is judged by content in three layers: partial overlap gives `-`
  cells, a changed ruler gives DRIFT, only zero shared scenarios hard-errors ->
  Decisions, "Incomparability is layered, and content-based"
- Same-stem columns (one target across time) are allowed and disambiguated, not
  rejected -> Decisions, "Columns are identified by stem, and a repeated stem is
  allowed"
- The footer is recomputed over the scenarios common to all columns, and the
  excluded scenarios are listed -> Decisions, "Footer over the intersection"
- `run`'s table stays on stderr; stdout remains the machine channel -> Decisions,
  "`matrix` is a pure renderer; `run` reuses it"
- Repeated `--target` over `--matrix` or comma-separated `--targets` -> Decisions,
  "Repeatable `--target`, not `--matrix` or `--targets a,b`"
- Layout B (N=1 flat, N>1 nested under the stem), with the stem taken from the
  filename rather than provider-model -> Decisions, "Layout B"; "Stem = target
  filename without extension"
- The results prefix is computed at two sites and both must gain the stem ->
  Decisions, "Layout B" (implementation trap block); Risks
- SPREAD excludes `-` cells from max, min, and the threshold, which also avoids the
  divide-by-zero -> Decisions, "SPREAD is data-derived and hole-safe"; Risks
- `experiments/` is committed and never regenerated -> Decisions, "`experiments/`
  is a committed, never-regenerated record"
