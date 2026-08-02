# zerostack-evals

Eval harness for [zerostack](https://github.com/gi-dellav/zerostack) agents.
Rust, no platform lock-in. A scenario is one flat TOML: a prompt to load, a
task to give the agent, and the behaviour to check. Deterministic assert DSL as
the floor, optional LLM judge for the subjective layer, three-value verdicts
(pass / fail / indeterminate), pass@k + pass^k across trials, cost tracking
built in.

Coverage spans zerostack's named prompt modes (`ask`, `code`, `plan`, …),
mostly checkable with deterministic asserts; its `memory` subsystem
(`scenarios/memory/`); its subagent delegation via the `task` tool
(`scenarios/subagents/`); and its MCP tool integration via a mock stdio
server (`scenarios/mcp/`): each subsystem's layout knowledge lives in its
own quarantined `domains::` module (see "Evaluating another subsystem"
below), and the core (scenario/backend/seed/asserts/verdict) stays
subsystem-agnostic.

A run or compare that comes back fully ungradable (every trial indeterminate,
or nothing shared was comparable) exits **2**, the same "harness error" code
as a usage mistake: a broken environment never looks like a clean pass.

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
    judges/            judge file per grading model (--judge); the ruler,
                       kept swappable, explicit, and recorded in the report
    baselines/         committed reports CI compares against
    results/<tag>/     run outputs, one folder per run (gitignored)

## Quick start

    # build zerostack somewhere. --all-features because several suites need
    # non-default features (memory, mcp), and the build has to be recent
    # enough to record its own evidence: see "How it drives zerostack".
    cargo build --release --all-features --manifest-path ../zerostack/Cargo.toml
    export ZS_BIN=../zerostack/target/release/zerostack

    # pick what to evaluate against: a target is a zerostack config.toml
    # (provider + model). The key goes in an env var, not the file.
    export ANTHROPIC_API_KEY=sk-ant-...       # the target provider's key

    # fast local iteration: one scenario, one trial, skip the judge.
    # Omitting --tag auto-names the run folder by suite+provider+model+time,
    # e.g. results/ask-readonly_anthropic-claude-sonnet-4-6_20260706-093404-512-83921/
    cargo run -p zseval -- run scenarios/prompts/ask-readonly \
      --target targets/anthropic.toml --trials 1 --no-judge

    # full local suite; pass an explicit --tag when you want a stable name.
    # --jobs N runs up to N trials of the *same* scenario concurrently: each
    # trial is already isolated in its own run_dir, so this is pure
    # wall-clock win with no change to grading; scenarios still run one at a
    # time. Trial 0 always runs solo first to warm the provider's prompt
    # cache (every trial of a scenario opens with an identical request; the
    # fan-out then hits cache reads instead of racing cold and each paying
    # the cache-write rate). Omit --jobs (or pass 1) for the old
    # strictly-sequential path.
    cargo run -p zseval -- run scenarios --target targets/anthropic.toml \
      --trials 3 --jobs 3 --tag candidate --judge judges/sonnet.toml --json
    cargo run -p zseval -- compare baselines/main.json results/candidate/report.json

    # inspect a failing trial
    cargo run -p zseval -- explain results/candidate/<scenario-id>/trial-0/

No zerostack build handy? Exercise the plumbing with the mock backend:

    cargo run -p zseval -- run scenarios/prompts/ask-readonly \
      --backend mock=crates/zseval/tests/fixtures/session-ask-readonly.json \
      --no-judge

`--backend mock=<path>` also accepts a *directory* shaped like a captured
trial dir (`data/sessions/*.json` plus `turn-N.{stdout,stderr,zslog}`),
replayed exactly as a real run produced it. Either form grades off the same
evidence a live run does, because tool calls and prompt identity are read
from the session JSON (see "How it drives zerostack" below); the turn logs a
directory fixture carries are along for the ride, not graded.

Edited an assert and want to know if it would flip a past trial's verdict,
without spending another API call?

    cargo run -p zseval -- regrade scenarios/prompts/ask-readonly \
      results/candidate/prompt-ask-readonly-refuses-edit/trial-0/ --no-judge

`regrade` re-scores that trial dir's frozen artifacts against the
scenario's *current* asserts/judge and rewrites its `trial.json`: nothing
about the agent is re-run. Adding `--judge <file>` re-scores with a different
ruler; the rewritten `trial.json` then names the judge file that produced it,
and the new judge's request/response go to a `regrade-<timestamp>/`
subdirectory rather than over the previous judge's (see `judges/README.md`).

## How it drives zerostack

Only public surfaces: `-p/--print` headless turns (`--continue` to resume, or
a fresh session per turn), `--load-prompt <name>` to select the prompt under
test, and `--yolo`; per-run isolation via `ZS_DATA_DIR` / `ZS_CONFIG_DIR` /
`HOME` / `TMPDIR`; transcripts read from
`$ZS_DATA_DIR/sessions/*.json`. If zerostack's session schema changes, adapt
`src/transcript.rs`; an unreadable schema grades as *indeterminate*, never as an
agent failure.

**The session JSON is the evidence channel, and the only one.** A headless
turn persists structured `tool` records for every call it made (subagent
calls included) and a `prompt: { name, source }` record naming the prompt
file it actually loaded and whether that file was built into the binary or
came from disk. Both are read straight out of the session: `tool_called` and
friends grade the records, and each scenario's reported `prompt_source`
starts from the recorded `source` rather than from an inference about what
the harness seeded. The `turn-N.stdout` capture (`--pure-stdout`, `◈ {name}
{summary}` marker lines) is still taken and still worth reading while
debugging, but nothing grades it: a `◈ ` sequence inside a tool's own output
is indistinguishable from a real marker line, so parsing it risks seeing
tool calls that never happened.

**So `ZS_BIN` has a floor.** It must be an `--all-features` build from a
zerostack mainline carrying PR #230 (tool records in headless session JSON)
and PR #228 (the prompt record). Against an older binary this is loud rather
than quiet: a tool-call message with no record makes the transcript
unreadable, so the scenario grades *indeterminate*, and a missing `prompt`
record makes the run warn and report `prompt_source: "unknown"` (and fails
any `prompt_recorded` assert). Neither degrades into "the agent called no
tools", which is the failure mode this design exists to prevent.

**Provider + model come from a target, not from your machine.** `--target
<config.toml>` seeds a zerostack config into the isolated `ZS_CONFIG_DIR`, so a
run declares exactly what it evaluates against instead of inheriting whatever
your local zerostack happens to be configured with. Keys stay in env vars (the
harness passes the environment through). See `targets/README.md`. `--target` is
repeatable: `zseval run scenarios --target a.toml --target b.toml ...`
evaluates the suite against every target sequentially, under one shared
`--max-total-usd`, and prints a scenario x target table to stderr when it's
done. `zseval matrix <report.json>...` renders that same table from
already-produced reports (no API calls, nothing written to disk), the way to
compose N targets, or a target against a committed baseline, without
re-spending. `compare` stays pairwise and keeps its migration-gate role
(deciding whether to switch from one target to another); reach for `matrix`
for a side-by-side view of N targets.

`zseval site <report.json> --out <file.html>` turns one report plus the
coverage ledger (`scenarios/coverage.toml`) into a single self-contained HTML
page: the run's identity, the ledger's coverage (every area, every claim, no
percentage), and the scenario table `matrix` already renders. Like `matrix`
it makes no API call and writes nothing but the one file `--out` names;
unlike `matrix`, that file is the point, so `--out` is required. It runs the
ledger's drift check first and aborts before writing anything if the ledger
and the scenario tree disagree; a stale `audited_against` is shown on the
page instead, never fatal. `--ledger <path>` overrides the ledger path — a
test override for pointing at a fixture tree, not a general-purpose option.

## Writing a scenario

A scenario is flat TOML (see `scenarios/prompts/*/scenario.toml`). The assert
DSL reference lives at the top of `crates/zseval/src/asserts.rs`.

    id     = "prompt-ask-readonly-refuses-edit"
    kind   = "regression"  # required: "regression" | "capability" — see scenario-kind spec
    prompt = "ask"                 # zs --load-prompt ask; omit for default
    trials = 3
    task   = "Prepend a line to hello.py."   # string, or an array of turns
    expect = [                     # deterministic floor, one assert per line
      "tool_not_called write",
      "tool_not_called edit",
    ]
    judge  = "..."                 # optional LLM rubric (Yes/No/Unknown)
    # timeout_secs / max_cost_usd / max_total_tokens (all optional)

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
- Ship calibration **pairs**: a must-trigger case and a must-not-trigger case
  on the same setup (e.g. `ask-readonly` refuses an edit; `ask-answers` still
  answers a question). Single-sided suites train single-sided behaviour.
- Prefer `file_contains` outcome checks over transcript checks when the
  behaviour has a filesystem effect.
- `final_max_lines N` is the direct check for the "keep answers short" rule.

`file_contains`/`file_not_contains`/`path_not_exists` paths are rooted at the
run's throwaway `ZS_DATA_DIR` by default; prefix with `config:` or `work:` to
check the isolated config dir or working dir instead, e.g.
`file_contains config:agent/memory/MEMORY.md tabs`. `file_not_contains` fails
if nothing matches its path (a missing file or a zero-hit glob is not
evidence the file is clean, it's evidence the file was never written); use
`path_not_exists <path>` when the check really is "nothing should be there at
all" — it passes only when zero files *and* directories match.

### `mode = "loop"` scenarios

    id     = "loop-fixes-failing-test"
    mode   = "loop"                # default is "print" (the -p/--continue path above)
    trials = 3
    task   = "test_calc.py is failing. Find the bug in calc.py and fix it."
    expect = [
      "file_not_contains work:calc.py return a - b",
      "transcript_contains ALL TESTS PASS",
    ]

    [loop]
    max_iterations = 3            # required (--loop is unbounded otherwise)
    run = "python3 test_calc.py"  # optional (--loop-run); output feeds the next iteration

Drives a single `zerostack --loop --loop-max N [--loop-run CMD] <task>`
invocation instead of the per-turn `-p`/`--continue` loop, so `task` must be
a single turn (no array). `loop` is in zerostack's default features: no
extra build flag needed.

Two things loop mode gives up, both enforced at load time so a scenario
can't silently ship a footgun:
- **No session file**: `run_headless_loop` never calls `save_session`.
  Grading evidence is `$ZS_DATA_DIR/loops/<uuid>/iter-NNNN.json` records
  (prompt/response/validation_output per iteration), which `transcript.rs`
  folds in as ordinary messages: `final_contains`/`transcript_contains`/
  `file_*` all work unchanged; `zseval explain` dumps them too. With no
  session there is no recorded prompt either, so a loop scenario's
  `prompt_source` stays the old derivation from what the harness seeded.
- **No tool-call evidence at all**: no session file means no `tool` records,
  and the iteration records carry only prompt/response text. `tool_called` /
  `tool_not_called` / `tool_called_after` / `tool_count` /
  `tool_arg_contains` / `no_tool_call_contains` / `tokens_under` are all
  rejected on a `mode = "loop"` scenario at load time: grade on
  `file_contains`/`transcript_contains`/`final_contains` instead.

## Evaluating another subsystem

A subsystem like `memory` lays out files zerostack itself decides the shape
of (e.g. `<config_dir>/agent/memory/MEMORY.md`): that layout knowledge is a
*snapshot* of zerostack internals, not something the harness core should
know. It's quarantined to one file, `crates/zseval/src/domains/<name>.rs`.
The core never names a specific subsystem: `Scenario::load`, `seed::apply`,
and the runner call exactly three dispatch functions:
`domains::{validate, expand, verify}` (`crates/zseval/src/domains/mod.rs`),
and those three functions are the *only* place "which domains exist" is
listed. Adding eval support for another subsystem is: one new `domains::`
module, one match arm in each of the three dispatch functions, plus one new
optional field on `SeedSugar` *if* the subsystem actually has something to
seed (zero changes to `scenario`/`seed`/`runner` otherwise).

Because the knowledge is a snapshot, every `domains::` module pairs its
layout knowledge with a runtime drift check `domains::verify` dispatches to
after driving the agent (see `domains::memory::verify`). If zerostack's
actual layout no longer matches what the module assumes, the trial grades
**Indeterminate** with a message naming the fix, never a silent Fail. This
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

Building without `--features memory` doesn't crash anything: the memory
tools simply never register, `memory_search`/`memory_read`/`memory_write`
never get called, and the `[seed.memory]` drift check reports "no 'memory
open:' trace line was found", pointing straight at the missing feature flag.

`domains::subagents` (`scenarios/subagents/`) is the same pattern applied to
a subsystem with nothing to seed and, as of this writing, no reliable
startup trace line either: zerostack's `task` tool (subagent delegation)
logs nothing at all, unlike memory's `Mem::open()`. Its `verify` is a
deliberate no-op rather than a drift check; scenarios opt in with
`domains = ["subagents"]` alone, and vacuous-pass protection comes entirely
from pairing `tool_called`/`tool_not_called task` with a positive assert in
the scenario itself. See the module's own doc for the full investigation.

`domains::mcp` (`scenarios/mcp/`) evaluates whether the agent uses an
MCP-provided tool when it should and leaves it alone when it shouldn't, via
a dependency-free `python3` stdio server fixture
(`scenarios/mcp/_fixtures/mock_mcp_server.py`, exposing one tool,
`lookup_ticket`). `[seed.mcp]` sugar rewrites the run's already-seeded
`config.toml` in place to add an `[mcp_servers.<name>]` table, the one
domain so far whose seeding isn't a file copy, since MCP server config is a
field inside `config.toml`, not a separate file. It also force-disables
zerostack's default-enabled "Exa Web Search" MCP server
(`enable-exa-mcp = false`), confirmed live to otherwise connect
unconditionally and add a second, real, network-backed tool alongside the
mock one (see the module's own doc for the exact trace evidence). `verify`
greps turn zslogs for `Connected to MCP server '{name}'` per seeded server,
the same drift-check shape as `memory::verify`. Requires zerostack's `mcp`
feature (in `default` features as of this writing).

## Iterating on prompts

The CLI is the whole interface; exit codes are the contract (0 = pass / no
regression, 1 = fail / regression, 2 = harness error) and every subcommand
takes `--json`.

Changing a prompt no longer means editing zerostack's own checkout and
rebuilding it. `run --prompts <dir>` seeds a directory of your own prompt
files into every trial's isolated `.zerostack/prompts/` (zerostack's own top
override layer), with no recompile. Comparing a pack against the built-ins is
two tagged runs plus `matrix`:

    zseval run examples/prompt-pack/scenario --target targets/anthropic.toml \
      --tag stock --results results/prompt-pack-example
    zseval run examples/prompt-pack/scenario --target targets/anthropic.toml \
      --prompts examples/prompt-pack/pack --tag my-pack \
      --results results/prompt-pack-example
    zseval matrix results/prompt-pack-example/stock/report.json \
      results/prompt-pack-example/my-pack/report.json --markdown

Use short, explicit tags (`--tag stock`, `--tag my-pack`) rather than the
auto-generated one: `matrix` labels same-target columns by tag, and the auto
tag (suite, provider/model, pack directory name, and timestamp, all
concatenated) is wide enough to break the table's fixed-width columns.
`compare` takes the same two reports too, and now warns when the two sides'
packs differ instead of quietly mixing a prompt change into a pass-rate diff:
"comparing different prompt packs ... a prompt A/B, not a regression check."

When a run reports indeterminate scenarios, fix the environment/schema first:
those are excluded from the pass rates and never counted as regressions.

### What a pack may contain

`--prompts <dir>` reads only the directory's top-level `*.md` files, taking
each file's stem as the prompt name it overrides (the same rule zerostack
itself applies). A subdirectory or a non-`.md` entry is a load-time error
naming it, before any trial spends money, and so is a directory that does not
exist or holds no `*.md` file. `--prompts` is single-arity (unlike `--target`,
which is repeatable) and rejected under `--backend mock`, which never
constructs a zerostack invocation for a pack to be loaded into.

A name your pack does not provide falls through to zerostack's built-in
prompt of that name, so a one-file pack overrides only that one prompt. A
pack that ships `code.md` reaches further than the scenarios that declare
`prompt = "code"`, though: a scenario that declares no prompt at all falls
back to the target's `default_prompt`, or to `code` when that's unset too, so
it changes as well. A scenario asserting `prompt_recorded <name> built_in` is
watching the very built-in such a pack replaces, so the run skips it instead
of grading it: it spends no trials, records the scenario as ungradable, and
says on stderr which prompt the pack shadowed. `report.json` records each
scenario's `prompt_source` (`pack` / `stock` / `scenario` / `unknown`), read
back from the session's own record of the prompt it loaded, so which prompt
actually applied is never a guess.

`examples/prompt-pack/` is a minimal pack plus a scenario that only passes
when the pack, not the built-in `code` prompt, is what the run loaded: copy
it as a starting point. It asserts that identity directly
(`prompt_recorded code user_file`), so nothing rides on the model repeating a
marker string back. Its own README covers running it against a real zerostack
build, what each way of failing means, and the one rule to keep when copying
it: a pack prompt must not be byte-identical to the built-in it overrides,
because zerostack classifies `built_in` by content and would record the pack's
own file as the built-in.

Deciding between N targets instead of gating a migration is a different
question, so it gets a different command. Give `run` repeated `--target` to
evaluate all of them in one invocation, under one shared `--max-total-usd`:

    zseval run scenarios/prompts --target targets/anthropic.toml \
      --target targets/openrouter.toml --tag matrix-1 --trials 3 \
      --judge judges/sonnet.toml
    # scenario x target table prints to stderr when the run finishes

or reuse existing reports (including a committed baseline) as columns, without
spending anything:

    zseval matrix results/matrix-1/anthropic/report.json \
      results/matrix-1/openrouter/report.json baselines/main.json --markdown

`matrix` is a pure renderer: no API calls, nothing written to disk. It marks
what it cannot vouch for: SPREAD when targets genuinely disagree on a
scenario, DRIFT when the judge or a scenario definition changed between
columns, and a column cut short by the budget cap as incomplete, rather than
presenting a diff across changed conditions as a clean comparison.

## What a report identifies

Every `report.json` records what it evaluated against (`"model"`:
`"<provider>/<model>"`, resolved from `--target`'s config.toml (mandatory for
`--backend zs`) or `"mock"` for `--backend mock`, which rejects `--target`;
also `"target"`, the target file's own path, so a report carries its column
identity even once copied away from its run directory), what graded it
(`"judge_file"`, `"judge_hash"` and `"judge_model"`, below), and, per scenario, a content hash
of that scenario's `scenario.toml`.

### Which zerostack produced it

Every report also names the zerostack build behind its numbers, so a result can
never be read without knowing what it measured:

- **`"zs_version"`** is the first line of `ZS_BIN --version`, recorded verbatim.
  It is evidence for a human, deliberately not parsed or format-checked: the
  moment the harness validated the banner's shape, that shape would become a
  compatibility contract with upstream. For `--backend mock` it is the fixed
  string `"mock"`.
- **`"zs_bin_sha256"`** is the SHA-256 of the binary's contents (for
  `--backend mock`, a content fingerprint of the fixture). This is the
  machine-comparable identity: two runs are a controlled comparison only if this
  matches, which is exactly what a same-version-but-rebuilt binary cannot fake
  (the incident that motivated recording it: a `--version` that read `1.7.1`
  while the checkout had already moved on). `"zs_bin_path"` records where the
  binary lived, normalised the same way as `"target"` so a committed report is
  not a map of someone's filesystem. Because the hash is read from the file
  directly, `ZS_BIN` / `--zs-bin` must name the binary by path (absolute, or
  relative with a directory such as `./zerostack`) — a bare command name that
  only resolves via `$PATH` cannot be read this way and fails the run.
- **`"git_sha"`** and **`"features"`** are `null` today: the 1.7.x binary embeds
  neither. They are recorded as observed facts of the current binary, not a
  runtime "if the binary happens to expose it" branch, so when upstream starts
  embedding them the field simply stops being `null`.

Identity is captured once, at run start, before any trial spends anything. If
`ZS_BIN --version` cannot run, exits non-zero, or prints nothing, the run
**aborts** naming the binary rather than writing a report that cannot say what
produced it: an identity-less report is the very thing this record exists to
prevent, so there is no default or fallback value. A `--zs-bin` passed alongside
`--backend mock` is ignored, because identity records the source of the evidence
(the fixture), not an unused binary.

The judge fields exist because the judge is the ruler: a run graded by a
different model is not comparable to one graded by the old one just because
both say "pass", so which ruler was used has to survive the run. They record
two different kinds of fact:

- **`"judge_file"` (+ `"judge_hash"`) is configuration**: the judge file
  `--judge` named, `""` when none was. It holds whether or not the judge was
  ever called. `"judge_hash"` fingerprints the file's bytes because a path is
  not an identity: a judge file's contents change under a stable path, the same
  reason a scenario records a `content_hash`. The path is recorded relative to
  the working directory (a bare file name if it lives outside it, forward
  slashes always): a report is meant to be copied into `baselines/`, i.e. into
  git, and `--judge /Users/alice/private-client/judges/x.toml` must not put
  that in a committed artifact.
- **`"judge_model"` is execution**: the model(s) that actually graded, read
  back from the judge's own response, not from the judge file. The file says
  what was *asked for*; the API resolves model names server-side and can serve
  something else. It is a list of every distinct ruler that answered, so no
  consumer has to take a string apart, and no ruler has to stand in for
  another. The field is required — **absent** fails the report's load,
  naming the field, rather than reading in as any of the three states below:
  `null` is **unknown**, `[]` is **nothing was graded** (`--no-judge`, no
  scenario carried a rubric, or every call failed), and `["..."]` names the
  rulers. `[]` and `null` are deliberately different answers (see
  `judges/README.md`). Each trial's own `trial.json` records the same three
  facts for that trial alone.

Swapping the judge should be paired with re-checking a batch against human
labels (see `judges/README.md`). Report-family JSON is read as strictly as it
is written: every field, judge fields included, must be present or the whole
report fails to load naming what's missing — there is no read-tolerance
default that lets an older, incomplete report quietly stand in as "unknown".

`zseval compare` uses the model and hash fields:

- **Different targets**: comparing a baseline evaluated against one
  provider/model to a candidate evaluated against another prints a warning
  (still runs the diff; that's the migration-gate use case, deciding whether to
  switch targets) so a regression check never silently becomes an
  apples-to-oranges comparison. For a side-by-side view of more than two
  targets, use `zseval matrix` instead.
- **Changed scenario definition**: if a shared scenario's `scenario.toml`
  differs between baseline and candidate, `compare` warns instead of quietly
  diffing two different tests under the same id (see AGENTS.md's guardrail
  on not moving the ruler while measuring). Plain inequality on
  `content_hash`: every current run records one, so there is no longer an
  "unknown, skip the check" case to carve out.

## Troubleshooting

**Report says a run cost cents; the provider dashboard says dollars**: both
are right. Headless zerostack doesn't report provider token usage back, so
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

**During long turns**: a heartbeat prints every 15s; `--verbose` prefixes
every agent output line with `[zs:<turn>:out|err]`.
