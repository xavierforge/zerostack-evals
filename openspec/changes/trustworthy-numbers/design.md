## Context

zseval is the eval harness for zerostack (~12.6k lines Rust, single-crate workspace). ROADMAP Day 1 items 1+2 ("make the numbers trustworthy") were pre-planned as the E1 card in the workflow handoff, then refined in a grilling session on 2026-07-27 that overturned two of the card's assumptions and grew the scope by one section (S7, legacy-default cleanup). All decisions below were locked in that session; the reconciliation list at the end maps each to its section.

Current state, verified in-tree:

- `asserts.rs:255`: `file_not_contains`'s `Err` arm returns pass — `read_glob` bails both on a missing file and on a zero-hit glob, so both funnel into the unconditional pass. All 8 in-tree uses today are guarded by a sibling `file_contains` on the same path, so no live false pass exists yet; the fix is mine-clearing for Wave 1, not a Day-2 number change.
- `scenario.rs`: no `deny_unknown_fields` anywhere in the scenario family. Audit of all 42 scenario.toml files found zero unknown top-level keys — the top layer alone would catch nothing. The dangerous layer is the untagged `Turn::Full`: a typo'd `new_session` silently becomes `false`, which flips `session-fresh-forgets` from testing isolation to testing continuation, still green.
- `Report`/`ScenarioResult`/`TrialResult` (verdict.rs): ~32 `#[serde(default)]` attributes whose stated justification is reading pre-field committed baselines. The only deserialization entry point is `compare::load_report` (used by `compare` and `matrix`); `trial.json` is written and printed but never parsed back.
- `compare.rs:200`: `exit_code()` is a pure function of rows/errored/regressions (0 clean / 1 regressions / 2 nothing comparable). Four warning kinds exist; none touch it. The `controlled-variables` spec's third requirement codifies "build is always moved, for now" because no report records build identity.
- `--version` was live-tested 07-26: one line (`zerostack 1.7.1`), exit 0, no git sha / features / build date — and the tested binary was one version behind its checkout, which is the motivating incident for recording binary identity, not just version strings.

## Goals / Non-Goals

**Goals:**

- Close the three false-pass paths (assert semantics, unknown fields, silent truncation in compare).
- Make every report answer "which zerostack produced this" or not exist at all.
- Make "is this 0% a problem?" answerable mechanically via `kind`, with per-kind metrics the future CI gate and Day-2 site can consume directly.
- One warning policy for `compare`, codified once (ADR), enforced structurally (pure `exit_code()`, invariant test).
- No compatibility debt: strict read and write, dead escape hatches removed the same day they die.

**Non-Goals:**

- Coverage ledger, baseline regeneration, `zseval site`, `report.model` facts, resume — see proposal.
- Relaxing `pack_mismatch`'s two-variables conservatism (needs Day-2 data).
- Fixing `PromptPack::fingerprint`'s registered NUL-collision finding (new code must not copy the flaw; the old code stays as-is).
- The "experiment invalid" exit code 3 (ROADMAP #12) — reserved in the ADR, not built (no consumer until the CI gate).

## Decisions

### D1. Absence asserts share the existing path language (S1)

`file_not_contains`'s `Err` arm flips to fail, symmetric with `file_contains`. New `path_not_exists` passes only when zero paths match; a directory counts as existing. Both use the same root prefixes (`data:`/`config:`/`work:`, default `data:`) and the same at-most-one-star glob as every other file assert — one path language, no per-assert exceptions. Zero-hit glob semantics are the exact complement of `read_glob`'s bail: `data:sessions/*` green iff the directory is missing or empty.

Implementation: split a shared matcher out of `read_glob` — "list all hits for this pattern, files and directories" — with `file_contains`/`file_not_contains` filtering to files and reading contents (behavior unchanged), and `path_not_exists` counting hits and naming them on failure. This matcher is also the base Gap D (stdout/stderr asserts) will stand on.

*Alternative rejected:* narrow `path_not_exists` (error on `*`) — creates the mini-DSL's first inconsistent rule and fails Gap E's `--no-session` use case ("no session file may exist, whatever its name") before Wave 1.

### D2. deny_unknown_fields covers all three layers (S2)

Top-level `Scenario`, the six named nested structs, and the untagged `Task`/`Turn` enums. The top layer alone is a no-op (audit: zero unknown keys in-tree); the real protection is at the nested and untagged layers, where a typo'd `new_session` currently rewrites what a scenario measures without failing anything.

Accepted cost: untagged-enum errors degrade to "did not match any variant" without naming the offending field. The loader wraps errors with the scenario path; acceptable now, revisit only if it bites.

Known risk (worker note): serde's `deny_unknown_fields` behavior on untagged enums must be proven by the typo-fixture test, not assumed. Fallback if it does not bite: hand-written `Deserialize` for `Turn` (string-or-strict-struct), which is the standard pattern. Write the failing-fixture test first; it forces the truth either way.

### D3. Identity is captured, verbatim, or the run dies (S3)

At run start (once, not per trial): execute `ZS_BIN --version`; success = exit 0 and non-empty stdout; record the first line verbatim as `zs_version`. **No format validation** — the version string is evidence for humans; the machine-comparable identity is `zs_bin_sha256` (file content hash, computed once). Validating the format would make upstream's output shape a compatibility contract exactly while we lobby upstream to enrich it.

Any capture failure (unrunnable, exit != 0, empty output) aborts the run before any API spend. Rationale: this feature exists to make identity-less reports impossible; a null fallback would reintroduce them.

`git_sha` and `features` are `Option`, always `null` today — live-tested: the binary embeds neither (no build.rs, no clap customization). The spec states this as fact, not as a runtime "record if available" branch.

Mock backend records what produced its evidence: `zs_version = "mock"` (the backend's existing `name()`), `zs_bin_path` = fixture path as given, `zs_bin_sha256` = fixture content fingerprint (single file: sha256 of bytes; directory: fold sorted relative paths + length-prefixed bytes — length-prefixed specifically to not copy the registered `PromptPack::fingerprint` NUL flaw). A `--zs-bin` passed alongside `--backend mock=` stays ignored: identity records the evidence source, not an unused binary. Distinct fixtures therefore trip `zs_mismatch` in compare, which is correct — different fixtures are different subjects — and gives the harness's own tests a zero-API path through the mismatch logic.

Testing: stub `--zs-bin` shell scripts, four behaviors (prints version / exits nonzero / prints nothing / does not exist). Zero API cost.

### D4. kind is required, undefaulted, and recorded in results (S4)

`kind = "capability" | "regression"`, required, no serde default. Any default direction lets new scenarios take a side silently: defaulting to `capability` waves new scenarios past the future CI gate; defaulting to `regression` throttles the gate with unvetted rows. The field's entire purpose is to force the author to answer "is a low score a problem?"; a default un-asks the question.

The 42-row classification table (29 regression / 13 capability, pre-adjudicated 07-26, ids mechanically verified against the tree 07-27) lives in the spec, not just the handoff. Five contested rows keep their labels with Day-2 verification hooks; label-before-evidence: a regression failing the new baseline is a finding for the issue list, never a silent downgrade to capability. Data-driven relabels require a one-line reason in the commit message.

`ScenarioResult` records `kind`: matrix and the Day-2 site group from report JSON alone, never by re-reading scenario.toml.

Sequencing: the schema change and the 42-file labeling land in one batch (same change, S4); between them the suite is red by design. Not splittable across merges.

### D5. Per-kind metrics are fixed fields, overall stays put (S5)

`Summary` gains two fixed named sub-structs, `regression` and `capability` (each: `n_scenarios`, `n_gradable`, `pass_at_k`, `pass_hat_k`). Not a map keyed by kind: the enum is closed and two-valued; adding a third kind *should* be a loud schema decision (it changes CI-gate semantics), so the friction of a struct change is a feature. Rust-side this is plain structs, no serde tricks.

Overall `pass_at_k`/`pass_hat_k` stay at top level, unmoved — the historical yardstick (07-21's 0.878/0.732 were this) and last in display order, since a blended number that averages expected-low capability probes into contract regressions is the least interpretable of the three.

`n_gradable = 0` for a kind: rates render `n/a` per the existing `rate()` convention, JSON records `0.0` (matching current overall behavior; no third representation). `scenarios` array order in JSON is unchanged (discovery order); grouping happens at render time only. Run summary prints three lines (regression, capability, overall); matrix footer renders three groups (adds rows, not width — the 48+12N budget is untouched).

### D6. One warning policy, one ADR, structural enforcement (S6)

Policy, once: **`compare`'s exit code answers only the gate question (0 clean / 1 regressions / 2 nothing comparable); every fact that weakens or invalidates that answer is a warning, no exceptions.** The maintenance debt in warnings is not any single warning's behavior; it is each warning carrying its own policy ("same as target_mismatch" chains). This change replaces precedent-chaining with a single source of truth.

Structural anchors:
1. `exit_code()` stays a pure function of rows/errored/regressions — its signature admits no warning input. Acceptance criterion: unchanged by S6.
2. An invariant test constructs a `Comparison` with **every** warning lit and asserts the exit code equals the all-dark value. Future warnings extend one test, not N conventions.
3. Warnings render through one block in fixed order; adding a warning is adding a list entry.

The policy becomes the repo's first ADR ("compare always warns; matrix owns MULTI-VAR") — already queued in the workflow handoff with prerequisites met; S6 adding two warnings is the natural trigger. The ADR reserves exit code 3 ("experiment invalid") as a future aggregate predicate over the warning set inside `exit_code()` — one place, when the CI gate creates a consumer, not per-warning escalation flags.

The two new warnings:
- **Budget truncation**: warn when either side's `budget_truncated` is set, naming the side(s). The truncated side's scenarios already surface in added/removed; the warning supplies the *why*. No exit-code effect (four combinations: 0/1 × quiet/warned).
- **zs_mismatch**: warn when `zs_bin_sha256` differs. This retires `controlled-variables`' "build is always moved, for now" requirement: the build is now an observed variable like target and pack. The compare.rs doc comment and openspec design notes referencing "for now" are updated in the same section.

Matrix legend gains a zs identity line per column (version + short sha), same shape as the existing pack identity line.

### D7. Strict read and write, cleanup lands today (S7)

Read-side strictness matches write-side: report-family JSON missing any field fails at `load_report`, with an error naming the gap. The eight-times-repeated `#[serde(default)]` pattern existed to read pre-field baselines; the user has discarded the only such artifact (`baselines/main.json`, superseded — it measured pre-fix zerostack and Day 2 regenerates against 1.7.2). Unlike those eight precedents, the identity fields would have *no legitimate runtime path* producing their default after hard-fail capture — a default would manufacture a state only reachable by reading foreign files, and `zs_mismatch` would need a third truth value to cope with it.

Scope: remove the ~32 legacy `#[serde(default)]`s (fields whose default value is also a legitimate runtime value — e.g. `judge_file = ""` meaning "no judge named" — keep the value's meaning; only the deserialization escape hatch goes); simplify `compare.rs:121`'s empty-hash skip branch to plain inequality (current runs always hash); flip the three verdict.rs tolerates-old-JSON tests to assert rejection; `git rm baselines/main.json` (+ README wording); README `judge_model` three-state wording tightens (absent = load error; `null` = unknown; `[]` = nothing graded).

Consequence, accepted and already reflected in ROADMAP: `compare` can no longer read the 07-21 baseline; the Day-2 old-vs-new interpretation cites git-history numbers by hand.

### D8. Section topology and dispatch

| Section | Content | Dispatch | Depends on |
|---|---|---|---|
| S1 | absence asserts + shared matcher | too-te | — |
| S2 | strict scenario loading | too-te | — |
| S3 | zs identity capture | sai-hu | — |
| S4 | kind schema + 42 tomls | too-te | S2 |
| S5 | per-kind metrics | too-te | S4 |
| S7 | legacy-default cleanup | too-te | S3 |
| S6 | compare warnings + ADR | too-te | S3, S7 |

S2 before S4 (both touch `Scenario`); S7 between S3 and S6 (S7 shares `Report` with S3 and compare.rs with S6 — the middle slot minimizes conflicts). S1, S2, S3 can run in parallel. S3 is the one judgment-bearing section (external process handling, failure taxonomy) — sai-hu; everything else is fully specified — too-te.

Whole chain is zero-API: mock backend + stub `--zs-bin` scripts.

## Risks / Trade-offs

- [serde may not enforce `deny_unknown_fields` through untagged enums] → typo-fixture test written first forces the truth; fallback is a hand-written `Deserialize` for `Turn` (D2).
- [Suite is red between S4's schema change and the 42-file labeling] → same section, same change, never split across merges; factory runs sections atomically (D4).
- [Untagged-enum load errors don't name the bad field] → loader wraps scenario path; accepted, revisit on real pain (D2).
- [Strict read rejects any hand-built or third-party report JSON missing fields] → dev-phase stance: regenerate artifacts; error message names the missing field (D7).
- [Removing the empty-hash skip means a hand-edited report with `""` hash on one side warns as definition-changed] → correct behavior under strictness; hand-edited reports are not a supported input (D7).
- [Mock fixture directory hashing adds a small cost per mock run] → fixtures are tiny; hash once per run, not per trial (D3).

## Migration Plan

Dev-phase, no external consumers. In-tree order is the section topology (D8). `baselines/main.json` is deleted in S7; Day 2 regenerates a baseline against zerostack 1.7.2 (rebuild all-features first — the 07-24 binary is stale, see Context). No rollback machinery: revert commits if needed.

## Open Questions

None — all decisions locked in the 07-27 grilling session. Day-2 verification hooks for the five contested kind rows are recorded in the spec (scenario-kind) and are not blockers for this change.

## Decision reconciliation list

1. `file_not_contains` zero-hit flips to fail → D1
2. `path_not_exists`: new assert, dirs count as existing, no consumer yet (Gap E groundwork) → D1
3. Path language parity (prefixes + one-star glob) for `path_not_exists`; shared matcher split from `read_glob` → D1
4. `deny_unknown_fields` on all three layers (top / named nested / untagged) → D2
5. Untagged-enum error-message degradation accepted → D2
6. serde-on-untagged risk: test-first, hand-written `Deserialize` fallback → D2
7. `--version` capture: exit 0 + non-empty, first line verbatim, **no format validation** → D3
8. Identity capture failure = hard fail, before any API spend → D3
9. `zs_bin_sha256` captured per target (same binary, so every per-target report records the same value) → D3
10. `git_sha`/`features` as `Option`, always `null` today, stated as fact → D3
11. Mock identity = fixture identity ("mock" / path / content fingerprint); length-prefixed dir fold; `--zs-bin` alongside mock stays ignored → D3
12. Four stub tests, zero API → D3
13. `kind` required, no default, either default direction rejected → D4
14. 42-row table into the spec; ids mechanically verified 07-27 → D4
15. Label-before-evidence; relabels need a commit-message reason → D4
16. `ScenarioResult` records `kind` → D4
17. S4 schema + labeling atomic (red window contained) → D4
18. Fixed two named sub-summaries, not a map → D5
19. Overall metrics stay top-level, render last → D5
20. `n/a` render / `0.0` JSON for empty kinds; JSON array order unchanged → D5
21. Warning policy single source: exit code answers only the gate question → D6
22. `exit_code()` purity as acceptance criterion; all-warnings-lit invariant test; single render block → D6
23. First ADR (compare always warns / matrix owns MULTI-VAR) with exit-3 reservation → D6
24. Budget-truncation warning names side(s), no exit-code effect → D6
25. `zs_mismatch` on sha difference; retires "build always moved, for now" (spec + doc comment + design notes) → D6
26. Matrix legend zs identity line (pack-identity shape) → D6
27. Read strictness = write strictness; identity fields never defaulted → D7
28. ~32 legacy `#[serde(default)]`s removed; legitimate runtime default *values* keep their meaning → D7
29. compare empty-hash skip branch simplified to plain inequality → D7
30. Five tolerates-old-JSON tests flip to rejection tests → D7
31. `baselines/main.json` deleted; ROADMAP Day-2 wording already updated (hand-written comparison) → D7
32. README three-state `judge_model` wording tightened → D7
33. Dispatch: S3 sai-hu, rest too-te; S1/S2/S3 parallelizable; S7 between S3 and S6 → D8
34. `pack_mismatch` conservatism unchanged (Day-2 data first) → Non-Goals
35. NUL-collision finding in `prompts.rs` stays open; new code must not copy it → Non-Goals / D3
