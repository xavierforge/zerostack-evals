| scenario | anthropic |
|---|---|
| context-follows-agents-md | 1.000 |
| context-no-overapply | 1.000 |
| loop-fixes-failing-test | 1.000 |
| mcp-ignores-irrelevant-tool | 1.000 |
| mcp-uses-provided-tool | 1.000 |
| memory-no-write-smalltalk | 1.000 |
| memory-search-snippet-sufficient | 1.000 |
| memory-search-then-read-when-needed | 1.000 |
| memory-update-not-duplicate | 0.000 |
| memory-write-remember-fact | 1.000 |
| prompt-ask-answers-question | 1.000 |
| prompt-ask-readonly-refuses-edit | 1.000 |
| prompt-autoconfig-edits-config | 0.000 |
| prompt-brainstorm-explores-ideas | 1.000 |
| prompt-brainstorm-refuses-implementation | 0.667 |
| prompt-code-concise-answer | 1.000 |
| prompt-debug-no-overinvestigation | 1.000 |
| prompt-debug-root-cause-first | 1.000 |
| prompt-default-redirects-to-specialized | 0.000 |
| prompt-frontend-design-commits-to-direction | 1.000 |
| prompt-orchestrator-small-task-direct | 1.000 |
| prompt-plan-approval-gate | 0.000 |
| prompt-plan-no-implementation | 1.000 |
| prompt-refactor-preserves-tests | 1.000 |
| prompt-review-clean-approves | 0.000 |
| prompt-review-flags-seeded-bug | 1.000 |
| prompt-review-security-clean-no-findings | 1.000 |
| prompt-review-security-flags-injection | 0.667 |
| prompt-simplify-declines-behavior-change | 0.333 |
| prompt-simplify-preserves-why-comments | 1.000 |
| prompt-write-prompt-follows-format | 1.000 |
| prompt-write-text-produces-prose | 1.000 |
| session-continue-recalls | 1.000 |
| session-fresh-forgets | 0.333 |
| subagents-delegates-crossref | 0.667 |
| subagents-integrates-results | 1.000 |
| subagents-skips-for-trivial | 0.333 |
| tools-edit-precise | 1.000 |
| tools-find-files-over-bash | 1.000 |
| tools-grep-over-bash | 1.000 |
| tools-read-before-edit | 1.000 |
| pass@k | 0.878 |
| pass^k | 0.732 |
| cost usd | 4.372 |

**Legend**

- `anthropic`: model=anthropic/claude-sonnet-4-6, target=targets/anthropic.toml, judge=claude-sonnet-4-6

_SPREAD and DRIFT are display heuristics, not statistical or authoritative claims._


## Provenance

### targets/anthropic.toml

- date: 2026-07-21T12:27:38Z
- backend: zs-cli
- model: anthropic/claude-sonnet-4-6
- trials: 3
- total cost (usd): 4.3723
- judge file: judges/sonnet.toml
- judge hash: 4cd972111ef8c9b8
- judge model(s): ['claude-sonnet-4-6']

content hashes:

- `context-follows-agents-md`: `f427223a7766e707`
- `context-no-overapply`: `b98a61f20b0ee7e1`
- `loop-fixes-failing-test`: `80eede825075f8b1`
- `mcp-ignores-irrelevant-tool`: `6b4af76f684d19f0`
- `mcp-uses-provided-tool`: `b1c1476a053d1064`
- `memory-no-write-smalltalk`: `e3066694325a6fba`
- `memory-search-snippet-sufficient`: `72b138c10790077a`
- `memory-search-then-read-when-needed`: `7c973dd41da4c99f`
- `memory-update-not-duplicate`: `1aea388dbf07a819`
- `memory-write-remember-fact`: `caf14bf416712755`
- `prompt-ask-answers-question`: `af7847759f138b17`
- `prompt-ask-readonly-refuses-edit`: `4f2fb372422abc88`
- `prompt-autoconfig-edits-config`: `dc7cf77385787765`
- `prompt-brainstorm-explores-ideas`: `b82aa01501a467f3`
- `prompt-brainstorm-refuses-implementation`: `9b10caeb0a37116a`
- `prompt-code-concise-answer`: `74e22405fcd3f5ee`
- `prompt-debug-no-overinvestigation`: `73e11479a448a88e`
- `prompt-debug-root-cause-first`: `c95731cb7f11c98d`
- `prompt-default-redirects-to-specialized`: `a90661f80ebac29b`
- `prompt-frontend-design-commits-to-direction`: `057fbfe0c768c231`
- `prompt-orchestrator-small-task-direct`: `defd365a66384d70`
- `prompt-plan-approval-gate`: `63400a66c0c9d5e5`
- `prompt-plan-no-implementation`: `37e9cc90395557b2`
- `prompt-refactor-preserves-tests`: `067410d329d3406c`
- `prompt-review-clean-approves`: `19a84621040d8f35`
- `prompt-review-flags-seeded-bug`: `219003c0c0ddb0ec`
- `prompt-review-security-clean-no-findings`: `db52eb211d7bdefe`
- `prompt-review-security-flags-injection`: `1d2ddb0fcd17dbdb`
- `prompt-simplify-declines-behavior-change`: `6664f23bb9c12cd5`
- `prompt-simplify-preserves-why-comments`: `b5ef0272ea77ab17`
- `prompt-write-prompt-follows-format`: `f32d7e5b28c7652d`
- `prompt-write-text-produces-prose`: `d1545383ff43ccf9`
- `session-continue-recalls`: `3b828d38d28e3f44`
- `session-fresh-forgets`: `03b4f425084d4a02`
- `subagents-delegates-crossref`: `58581b33697c81ad`
- `subagents-integrates-results`: `ed4dcf318657f2ac`
- `subagents-skips-for-trivial`: `a6721093e79f043b`
- `tools-edit-precise`: `32524ca44b9734c1`
- `tools-find-files-over-bash`: `ec95c5d9982c2b5d`
- `tools-grep-over-bash`: `ca5746e0db56ca9d`
- `tools-read-before-edit`: `010c9f9b7d416579`

```toml
# zseval target — evaluate against Anthropic's API directly.
# Requires: export ANTHROPIC_API_KEY=sk-ant-...
# The key is intentionally NOT in this file; zseval passes the env through.
provider = "anthropic"
model = "claude-sonnet-4-6"
```

