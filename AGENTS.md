# AGENTS.md — working on / with this harness

This repo evaluates zerostack agent behaviour. The CLI is the whole interface:
every subcommand takes `--json`, and exit codes are the contract
(0 = pass / no regression, 1 = fail / regression, 2 = harness error).

## Iterating on a prompt

1. Baseline (a target picks the provider + model; the key is an env var):
   `zseval run scenarios/prompts --target targets/anthropic.toml --tag baseline --trials 3 --json`
2. Edit the prompt in the zerostack checkout (`data/prompts/<name>.md`, or
   `src/agent/prompt.rs` for the base system prompt), then rebuild
   `cargo build --release --all-features`.
3. `zseval run scenarios/prompts --target targets/anthropic.toml --tag attempt-N --trials 3 --json`
4. `zseval compare results/baseline/report.json results/attempt-N/report.json`
   - exit 0 and the target scenario went up → done; hand the diff to a human.
   - exit 1 → `zseval explain results/attempt-N/<scenario>/trial-0/`, read the
     failed asserts and transcript, go back to step 2.
   - exit 2 → harness error, not a prompt problem: every shared scenario was
     ungradable on one side (broken build, bad target, expired key). Fix the
     environment before drawing any conclusion from the numbers.
   - lots of indeterminate (but not all) → fix the environment/schema first,
     not the prompt. Indeterminate scenarios are excluded from the pass rates
     and are never counted as regressions.

Tightened or fixed an assert on a scenario you already have trial artifacts
for? `zseval regrade <scenario-dir> <trial-dir> --no-judge` re-scores the
existing artifacts against the new assert without spending another API
call — useful for checking an assert edit is well-formed before the next
full run. `regrade` also takes `--judge <file>`: re-scoring frozen evidence
with a different judge is the cheap way to see how far apart two rulers are
on trials you have already human-labelled. A regraded `trial.json` names the
judge file that produced it (its report still names the run's own judge), and
the new judge's request/response land in a `regrade-<timestamp>/` subdirectory
so the previous judge's response survives as evidence.

## Guardrails

If you are the party whose prompt is being iterated, do not edit the things
that define or grade the test — that is measuring yourself with a ruler you
moved:

- scenarios (including their seed fixtures) and committed baselines,
- `judge.rs` (the referee), the judge files under `judges/`, and the assert
  implementations.

Change what the eval measures in a separate, human-reviewed change from the
prompt edit whose score it moves.

`--judge judges/<file>.toml` picks which model grades the subjective layer.
That it now takes one flag makes moving the ruler *easier*, not cheaper: a
judge swap must be paired with re-checking a batch against human labels, or a
pass-rate diff across the swap is unreadable — you cannot tell the ruler moving
from the agent's behavior changing. Never swap the judge in the same change as
the prompt whose score it moves. `--judge` may be given at most once per run
(unlike `--target`): the premise of a matrix is that everything but the target
is fixed, so the ruler must not vary with the column. Every report records the
judge file it was configured with (`judge_file`, plus `judge_hash` of its bytes
— a path is not an identity) and the models that actually graded (`judge_model`,
read back from the judge's own responses: `[]` when nothing was graded, absent
when unknown, never the configured model echoed back) — see `judges/README.md`.

## Budget

- Pass `--max-total-usd <cap>` on every run. A scenario runs its full trial
  count or not at all, so the cap never produces a misleading partial pass^k.
- **Know the cap's blind spot**: in headless `-p` mode zerostack does not
  report provider token usage back (session JSON carries only its own
  word-count estimate — see `transcript.rs`), so a trial's `cost_usd`, the
  report's `total_cost_usd`, and therefore `--max-total-usd` only see the
  *judge's* spend. The agent-side API cost is invisible to the harness and
  only shows on the provider dashboard. Observed 2026-07-07: a full
  35-scenario × 3-trial run cost ≈ $3–4 on claude-sonnet-4-6 while
  `report.json` reported $0.05. Budget accordingly — the cap is a judge-spend
  guard, not a total-spend guard, until zerostack persists real usage into
  its session files (it already has the numbers in the TUI path's
  `AgentEvent`; they just never reach headless session JSON).
- Because the cap effectively only sees judge spend, `--judge` moves it
  directly: the judge file's `price_*_usd_per_mtok` are what a judge call is
  costed at, so grading with `judges/opus.toml` instead of the default
  roughly doubles the only cost the cap can actually see.
- Most of that invisible spend is input tokens, dominated by the fixed
  request prefix (tool definitions + system prompt) re-sent on every agent
  turn. Prompt caching covers much of it (zerostack enables it —
  `provider.rs` `.with_prompt_caching()`), and `--jobs` warms the cache with
  a solo first trial before fanning out, so parallel trials read the prefix
  instead of each re-writing it.
- If one scenario keeps failing after ~5 attempts, stop and report the failing
  assert and your hypothesis rather than burning more budget.

## Reading results

- The failed assert names the behaviour that broke — fix that, don't rewrite
  the whole prompt.
- If scores stall, suspect the eval first (read the transcript), the prompt
  second.
- `pass^k` is the stability floor; a change that lifts `pass@k` but not
  `pass^k` is not yet an improvement.
- The evidence channel is the session JSON zerostack writes: its `tool`
  records are what tool asserts grade, and its `prompt` record is what
  `prompt_source` and `prompt_recorded` read. `turn-N.stdout` is a debugging
  artifact, graded by nothing. `ZS_BIN` therefore has a floor: an
  `--all-features` build from a mainline carrying PR #230 (tool records) and
  PR #228 (the prompt record). Older binaries grade tool scenarios
  *indeterminate* and report `prompt_source: "unknown"` with a warning naming
  the rebuild, rather than silently reporting an agent that used no tools.
- `zseval compare` also warns when tool-call evidence drops to zero on the
  candidate (`⚠ tool-call evidence dropped to zero`). A `tool_not_called`-only
  scenario passes vacuously if the evidence channel itself breaks — that's
  exactly how headless mode's missing tool-call recording went unnoticed
  (see `transcript.rs`'s module doc). This warning doesn't flip the exit
  code (it's not a behavior regression), but treat it as seriously as one:
  the pass rate above it may be meaningless.
- `zseval compare` also warns when a shared scenario's own definition changed
  (`⚠ scenario definition changed`) or when baseline and candidate were
  evaluated against different providers/models (`⚠ comparing different
  targets`) — see the README's "What a report identifies" section. Neither
  flips the exit code by itself, but both mean the pass-rate diff above may
  not be the apples-to-apples comparison it looks like.
- The default `--threshold 0.05` assumes finer resolution than a low trial
  count can actually produce: with 3 trials, pass rate only moves in steps of
  1/3 ≈ 0.333, so *any* single trial flipping outcome already reads as a
  regression, regardless of the threshold's nominal value. `compare` flags
  this (`⚠ threshold is finer than these scenarios' trial count can
  resolve`) rather than let a 5%-looking gate quietly behave like a
  zero-tolerance one. Before turning on a CI gate, either raise `--trials` or
  raise `--threshold` to actually fit — don't just ignore the warning.

## Subsystems beyond prompts

Memory evals (`scenarios/memory/`) are live, backed by the
`crates/zseval/src/domains/memory.rs` module — see the README's "Evaluating
another subsystem" section for how that quarantine works and why a stale
snapshot of zerostack's internals grades Indeterminate instead of Fail.
Subagent evals (`scenarios/subagents/`) are also live, backed by
`crates/zseval/src/domains/subagents.rs` — a leaner variant of the same
pattern, since the `task` tool has no seeding surface and (as of this
writing) no reliable drift-check evidence in zerostack's own logs.
MCP evals (`scenarios/mcp/`) are also live, backed by
`crates/zseval/src/domains/mcp.rs`, unblocked once zerostack v1.6.2 started
connecting configured MCP servers in headless mode (upstream R2). A
dependency-free `python3` mock stdio server stands in for a real MCP
server; seeding rewrites the run's `config.toml` in place (an
`[mcp_servers.<name>]` table plus forcing off zerostack's
default-enabled "Exa Web Search" server, which otherwise silently adds a
second, real, network-backed tool to every scenario — see the module's
own doc for the trace evidence).
