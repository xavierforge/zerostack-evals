# session-evidence-readback — design

## Context

zerostack upstream merged two evidence-channel PRs (both authored from this project's roadmap, ROADMAP_ZS lines a2.1/a2.2):

- **PR #230** — headless `-p` sessions now persist structured tool records unconditionally. `SessionMessage` gained `tool: Option<ToolRecord>` where `ToolRecord` is serde-untagged: `Call { id, name, args }` on `role: "tool_call"`, `Result { call_id, name, truncated, full_output_path }` on `role: "tool_result"`, and `SubagentCall { parent_call_id, name, args }` on `role: "subagent_tool_call"` (deliberately no `id`, so its JSON cannot satisfy `Call`). No new CLI flag; recording is independent of `--pure-stdout`.
- **PR #228** — sessions record `prompt: { name, source }` where `source` is `"built_in"` or `"user_file"`, decided by comparing the loaded file's bytes against the compiled-in default (because zerostack seeds every embedded prompt to disk on first run, existence proves nothing). Written at startup prompt resolution and on every prompt switch; last write wins. Merged into `upstream/main`; the fork's `main` has not absorbed it yet.

zseval currently papers over both gaps: `transcript.rs` reconstructs `ToolCall`s from `◈ {name} {summary}` stdout markers captured per turn (`tool_calls_from_stdout`, with a documented misread caveat), and `runner.rs::resolve_prompt` infers `prompt_name`/`prompt_source` from what the harness seeded, abandoning derivation (`unknown`) when a scenario seeds the effective config. This change replaces both with readback, per roadmap line a2.3.

## Goals / Non-Goals

**Goals:**

- Tool-call evidence comes from session JSON `tool` records; the stdout marker reconstruction is deleted, not kept as a fallback.
- Prompt identity comes from the session's recorded `prompt`; the seed-based derivation survives only as a cross-check that warns on disagreement.
- The evidence channel itself is pinned by regression scenarios, so an upstream regression fails loudly instead of silently emptying reports.
- One evidence path for real and mock backends alike: mock fixtures are regenerated to the new shape rather than served by a legacy parser (dev phase, no compat shims).

**Non-Goals:**

- Token accounting: headless zerostack still records no real provider usage, so `total_tokens()`'s estimate fallback stays exactly as is.
- Report schema changes: `prompt_source` keeps its four values (`scenario`/`pack`/`stock`/`unknown`) and its serde defaults; no `REPORT_SCHEMA_VERSION` bump.
- Surfacing tool `args` to asserts: `tool_arg_contains` keeps matching the summary string. The structured `args` value is newly available but adopting it into assert semantics is a separate, later decision.
- Upstream loop mode: `run_headless_loop` still writes no session file, so loop scenarios keep their current derivation-based prompt recording.

## Decisions

### D1. Tool calls: session `tool` records are the only source

`transcript.rs` drops `tool_calls_from_stdout` / `tool_calls_from_stdout_file` and the `from_run` loop over `turn-N.stdout` files. `RawMessage` gains a `tool` mirror field, deserialized as one tolerant struct rather than an untagged enum:

```rust
struct RawToolRecord {
    name: String,                    // present on all three shapes
    id: Option<u64>,                 // Call
    call_id: Option<u64>,            // Result
    parent_call_id: Option<u64>,     // SubagentCall
    args: Option<serde_json::Value>, // Call / SubagentCall
    truncated: Option<bool>,         // Result
    full_output_path: Option<String>,
}
```

The message `role` already discriminates the three shapes, so mirroring upstream's untagged enum would buy nothing and would break on any upstream field addition. Only `name` is consumed today; the rest deserializes and is ignored.

A `tool_call`/`subagent_tool_call`-role message **without** a `tool` field is a schema mismatch `Err` (→ Indeterminate at the runner), the same rule the parser already applies to any shape it cannot read. This is what forces mock fixtures onto the new shape and makes a pre-#230 `ZS_BIN` visible instead of silently gradable. `ToolCall.name` comes from `tool.name` (no more first-whitespace-token parsing); `ToolCall.summary` stays derived from `content` with the leading name token stripped, so existing `tool_arg_contains` asserts keep their meaning. `tool_result`-role messages remain messages, not calls.

**Alternative rejected:** keeping the stdout parser as a fallback for old binaries. Roadmap a2.3 says remove, the project is pre-launch, and a fallback would keep the misread caveat (`◈ ` inside a tool's output) alive forever.

### D2. `--pure-stdout` stays, demoted to diagnostics

`ZsCli` keeps passing `--pure-stdout` and keeps teeing `turn-N.stdout`: the marker lines make turn logs readable when a human debugs a trial, and capture costs nothing. What changes is the module doc: it currently sells the flag as "the only channel that reveals tool calls at all in headless mode", which after this change is both false and dangerous (it invites the next reader to parse markers again). The rewritten doc names session JSON as the evidence channel and the stdout log as a human-facing artifact.

**Alternative rejected:** dropping the flag. It saves nothing (the records are written regardless) and strictly worsens the debugging artifacts.

### D3. Prompt: readback is the value, derivation is the check

`RawSession` gains `prompt: Option<RawPromptRef>` (`{ name: String, source: String }`); `Transcript` exposes it; `absorb` applies last-wins, matching both upstream's own last-write-wins and `final_assistant`'s existing rule. An unrecognized `source` string is a schema `Err` (dev phase: upstream vocabulary drift should stop the run, not be guessed around).

Mapping readback to the report's four-way `prompt_source`, per scenario:

1. `built_in` → `stock`, name from readback.
2. `user_file` + the scenario seeded `work:.zerostack/prompts/<name>.md` → `scenario`.
3. `user_file` + the pack provides `<name>` → `pack`.
4. `user_file` + neither → `unknown`, with a stderr warning: a user-file prompt the harness didn't plant means the trial environment isn't what the harness thinks it is.

The old derivation (`resolve_prompt`'s name resolution and layer pick) is retained as a cross-check: when it disagrees with the readback, the readback wins and a warning names both values. This is where the known upstream edge surfaces benignly — a pack prompt whose bytes equal the built-in is classified `built_in` upstream (content-based `source_of`), records `stock`, and the cross-check warning explains why.

The `seeds_effective_config` → `unknown` branch is deleted for session-backed scenarios: it existed because the harness's seeded config was "no longer the last word" — but the readback is the last word regardless of who wrote the config. Loop-mode scenarios (no session file, see Non-Goals) keep the derivation path including that branch.

### D4. Scenario-level reconciliation across trials

`prompt_name`/`prompt_source` are scenario-level facts. Each trial's transcript now carries its own readback; the scenario value is their consensus. All trials agreeing is the expected case (same seeds, deterministic prompt resolution). On disagreement, or when every session lacks the `prompt` field (pre-#228 binary), the scenario records `unknown` and a warning says so — the warning for the missing-field case explicitly points at rebuilding `ZS_BIN`. Old *reports* still deserialize to `unknown` exactly as the prompts-pack-identity spec already requires; nothing there changes.

### D5. Pack-load verification rides on the same values

The run-level "pack seeded but never loaded" check in `run_suite` is untouched in logic — it already reads `prompt_source == Pack` off each `ScenarioResult` — but its inputs are now observed rather than inferred, which is the point of the whole change. The `examples/prompt-pack` example replaces its marker-file proxy asserts with asserts on the recorded prompt identity, and its pack prompts must not be byte-identical to zerostack's built-ins (else upstream classifies them `built_in`; see D3).

### D6. A new assert pins the channel, and two scenarios use it

New scenario assert `prompt_recorded <name> <built_in|user_file>`, graded against the trial transcript's raw readback — deliberately upstream's two-value vocabulary, not the report's four-way mapping, because the assert's job is to pin the evidence channel itself, not zseval's interpretation of it. Two new regression scenarios under `scenarios/session/`:

- one that has the agent perform a trivial tool call and asserts `tool_called` — after D1 this passing *is* proof that headless sessions carry tool records;
- one that asserts `prompt_recorded code built_in` on a bare run — proof that sessions carry the prompt field.

Both go into `scenarios/coverage.toml` (the coverage-ledger drift gate requires it).

## Risks / Trade-offs

- [Pre-#230/#228 `ZS_BIN` in someone's environment] → tool-record absence is a hard schema error naming the file; prompt absence is `unknown` plus a warning naming the rebuild; README documents the build prerequisite. The regression scenarios catch it in any real run.
- [Upstream reshapes the `tool` JSON again] → the tolerant per-role mirror only requires `name`; anything else deserializes loose. A true break still fails as a schema `Err` → Indeterminate, the designed failure mode.
- [Content-based `source_of` misclassifies a pack prompt identical to the built-in] → readback wins, cross-check warning explains, example pack avoids identical bytes by construction.
- [Last-wins on multi-session prompt hides a mid-run switch] → headless `-p`/`--continue` cannot switch prompts today; if upstream adds that, the cross-check warning fires on the derivation mismatch, which is the alarm we want.
- [Regenerated fixtures drift from real upstream output] → fixtures are regenerated from the shapes in upstream's own `session_storage_tests.rs`, and the harness integration test (fake `ZsCli`) emits the same shape.

## Migration Plan

1. Sync the fork's `main` from `upstream/main` (picks up merge `c6314ad` of PR #228; PR #230 is already in both) and rebuild `ZS_BIN` as the all-features build.
2. Land this change; regenerate mock fixtures; `cargo test --workspace`.
3. One real smoke run (`zseval run` against a trivial scenario) to see the readback on live output; then the two regression scenarios join the suite.

No rollback machinery: pre-launch, the old workaround path is deleted outright.

## Open Questions

None blocking. Adopting structured `args` into assert semantics is deferred (see Non-Goals).

## Decision reconciliation

- Remove the stdout `◈` reconstruction outright, no fallback → D1.
- Missing `tool` field on a tool-call-role message is a schema error → D1.
- Keep `--pure-stdout`, rewrite its rationale as diagnostics-only → D2.
- Readback wins; derivation demoted to warning-on-mismatch cross-check → D3.
- Delete the config-seeding `unknown` branch for session-backed scenarios → D3.
- Loop mode keeps derivation (no session file upstream) → Non-Goals + D3.
- Trials reconcile to the scenario value; all-agree or `unknown` + warning → D4.
- Pack-never-loaded check and examples assert observed values; example avoids built-in-identical bytes → D5.
- Evidence-channel regression scenarios, with a new `prompt_recorded` assert → D6.
- Token estimate fallback untouched → Non-Goals.
- `ZS_BIN` rebuild from synced mainline is the operational prerequisite → Migration Plan.
