# Proposal: judge-provider-card

## Why

The committed judge file currently drives an authenticated network call: its `api_url` and `api_key_env` fields let any PR-introduced file send an arbitrary env secret to an arbitrary host (verified exfiltration vector on PR #2), while the configurable endpoint never bought real capability (the response parser only speaks Anthropic). Meanwhile the actual goal, freely choosing the judge model across providers, was never achieved. This change replaces the judge config with an inert "ruler card" (provider, model, prices) and delegates provider transport to rig-core, so the security hole is removed by construction and multi-provider judging is gained at the same time.

## What Changes

- Judge file shrinks to four required fields: `provider` (closed set: `anthropic | openai | openrouter | gemini`), `model`, `price_in_usd_per_mtok`, `price_out_usd_per_mtok`. **BREAKING**: `api_url` and `api_key_env` are removed; files containing them fail loudly with a targeted security/migration error (exit 2).
- **BREAKING**: no default judge. A suite containing judge-graded scenarios requires an explicit `--judge <file>` or `--no-judge`; neither given is a usage error (exit 2). The old "omit `--judge` for pinned default" behavior is deliberately dropped: experimenters must fully own their experiment.
- Judge transport moves from a curl subprocess to rig-core 0.40 (thin low-level completion path, no Agent machinery). The API key never leaves the process; endpoint and key env var are decided by a code-level match on the provider enum, never by config. OpenAI uses the Responses API.
- Temperature 0 is sent by default; if the provider rejects it (reasoning models), retry once without it and record the omission in the request artifact.
- `max_tokens` is a fixed constant 1024 (thinking models need headroom; non-thinking models incur no extra cost).
- Preflight before any trial spends money: (1) the provider's key env var must be set, else exit 2 with setup guidance; (2) a live dry-run probe in the real judge shape must return a parseable verdict, else exit 2 relaying the provider error.
- Transient judge errors (429/5xx/transport) are retried once with a short backoff before a trial goes Indeterminate.
- Report identity fields (`judge_file`, `judge_hash`, `judge_model`) and schema version 3 are unchanged; `judge_model` is now read from rig's raw response. Run artifacts become a reconstructed request record plus the serialized raw response.
- `TrialResult.run_dir` is recorded working-directory-relative instead of absolute, fixing the local-path leak into committed baselines.

## Capabilities

### New Capabilities
- `judge-card`: the judge file format (four required fields, closed provider set), its validation, and the structural security invariant that a committed file can never name a network destination or a secret's env var.
- `judge-selection`: the `--judge` / `--no-judge` CLI contract: single arity, mutual exclusion, mandatory explicit choice for rubric suites, no built-in default.
- `judge-execution`: how a judge call runs: rig transport per provider, temperature policy, token budget, transient retry, verdict parsing, artifacts, and metadata recording (file/hash/served model, cost into `--max-total-usd`).
- `judge-preflight`: fail-fast checks (key presence + live dry-run probe) before any trial spends money, for both `run` and `regrade`.
- `report-paths`: recorded paths in reports (`run_dir`, `judge_file`) are working-directory-relative, forward-slashed, never absolute.

### Modified Capabilities

(none: no prior specs exist in `openspec/specs/`)

## Impact

- Code: `crates/zseval/src/judge.rs` (rewrite), `crates/zseval/src/main.rs` (resolve_judge three-state, preflight wiring, usage text), `crates/zseval/src/runner.rs` (run_dir recording, availability backstop message), `crates/zseval/src/verdict.rs` (expose `record_path` crate-wide), `crates/zseval/Cargo.toml`.
- Dependencies: adds `rig-core =0.40.0` (rustls default features) and `tokio` (rt, rt-multi-thread); removes the curl subprocess and its escaping machinery.
- Files/docs: `judges/sonnet.toml`, `judges/opus.toml`, `judges/README.md` (rewrite, points to models.dev), root `README.md`, usage banner.
- Process: PR #2's feature commit is rewritten (force-push); the old design's demise is documented in the PR description. PR #2 must not merge before this lands.
- Out of scope, tracked: the same committed-file shape (`base_url` + `api_key_env`) still exists in target files and needs its own audit; Ollama / OpenAI-compatible endpoints / judge tools are future work.
