# zerostack-evals

Eval harness for [zerostack](https://github.com/gi-dellav/zerostack) agents.
Rust, no platform lock-in. A scenario is one flat TOML: a prompt to load, a
task to give the agent, and the behaviour to check. Deterministic assert DSL as
the floor, optional LLM judge for the subjective layer, three-value verdicts
(pass / fail / indeterminate), pass@k + pass^k across trials, cost tracking
built in.

Coverage spans zerostack's named prompt modes (`ask`, `code`, `plan`, …),
mostly checkable with deterministic asserts; its `memory` subsystem
(`scenarios/memory/`); and its subagent delegation via the `task` tool
(`scenarios/subagents/`) — each subsystem's layout knowledge lives in its
own quarantined `domains::` module (see "Evaluating another subsystem"
below) — the core (scenario/backend/seed/asserts/verdict) stays
subsystem-agnostic.

A run or compare that comes back fully ungradable (every trial indeterminate,
or nothing shared was comparable) exits **2**, the same "harness error" code
as a usage mistake — a broken environment never looks like a clean pass.

## Layout

    crates/zseval/     harness (bin: zseval)
                          src/domains/  zerostack-subsystem-specific
                                        knowledge (e.g. memory file layout),
                                        one module per subsystem, quarantined
                                        from the generic core
    scenarios/<suite>/<case>/scenario.toml   fixtures sit in a _fixtures dir
                                             beside a case, or in the suite
                                             dir's _fixtures if shared across it
    targets/           zerostack config.toml per provider/model to evaluate
    baselines/         committed reports CI compares against
    results/<tag>/     run outputs, one folder per run (gitignored)

## Quick start

    # build zerostack somewhere
    cargo build --release --manifest-path ../zerostack/Cargo.toml
    export ZS_BIN=../zerostack/target/release/zerostack

    # pick what to evaluate against — a target is a zerostack config.toml
    # (provider + model). The key goes in an env var, not the file.
    export ANTHROPIC_API_KEY=sk-ant-...       # the target provider's key

    # fast local iteration: one scenario, one trial, skip the judge.
    # Omitting --tag auto-names the run folder by suite+provider+model+time,
    # e.g. results/ask-readonly_anthropic-claude-sonnet-4-6_20260706-093404-512-83921/
    cargo run -p zseval -- run scenarios/prompts/ask-readonly \
      --target targets/anthropic.toml --trials 1 --no-judge

    # full local suite; pass an explicit --tag when you want a stable name.
    # --jobs N runs up to N trials of the *same* scenario concurrently — each
    # trial is already isolated in its own run_dir, so this is pure
    # wall-clock win with no change to grading; scenarios still run one at a
    # time. Omit it (or pass --jobs 1) for the old strictly-sequential path.
    cargo run -p zseval -- run scenarios --target targets/anthropic.toml \
      --trials 3 --jobs 3 --tag candidate --json
    cargo run -p zseval -- compare baselines/main.json results/candidate/report.json

    # inspect a failing trial
    cargo run -p zseval -- explain results/candidate/<scenario-id>/trial-0/

No zerostack build handy? Exercise the plumbing with the mock backend:

    cargo run -p zseval -- run scenarios/prompts/ask-readonly \
      --backend mock=crates/zseval/tests/fixtures/session-ask-readonly.json \
      --no-judge

`--backend mock=<path>` also accepts a *directory* shaped like a captured
trial dir (`data/sessions/*.json` plus `turn-N.{stdout,stderr,zslog}`), which
replays the stdout-based tool-call markers too — the only channel that
carries tool calls in headless mode (see "How it drives zerostack" below).

Edited an assert and want to know if it would flip a past trial's verdict,
without spending another API call?

    cargo run -p zseval -- regrade scenarios/prompts/ask-readonly \
      results/candidate/prompt-ask-readonly-refuses-edit/trial-0/

`regrade` re-scores that trial dir's frozen artifacts against the
scenario's *current* asserts/judge and rewrites its `trial.json` — nothing
about the agent is re-run.

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

`file_contains`/`file_not_contains` paths are rooted at the run's throwaway
`ZS_DATA_DIR` by default; prefix with `config:` or `work:` to check the
isolated config dir or working dir instead, e.g.
`file_contains config:agent/memory/MEMORY.md tabs`.

## Evaluating another subsystem

A subsystem like `memory` lays out files zerostack itself decides the shape
of (e.g. `<config_dir>/agent/memory/MEMORY.md`) — that layout knowledge is a
*snapshot* of zerostack internals, not something the harness core should
know. It's quarantined to one file, `crates/zseval/src/domains/<name>.rs`.
The core never names a specific subsystem: `Scenario::load`, `seed::apply`,
and the runner call exactly three dispatch functions —
`domains::{validate, expand, verify}` (`crates/zseval/src/domains/mod.rs`) —
and those three functions are the *only* place "which domains exist" is
listed. Adding eval support for another subsystem is: one new `domains::`
module, one match arm in each of the three dispatch functions, plus one new
optional field on `SeedSugar` *if* the subsystem actually has something to
seed — zero changes to `scenario`/`seed`/`runner` otherwise.

Because the knowledge is a snapshot, every `domains::` module pairs its
layout knowledge with a runtime drift check `domains::verify` dispatches to
after driving the agent (see `domains::memory::verify`). If zerostack's
actual layout no longer matches what the module assumes, the trial grades
**Indeterminate** with a message naming the fix — never a silent Fail. This
is what makes memory evals resilient to zerostack iterating quickly: a
scenario doesn't quietly start "failing" just because an internal path
moved, it stops being gradable until someone updates the domain module.

`verify` normally runs because the scenario declared `[seed.memory]` sugar,
but a scenario that starts from an *empty* store (nothing to seed, only an
assertion that the agent wrote something new) has no sugar to trigger it.
For that case, opt in explicitly: `domains = ["memory"]` at the top level of
`scenario.toml` runs the same drift check with no seeding attached. An
unknown name in `domains = [...]` is a load-time error, same as any other
typo'd scenario field.

`scenarios/memory/` requires a zerostack build with the `memory` feature
(not in zerostack's `default` features):

    cargo build --release --features memory --manifest-path ../zerostack/Cargo.toml

Building without `--features memory` doesn't crash anything — the memory
tools simply never register, `memory_search`/`memory_read`/`memory_write`
never get called, and the `[seed.memory]` drift check reports "no 'memory
open:' trace line was found", pointing straight at the missing feature flag.

`domains::subagents` (`scenarios/subagents/`) is the same pattern applied to
a subsystem with nothing to seed and, as of this writing, no reliable
startup trace line either — zerostack's `task` tool (subagent delegation)
logs nothing at all, unlike memory's `Mem::open()`. Its `verify` is a
deliberate no-op rather than a drift check; scenarios opt in with
`domains = ["subagents"]` alone, and vacuous-pass protection comes entirely
from pairing `tool_called`/`tool_not_called task` with a positive assert in
the scenario itself. See the module's own doc for the full investigation.

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

## What a report identifies

Every `report.json` records what it evaluated against (`"model"`:
`"<provider>/<model>"`, resolved from `--target`'s config.toml plus any
`--model` override — `"provider-default"` when neither is known) and, per
scenario, a content hash of that scenario's `scenario.toml`. `zseval compare`
uses both:

- **Different targets** — comparing a baseline evaluated against one
  provider/model to a candidate evaluated against another prints a warning
  (still runs the diff; that's the A/B use case) so a regression check never
  silently becomes an apples-to-oranges comparison.
- **Changed scenario definition** — if a shared scenario's `scenario.toml`
  differs between baseline and candidate, `compare` warns instead of quietly
  diffing two different tests under the same id (see AGENTS.md's guardrail
  on not moving the ruler while measuring). A baseline committed before this
  field existed has an empty hash and is treated as "unknown", not a false
  positive.

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
