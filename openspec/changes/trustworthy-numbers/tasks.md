## 1. Absence asserts: strict zero-hit semantics and path_not_exists [dispatch: too-te, parallel: yes, reason: spec states exact semantics, path language, and failure details; template-shaped assert work with in-file test conventions to copy; est. ~20 tool calls]

- [x] 1.1 Write failing unit tests in `asserts.rs`: `file_not_contains` fails when no file matches (missing file and zero-hit glob both); `path_not_exists` passes on absent path, fails on existing file, fails on existing directory, passes on empty/missing glob dir, fails on populated glob naming the hits; load-time rejection of a `path_not_exists` line with two `*` segments. Cite `cargo test` output showing them red.
- [x] 1.2 Split a shared matcher out of `read_glob` ("list all hits for pattern, files and directories"); `file_contains`/`file_not_contains` filter hits to files and read contents — existing green-path behavior unchanged (existing tests stay green).
- [x] 1.3 Flip `file_not_contains`'s `Err` arm to fail (symmetric with `file_contains`) and implement `path_not_exists` on the shared matcher, including its parse arm in the assert mini-DSL.
- [x] 1.4 Update README's assert table: add `path_not_exists`, and document that `file_not_contains` now requires the file to exist. Note: section 3 also edits README in this parallel group (distant sections; if the lane merge conflicts, resolve by keeping both edits).
- [x] 1.5 Evidence: full `cargo test` output green, plus a grep showing all 14 in-tree `file_not_contains` uses (across 7 scenario.toml files) still pass suite loading (`zseval list`).

## 2. Strict scenario loading: deny unknown fields at every layer [dispatch: too-te, parallel: yes, reason: layers and fallback are pre-decided in design D2; typo fixtures fully enumerated in spec; est. ~25 tool calls]

- [x] 2.1 Write failing load tests with typo fixtures: unknown top-level key, unknown key in `[[files]]`, unknown key in `[loop]`, and `{ msg = "...", new_sesion = true }` in a multi-turn task — each expecting a load error that includes the scenario path. Cite red test output.
- [x] 2.2 Add `deny_unknown_fields` to `Scenario` and the six named nested structs (`FileSeed`, `LoopCfg`, `MemorySeed`, `NoteSeed`, `McpSeed`, `McpServerSeed`).
- [x] 2.3 Make the untagged `Task`/`Turn` layer strict. If serde's `deny_unknown_fields` does not enforce through untagged enums (the 2.1 turn-typo test proves it either way), fall back to a hand-written `Deserialize` for `Turn` (string-or-strict-struct). Constraint from the pre-start rulings: if any scenario fails the new strictness, fix the scenario — never add a whitelist or escape hatch.
- [x] 2.4 Evidence: 2.1 tests green, and `zseval list` loads all 42 in-tree scenarios cleanly (cite output).

## 3. zerostack identity: capture or die [dispatch: sai-hu, parallel: yes, reason: external-process handling and failure taxonomy carry real judgment (where capture lives, error shaping); spec pins the contract, not the structure; no tool-call cap]

- [x] 3.1 Add `Report` fields: `zs_version`, `zs_bin_path`, `zs_bin_sha256` (required), `git_sha`/`features` (`Option`, always `null` today — no runtime "if available" branch, per design D3).
- [x] 3.2 Capture at run start, once per invocation, before any trial: run `ZS_BIN --version`; success = exit 0 and non-empty stdout; record first line verbatim (no format validation, multi-line tolerated). Any failure aborts the run naming the binary.
- [x] 3.3 Compute `zs_bin_sha256` from binary file contents, once per run (multi-target runs share one capture; every per-target report records it).
- [x] 3.4 Mock backend records fixture identity: `zs_version = "mock"`, fixture path, content fingerprint (file: sha256 of bytes; directory: fold files sorted by relative path with length-prefixed fields — deliberately not copying `PromptPack::fingerprint`'s registered NUL flaw). `--zs-bin` alongside mock stays ignored.
- [x] 3.5 Tests: four stub `--zs-bin` scripts (prints version / exits nonzero / prints nothing / path missing) proving verbatim capture and the three aborts; mock identity tests (same dir contents at two paths → same hash; different contents → different hash). README gains a zerostack-identity section (note: section 1 also edits README in this parallel group; distant sections, if the lane merge conflicts, resolve by keeping both edits). Evidence: test output, zero API calls.

## 4. kind classification: required field and the adjudicated 42 [dispatch: too-te, parallel: no, reason: schema shape, enum values, and all 42 labels are pre-adjudicated in the spec table; bulk labeling is scripted, not judged; est. ~25 tool calls]

- [x] 4.1 Write failing tests: scenario without `kind` fails to load naming the field; `kind = "probe"` fails; after labeling, loading the full suite yields 29 regression + 13 capability. Cite red output.
- [x] 4.2 Add required `kind` field to `Scenario` (lowercase two-value enum, no default) and record it verbatim on `ScenarioResult` (constructors and existing test fixtures updated).
- [x] 4.3 Label all 42 scenario.toml files (41 under `scenarios/` + `examples/prompt-pack` marker) exactly per the spec table, via a scripted pass keyed off the table; verify by grep: 29 `kind = "regression"`, 13 `kind = "capability"`, no file without a kind line.
- [x] 4.4 Evidence: 4.1 tests green, `zseval list` green, grep counts cited. Note: suite is red between 4.2 and 4.3 by design — this section lands as one commit, never split.

## 5. Per-kind summary metrics in report output [dispatch: too-te, parallel: yes, reason: struct shape, field names, n/a convention, and line order are all pinned in spec and design D5; est. ~20 tool calls]

- [x] 5.1 Write failing tests: `Summary` carries fixed `regression`/`capability` sub-structs (`n_scenarios`, `n_gradable`, `pass_at_k`, `pass_hat_k`) computed over that kind only; a kind with `n_gradable = 0` serializes rates as `0.0`; `scenarios` array order unchanged from discovery order. Cite red output.
- [x] 5.2 Implement the per-kind computation in `Summary` construction; overall metrics stay at top level, untouched.
- [x] 5.3 Run summary prints three lines — regression, capability, overall, in that order — with `n/a` rendering for an empty kind (existing `rate()` convention).
- [x] 5.4 Evidence: test output green; a mock-backend run's stderr showing the three-line summary.

## 6. Matrix grouping by kind [dispatch: too-te, parallel: yes, reason: row sectioning, footer groups, and JSON flatness are fully specified in the matrix-render delta; rendering code has established test patterns to copy; est. ~30 tool calls]

- [x] 6.1 Write failing tests: rows render in two sections (regression first, capability after, section markers); footer renders three metric groups over the common gradable set filtered per kind, overall last; a kind absent from the common set renders `n/a`; `--json` output stays one flat array with `kind` per row. Cite red output.
- [x] 6.2 Group rows by `kind` read from report rows (never from scenario.toml) in fixed-width and markdown renderers.
- [x] 6.3 Compute and render the three footer groups; width budget unchanged (rows added, no columns).
- [x] 6.4 Evidence: test output green; rendered fixed-width sample cited.

## 7. Strict report reads: remove the legacy escape hatches [dispatch: too-te, parallel: no, reason: the ~32 attribute sites, the skip branch, and the three test flips are enumerated; delete-and-tighten with no design freedom; est. ~30 tool calls]

- [x] 7.1 Flip the three verdict.rs tolerates-old-JSON tests into rejection tests: report JSON missing a field fails `load_report` with an error naming the field; a current-binary report round-trips cleanly. Red first, cite output.
- [x] 7.2 Remove the legacy `#[serde(default)]` attributes across `Report`/`ScenarioResult`/`TrialResult` (~32 sites) and their "old committed baseline" doc justifications. Fields whose default value is a legitimate runtime value (e.g. `judge_file = ""`) keep the value's meaning — only the deserialization escape hatch goes.
- [x] 7.3 Simplify `compare.rs`'s empty-hash skip branch to plain inequality; update its comment (the "old baseline" referent is gone); adjust any test relying on the skip.
- [x] 7.4 `git rm baselines/main.json`; update `baselines/README.md` (Day 2 regenerates against v1.7.2; the 07-21 numbers live in git history only).
- [x] 7.5 Tighten README's `judge_model` three-state wording: absent = load error, `null` = unknown, `[]` = nothing graded.
- [x] 7.6 Evidence: full `cargo test` green; `zseval compare baselines/main.json x` no longer applicable (file gone) — cite the failing-load test from 7.1 as the strictness proof.

## 8. Compare warning policy: two new warnings under one rule [dispatch: too-te, parallel: no, reason: policy, warning semantics, invariant test shape, and ADR content are all pre-decided in design D6; est. ~35 tool calls]

- [x] 8.1 Write failing tests: truncation warning fires when either side (or both) recorded `budget_truncated`, naming the side(s), exit code unmoved (0 without regressions, 1 with); `zs_mismatch` fires on differing `zs_bin_sha256` including same-version-different-hash, quiet on identical; the all-warnings-lit invariant — a `Comparison` with every warning kind set has the same exit code as the same comparison with none. Cite red output.
- [x] 8.2 Implement both warnings on `Comparison`; render all warnings through one block in fixed order.
- [x] 8.3 Acceptance check: `exit_code()` body untouched by this section (cite the diff).
- [x] 8.4 Write the repo's first ADR at `docs/adr/0001-compare-always-warns-matrix-owns-multivar.md`: the policy (exit code answers only the gate question; every comparability threat is a warning), the structural anchors (pure `exit_code()`, invariant test, single render block), and the exit-3 reservation (future aggregate predicate inside `exit_code()`, built only when the CI gate consumes it). Update `compare.rs`'s "build is always moved, for now" doc comment to cite the ADR and the now-recorded build identity; archived change design docs stay untouched (historical record).
- [x] 8.5 Evidence: test output green including the invariant test.

## 9. Build identity displayed wherever it can differ [dispatch: too-te, parallel: no, reason: display shape copies the existing pack_identity precedent verbatim (helper + two call sites); est. ~15 tool calls]

- [ ] 9.1 Write failing tests: compare's identity lines show `zs=<version>#<short-hash>` beside the pack identity; matrix legend carries the same per column; a mock report's line shows `mock#<short-hash>`. Cite red output.
- [ ] 9.2 Add a `zs_identity` display helper beside `pack_identity` and wire both call sites (compare human output, matrix legend).
- [ ] 9.3 Evidence: test output green; one rendered legend sample cited.
