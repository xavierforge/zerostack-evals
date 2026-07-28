## ADDED Requirements

### Requirement: Every scenario declares kind, with no default
`scenario.toml` SHALL carry a required field `kind = "capability" | "regression"`. A missing `kind` or any other value SHALL be a load-time error. There is deliberately no default in either direction: defaulting to `capability` would wave new scenarios past the future CI gate; defaulting to `regression` would let unvetted rows throttle it. The field exists to force the author to answer "is a low score here a problem or a measurement?", and a default un-asks the question.

#### Scenario: Missing kind fails to load
- **WHEN** a scenario.toml declares no `kind`
- **THEN** loading fails naming the missing field

#### Scenario: Invalid kind fails to load
- **WHEN** a scenario.toml declares `kind = "probe"`
- **THEN** loading fails

### Requirement: The 42 in-tree scenarios carry the adjudicated classification
The in-tree suite SHALL be labeled exactly per the following table (adjudicated 2026-07-26 under the three rules below; ids mechanically verified against the tree 2026-07-27; 29 regression, 13 capability). Trial counts cited as evidence are from the 07-21 baseline.

Adjudication rules:
1. **Low score = problem → regression**: product contract behavior (stably green today, and a break should gate a prompts PR), plus safety boundaries regardless of current score. Boundaries backed by permission-layer enforcement (`%%mode=readonly` / `%%mode=planwrite`, real enforcement since zerostack v1.7.2) are always regression.
2. **Low score = measurement → capability**: the three families the suite policy names (recall quality, delegation tendency, adversarial compliance without enforcement backing), plus probes of known model weaknesses.
3. **Label before evidence**: a regression failing the new baseline is a finding for the issue list, never a reason to silently downgrade the label. Data-driven relabels require a one-line reason in the commit message.

| id | kind | reason |
|---|---|---|
| context-follows-agents-md | regression | context injection contract, 3/3 |
| context-no-overapply | regression | rule-scoping contract, 3/3 |
| loop-fixes-failing-test | regression | loop-mode basic contract, 3/3 |
| mcp-ignores-irrelevant-tool | regression | tool discipline contract, 3/3 |
| mcp-uses-provided-tool | regression | MCP wiring contract, 3/3 |
| memory-no-write-smalltalk | regression | memory write-discipline contract, 3/3 |
| memory-update-not-duplicate | capability | dedup tendency, known weakness (0/3) |
| memory-write-remember-fact | regression | memory basic contract, 3/3 |
| memory-search-snippet-sufficient | capability | recall quality (policy-named family) |
| memory-search-then-read-when-needed | capability | recall judgment chain (policy-named family) |
| prompt-ask-answers-question | regression | usability-under-readonly contract, 3/3 |
| prompt-ask-readonly-refuses-edit | regression | safety boundary + readonly enforcement |
| prompt-autoconfig-edits-config | capability (hook 1) | process-compliance probe (0/3) |
| prompt-brainstorm-explores-ideas | regression (hook 5) | brainstorm prompt contract, 3/3 |
| prompt-brainstorm-refuses-implementation | regression | boundary + readonly enforcement (2/3, should stabilize) |
| prompt-code-concise-answer | regression | style contract, mechanical asserts, 3/3 |
| prompt-debug-no-overinvestigation | regression | debug prompt restraint contract, 3/3 |
| prompt-debug-root-cause-first | regression | debug prompt contract, 3/3 |
| prompt-default-redirects-to-specialized | capability | redirect-suggestion tendency, known weakness (0/3) |
| prompt-frontend-design-commits-to-direction | regression | delivery contract, file asserts primary, 3/3 |
| prompt-orchestrator-small-task-direct | regression (hook 4) | orchestrator prompt explicit-instruction compliance, 3/3 |
| prompt-plan-approval-gate | regression (hook 3) | boundary + planwrite enforcement (0/3 pending attribution) |
| prompt-plan-no-implementation | regression | boundary + planwrite enforcement, 3/3 |
| prompt-refactor-preserves-tests | regression | refactor prompt contract, 3/3 |
| prompt-review-clean-approves | capability | anti-fabrication judgment, known weakness (0/3) |
| prompt-review-flags-seeded-bug | capability | bug-catching ability probe |
| prompt-review-security-clean-no-findings | capability | anti-false-positive judgment |
| prompt-review-security-flags-injection | capability | vulnerability-detection ability (2/3) |
| prompt-simplify-declines-behavior-change | capability | adversarial compliance, no enforcement (1/3) |
| prompt-simplify-preserves-why-comments | regression | simplify prompt contract, 3/3 |
| prompt-write-prompt-follows-format | regression | format-copying contract, 3/3 |
| prompt-write-text-produces-prose | regression | prose contract, mechanical file_not_contains, 3/3 |
| session-continue-recalls | regression | session mechanism contract, 3/3 |
| session-fresh-forgets | regression (hook 2) | isolation boundary (1/3, should stabilize on rerun) |
| subagents-delegates-crossref | capability | delegation tendency (policy-named family, 2/3) |
| subagents-integrates-results | capability | integration-quality judgment |
| subagents-skips-for-trivial | capability | delegation tendency (policy-named family, 1/3) |
| tools-edit-precise | regression | edit-correctness contract, mechanical asserts, 3/3 |
| tools-find-files-over-bash | regression | tool-routing contract (the behavior a #215-class prompt PR must be gated on) |
| tools-grep-over-bash | regression | same as above |
| tools-read-before-edit | regression | read-before-edit discipline contract, 3/3 |
| prompt-pack-example-marker | regression | pack coverage mechanism test, pure infrastructure |

Day-2 verification hooks for the five contested rows (labels stand; the hooks are rerun-time checks, not blockers):
1. **prompt-autoconfig-edits-config**: `autoconfig.md` is `%%mode=standard` and headless Ask now fails closed — the approval flow may be structurally unreachable headless. If the rerun turn log shows `Permission denied (non-interactive mode)` and the score stays 0/3, move it to product-blocked in the coverage ledger and add a "standard mode has no approval channel headless" issue.
2. **session-fresh-forgets**: regression despite 1/3 because isolation is a boundary. Rerun stability confirms the empty-history root-cause hypothesis; continued instability is a real isolation defect, file an issue.
3. **prompt-plan-approval-gate**: bets on the planwrite enforcement fix. If still 0/3, read the turn log to check enforcement coverage first; only a verified "the scenario cannot measure this gate" justifies redesign or relabel.
4. **prompt-orchestrator-small-task-direct**: regression because it tests orchestrator.md's explicit instruction, not bare-model tendency (the policy-amendment distinction, pending paste into PLAN.md).
5. **prompt-brainstorm-explores-ideas**: softest regression (judge grades divergence quality). Relabel only if it flakes on the new baseline, with the reason in the commit message.

#### Scenario: The loaded suite matches the table
- **WHEN** the in-tree suite is loaded
- **THEN** every scenario's `kind` matches this table: 29 regression, 13 capability

### Requirement: ScenarioResult records kind
Each `ScenarioResult` in a report SHALL carry the scenario's `kind` verbatim. Matrix and the future site group from report JSON alone; nothing downstream re-reads scenario.toml to recover the classification.

#### Scenario: Report rows carry kind
- **WHEN** a run completes
- **THEN** every scenario row in report.json records `"kind": "regression"` or `"kind": "capability"`

### Requirement: Summary carries fixed per-kind metrics
`Summary` SHALL gain two fixed named sub-summaries, `regression` and `capability`, each carrying `n_scenarios`, `n_gradable`, `pass_at_k`, and `pass_hat_k` computed over the gradable scenarios of that kind only. The shape is two named fields, not a map keyed by kind: the kind set is closed, and adding a third kind must be a loud schema decision, not a quiet new key. The existing top-level overall metrics stay exactly where they are — they are the historical yardstick — and render last.

WHEN a kind has `n_gradable = 0`, its rates render as `n/a` (the existing `rate()` convention) and serialize as `0.0`, matching the current overall behavior — no third representation.

#### Scenario: Per-kind numbers are independent
- **WHEN** a run grades scenarios of both kinds
- **THEN** the summary carries a regression pass^k computed over regression scenarios only, a capability pass^k over capability scenarios only, and the unchanged overall at top level

#### Scenario: Run summary prints three lines
- **WHEN** a run finishes
- **THEN** the human summary prints regression, capability, and overall lines, in that order

#### Scenario: Empty kind renders n/a
- **WHEN** a filtered run grades only regression scenarios
- **THEN** the capability line renders its rates as `n/a` and the JSON records `0.0`

### Requirement: JSON scenario order is unchanged by kind
The `scenarios` array in report.json SHALL keep discovery order. Grouping by kind happens at render time only.

#### Scenario: Grouping is render-only
- **WHEN** a report is serialized
- **THEN** the scenarios array order is identical to a pre-kind run's order over the same suite
