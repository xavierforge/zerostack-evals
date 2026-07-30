## Why

The v1 site's coverage section needs a denominator, and "the scenarios that happen to exist" is not one — a page built on that denominator can only report on the half of zerostack the harness already flatters itself by testing. Today nothing in the repo answers "which parts of zerostack are untested, and why not": the answer lives in `scenarios/PLAN.md`, a 309-line planning document that mixes scheduling (waves) with facts (gaps), and whose product-side claims went stale within ten days when upstream fixed five of them. This is ROADMAP Day 1 item 3, and the last piece of "make the numbers trustworthy" before Day 2 renders them.

## What Changes

- New tracked data file `scenarios/coverage.toml`: 15 areas of zerostack's functional surface (the 12 carved out in PLAN.md on 2026-07-16, plus `mcp`, `loop`, and `project-instructions`, which already have scenarios), each area carrying claims. English content, array-of-tables ordering (file order is presentation order). `project-instructions` (the `AGENTS.md` walk and rule scoping) is kept distinct from `context-window` (catalog and window sizing); they share a source directory name and nothing else.
- Four claim statuses with per-status required fields: `covered` requires `scenarios` (ids); `uncovered` takes an optional `blocked_by` (present = a harness gap blocks it, absent = buildable today); `product-blocked` and `excluded` each require a one-line `reason`. An optional `zs` pointer on `product-blocked` and an optional free-text `note` on any claim carry the prose that no machine can check.
- `covered` is an **existence** claim, not a pass claim and not a run-membership claim: a scenario that scores 0% is still coverage (`kind` already carries the "is this 0% expected" reading), and whether a covered scenario appears in a given report is the consumer's problem, computed from that report — never recorded in the ledger.
- A scenario id may appear in at most one `covered` claim ledger-wide. Deliberate calibration overlaps (PLAN.md suite policy 3) are recorded as prose in `note` and take no coverage slot, so per-area coverage counts have exactly one meaning.
- Header fields: `audited_against` (the zerostack version the ledger's judgments were made against) and `scenario_roots` (the trees the drift check walks: `scenarios`, `examples/prompt-pack`).
- New module `crates/zseval/src/coverage.rs`: strict loader (`deny_unknown_fields` at every layer, status-tagged enum so a wrong field combination is a load error), the bidirectional drift check, and `audit_matches` — a containment test against a `--version` banner, never a parse, preserving `backend.rs`'s standing refusal to make upstream's banner shape a compatibility contract.
- `crates/zseval/tests/harness.rs` gains a drift test that runs the check against the real repo tree. Adding a scenario without claiming it in the ledger turns `cargo test` red; so does a ledger reference to an id that no longer exists. CI already runs `cargo test --workspace` on every PR, so no workflow edit.
- No new CLI subcommand and no README section: the ledger serves this repo's own honesty, not an end user. `zseval site` (Day 2) is its first and only renderer.

## Capabilities

### New Capabilities

- `coverage-ledger`: the ledger file's schema and semantics (15 areas, four statuses with their required fields, `covered` as an existence claim, one-covered-claim-per-scenario), its strict loader, the bidirectional drift check and where it is enforced, and `audited_against`'s containment-only comparison rule.

### Modified Capabilities

None. No existing requirement changes: scenario loading, report schema, and comparison are untouched.

## Impact

- Code: new `crates/zseval/src/coverage.rs`; one `pub mod coverage;` line in `lib.rs`. No changes to `verdict.rs`, `main.rs`, or `backend.rs` — the ledger is read by tests today and by `zseval site` tomorrow, never by a run. `scenario.rs` was originally scoped out too; `discover` is now the drift check's source of truth for what the tree holds, and its three silent skips made that answer unreliable, so it gained loud failures instead (2026-07-30, see `decisions.md`).
- Data: new `scenarios/coverage.toml` (~400 lines, roughly 60-70 claims across 15 areas). `scenario_roots` names `scenarios` and `examples/prompt-pack`, so the pack marker scenario is claimed rather than orphaned.
- Tests: one new integration test in `crates/zseval/tests/harness.rs` (real repo tree, zero API cost) plus unit tests in `coverage.rs` for the schema's rejection paths and `audit_matches`.
- Naming: the ledger says `area`, never `domain` — `domain` is already load-bearing in `scenario.toml`'s `domains = [...]` and `domains/mod.rs`'s `KNOWN_DOMAINS`, and three names (memory, subagents, mcp) collide across the two meanings.
- Ongoing cost: adding any scenario now also requires a ledger edit. That friction is the point (ROADMAP short-term item 10 already assumes new scenarios are claimed into the ledger).

## Non-Goals

- Rendering — the coverage section of `zseval site`, including the `audited_against` mismatch disclosure and the "covered but not in this run" marking, is Day 2's change. This change ships the ledger, the loader, and the helper the renderer will call.
- Coverage percentages, anywhere. Fine-grained `covered` claims beside coarse `uncovered` ones make any ratio manipulable by re-slicing; the headline figure is "7 of 15 areas have no scenario at all", which no re-slicing moves.
- Surface diff mechanization (extracting config keys, CLI flags, feature and prompt names from a zerostack checkout and diffing them against the ledger) — ROADMAP post-v1 item 9. This ledger is hand-audited, and `audited_against` is the only staleness signal it gets.
- A `verified_on` date per claim. The prose half of the ledger will rot; `audited_against` is the smoke alarm, and paying for staleness tracking twice buys nothing.
- Rewriting `scenarios/PLAN.md`. It keeps the scheduling (waves, gap letters, the unbuilt-proposal inventory); the ledger keeps the facts and points at no numbering that a re-plan can invalidate.
- Authoring any new scenario, and re-classifying any existing `kind`.
