# scenario-launch-flags tasks

## 1. Extract launch argument assembly into pure functions [dispatch: sai-hu, parallel: no, reason: the seam's shape (function signatures, what context they take, where they live) is a structural decision the spec leaves open]

- [x] 1.1 Write assembly-locking tests against the new function names before extracting: the `-p` vector for a scenario with a prompt, the `-p` vector for a later turn carrying `--continue`, and the `--loop` vector with `--loop-max`/`--loop-run`; assert the exact current argument lists (RED because the functions do not exist yet).
- [x] 1.2 Extract pure assembly functions from `run_print` (backend.rs:574-590) and `run_loop` (backend.rs:653-667) that return the full argument vector; both spawn sites consume them; no behavior change (tests from 1.1 go GREEN).
- [x] 1.3 Verify and cite evidence: `cargo test --workspace` output pasted into the task log, plus a grep showing no remaining call site builds zerostack args outside the two functions.

## 2. security_mode field with mode mapping [dispatch: too-te, parallel: no, reason: spec is complete (seven values, mechanical flag mapping, default, load-error case, loop parity all stated); est. ~25 tool calls]

- [ ] 2.1 RED: tests for (a) no `security_mode` declared produces an argument vector byte-identical to today's including `--yolo`, (b) `read-only` maps to `--read-only` and no other permission flag, (c) `standard` emits none of the six permission flags, (d) a `--loop` scenario carries the same flag as `-p` would, (e) `security_mode = "readonly"` fails deserialization naming the field.
- [ ] 2.2 Add the `SecurityMode` serde enum (kebab-case, default `yolo`) to the strict `Scenario` schema and map it in both assembly functions (`standard` emits no flag, every other value emits `--<value>`); tests go GREEN.
- [ ] 2.3 Verify and cite evidence: `cargo test --workspace` output, plus `zseval list` over the in-tree suite showing all scenarios still load.

## 3. cli_args field with harness-flag denylist [dispatch: too-te, parallel: no, reason: spec is complete (splice point, token shape, exact denylist, error content, hash behavior all stated); est. ~30 tool calls]

- [ ] 3.1 RED: tests for (a) `["--quick-model", "fast"]` appears verbatim after harness-owned flags and before the turn message in both paths, (b) on a `--continue` turn the tokens still follow `--continue` and precede the message, (c) `["--yolo"]` fails load naming the scenario path and token, (d) `["--log-file=/tmp/x"]` fails load naming `--log-file`, (e) `["--no-context-files"]` loads and reaches the vector, (f) adding a `cli_args` token changes `content_hash`.
- [ ] 3.2 Add `cli_args: Vec<String>` to the schema; validate at load: every dash-prefixed token, `=value` suffix stripped, checked against `-p`, `--loop`, `--log-file`, `--continue`, `--load-prompt`, `--no-color`, `--pure-stdout`, `--loop-max`, `--loop-run`, the six permission flags, and `-R`; splice surviving tokens in both assembly functions; keep the denylist adjacent to the assembly code so they drift together (design D4/D5); tests go GREEN.
- [ ] 3.3 Verify and cite evidence: `cargo test --workspace` output, plus `zseval list` over the in-tree suite showing zero load regressions.

## 4. Documentation and coverage-claim refresh [dispatch: too-te, parallel: no, reason: established doc conventions; targets are enumerable by grep (coverage.toml hardcoded-list claims, README field table); est. ~20 tool calls]

- [ ] 4.1 Document `security_mode` and `cli_args` in scenarios/README.md: value table, denylist and its load-time error, the separate-value two-token shape, the stray-positional caveat, and the D6 convention that a scenario overriding target identity notes it ignores `--target`.
- [ ] 4.2 Rewrite the coverage.toml claims that state the argument list is hardcoded and permission modes are unreachable (grep for them; the Explore pass saw lines 87, 93, 354, 405-406, 657); keep untested-claim wording honest: modes are now reachable but still untested until the permission suite lands.
- [ ] 4.3 Update the local scenarios/PLAN.md Gap A entry to resolved, naming the two fields (untracked file, edit in place, never commit).
- [ ] 4.4 Verify and cite evidence: the coverage drift check test run output (green), plus `cargo test --workspace` output.
