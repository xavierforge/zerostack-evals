# Baselines

Committed verdict reports that CI compares against. Committed and
*refreshable*, unlike `experiments/`, which is committed and immutable.

Create/refresh one from a trusted state:

    zseval run scenarios --target targets/anthropic.toml --tag main --trials 3 --json
    cp results/main/report.json baselines/main.json

Update baselines/main.json in the SAME pull request as the change that
moves the numbers, so reviewers see code diff and score diff together.

## The two shipped reports

Both were produced on 2026-08-03 against the same zerostack 1.7.2 (an
`--all-features` build of upstream 7b5581a, binary `d8bbfe5d`, carrying the
#228/#230 evidence channels), the same 43 scenarios x 3 trials, and the same
`judges/sonnet.toml` ruler.

- `main.json`: target anthropic/claude-sonnet-5, pass@k 0.861, pass^k 0.744.
  This is the report CI's regression gate compares against.
- `deepseek-v4-pro.json`: openrouter/deepseek/deepseek-v4-pro, zerostack's own
  default model — the "as shipped" comparison column, not a gate. pass@k 0.837,
  pass^k 0.651.

Only the target moved between them, which is what makes them a controlled pair:

    zseval matrix baselines/main.json baselines/deepseek-v4-pro.json --markdown

renders the two side by side with no DRIFT and no MULTI-VAR: same ruler, same
build, same scenario definitions, one variable.

The 2026-07-21 predecessor (41 x 3, pass@k 0.878) measured pre-fix zerostack
and predates strict report reading; its numbers live in git history only
(`git log -p -- baselines/main.json`).

## Publishing a baseline page

Both reports are rendered by `zseval site` and committed under `docs/`, which
GitHub Pages serves, so pushing a regenerated page publishes it:

    zseval site baselines/main.json --out docs/baseline-sonnet5.html
    zseval site baselines/deepseek-v4-pro.json \
      --out docs/baseline-deepseek-v4-pro.html

    https://xavierforge.dev/zerostack-evals/                            index
    https://xavierforge.dev/zerostack-evals/baseline-sonnet5.html
    https://xavierforge.dev/zerostack-evals/baseline-deepseek-v4-pro.html

`docs/index.html` is the one page `site` does not produce: a hand-written
landing page linking the two.

`zseval site <report.json> --out <file.html>` turns one report plus the
coverage ledger (`scenarios/coverage.toml`) into a single self-contained HTML
page, in three sections:

1. **Header**: the run's identity, read back field by field (zerostack version
   and binary hash, model, target, the judge configured and the ruler that
   actually answered, trials, cost). Identity leads, because no number under it
   means anything until you know what produced it.
2. **Results**: the scenario table `matrix` already renders, led by a summary
   table of the pass@k / pass^k / cost figures, with the per-scenario rows
   folded into a collapsed `details` element. The fold is native markup, so the
   page still carries no script. The footer-excluded disclosure, the column
   marks and the heuristics caveat stay outside the fold: a caveat a reader has
   to expand something to find is one the page has dropped.
3. **Coverage**: the ledger's every area and every claim, each claim's status
   colour-coded (covered green, uncovered orange, product-blocked red, excluded
   grey), no percentage anywhere, headlined by the count of areas no scenario
   touches at all. Coverage comes last as the denominator: how much of the
   surface the results above it actually touched.

Like `matrix` it makes no API call and writes nothing but the one file `--out`
names; unlike `matrix`, that file is the point, so `--out` is required. It runs
the ledger's drift check first and aborts before writing anything if the ledger
and the scenario tree disagree; a stale `audited_against` is shown on the page
instead, never fatal. `--ledger <path>` overrides the ledger path — a test
override for pointing at a fixture tree, not a general-purpose option.
