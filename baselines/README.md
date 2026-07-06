# Baselines

Committed verdict reports that CI compares against.

Create/refresh one from a trusted state:

    zseval run scenarios --target targets/anthropic.toml --tag main --trials 3 --json
    cp results/main/report.json baselines/main.json

Update baselines/main.json in the SAME pull request as the change that
moves the numbers, so reviewers see code diff and score diff together.
