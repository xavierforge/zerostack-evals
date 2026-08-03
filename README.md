# zerostack-evals

**Live baselines: https://xavierforge.dev/zerostack-evals/**
zerostack 1.7.2 (build `d8bbfe5d`), 43 scenarios x 3 trials, judge sonnet:
`claude-sonnet-5` at pass@k 0.861 / pass^k 0.744, and zerostack's own default
model `deepseek-v4-pro` at pass@k 0.837 / pass^k 0.651.

Eval harness for [zerostack](https://github.com/gi-dellav/zerostack) agents.
Rust, no platform lock-in. A scenario is one flat TOML: a prompt to load, a
task to give the agent, and the behaviour to check. Deterministic assert DSL as
the floor, optional LLM judge for the subjective layer, three-value verdicts,
pass@k + pass^k across trials, cost tracking built in. This file is the hub —
what a run needs, what the vocabulary means, where things live, and the
behaviour other tools rely on; the depth lives in each directory's own README.

## Quick start

A run stands on four legs. Three cost nothing to check, so `zseval run` checks
them together before anything is spent (see "The preflight gate").

**1. A zerostack binary, built with `--all-features`.**

    cargo build --release --all-features --manifest-path ../zerostack/Cargo.toml
    export ZS_BIN=../zerostack/target/release/zerostack     # or --zs-bin <path>

`--all-features` is load-bearing, not tidiness: `memory` is *not* one of
zerostack's default features, so a plain `cargo build` yields a binary whose
memory tools never register, and those scenarios then grade **indeterminate**
instead of failing — the suite looks thinner for a reason that has nothing to
do with the agent. The build must also be recent enough to record its own
evidence (see "How it drives zerostack"), and must be named by path: a bare
command resolved through `$PATH` cannot be hashed for the report's identity.

**2. A target, plus that provider's key in the environment.**

    export ANTHROPIC_API_KEY=sk-ant-...

A target is a zerostack `config.toml` naming the provider and model to evaluate
against; `--target targets/anthropic.toml` seeds it into the run's isolated
config dir, so a run declares what it measured rather than inheriting your
machine's config. **The key never goes in the target file**: targets are
committed, so a key inside one is a secret in git — and zerostack would accept
it, which is the trap, so preflight refuses a target whose `[api_keys]` table
holds a value. See `targets/README.md`.

**3. A judge decision, for any suite with judge-graded scenarios.**
`--judge <file>` names the ruler, `--no-judge` grades the deterministic asserts
only; neither flag is a usage error before any trial runs. zseval never picks
one for you, because which ruler grades a batch is what its scores mean. Any
sufficiently capable, cheap model does the job; the committed
`judges/sonnet.toml` is what the published baselines used. A suite with no
judge-graded scenario needs neither flag. See `judges/README.md`.

**4. Nothing else.**

`zseval list` is the command to reach for first: it prints the suite and, when
the suite has judge-graded scenarios, how many — the decision leg 3 is about.

    cargo run -p zseval -- list scenarios
    # 43 scenario(s)
    # 21 judge-graded — running this suite needs --judge <file> or --no-judge

No zerostack build handy? The mock backend needs none of the four legs and
grades off the same session evidence a live run does:

    cargo run -p zseval -- run scenarios/prompts/ask-readonly \
      --backend mock=crates/zseval/tests/fixtures/session-ask-readonly.json \
      --no-judge

With the four legs in place — one scenario for a fast loop, then the suite,
then the diff. `--jobs N` runs up to N trials of the *same* scenario
concurrently (each is already isolated, so this is wall-clock only, and trial 0
runs solo first to warm the prompt cache); without `--tag`, the run folder is
auto-named by suite + provider/model + time.

    cargo run -p zseval -- run scenarios/prompts/ask-readonly \
      --target targets/anthropic.toml --trials 1 --no-judge
    cargo run -p zseval -- run scenarios --target targets/anthropic.toml \
      --trials 3 --jobs 3 --tag candidate --judge judges/sonnet.toml --json
    cargo run -p zseval -- compare baselines/main.json results/candidate/report.json
    cargo run -p zseval -- explain results/candidate/<scenario-id>/trial-0/

## Core concepts

**Scenario** — one flat `scenario.toml`: the prompt to load, the task to give
the agent, and the asserts (plus an optional judge rubric) its behaviour is
graded against. **Suite** — any directory of them; `zseval run <path>` walks
the tree, so `scenarios/prompts` and `scenarios` are both suites
(`scenarios/README.md`). **Target** — a committed zerostack `config.toml`
naming the provider and model under evaluation (`targets/README.md`).
**Judge** and **judge card** — the LLM that grades the subjective layer of a
scenario carrying a `judge = "..."` rubric, and the committed TOML file naming
it: inert data, four required fields, unable to name a network destination or
an environment variable (`judges/README.md`).

**Kind: regression vs capability** — every scenario declares one, with no
default, because the field exists to make the author answer whether a low score
here is a *problem* or a *measurement*. `regression` is product-contract
behaviour and safety boundaries: stably green today, and a break should gate a
prompts PR. `capability` is a tracked number — recall quality, delegation
tendency, adversarial compliance with no enforcement behind it, probes of known
model weaknesses — where a low score is data, not a defect. Reports summarise
the two groups separately.

**The three-value verdict** — a trial is **pass** (every assert passed and the
judge, if any, said Yes), **fail** (a graded negative: an assert failed, the
judge said No, or a budget was exceeded), or **indeterminate** (nothing could
be graded: a backend error, session-schema drift, a domain module's layout
assumption no longer holding, the run's own inputs drifting underneath it).
Indeterminate is not a soft fail: it says the evidence or the environment
broke, so the trial is **excluded from the rate denominators** and never
counted as a regression, and a scenario whose every trial is indeterminate
drops out of the summary rather than scoring 0. That exclusion is its whole
difference from fail — a broken harness must never read as a worse agent.

**pass@k / pass^k** — over a scenario's k trials, pass@k is 1 if *any* graded
trial passed (the capability ceiling), pass^k is 1 if *all* graded trials
passed (the stability floor); both are averaged over gradable scenarios.
Capability scenarios are conventionally read on pass@k ("can it do this at
all"), regression scenarios on pass^k ("does it do this every time"); a change
that lifts pass@k but not pass^k is not yet an improvement.

**Transcript / session** — the evidence every assert grades: the session JSON
zerostack writes per headless turn, and the harness's only evidence channel.
**Baseline** — a committed `report.json` a candidate run is compared against
(`baselines/README.md`). **Prompt pack** — a directory of prompt files seeded
into every trial, so a prompt change is evaluated without rebuilding zerostack
(`--prompts`, `examples/prompt-pack/README.md`). **Domain** — a quarantined
module holding one zerostack subsystem's layout knowledge (`memory`,
`subagents`, `mcp`), so the core stays subsystem-agnostic
(`scenarios/README.md`).

## Directory map

    crates/zseval/  the harness (bin: zseval); subsystem knowledge quarantined
                    under src/domains/
    scenarios/      the suites, plus coverage.toml, the ledger of what this
                    suite does and does not measure — scenarios/README.md
    targets/        what a run evaluates against — targets/README.md
    judges/         who grades the subjective layer — judges/README.md
    baselines/      committed reports; main.json is the regression gate's
                    comparison point — baselines/README.md
    experiments/    committed, dated matrix snapshots — experiments/README.md
    results/        run outputs, one folder per run
    examples/       runnable examples — examples/prompt-pack/README.md
    docs/           published baseline pages, the ADRs, and the deep-dives

Three trees hold output, under three different policies: **`results/`** is
gitignored and ephemeral (raw transcripts and per-trial sandboxes, safe to
delete at any time); **`experiments/`** is committed and immutable (dated
records, appended to, never regenerated or edited to match a later run);
**`baselines/`** is committed and refreshable (the current comparison point,
refreshed in the same pull request as the change that moves its numbers).

`openspec/` is the owner's personal spec-driven workflow. Contributors are
welcome to read it, and are not expected to write specs.

## Behavior contracts

### The CLI surface

The CLI is the whole interface. Exit codes are the machine contract — **0**
pass / no regression, **1** fail / regression, **2** usage or harness error —
and every subcommand whose output machines consume takes `--json`: `run`,
`compare`, `regrade`, `matrix`, `site`. `explain` and `list` are human-only
inspection aids. `matrix` and `site` are views rather than gates, so neither
has an exit 1: a low pass rate is something they display, never something they
judge. A run or compare that is fully ungradable exits 2, the same code as a
usage mistake — a broken environment never looks like a clean pass.

### How it drives zerostack

Only public surfaces: `-p/--print` headless turns (`--continue` to resume),
`--load-prompt <name>`, `--yolo`, per-run isolation via `ZS_DATA_DIR` /
`ZS_CONFIG_DIR` / `HOME` / `TMPDIR`, transcripts read from
`$ZS_DATA_DIR/sessions/*.json`. **The session JSON is the evidence channel, and
the only one**: a headless turn persists structured `tool` records and a
`prompt` record naming the file it loaded, tool asserts grade the former,
`prompt_source` reads the latter, and `turn-N.stdout` is a debugging artifact
graded by nothing. Hence `ZS_BIN`'s floor (leg 1) — against an older binary a
tool call with no record makes the transcript unreadable and the scenario
grades indeterminate, rather than degrading into "the agent called no tools".
Detail, mock replay and what loop mode gives up:
`docs/evidence-and-reports.md`.

### Escape containment

Every trial drives a real zerostack under `--yolo`, so the harness assumes the
sandbox under it can fail open and closes two doors itself:

- **A git ceiling per trial.** Every invocation carries
  `GIT_CEILING_DIRECTORIES` set to the trial's own run dir, so a `git` the
  agent runs stops its upward walk there: a repo the scenario seeded inside the
  trial is still found, the harness's own checkout above it is not. An agent
  running `git commit` under `--yolo` otherwise finds this repo, as one has.
- **The run watches its own inputs.** The scenario tree as you named it, every
  `--target`, the judge file and the prompt pack are hashed before the first
  trial and re-hashed after each scenario. On any change that scenario's trials
  are **withdrawn to indeterminate** with the drift recorded in their
  `trial.json`, no further scenario is launched, the report for what did run is
  still written, and the process exits **2** — nothing was learned about the
  agent, the experiment was edited mid-flight.

### The preflight gate

Everything free is checked together, before the first thing that costs money.
The three free legs are collected rather than raised one at a time and reported
as one numbered list under "nothing ran and nothing was spent": a first-day
setup is usually missing more than one, and discovering them one failed run at
a time is the experience this gate deletes. A check that could not be *made* (a
binary too old to announce its feature set, a provider whose key requirement
zseval cannot read) warns and never blocks — missing information is not a
missing prerequisite. A key embedded in a target file's `[api_keys]` is refused
outright: the run would work, which is the trap, and the file is committed.
Only the judge's live dry-run probe costs anything, so it stays behind the
gate, and `--backend mock` skips the binary and key legs outright: it drives no
binary and calls no provider.

### What a report identifies

Every `report.json` says what produced it, so no number in it is read without
knowing what it measured: **what it evaluated** (`model`, `target`, and a
`content_hash` per scenario), **which zerostack** (`zs_version` verbatim,
`zs_bin_sha256` — the machine-comparable identity a same-version-but-rebuilt
binary cannot fake — and `zs_bin_path`, all captured at run start before any
spend), and **what graded it** (`judge_file` + `judge_hash` as configuration,
`judge_model` as execution, where `[]` and `null` are different answers).
Report-family JSON is read as strictly as it is written: every field must be
present or the whole report fails to load, naming what is missing. `compare`
reads those fields to flag any comparison it cannot fully vouch for, and every
such flag is a warning, never an exit-code escalation — this repo's first ADR
(`docs/adr/0001-compare-always-warns-matrix-owns-multivar.md`), with an
invariant test behind it. Field-by-field detail, the warning taxonomy and
`matrix`'s SPREAD / DRIFT / MULTI-VAR marks: `docs/evidence-and-reports.md`.

### Iterating on prompts

The loop itself (baseline, edit, re-run, `compare`, read the failed assert) is
in `AGENTS.md`; `run --prompts <dir>` evaluates your own prompt files without
rebuilding zerostack, and a pack against the built-ins is two tagged runs plus
`matrix` (`examples/prompt-pack/README.md`). Deciding between N targets is a
different question from gating a migration, so it gets a different command:
repeated `--target` on one `run`, or `zseval matrix` over reports you already
have, which spends nothing (`targets/README.md`). `compare` stays pairwise.

### Troubleshooting

A run reporting indeterminate scenarios is telling you to fix the environment
or the schema first: those trials are excluded from the pass rates, so the
numbers above them cover less than they look like they do. The recurring
symptoms — a report costing cents while the provider dashboard says dollars,
`????` with `timeout after Ns at turn 0`, where the per-turn trace logs live —
are in `docs/troubleshooting.md`.
