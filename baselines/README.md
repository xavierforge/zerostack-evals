# Baselines

Committed verdict reports that CI compares against.

Create/refresh one from a trusted state:

    zseval run scenarios --target targets/anthropic.toml --tag main --trials 3 --json
    cp results/main/report.json baselines/main.json

Update baselines/main.json in the SAME pull request as the change that
moves the numbers, so reviewers see code diff and score diff together.

`main.json` (2026-08-03): zerostack 1.7.2 at upstream 7b5581a (all-features
build d8bbfe5d, carries the #228/#230 evidence channels), 43 scenarios x 3
trials, target anthropic/claude-sonnet-5, judge sonnet. pass@k 0.861,
pass^k 0.744. This is the report CI's regression gate compares against.

`deepseek-v4-pro.json` (same day, same build, same judge, same suite):
openrouter/deepseek/deepseek-v4-pro, zerostack's own default model — the
"as shipped" comparison column, not a gate. pass@k 0.837, pass^k 0.651.
`zseval matrix baselines/main.json baselines/deepseek-v4-pro.json` renders
the two side by side with zero DRIFT/MULTI-VAR: same ruler, same build,
only the target moved.

The 2026-07-21 predecessor (41 x 3, pass@k 0.878) measured pre-fix zerostack
and predates strict report reading; its numbers live in git history only
(`git log -p -- baselines/main.json`).
