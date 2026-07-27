## Why

The v1 site cannot ship on numbers that lie. Today three false-pass paths exist (`file_not_contains` passes on zero hits, unknown scenario fields are silently ignored, `compare` says nothing when one side was budget-truncated), a report cannot answer "which zerostack did this measure" (the 07-24 binary printing 1.7.1 while the checkout was 1.7.2 proved this is a live hazard, not a hypothetical), and a 0% score cannot be told apart from an expected-low capability probe. These are ROADMAP Day 1 items 1 and 2: the non-negotiable floor under every Day 2 deliverable (new baseline, site, issue list).

## What Changes

- `file_not_contains` fails when no file matches (was: unconditional pass); new `path_not_exists` assert (passes only when nothing exists at the path, directories count as existing) sharing the same path language (root prefixes + one-star glob) via a matcher split out of `read_glob`. Closes PLAN.md Gap E.
- **BREAKING** (scenario schema): `deny_unknown_fields` on all three scenario deserialization layers — `Scenario`, the named nested structs (`FileSeed`, `LoopCfg`, `MemorySeed`, `NoteSeed`, `McpSeed`, `McpServerSeed`), and the untagged `Task`/`Turn` enums — making the README's claimed load-time error real. Audited: all 42 existing scenario.toml files are clean; nothing breaks in-tree.
- **BREAKING** (scenario schema): required `kind = "capability" | "regression"` field, no default; all 42 scenarios classified per the pre-adjudicated table; `ScenarioResult` records `kind`.
- `Report` records zerostack identity: `zs_version` (verbatim `--version` first line, no format validation), `zs_bin_path`, `zs_bin_sha256` (computed once per run), plus `git_sha`/`features` as `Option` fields that are always `null` today (upstream embeds neither). Capture failure aborts the run — no report without an identity. Mock backend records fixture identity (`"mock"` / fixture path / fixture content fingerprint) instead.
- `Summary` gains fixed per-kind sub-summaries (`regression` / `capability`, each with counts and pass@k / pass^k); run summary prints three lines; matrix footer renders three groups; overall stays at top level.
- `compare` gains two warnings — budget-truncation on either side, `zs_mismatch` on differing `zs_bin_sha256` — under a single codified policy: exit code answers only the gate question; every comparability threat is a warning, enforced by an all-warnings-lit invariant test and recorded as the repo's first ADR. Matrix legend gains a zs identity line alongside the existing pack identity line.
- **BREAKING** (report schema): report-family deserialization becomes strict — the ~32 legacy `#[serde(default)]` escape hatches on `TrialResult`/`ScenarioResult`/`Report` are removed, `compare`'s empty-hash skip branch goes, and the committed pre-fix baseline `baselines/main.json` is deleted (superseded; Day 2 regenerates against v1.7.2).

## Capabilities

### New Capabilities

- `absence-asserts`: strict zero-hit semantics for `file_not_contains`, the new `path_not_exists` assert, and the shared path-matching layer both stand on.
- `scenario-strict-load`: unknown fields anywhere in scenario.toml are load-time errors, across all three deserialization layers.
- `scenario-kind`: the required `kind` classification, its recording in results, and per-kind summary metrics in report output.
- `report-zs-identity`: the zerostack identity fields, their capture (hard fail), and mock-backend fixture identity.
- `report-strict-read`: report-family JSON is read strictly; a report missing identity or any schema field is rejected at load, not silently defaulted.

### Modified Capabilities

- `controlled-variables`: two new warnings (budget-truncated side, zs build mismatch); the "build is always moved, for now" assumption retired — build identity is now recorded and compared; single warning policy codified (warnings never touch exit code).
- `matrix-render`: rows and footer grouped by kind; legend carries zs identity per column.

## Impact

- Code: `asserts.rs` (S1), `scenario.rs` + `domains/` (S2), `main.rs`/`backend.rs`/`verdict.rs` (S3), 42 `scenario.toml` files (S4), `verdict.rs`/`main.rs`/`matrix.rs` (S5), `compare.rs` + `design.md` + first ADR (S6), `verdict.rs`/`compare.rs`/`baselines/` (S7).
- Artifacts: `baselines/main.json` removed from git; `compare` can no longer read the 07-21 baseline (ROADMAP Day 2 wording already updated: the old-vs-new interpretation is written by hand from git history, not tool-compared).
- Tests: full chain runs with zero API cost (mock backend + stub `--zs-bin` scripts); three verdict.rs tests flip from "tolerates old reports" to "rejects old reports".
- README: assert table, load-time-error claim (now true), three-state `judge_model` wording (absent becomes an error; only `null` means unknown), zerostack identity section.

## Non-Goals

- Coverage ledger (`scenarios/coverage.toml`) — Day 1's other lane, separate change.
- Baseline regeneration, `zseval site`, GitHub Pages — Day 2.
- `report.model` recording facts — track B (second backend).
- Resume/re-run (`content_hash` at trial level) — post-v1.
- Relaxing `pack_mismatch`'s conservative two-variables treatment — revisit on Day 2 data.
- The registered `PromptPack::fingerprint` NUL-collision finding — stays open; new fingerprint code in this change must not copy the flaw (length-prefix when folding), but fixing prompts.rs is out of scope.
