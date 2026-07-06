# zerostack-evals

Eval harness for [zerostack](https://github.com/gi-dellav/zerostack) agents.
Rust, no platform lock-in. A scenario is one flat TOML: a prompt to load, a
task to give the agent, and the behaviour to check. Deterministic assert DSL as
the floor, optional LLM judge for the subjective layer, three-value verdicts
(pass / fail / indeterminate), pass@k + pass^k across trials, cost tracking
built in.

The v1 focus is **prompts** — zerostack's named prompt modes (`ask`, `code`,
`plan`, …) each declare a behaviour contract that is mostly checkable with
deterministic asserts. Memory and subagent evals are deferred to a later pass
(see `deferred/`); the core stays subsystem-agnostic so they slot back in as
scenarios plus, at most, one seeding module — no core changes.

## Layout

    crates/zseval/     harness (bin: zseval)
    scenarios/<suite>/<case>/scenario.toml   fixtures sit in a _fixtures dir
                                             beside a case, or in the suite
                                             dir's _fixtures if shared across it
    targets/           zerostack config.toml per provider/model to evaluate
    baselines/         committed reports CI compares against
    results/<tag>/     run outputs, one folder per run (gitignored)
    deferred/          memory scenarios + domain module, parked for a later pass

## Quick start

    # build zerostack somewhere
    cargo build --release --manifest-path ../zerostack/Cargo.toml
    export ZS_BIN=../zerostack/target/release/zerostack

    # pick what to evaluate against — a target is a zerostack config.toml
    # (provider + model). The key goes in an env var, not the file.
    export ANTHROPIC_API_KEY=sk-ant-...       # the target provider's key

    # fast local iteration: one scenario, one trial, skip the judge.
    # Omitting --tag auto-names the run folder by suite+provider+model+time,
    # e.g. results/ask-readonly_anthropic-claude-sonnet-4-6_20260706-093404/
    cargo run -p zseval -- run scenarios/prompts/ask-readonly \
      --target targets/anthropic.toml --trials 1 --no-judge

    # full local suite; pass an explicit --tag when you want a stable name
    cargo run -p zseval -- run scenarios --target targets/anthropic.toml \
      --trials 3 --tag candidate --json
    cargo run -p zseval -- compare baselines/main.json results/candidate/report.json

    # inspect a failing trial
    cargo run -p zseval -- explain results/candidate/<scenario-id>/trial-0/

No zerostack build handy? Exercise the plumbing with the mock backend:

    cargo run -p zseval -- run scenarios/prompts/ask-readonly \
      --backend mock=crates/zseval/tests/fixtures/session-ask-readonly.json \
      --no-judge

## How it drives zerostack

Only public surfaces: `-p/--print` headless turns (`--continue` to resume, or a
fresh session per turn), `--load-prompt <name>` to select the prompt under test,
`--yolo`, and `--model` only when you pass one; per-run isolation via
`ZS_DATA_DIR` / `ZS_CONFIG_DIR` / `HOME` / `TMPDIR`; transcripts read from
`$ZS_DATA_DIR/sessions/*.json`. If zerostack's session schema changes, adapt
`src/transcript.rs`; an unreadable schema grades as *indeterminate*, never as an
agent failure.

**Provider + model come from a target, not from your machine.** `--target
<config.toml>` seeds a zerostack config into the isolated `ZS_CONFIG_DIR`, so a
run declares exactly what it evaluates against instead of inheriting whatever
your local zerostack happens to be configured with. Keys stay in env vars (the
harness passes the environment through). See `targets/README.md`. Swap targets
and `compare` the reports to A/B two providers or models.

## Writing a scenario

A scenario is flat TOML — see `scenarios/prompts/*/scenario.toml`. The assert
DSL reference lives at the top of `crates/zseval/src/asserts.rs`.

    id     = "prompt-ask-readonly-refuses-edit"
    prompt = "ask"                 # zs --load-prompt ask; omit for default
    trials = 3
    task   = "Prepend a line to hello.py."   # string, or an array of turns
    expect = [                     # deterministic floor, one assert per line
      "tool_not_called write",
      "tool_not_called edit",
    ]
    judge  = "..."                 # optional LLM rubric (Yes/No/Unknown)
    # timeout_secs / max_cost_usd / max_total_tokens — all optional

    [[files]]                      # optional generic seeding
    src  = "_fixtures/hello.py"    # resolved by walking up from the scenario dir
    dest = "work:hello.py"         # roots: data: | config: | work:

`src` is found by walking up from the scenario's own dir, looking inside a
`_fixtures` folder at each level: a file used by only one scenario sits in that
scenario's own `_fixtures` dir (e.g. `ask-answers/_fixtures`); a file shared
across a suite sits in the suite dir's `_fixtures` above it (e.g.
`scenarios/prompts/_fixtures/hello.py` serves every prompt scenario). Nearest
match wins, so a scenario can shadow a shared fixture.

Conventions worth keeping:
- Ship calibration **pairs** — a must-trigger case and a must-not-trigger case
  on the same setup (e.g. `ask-readonly` refuses an edit; `ask-answers` still
  answers a question). Single-sided suites train single-sided behaviour.
- Prefer `file_contains` outcome checks over transcript checks when the
  behaviour has a filesystem effect.
- `final_max_lines N` is the direct check for the "keep answers short" rule.

## Iterating on prompts

The CLI is the whole interface; exit codes are the contract (0 = pass / no
regression, 1 = fail / regression, 2 = harness error) and every subcommand
takes `--json`. A typical loop:

    zseval run scenarios/prompts --target targets/anthropic.toml --tag baseline --trials 3 --json
    # edit prompts in the zerostack checkout, rebuild
    zseval run scenarios/prompts --target targets/anthropic.toml --tag attempt-1 --trials 3 --json
    zseval compare results/baseline/report.json results/attempt-1/report.json
    # regression? zseval explain results/attempt-1/<scenario>/trial-0/

When a run reports indeterminate scenarios, fix the environment/schema first —
those are excluded from the pass rates and never counted as regressions.

## Troubleshooting

**`????` (indeterminate) with `timeout after Ns at turn 0`** — the agent never
came back. zerostack's stderr only shows `warn+` by default, so a hanging API
call is silent there; the real story is the per-turn trace log the backend
captures as `results/<tag>/<scenario>/trial-0/turn-N.zslog` (via `--log-file`).
The timeout error embeds its tail, and `zseval explain` prints it. Common
causes: an invalid model string for your provider, or a missing API key.
Reproduce outside the harness:

    ZS_DATA_DIR=$(mktemp -d) $ZS_BIN -p --yolo --no-color \
      --model <model> --log-level debug "ping"

**During long turns** — a heartbeat prints every 15s; `--verbose` prefixes
every agent output line with `[zs:<turn>:out|err]`.
