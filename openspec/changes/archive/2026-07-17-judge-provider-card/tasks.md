# Tasks: judge-provider-card

Conventions: sections are the execution unit (one section per session, in order) and every task carries its own Verify step; a task is not done until its Verify passes. Section 6 is independent of all others and lands as its own commit.

## 1. Dependencies and async bridge [dispatch: main, parallel: no]

- [x] 1.1 Add `rig-core = "=0.40.0"` (default features, rustls) and `tokio` (`rt`, `rt-multi-thread`, exact-pinned per repo convention) to `crates/zseval/Cargo.toml`. Verify: `cargo build` succeeds on rustc 1.96 with edition 2021.
- [x] 1.2 Add a shared `OnceLock<tokio::runtime::Runtime>` (multi-thread, small worker count) in `judge.rs` so the sync `Judge` trait can `block_on` concurrently under `--jobs`. Verify: a unit test spawns several OS threads that each `block_on` a trivial future on the shared runtime at the same time and all complete; this test is the guard for the `--jobs` concurrency claim.

## 2. Judge card (config layer, spec: judge-card) [dispatch: main, parallel: no]

- [x] 2.1 Add `JudgeProvider` enum (`anthropic | openai | openrouter | gemini`, serde lowercase) with `key_env()` mapping; document the structural invariant on the enum and constants: a committed card must never name a network destination or an env var, routing derives from `provider` in code only. Verify: unit tests cover the four-provider happy path and an unknown provider being rejected, plus `key_env()` returning the four fixed names.
- [x] 2.2 Shrink `JudgeConfig` to `{ provider, model, price_in_usd_per_mtok, price_out_usd_per_mtok }`, all required, `deny_unknown_fields`, and delete `impl Default` (no built-in judge); delete the mirror-of-default test. Verify: unit tests cover a missing field and an unknown field being rejected; `cargo test -p zseval` compiles with no remaining `Default` references for `JudgeConfig`.
- [x] 2.3 In `JudgeConfig::load`, keep `reject_key_shaped_fields` and add targeted rejection of `api_url` / `api_key_env` (case-insensitive) whose error states removal for security and how to migrate; delete `validate_api_url` / `validate_api_key_env`. Verify: regression attack tests show a legacy card with both fields, and each field alone, failing with the security/migration error on the exit-2 path; existing key-shaped-field tests still pass.
- [x] 2.4 `validate()`: model non-empty, no whitespace or control characters; prices finite and non-negative (reuse existing logic). Verify: unit tests cover empty/whitespace/control-character model and non-finite/negative prices.

## 3. Execution via rig (spec: judge-execution) [dispatch: main, parallel: no]

Every task in this section carries its offline test through the test-only rig `base_url` seam built in 3.1 (test code, not config, so the security invariant stands).

- [x] 3.1 Replace the transport in `judge()` with the rig thin path: `match provider` builds the client with explicit key and default base URL (never `from_env`), with a test-only `base_url` injection seam for pointing at a local mock server; then `completion_model(model).completion_request(prompt).temperature(0.0).max_tokens(1024).send()` on the shared runtime; extract verdict text from `choice`, keeping the pure first-word parse function and its tests. Delete only the old transport tests this directly breaks. Verify: an offline success-path test through the seam returns a verdict from the mock server and asserts cost math uses card prices and lands in `cost_usd`.
- [x] 3.2 Delete `curl_config_escape` and all remaining curl plumbing and their tests, now dead. Verify: `grep -ri curl crates/zseval/src` returns nothing; `cargo test -p zseval` passes.
- [x] 3.3 Temperature fallback: on a provider error rejecting the temperature parameter, retry once without it and record the omission in `judge-request.json`; no model-name lists. Verify: a mock-server test rejects temperature on the first call, succeeds without it on the second, and the recorded `judge-request.json` carries the omission note.
- [x] 3.4 Transient retry: classify rig errors conservatively (429/5xx/transport retryable, everything else not), retry once with a short fixed backoff, then surface the error (trial goes Indeterminate). Verify: mock-server tests cover 429-then-success succeeding and double-failure surfacing the error; a non-retryable error (e.g. 401) is not retried.
- [x] 3.5 Artifacts and served model: read the served model from `raw_response` per provider (confirm Gemini's field name at implementation, fall back to the unknown-ruler tri-state if absent); write `judge-request.json` (provider, model, temperature or omission note, max_tokens, prompt) and `judge-response.json` (serialized raw response). Verify: mock-server tests assert both artifacts' contents and the served-model recording, including the absent-field fallback to the tri-state.
- [x] 3.6 Rework `available()` to check the configured provider's env var; make the runner's unavailable-judge backstop message provider-generic (drop the hard-coded ANTHROPIC_API_KEY wording at runner.rs:427). Verify: a unit test shows `available()` keying off the configured provider's env var for at least two providers; grep confirms no hard-coded ANTHROPIC_API_KEY remains in runner.rs.

## 4. CLI selection and preflight (specs: judge-selection, judge-preflight) [dispatch: main, parallel: no]

Prerequisite: section 3's mock-server seam (dry-run tests go through it).

- [x] 4.1 Make `resolve_judge` return a three-state choice (File(path, config) / NoJudge / Unspecified), keeping single-arity and `--judge`+`--no-judge` conflict errors; update `judge_flag_tests` fixtures to the new card schema. Verify: unit tests cover single-arity, the conflict error, and all three states.
- [x] 4.2 In `cmd_run`: after `discover()`, if any scenario has a rubric and the choice is Unspecified, bail (exit 2 via the existing error path) naming `--judge` and `--no-judge`; Unspecified with no rubrics behaves as NoJudge; mirror the same gate in `cmd_regrade` using the loaded scenario. Verify: tests show rubric suite + Unspecified exits 2 naming both flags, no-rubric suite + Unspecified runs, and `--no-judge` on a rubric suite runs with the skip recorded.
- [x] 4.3 Implement `LlmJudge::preflight()`: key-presence check (error names the exact env var and the `--no-judge` escape) then the live dry-run in the real judge shape (same prompt template with a fixed self-calibration question, same max_tokens, same temperature fallback) requiring a parseable verdict. Verify: tests through the mock-server seam cover dry-run success and failure; a presence-check failure names the configured provider's env var.
- [x] 4.4 Wire `preflight()` into `cmd_run` and `cmd_regrade` before any trial, with probe cost excluded from the report. Verify: tests show a failing preflight exits 2 before any trial executes (in both commands) and a successful probe's cost is absent from report totals.
- [x] 4.5 Update the usage banner: four card fields, mandatory explicit choice for rubric suites, preflight behavior, fixed key env vars. Verify: read the rendered banner and check each of the four points is present.

## 5. Cards and docs [dispatch: main, parallel: no]

- [x] 5.1 Update `judges/sonnet.toml` and `judges/opus.toml` to the four-field schema (add `provider = "anthropic"`, drop routing fields), with a comment stating the card is inert data. Verify: the harness tests that load the shipped cards pass against the new schema.
- [x] 5.2 Rewrite `judges/README.md`: ruler-card concept, four-field table, model ids and prices looked up at models.dev, no-default policy, preflight behavior, and the security section explaining why routing fields were removed and fail loudly. Verify: read the result against that six-point checklist; no removed field is described as still supported.
- [x] 5.3 Update root `README.md` judge sections and the path-privacy sentence. Verify: read the changed sections; every CLI invocation shown matches the new `--judge`/`--no-judge` contract; `cargo test --workspace` still passes.

## 6. run_dir path leak (independent of sections 1 to 5, separate commit, spec: report-paths) [dispatch: main, parallel: yes]

- [x] 6.1 Expose `verdict.rs::record_path` as `pub(crate)` and use it at both `TrialResult.run_dir` write sites (runner.rs:491 and :520) instead of `display().to_string()`. Verify: tests show recorded `run_dir` is relative, forward-slashed, never starts with `/`, and regrade locates a run dir from a relative `run_dir`.

## 7. Verification and gate [dispatch: main, parallel: no]

- [x] 7.1 Run the ship gate: `cargo test --workspace`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` (measure lint claims against a detached worktree at main before blaming the branch). Verify: all three commands exit 0.
- [x] 7.2 /verify with the anti-contamination method (secrets generated in-script from /dev/urandom, `grep -f patternfile`, passing control beside every error probe): bogus keys per provider produce real auth errors only at official endpoints; the legacy attack card exits 2 beside a passing control; a mistyped model exits 2 at preflight; no curl child process and no secret in any argv. Verify: every probe has its passing control and all probes behave as stated.

## 8. Ship [dispatch: main, parallel: no]

- [x] 8.1 Rewrite the feature commit (replace `4b1a3a8` with the new design; keep the lint chore commit; `fix(report): record run_dir relative to cwd` as its own commit) and force-push `feat/judge-file`. Verify: `git log` shows exactly the intended commits and the remote branch matches local.
- [x] 8.2 Update PR #2's description: the new design, the old design's verified failure as rationale, the deliberate behavior breaks (no default judge, removed fields); run /code-review on the working diff before /ship; do not merge until review passes. Verify: the PR page shows the updated description and a passing review.
