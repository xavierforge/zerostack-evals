# AGENTS.md — working on / with this harness

This repo evaluates zerostack agent behaviour. The CLI is the whole interface:
every subcommand takes `--json`, and exit codes are the contract
(0 = pass / no regression, 1 = fail / regression, 2 = harness error).

## Iterating on a prompt

1. Baseline (a target picks the provider + model; the key is an env var):
   `zseval run scenarios/prompts --target targets/anthropic.toml --tag baseline --trials 3 --json`
2. Edit the prompt in the zerostack checkout (`data/prompts/<name>.md`, or
   `src/agent/prompt.rs` for the base system prompt), then rebuild
   `cargo build --release`.
3. `zseval run scenarios/prompts --target targets/anthropic.toml --tag attempt-N --trials 3 --json`
4. `zseval compare results/baseline/report.json results/attempt-N/report.json`
   - exit 0 and the target scenario went up → done; hand the diff to a human.
   - exit 1 → `zseval explain results/attempt-N/<scenario>/trial-0/`, read the
     failed asserts and transcript, go back to step 2.
   - lots of indeterminate → fix the environment/schema first, not the prompt.
     Indeterminate scenarios are excluded from the pass rates and are never
     counted as regressions.

## Guardrails

If you are the party whose prompt is being iterated, do not edit the things
that define or grade the test — that is measuring yourself with a ruler you
moved:

- scenarios (including their seed fixtures) and committed baselines,
- `judge.rs` (the referee) and the assert implementations.

Change what the eval measures in a separate, human-reviewed change from the
prompt edit whose score it moves.

## Budget

- Pass `--max-total-usd <cap>` on every run. A scenario runs its full trial
  count or not at all, so the cap never produces a misleading partial pass^k.
- If one scenario keeps failing after ~5 attempts, stop and report the failing
  assert and your hypothesis rather than burning more budget.

## Reading results

- The failed assert names the behaviour that broke — fix that, don't rewrite
  the whole prompt.
- If scores stall, suspect the eval first (read the transcript), the prompt
  second.
- `pass^k` is the stability floor; a change that lifts `pass@k` but not
  `pass^k` is not yet an improvement.
- `zseval compare` also warns when tool-call evidence drops to zero on the
  candidate (`⚠ tool-call evidence dropped to zero`). A `tool_not_called`-only
  scenario passes vacuously if the evidence channel itself breaks — that's
  exactly how headless mode's missing tool-call recording went unnoticed
  (see `transcript.rs`'s module doc). This warning doesn't flip the exit
  code (it's not a behavior regression), but treat it as seriously as one:
  the pass rate above it may be meaningless.

## Subsystems beyond prompts

Memory evals (`scenarios/memory/`) are live, backed by the
`crates/zseval/src/domains/memory.rs` module — see the README's "Evaluating
another subsystem" section for how that quarantine works and why a stale
snapshot of zerostack's internals grades Indeterminate instead of Fail.
Subagent evals are future work, following the same one-module pattern.
