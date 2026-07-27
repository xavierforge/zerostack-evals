# Baselines

Committed verdict reports that CI compares against.

Create/refresh one from a trusted state:

    zseval run scenarios --target targets/anthropic.toml --tag main --trials 3 --json
    cp results/main/report.json baselines/main.json

Update baselines/main.json in the SAME pull request as the change that
moves the numbers, so reviewers see code diff and score diff together.

There is no `baselines/main.json` right now. The one committed on 2026-07-21
(41 scenarios x 3 trials, pass@k 0.878, pass^k 0.732, $4.37) measured
zerostack before this project's fixes, and report-family JSON now loads
strictly (the read-strictness change removed the last `#[serde(default)]`
read-tolerance hatches), so that file would no longer parse anyway. It was
removed rather than kept around unreadable; the 07-21 numbers live in git
history only (`git log -p -- baselines/main.json`). Day 2 regenerates a fresh
baseline against zerostack v1.7.2 using the command above.
