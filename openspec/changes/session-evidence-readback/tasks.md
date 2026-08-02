# session-evidence-readback — tasks

## 1. Transcript reads tool records (design D1)

- [ ] 1.1 Mirror the upstream shape in `transcript.rs`: add `RawToolRecord` (tolerant per-role struct, only `name` required) and `tool: Option<RawToolRecord>` on `RawMessage`; in `parse_str`, take `ToolCall.name` from `tool.name`, keep `summary` as the content minus the leading name token, set `subagent` from the role, and make a `tool_call`/`subagent_tool_call`-role message without a `tool` record a schema `Err`. Rewrite the module-doc contract block (headless sessions now carry tool records; stdout is diagnostics).
- [ ] 1.2 Delete the stdout reconstruction: `tool_calls_from_stdout`, `tool_calls_from_stdout_file`, the `from_run` loop over `artifacts.turns`, and the `stdout_tool_call_tests` module; update `from_run`'s doc (one evidence channel for both backends).
- [ ] 1.3 Regenerate every fixture that authors tool-call-role messages to the new shape (`crates/zseval/tests/fixtures/session-*.json` and any session JSON embedded in tests), modeled on upstream's `session_storage_tests.rs` examples; rework `from_run_tests` to author tool records in the session instead of a `turn-N.stdout` file.
- [ ] 1.4 Unit tests per the session-evidence spec: structured record becomes a call; missing `tool` record errs naming the file; subagent record marks `subagent`; a session without records plus a marker-bearing stdout log yields zero tool calls.

## 2. Transcript reads prompt provenance (design D3)

- [ ] 2.1 Add `RawPromptRef { name, source }` and `prompt: Option<RawPromptRef>` on `RawSession`; expose it on `Transcript`; `absorb` applies last-wins; `source` other than `built_in`/`user_file` is a schema `Err`; an absent field parses as `None`. Unit tests for all three spec scenarios.

## 3. Runner maps, reconciles, cross-checks (design D3/D4/D5)

- [ ] 3.1 Surface each trial's readback out of `run_trials_for_scenario` and reconcile to the scenario level: all-agree wins; disagreement or all-absent records `unknown` with a stderr warning (the absent case names the `ZS_BIN` rebuild).
- [ ] 3.2 Rework `resolve_prompt` into the mapping (`built_in`→`stock`; `user_file`→`scenario` via `scenario_seeds_prompt`, else `pack` via pack names, else `unknown`+warn) with the old derivation kept as a cross-check that warns on mismatch, readback winning; delete the `seeds_effective_config`→`unknown` early return for session-backed scenarios; keep the full derivation (including that branch) for loop-mode scenarios.
- [ ] 3.3 Tests: the four mapping arms, the byte-identical-pack disagreement warning, trial disagreement → `unknown`, loop-mode recording unchanged.

## 4. Backend doc posture (design D2)

- [ ] 4.1 Rewrite `backend.rs`'s module doc and the tool-call comment near the artifact-collection site: session JSON is the evidence channel; `--pure-stdout` stays only so `turn-N.stdout` reads well for humans.

## 5. The prompt_recorded assert (design D6)

- [ ] 5.1 Add `prompt_recorded <name> <built_in|user_file>` to `asserts.rs` (header doc, parser, grading against the transcript's raw readback; no readback = fail, never vacuous pass) with unit tests for pass, mismatch, and missing-record.

## 6. Regression scenarios and the ledger (design D6)

- [ ] 6.1 Add the two `scenarios/session/` regression scenarios: a trivial tool task asserting `tool_called`, and a bare run asserting `prompt_recorded code built_in`.
- [ ] 6.2 Register both in `scenarios/coverage.toml` so the drift gate stays green.

## 7. Example and docs

- [ ] 7.1 `examples/prompt-pack`: replace the marker-file proxy asserts with recorded-prompt asserts, and make sure no pack prompt is byte-identical to a zerostack built-in.
- [ ] 7.2 README (and AGENTS.md if it describes the evidence channel): document session-JSON readback and the `ZS_BIN` prerequisite (all-features build from a mainline containing PR #230 and #228).
- [ ] 7.3 Tick roadmap lines a2.1/a2.2/a2.3 in ROADMAP_ZS.md and the matching ROADMAP.md items (edit only — these files stay untracked).

## 8. Verification

- [ ] 8.1 Update the harness integration test's fake zerostack so its emitted sessions carry tool records and the prompt field, matching the real binary's new output.
- [ ] 8.2 `cargo test --workspace` green.
- [ ] 8.3 Operator step (needs the rebuilt `ZS_BIN` and API key): one real smoke run of a tools scenario plus the two new regression scenarios, confirming readback values on live output.
