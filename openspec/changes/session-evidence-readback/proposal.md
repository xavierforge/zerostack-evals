# session-evidence-readback

## Why

Upstream zerostack now records the evidence zseval has been reconstructing by hand: PR #230 writes structured tool records (including subagent tool calls) into headless session JSON, and PR #228 records which prompt a session actually loaded and whether it was the built-in or a user file. Both are merged into upstream mainline, so the two standing workarounds — rebuilding tool calls from `◈` stdout markers and inferring prompt identity from what the harness seeded — can be replaced by reading the session JSON, which is exactly what roadmap line a2.3 planned for this moment.

## What Changes

- The transcript sources tool calls from the session JSON's `tool` records (`Call` / `Result`, plus the newly visible `SubagentCall`) instead of parsing `◈ {name} {summary}` lines out of captured stdout. The stdout reconstruction is removed, not kept as a fallback (dev phase, no compat shim). **BREAKING**: requires a `ZS_BIN` built from a zerostack containing PR #230 and #228.
- Prompt identity flips from inference to readback: `prompt_name` comes from the session's recorded `prompt.name`, and `prompt_source` is mapped from the recorded `built_in` / `user_file` plus what the harness knows it seeded. The old seed-based derivation is kept only as a cross-check that warns on disagreement; the "scenario seeded the config, so record `unknown`" branch is removed because the readback is valid regardless of who wrote the config.
- The run-level "pack seeded but never loaded" check and the `examples/prompt-pack` example assert against the read-back prompt name and source instead of marker-file proxies.
- New evidence-channel regression scenarios: a headless run's session JSON must contain tool records, and must contain the `prompt` field, so an upstream regression of either channel fails loudly instead of silently emptying reports.
- Unchanged: the token estimate fallback stays, because headless zerostack still does not record real provider usage.

## Capabilities

### New Capabilities

- `session-evidence`: the session JSON zerostack writes is the harness's evidence channel — how tool records (including subagent calls) and prompt provenance are read from it, what a schema mismatch does, and the regression scenarios that pin the channel against upstream drift.

### Modified Capabilities

- `prompts-pack-identity`: the requirements "Each scenario records the prompt it actually loaded" and "The default prompt name is derived only where derivation holds" change from seed-based derivation to session readback with a derivation cross-check; the config-seeding `unknown` scenario is replaced (readback stays valid there); `unknown` narrows to "the evidence was not observed" (old reports).
- `scenario-kind`: the adjudicated classification table gains the two evidence-channel regression scenarios this change adds, so the suite moves from 42 scenarios (29 regression) to 44 (31 regression). The count is enforced by `the_committed_suite_is_...` in `tests/harness.rs`, so the main spec had to move with the scenarios rather than at archive time; this delta is the record of that edit, carrying the requirement in the same form the main spec now holds.

## Impact

- `crates/zseval/src/transcript.rs`: session message model gains the `tool` field mirror; `ToolCall` construction moves off stdout markers; the marker parser and its misread caveat go away.
- `crates/zseval/src/backend.rs`: the "only evidence channel" documentation is rewritten; whether `--pure-stdout` is still passed depends on what else stdout is used for (decided in design).
- `crates/zseval/src/runner.rs`: `resolve_prompt` becomes a readback consumer plus cross-check; the pack-load verification reads recorded sources.
- Test fixtures and the fake `ZsCli` backend must emit the new session JSON shape (`tool` records, `prompt` field).
- `examples/prompt-pack`: marker asserts replaced by prompt-identity asserts (avoiding pack prompts whose bytes equal the built-in, which upstream classifies as `built_in`).
- `scenarios/`: new evidence-channel regression scenarios.
- Operational prerequisite (not a task in this repo): rebuild `ZS_BIN` as an all-features build from a zerostack containing both PRs; the fork's `main` has not absorbed PR #228 yet, so sync it from upstream first.
