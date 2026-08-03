# Troubleshooting

**Report says a run cost cents; the provider dashboard says dollars**: both are
right. Headless zerostack doesn't report provider token usage back, so
`report.json`'s `cost_usd`/`total_cost_usd` (and the `--max-total-usd` cap)
only capture judge calls; the agent's own API spend is invisible to the
harness. See AGENTS.md's Budget section for the details and magnitude.

**`????` (indeterminate) with `timeout after Ns at turn 0`**: the agent never
came back. zerostack's stderr only shows `warn+` by default, so a hanging API
call is silent there; the real story is the per-turn trace log the backend
captures as `results/<tag>/<scenario>/trial-0/turn-N.zslog` (via `--log-file`).
The timeout error embeds its tail, and `zseval explain` prints it. Common
causes: an invalid model string for your provider, or a missing API key.
Reproduce outside the harness:

    ZS_DATA_DIR=$(mktemp -d) $ZS_BIN -p --yolo --no-color \
      --log-level debug "ping"

**During long turns**: a heartbeat prints every 15s; `--verbose` prefixes every
agent output line with `[zs:<turn>:out|err]`.

**A run reports indeterminate scenarios**: fix the environment or schema first.
Those are excluded from the pass rates and never counted as regressions, so the
rates above them cover fewer scenarios than they look like they do. The README's
"Core concepts" defines what the verdict means; the usual causes are a binary
missing a feature the suite needs (`--all-features`), a session schema the
harness can no longer read, or a domain module whose layout snapshot went stale
(`scenarios/README.md`).

**A run exited 2 complaining about input-integrity drift**: something changed
this run's own inputs (the scenario tree, a target, the judge file, the prompt
pack) while it was running — most often an agent under `--yolo` writing outside
its trial dir. The affected scenario's trials were withdrawn to indeterminate
and the run stopped; the report for everything before it was still written.
Restore the listed paths (`git status` in the harness checkout) before trusting
anything the run measured. See the README's "Escape containment".
