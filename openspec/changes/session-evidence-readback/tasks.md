# session-evidence-readback — tasks

Conventions: a section is the execution unit (one section per session, landing
exactly one commit) and every task names the execution evidence that lets its
box be ticked. Sections are vertical slices: each carries its own failing tests
and the implementation that turns them green, so the suite is green at every
commit. `depends` names the sections that must be green first and is the real
ordering constraint.

Section 9 is not a factory round. It needs a rebuilt `ZS_BIN` and a paid API
call, so no subagent can execute or verify it; the loop stops after section 8
and hands it back.

Vocabulary, fixed for this document:

- **tool record** is the `tool` object on a session message (upstream's
  `ToolRecord`), whose three shapes the message `role` discriminates. **the
  mirror** is `RawToolRecord`, this crate's tolerant local copy of it.
- **readback** is a value read out of the session JSON. **derivation** is the
  old inference from what the harness seeded. Where both exist, readback is the
  value and derivation is a cross-check (design D3).
- **`source`** is upstream's two-value field on the session (`built_in` /
  `user_file`). **`prompt_source`** is this repo's four-value report field
  (`scenario` / `pack` / `stock` / `unknown`). The bare word is never used.
- **the evidence channel** is the session JSON zerostack writes. The
  `turn-N.stdout` capture is a human-facing artifact, not evidence (design D2).
- **the stub** is the script `write_stub_zs_bin` writes (`harness.rs:3399`). It
  copies a fixture verbatim into `$ZS_DATA_DIR/sessions/`, so its behaviour
  follows from the fixtures and is never edited directly.
- **a schema `Err`** is the transcript parser refusing a shape it cannot read,
  which the runner turns into Indeterminate.

## 1. Transcript reads tool records (design D1) [dispatch: sai-hu, depends: none, parallel: no, reason: it deletes a module's only evidence source and fixes how tolerant the replacement is; D1 pins the struct, but keeping every existing assert's meaning across regenerated fixtures is the judgment]

- [x] 1.1 Write the failing tests first in `crates/zseval/src/transcript.rs`, one per session-evidence spec scenario: a `tool_call`-role message carrying a record becomes a `ToolCall` whose `name` is `tool.name`; a `tool_call`-role message with no `tool` field is a schema `Err` naming the file; a `subagent_tool_call` record produces a call marked `subagent`; a session carrying no records, alongside a stdout log that does carry `◈` markers, yields zero tool calls. That last one is what makes the deletion observable rather than assumed. Cite red `cargo test` output.
- [x] 1.2 Add the mirror and consume it: `RawToolRecord { name: String, id: Option<u64>, call_id: Option<u64>, parent_call_id: Option<u64>, args: Option<serde_json::Value>, truncated: Option<bool>, full_output_path: Option<String> }` (only `name` required, everything else tolerant per D1) and `tool: Option<RawToolRecord>` on `RawMessage`. In `parse_str`: `ToolCall.name` comes from `tool.name` (no first-whitespace-token parsing), `summary` stays the content with the leading name token stripped so existing `tool_arg_contains` asserts keep their meaning, `subagent` comes from the role, and a `tool_call`/`subagent_tool_call`-role message without a record is a schema `Err`. `tool_result`-role messages stay messages, not calls.
- [x] 1.3 Delete the stdout reconstruction outright, no fallback: `tool_calls_from_stdout`, `tool_calls_from_stdout_file`, `from_run`'s loop over `artifacts.turns`, and the `stdout_tool_call_tests` module. Rewrite the module-doc contract block and `from_run`'s doc: one evidence channel for both backends, and the `◈` markers are diagnostics.
- [x] 1.4 Regenerate the two fixtures that author tool-call-role messages, `crates/zseval/tests/fixtures/session-ask-readonly.json` and `session-search-then-read.json`, to the new shape, modelled on upstream's own examples in `../zerostack` (now synced, `main` at `7b5581a`): `src/tests/headless_tool_record_tests.rs` and `src/tests/headless_subagent_record_tests.rs` are the closer models because they cover the headless `-p` path this harness drives, with `src/tests/session_storage_tests.rs` for the round-trip shape. The upstream types are `session/mod.rs`'s `ToolRecord` (untagged, so the JSON is flat) and `PromptRef`, whose `source` serializes `snake_case`, i.e. exactly `"built_in"` / `"user_file"`. Every stub call site copies one of these two files through `write_stub_zs_bin`, so they all follow from this edit; do not hand-edit the stub script. Rework `from_run_tests` to author tool records in the session rather than in a `turn-N.stdout` file.
- [x] 1.5 Evidence: 1.1 green and `cargo test --workspace` green. This is the section that would otherwise leave the suite red: the strict rule in 1.2 and the regenerated fixtures in 1.4 must land in the same commit, because the fixtures are what every stub-backed test replays.

## 2. Transcript reads prompt provenance (design D3) [dispatch: too-te, depends: 1, parallel: no, reason: one struct, one field, one last-wins rule, and three spec scenarios that name their own assertions]

- [x] 2.1 Write the failing tests first, one per session-evidence spec scenario: a session recording `prompt: { name, source }` exposes both on the `Transcript`; a `source` string that is neither `built_in` nor `user_file` is a schema `Err`; an absent `prompt` field parses as `None` rather than erroring. Cite red output.
- [x] 2.2 Add `RawPromptRef { name: String, source: String }` and `prompt: Option<RawPromptRef>` on `RawSession`, expose the readback on `Transcript`, and make `absorb` apply last-wins, matching both upstream's last-write-wins and `final_assistant`'s existing rule.
- [x] 2.3 Evidence: 2.1 green, `cargo test --workspace` green.

## 3. Runner maps, reconciles, cross-checks (design D3/D4/D5) [dispatch: sai-hu, depends: 2, parallel: no, reason: four mapping arms, a cross-check that must lose to the readback while still warning, and a reconciliation whose two failure cases (disagreement, all-absent) carry different messages; the spec constrains these but does not settle the shape]

- [x] 3.1 Write the failing tests first: the four mapping arms of D3 (`built_in`→`stock`; `user_file` + the scenario seeded `work:.zerostack/prompts/<name>.md`→`scenario`; `user_file` + the pack provides `<name>`→`pack`; `user_file` + neither→`unknown` plus a warning); a pack prompt whose bytes equal a built-in records `stock` and warns rather than failing; trials disagreeing reconcile to `unknown`; every session lacking the `prompt` field reconciles to `unknown` with a warning that names the `ZS_BIN` rebuild; a loop-mode scenario records exactly what it records today. Cite red output.
- [x] 3.2 Surface each trial's readback out of `run_trials_for_scenario` and reconcile it to the scenario level: all trials agreeing wins; disagreement or all-absent records `unknown` and warns on stderr, the two cases carrying different messages because they have different fixes.
- [x] 3.3 Rework `resolve_prompt` into D3's mapping with the old derivation kept as a cross-check that warns on mismatch and loses to the readback. Delete the `seeds_effective_config`→`unknown` early return for session-backed scenarios, and keep the whole derivation, that branch included, for loop-mode scenarios, which have no session file to read.
- [x] 3.4 Evidence: 3.1 green, `cargo test --workspace` green.

## 4. Backend doc posture (design D2) [dispatch: too-te, depends: 1, parallel: no, reason: two doc blocks, and the sentence that must go is quoted below]

- [x] 4.1 Rewrite `backend.rs`'s module doc and the tool-call comment at the artifact-collection site. The claim that has to go is that `--pure-stdout` is "the only channel that reveals tool calls at all in headless mode": after section 1 it is false, and it invites the next reader to parse markers again. The replacement says the session JSON is the evidence channel and `turn-N.stdout` is a human-facing artifact. `--pure-stdout` keeps being passed and the capture keeps happening; only the rationale changes. Evidence: a grep showing the old sentence is gone, and `cargo test --workspace` green.

## 5. The `prompt_recorded` assert (design D6) [dispatch: too-te, depends: 2, parallel: no, reason: one assert added to an existing DSL with its neighbours to copy]

- [ ] 5.1 Write the failing tests first: `prompt_recorded code built_in` passes against a transcript whose readback matches; a mismatch in either the name or the `source` fails naming both sides; a transcript with no readback **fails**, never passes vacuously. Cite red output.
- [ ] 5.2 Add `prompt_recorded <name> <built_in|user_file>` to `asserts.rs`: the header-doc entry, the parser arm, and grading against the transcript's raw readback. It takes upstream's two-value vocabulary deliberately, not the report's four-value `prompt_source`: the assert's job is to pin the evidence channel, not this repo's reading of it. Evidence: 5.1 green.

## 6. Regression scenarios and the ledger (design D6) [dispatch: too-te, depends: 5, parallel: no, reason: two scenario files copying the conventions of their neighbours, plus the ledger entry the drift gate requires]

- [ ] 6.1 Add the two `scenarios/session/` regression scenarios: one that has the agent perform a trivial tool call and asserts `tool_called` (after section 1, its passing is itself proof that headless sessions carry tool records), and one bare run asserting `prompt_recorded code built_in`.
- [ ] 6.2 Register both ids in `scenarios/coverage.toml` under the area they belong to, because the coverage-ledger drift check fails on any scenario no `covered` claim cites. Evidence: `cargo test --workspace` green, which is where that drift check runs.

## 7. Example and docs [dispatch: too-te, depends: 3, 5, 6, parallel: no, reason: mechanical edits against criteria this document and the design already state]

- [ ] 7.1 `examples/prompt-pack`: replace the marker-file proxy asserts with asserts on the recorded prompt identity, and make sure no pack prompt is byte-identical to a zerostack built-in, since upstream's content-based classification would call it `built_in` and the example would then assert the wrong thing.
- [ ] 7.2 README, and `AGENTS.md` if it describes the evidence channel: document the session-JSON readback and the `ZS_BIN` prerequisite (an all-features build from a mainline carrying PR #230 and #228).
- [ ] 7.3 Tick roadmap lines a2.1/a2.2/a2.3 in `ROADMAP_ZS.md` and the matching items in `ROADMAP.md`. Edit only: both files are deliberately untracked, so they will not appear in this section's commit.

## 8. Harness integration and the full gate [dispatch: too-te, depends: 7, parallel: no, reason: one test fixture update and the standing gate commands]

- [ ] 8.1 Confirm the harness's stub-backed paths carry the new shape end to end. Section 1.4 regenerated the two fixtures the stub copies, so this is a check rather than a second edit: run the stub-backed tests, and if any session JSON authored inline in `tests/harness.rs` still writes a tool-call-role message without a record, bring it to the new shape here.
- [ ] 8.2 Evidence: `cargo test --workspace` green, `cargo fmt --all --check` clean, `cargo clippy --workspace --all-targets` clean.

## 9. Operator verification (not a factory round) [dispatch: operator, depends: 8, parallel: no, reason: needs a rebuilt binary and a paid API call, so no subagent can run or verify it]

- [x] 9.1 Prerequisite, in `../zerostack`: sync the fork's `main` from `upstream/main` (which carries `c6314ad`, the merge of PR #228; PR #230 is already in the fork's `main` via `e268bea`), then rebuild `ZS_BIN` as the all-features release build.
- [ ] 9.2 One real smoke run against the rebuilt binary: a tools scenario plus the two regression scenarios from section 6, confirming the readback values on live output rather than on regenerated fixtures. This is the only step that proves the fixtures match what the real binary writes.
