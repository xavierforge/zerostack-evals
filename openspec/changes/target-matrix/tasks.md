# Tasks: target-matrix

Conventions: a section is the execution unit (one section per session, in order,
landing exactly one commit) and every task names the execution evidence that lets
its box be ticked. Sections are vertical slices: each carries its own failing tests
and the implementation that turns them green. Every section depends on earlier
sections only, and none may overlap another.

## 1. Record the target a run evaluated [dispatch: mech-executor, parallel: no, reason: spec names the field, the record_path reuse, and every assertion; no design judgment]

- [x] 1.1 Write failing unit tests in `verdict.rs` for `Report.target`: a target under cwd records a relative forward-slashed path never starting with `/`; a target outside cwd records only its file name; a report copied away from its run dir still names its target; a report JSON lacking the field deserialises to an empty target. Verify: `cargo test -p zseval target` shows the four failing on the missing field, not on unrelated compile errors
- [x] 1.2 Add `Report.target: String` with `#[serde(default)]`, documented as column identity (versus `target.toml` as content), normalised through the existing `record_path`. Verify: the four tests from 1.1 pass; paste the passing output
- [x] 1.3 Add `target` to `ReportMeta` and populate it in `run_suite`'s `Report::build` call from `opts.target`. Verify: run a mock-backend suite and `cat` the produced `report.json`, showing a non-empty relative `target`
- [x] 1.4 Freeze `REPORT_SCHEMA_VERSION` at `1` and update the version assertion test. Verify: `grep -rn "schema_version" crates/zseval/src` shows no branch on the value, and `cargo test -p zseval schema_version` passes

## 2. Make --target mandatory for zs and rejected for mock [dispatch: mech-executor, parallel: no, reason: each rule and its exit code is fixed in the spec; only flag plumbing remains]

- [x] 2.1 Write failing tests: `run --backend zs` with no `--target` exits 2 before any trial; `run --backend mock=<fixture>` given `--target` exits 2; a completed mock run records `model == "mock"`. Verify: `cargo test -p zseval` shows the three failing
- [x] 2.2 Add `Flags::get_all(&self, k) -> Vec<&str>` returning every occurrence in order, beside the existing `get`/`count`. Verify: a unit test asserting `get_all("target")` returns both values for `--target a --target b` passes
- [x] 2.3 Enforce the zs rule: exit 2 with a usage error before the backend is constructed when no `--target` is given. Verify: the 2.1 test passes; run the binary and paste its exit code and message
- [x] 2.4 Enforce the mock rule and record `"mock"` as a mock run's model. Verify: the two mock tests from 2.1 pass; paste the `model` field from a produced mock report
- [x] 2.5 Delete the `"provider-default"` branch from `target::describe` and its test, now an unreachable state. Verify: `grep -rn "provider-default" crates/` returns nothing and `cargo test -p zseval` is green

## 3. Nest multi-target results under the target stem [dispatch: executor, parallel: no, reason: threads a new run_root through the runner call chain, an interface change design.md describes but does not fully pin down]

- [x] 3.1 Pure refactor first: derive the results prefix once in `run_suite` and thread that `run_root` into `run_trials_for_scenario`, replacing the separate computation at the trial-dir site (`runner.rs:169`). No behaviour change. Verify: `cargo test --workspace` green and a mock run's `results/<tag>/` tree diffed against one produced before the refactor shows empty output
- [x] 3.2 Write failing tests for the N>1 layout: two targets write report, run-level target copy, and trial dirs together under `results/<tag>/<stem>/`; one target stays flat at `results/<tag>/`. Verify: both fail before the layout change lands
- [x] 3.3 Add the stem level to `run_root` when more than one target runs, deriving `<stem>` as the target filename without extension. Verify: the 3.2 tests pass; paste the produced tree for a two-target mock run
- [x] 3.4 Write the run-level clean copy of `target.toml` under `run_root`, distinct from the per-trial config seed at `backend.rs:307`. Verify: a test shows the run-level copy holding the original bytes while a scenario `config:` seed overrides only the per-trial copy
- [x] 3.5 Error on a stem collision between two `--target` files in one run. Verify: a test passing `a/opus.toml` and `b/opus.toml` shows a non-zero exit before any trial; paste the message
- [x] 3.6 Drop the provider-model segment from `auto_tag` when N>1 so it does not appear twice in the path. Verify: a test asserts the N>1 path carries the stem once with no provider-model segment, and N=1 tags are unchanged

## 4. Run the suite against repeated --target [dispatch: executor, parallel: no, reason: the shared-budget threading is a behaviour decision the spec states but leaves the mechanism open]

- [x] 4.1 Write failing tests: two `--target` flags produce two reports; total spend across targets stays under one `--max-total-usd` instead of each target getting its own cap. Verify: both fail before the loop exists
- [x] 4.2 Loop targets sequentially in `cmd_run` over `get_all("target")`, building one backend per target. Verify: the two-report test passes; paste the two report paths
- [x] 4.3 Feed each `run_suite` a shrinking cap (`total - spent_so_far`) so `--max-total-usd` bounds the whole invocation. Verify: the budget test passes; paste the observed per-target caps
- [x] 4.4 Aggregate the exit code across targets as the most severe: 2 if any column is fully ungradable, else 1 if any trial failed, else 0. Verify: tests cover a failing column giving 1 and a fully-ungradable column giving 2; paste both codes
- [x] 4.5 Make `run --json` with more than one target exit 2 naming `zseval matrix --json`, keeping N=1 `--json` intact. Verify: a test shows exit 2 with the pointer, and an N=1 `--json` run still emits report JSON on stdout

## 5. Build the scenario-by-target table [dispatch: executor, parallel: no, reason: introduces a renderer module whose data model and column-identity interface are open design choices]

- [x] 5.1 Write failing tests for the table model: rows keyed by scenario id in id order; a graded all-fail cell renders `0.000` while an absent or fully-indeterminate cell renders `-`. Verify: the tests fail before the module exists
- [x] 5.2 Add the renderer module building a scenario x target model from N reports, consulting `n_graded_trials()` and scenario presence to separate a real zero from a hole. Verify: the 5.1 tests pass; paste the rendered rows
- [x] 5.3 Compute the footer over the scenarios gradable in every column and list the scenarios excluded from it. Verify: a two-column test with differing scenario sets shows the intersection footer plus the exclusion list, and an identical-suite test shows the footer matching each report's own summary
- [x] 5.4 Implement column identity: stem in the header; full provider/model, target path, and the `judge_model` tri-state in the legend. Verify: a test asserts the three judge states render distinguishably (unknown vs nothing-graded vs listed rulers); paste the rendered legend
- [x] 5.5 Disambiguate same-stem columns by tag or timestamp instead of colliding. Verify: a test with two reports for one target from different runs shows both columns present and distinctly labelled
- [x] 5.6 Add the fixed-width terminal renderer and the markdown renderer over the same model. Verify: a test renders one fixture both ways; paste both outputs

## 6. Mark what the table cannot compare [dispatch: mech-executor, parallel: no, reason: threshold formula, drift granularity, and hole-exclusion rules are all pinned in the spec; only application remains]

- [x] 6.1 Write failing tests for SPREAD: a row whose gap exceeds one trial's resolution is marked; one within it is not; a row containing a `-` cell neither divides by zero nor counts the hole in max/min. Verify: the three fail first
- [x] 6.2 Implement SPREAD as `max - min` over gradable cells exceeding `1 / min(n_graded_trials)` of those cells, excluding holes from max, min, and the threshold. Verify: the 6.1 tests pass; paste the output
- [x] 6.3 Implement per-row DRIFT on `content_hash` mismatch, listing the differing columns grouped by hash with timestamps and naming no column as correct. Verify: a test with one scenario hashed differently across two columns shows the row marked and both columns listed
- [x] 6.4 Implement per-column DRIFT when a column's `judge_hash` differs from the others or its judge is unknown. Verify: tests cover both the differing-hash and the unknown-judge case marking the column
- [x] 6.5 Mark a column incomplete when it ran fewer scenarios than the suite defines (budget truncation). Verify: a test with a truncated column shows the incomplete mark; paste the rendered column header
- [x] 6.6 Label SPREAD and DRIFT in the legend as display heuristics, not statistical or authoritative claims. Verify: paste the rendered legend showing both caveats

## 7. Add the matrix subcommand [dispatch: mech-executor, parallel: no, reason: command wiring, rejection cases, and exit codes are all fixed by the spec]

- [ ] 7.1 Write failing tests: `matrix` over report files renders and creates no files; a report with no target identity exits 2 naming it; a report sharing no scenario id exits 2 naming it; partial overlap renders holes without erroring. Verify: all four fail first
- [ ] 7.2 Add `cmd_matrix` accepting one or more report paths, making no API calls and writing nothing to disk. Verify: the no-side-effects test passes; paste a before/after listing of the working directory
- [ ] 7.3 Support `--json` and `--markdown` on stdout, defaulting to the fixed-width renderer. Verify: `matrix ... --json | python3 -m json.tool` succeeds; paste the opening lines of all three forms
- [ ] 7.4 Reject a target-less report and a zero-overlap report with exit 2, each naming the offending file, without reading `schema_version`. Verify: those 7.1 tests pass and `grep -n schema_version` over the new code returns nothing
- [ ] 7.5 Implement exit codes: 0 when a table rendered, 2 when unrenderable or any column fully ungradable, never 1. Verify: tests show a low-scoring table exiting 0 and a fully-ungradable column exiting 2; paste both codes
- [ ] 7.6 Add `matrix` to `USAGE` and the command dispatch table. Verify: paste the `zseval --help` line for matrix

## 8. Render the matrix table after a multi-target run [dispatch: mech-executor, parallel: no, reason: reuses the renderer built in section 5; stream choice and usage text are already decided]

- [ ] 8.1 Write a failing test that an N>1 run's table lands on stderr while stdout stays clean. Verify: it fails first
- [ ] 8.2 Print the table to stderr at the end of an N>1 run by calling the same renderer function `matrix` uses. Verify: the 8.1 test passes; run a two-target mock suite with stdout redirected to a file and show the file empty while the table reached the terminal
- [ ] 8.3 Update `USAGE` for repeatable `--target`, the N>1 `--json` rule, and the shared budget. Verify: paste the rendered usage block and confirm all three points appear

## 9. Add the experiment record directory [dispatch: executor, parallel: no, reason: the README's content is an open question in design.md, so the section carries authoring judgment]

- [ ] 9.1 Create the committed `experiments/` directory with `README.md`. Verify: `git check-ignore experiments/` prints nothing and `git status` shows both as new trackable files
- [ ] 9.2 Write the README covering the redirect ritual, the provenance a snapshot embeds, and the never-regenerate rule. Verify: quote the never-regenerate sentence and confirm the other two points are present
- [ ] 9.3 Produce one real snapshot through the documented ritual, as an end-to-end check that the ritual is accurate as written. Verify: run the README's command verbatim; paste the first 20 lines of the resulting markdown
- [ ] 9.4 Confirm the snapshot embeds each target's `target.toml` from the run-directory copy and degrades honestly when a column's content is unavailable. Verify: grep the snapshot for the embedded toml and for the not-embedded note on a detached column

## 10. Retire the A/B framing for cross-target compare [dispatch: executor, parallel: no, reason: prose judgment across three files, and the README additions and the leftover sweep both need sections 2 and 4-8 already landed]

- [ ] 10.1 Rewrite `compare.rs`'s `target_mismatch` warning string so the legitimate cross-target use reads as the migration gate, not A/B. Verify: paste the old and new strings; `cargo test -p zseval` green
- [ ] 10.2 Update the matching comment in `compare.rs` and the "that's the A/B use case" line in `README.md`. Verify: `grep -rn "A/B" crates README.md` output pasted with a one-line judgement per remaining hit
- [ ] 10.3 Re-read `targets/README.md` and rewrite its compare example as a matrix example if it still demonstrates "run two targets then compare"; otherwise record that it needs no change. Verify: quote the current example and state which branch was taken
- [ ] 10.4 Document `matrix` and repeatable `--target` in `README.md`. Verify: paste the added section
- [ ] 10.5 Sweep the tracked tree for leftovers with `git grep` (tracked files only, so untracked local notes are correctly out of scope): `provider-default`, `--matrix`, `--targets`, and "A/B use case". Verify: paste the `git grep` output with a one-line judgement per remaining hit

## 11. Regenerate the baseline under the current schema [dispatch: main, parallel: no, reason: spends real API budget and needs the user's go-ahead, so it cannot run unattended]

- [ ] 11.1 Run the ship gate: `cargo test --workspace`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`. Verify: all three exit 0; paste the tail of each
- [ ] 11.2 Drive the CLI end to end with the mock backend to confirm the N=1 path is unchanged and an N>1 table renders. Verify: paste both invocations and their output
- [ ] 11.3 With the user's explicit approval to spend, run a full-coverage suite under the current schema to produce a replacement baseline, and retire the legacy schema-2 `baselines/main.json`. Verify: the new baseline's `target` is non-empty and covers every scenario `zseval list` reports; paste both counts
- [ ] 11.4 Compose the new baseline as one column of a matrix table, as an end-to-end check of cross-time composition. Verify: paste the rendered table showing the baseline column labelled, with any DRIFT marks explained
