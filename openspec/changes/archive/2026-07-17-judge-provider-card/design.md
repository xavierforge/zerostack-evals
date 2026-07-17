# Design: judge-provider-card

## Context

The judge grades trials whose scenarios carry an LLM rubric (`Scenario.judge: Option<String>`), returning Yes/No/Unknown which the runner maps to Pass/Fail/Indeterminate. On branch `feat/judge-file` (PR #2) the judge became configurable via a committed TOML file whose `api_url` + `api_key_env` fields drive a curl call, a verified exfiltration vector: a PR-introduced file can send any env secret to any host, indistinguishable from a flaky judge. Three escalating validation patches made the attack loud, not impossible. Independently verified: the response parser only understands Anthropic's Messages API shape, so the configurable endpoint never provided real capability. The design tension: the ruler (which model grades, at what price) must be swappable and faithfully recorded, but a committed artifact must not be able to name a network destination for a secret. The sibling project `zerostack` (the agent this harness evaluates) already models providers via the `rig` crate.

Constraints that hold: keys stay in env, never in files; `--judge` is single-arity; invalid config fails loudly (exit 2); old baselines (report schema v3) still deserialize; judge spend is the only cost `--max-total-usd` sees in headless mode, so prices must exist.

## Goals / Non-Goals

**Goals:**
- Free choice of judge model across providers (anthropic, openai, openrouter, gemini): the feature PR #2 set out to deliver and did not.
- Structural security: no field in any committed config can influence where a secret goes or which env var is read. The attack class is removed, not fenced.
- Faithful experiment metadata: every report records which ruler was configured (`judge_file`, `judge_hash`) and which actually graded (`judge_model`), as fact, not declaration.
- Fail fast: misconfiguration (missing/invalid key, wrong model name, rejected params) surfaces as exit 2 before any trial spends money.

**Non-Goals:**
- Ollama or OpenAI-compatible/self-hosted endpoints (Chat Completions path), gateway `base_url` overrides, judge tool use, provider-reported cost (OpenRouter `usage.cost`), optional prices. All recorded as future work.
- Fixing the same committed-file shape (`base_url` + `api_key_env`) in target files: separate audit, separate change.
- Determinism guarantees: temperature 0 reduces borderline verdict flapping; it does not make APIs deterministic and the docs must not claim it does.

## Decisions

1. **Ruler card, not routing config.** `JudgeConfig { provider, model, price_in_usd_per_mtok, price_out_usd_per_mtok }`, all required, `deny_unknown_fields`, no `Default`. The file carries only inert data. Alternatives rejected: free-form `api_url` (the hole), validation fences (verified insufficient), hand-rolled provider registry (over-scoped, parked on `wip/provider-registry-attempt`), pure CLI flags (prices need a hashable, committable home; flags scatter the ruler identity into shell history).
2. **Provider is a closed enum; routing lives in code.** `JudgeProvider { Anthropic, OpenAI, OpenRouter, Gemini }` (serde lowercase). A `match` yields the rig client (explicit key, default base URL) and the key env var (`ANTHROPIC_API_KEY` / `OPENAI_API_KEY` / `OPENROUTER_API_KEY` / `GEMINI_API_KEY`, matching zerostack's conventions). Cross-pairing is impossible: a card can at most send a provider's own key to that provider. The invariant is written into the code as comments AND enforced by regression tests, because last time the unwritten invariant ("endpoint is a const") was silently removed.
3. **rig-core 0.40, thin path only.** `client.completion_model(model).completion_request(prompt).temperature(0.0).max_tokens(1024).send()`. No Agent machinery, no streaming. Rationale: rig outsources exactly the per-provider request/parse code the rejected registry would have hand-rolled; the dependency family is already trusted in zerostack; the key never leaves the process (deletes the curl argv/stdin/escaping surface and its tests). The core path was diffed byte-identical between 0.39 and 0.40, shrinking churn risk. OpenAI uses the Responses API (rig default client); Chat Completions is deferred to an OpenAI-compatible future. Explicit `Client::new(key)`, not `from_env()`: rig's `from_env` honors hidden `*_BASE_URL` env overrides, which would be an unrecorded input.
4. **No default judge.** Rubric suites require explicit `--judge <file>` or `--no-judge`; neither is exit 2. Suites without rubrics need neither. `resolve_judge` returns a three-state choice (File / NoJudge / Unspecified); the rubric check runs right after scenario discovery, before any trial. Rationale: the experimenter owns the experiment; a silent default ruler undermines comparability. This deliberately breaks the old "omit for pinned default" contract.
5. **Temperature: send 0, degrade loudly-recorded.** Greedy decoding is the right semantic for a classifier (modal judgment, not a sample) but is only moderately important, so it must not cost model freedom. On a provider 400 rejecting temperature (reasoning models), retry once without it and write "temperature omitted (provider rejected)" into `judge-request.json`. Error-driven, no model-name lists.
6. **`max_tokens` = 1024, fixed.** Output billing is per actual token, so the cap's only job is runaway prevention. 8 (the old value) silently breaks thinking models (OpenAI Responses reasoning tokens and Gemini 2.5 thoughts consume the output budget before any visible text: permanent Unknown, looks like flakiness). Not a card field: consistent with rejecting field creep (same stance as temperature).
7. **Preflight = presence check + dry-run in the real judge shape.** When a judge will be used: (a) key env var set, else exit 2 with an exact `export` hint and the `--no-judge` escape; (b) one live probe using the same prompt template, same max_tokens, same temperature-fallback, required to return a parseable verdict word. Catches bad auth, wrong model names, temperature rejection, thinking-budget truncation, and response-shape surprises before trial 1. Probe cost is not recorded in the report (documented). Applies to both `run` and `regrade`.
8. **Transient retry: once, short fixed backoff.** Only for clearly transient errors (429/5xx/transport), classified conservatively from rig error types (unclassifiable = not retried); auth/model errors never retry (preflight already gates them). Regrade remains the ultimate recovery path (evidence is preserved; re-judging costs only judge spend).
9. **Recording is unchanged where it was sound.** `judge_file`/`judge_hash`/`judge_model` and `REPORT_SCHEMA_VERSION = 3` stay. `judge_model` reads the served model from rig `raw_response` (all four providers expose it; Gemini's field is `modelVersion`, confirm at implementation, fall back to the existing unknown-ruler tri-state if absent). Artifacts: `judge-request.json` becomes a reconstructed record {provider, model, temperature (or omitted note), max_tokens, prompt}; `judge-response.json` becomes the serialized raw response.
10. **`run_dir` recorded relative.** Reuse `verdict.rs`'s `record_path` (canonicalize, cwd-relative, forward slashes, basename fallback) at both `TrialResult.run_dir` write sites. Verified safe: the stored String is independent of the runtime `&Path`; regrade re-canonicalizes on read.
11. **Sync trait, bridged runtime.** `Judge` trait stays sync (`--jobs` worker threads); a shared `OnceLock<tokio::Runtime>` (multi-thread, small worker count) serves concurrent `block_on` calls. `preflight()` is a concrete `LlmJudge` method, not a trait method: only the CLI layer calls it, and `TestJudge` stays untouched.

## Risks / Trade-offs

- [rig supply chain: the key is handed to rig/reqwest] → pin `=0.40.0` exactly (repo convention), upgrade in lockstep with zerostack; the family is already trusted there. Verify by attack, not by reading: bogus-key probes must produce real auth errors only at official endpoints.
- [rig is pre-1.0; API churn] → the used surface was diffed byte-identical 0.39→0.40; pinning + the thin path minimize exposure.
- [Compile weight: tokio + reqwest + rig into a previously sync-only crate] → accepted consciously; measured once; the deleted curl machinery and its test surface partially offset.
- [A PR can still change model or prices: judge swap or budget-cap deception] → accepted residual: inert data, visible in diff, pinned by `judge_hash`, cross-checked by `judge_model`; harm is bounded (spend/verdict skew), never secret exfiltration.
- [Wire-fidelity loss: no raw request bytes] → reconstructed request record + serialized raw response; documented.
- [1024 tokens may still truncate pathological thinkers] → verdict goes Unknown, visible in report; dry-run probe catches systematic cases upfront.
- [Behavior break: users who relied on the silent default judge or keyless partial runs] → loud exit-2 errors name the exact fix (`--judge`, `--no-judge`, `export ...`); judges/README documents the policy change.
- [Offline testability of network paths] → test-only rig `base_url` pointing at a local mock server (code, not config, so the invariant stands) covers probe success/failure, temperature fallback, and retry; live paths verified via the anti-contamination /verify recipe.

## Migration Plan

1. Land as a rewrite of PR #2's feature commit (force-push; the maintainer owns the branch, PR unmerged). The lint chore commit stays; the `run_dir` fix is a separate `fix(report)` commit. Old design's demise documented in the PR description.
2. Old judge files fail loudly with a migration-grade error naming the removed fields; `judges/*.toml` in-repo are updated in the same commit, `judges/README.md` points to models.dev for model ids and prices.
3. Old baselines (schema v3) deserialize unchanged; no report migration needed.
4. Rollback: revert the branch; `main` never contained the vulnerable design.

## Open Questions

(none: all design decisions were resolved in the grilling session of 2026-07-17; the only implementation-time verification is Gemini's served-model field name in rig's raw response, with a defined fallback.)
