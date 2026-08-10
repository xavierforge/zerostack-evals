# Scenarios

The suites this harness runs, and how to write another one. Terms used here
(scenario, suite, kind, verdict, pass@k) are defined in the repo README's "Core
concepts".

    scenarios/<suite>/<case>/scenario.toml   fixtures sit in a _fixtures dir
                                             beside a case, or in the suite
                                             dir's _fixtures if shared across it
    scenarios/coverage.toml                  the coverage ledger

## What the suites cover

Coverage spans zerostack's named prompt modes (`ask`, `code`, `plan`, …),
mostly checkable with deterministic asserts; its `memory` subsystem
(`memory/`); its subagent delegation via the `task` tool (`subagents/`); and
its MCP tool integration via a mock stdio server (`mcp/`). Each of those three
subsystems' layout knowledge lives in its own quarantined `domains::` module
(see "Evaluating another subsystem"), and the harness core
(scenario/backend/seed/asserts/verdict) stays subsystem-agnostic. Alongside
them sit the suites that need no subsystem knowledge at all: tool selection
(`tools/`), project context (`context/`), session continuity and evidence
recording (`session/`), and `--loop` mode (`loop/`).

What the suite does *not* measure is stated as plainly as what it does, in one
ledger: `coverage.toml` carves areas out of zerostack's functional surface, so
an area no scenario touches is a row in that file rather than an absence a
reader has to notice on their own. `zseval site` renders the ledger beside a
run's results, and refuses to render at all if the ledger and this tree
disagree about which scenarios exist.

`memory/` requires a zerostack build with the `memory` feature (not in
zerostack's `default` features), which is why the README's first prerequisite
is an `--all-features` build:

    cargo build --release --features memory --manifest-path ../zerostack/Cargo.toml

Building without it doesn't crash anything: the memory tools simply never
register, `memory_search`/`memory_read`/`memory_write` never get called, and
the `[seed.memory]` drift check reports "no 'memory open:' trace line was
found", pointing straight at the missing feature flag.

## Writing a scenario

A scenario is flat TOML (see `prompts/*/scenario.toml`). The assert DSL
reference lives at the top of `crates/zseval/src/asserts.rs`.

    id     = "prompt-ask-readonly-refuses-edit"
    kind   = "regression"          # required: "regression" | "capability"
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

`kind` has no default in either direction: it exists to make you answer whether
a low score here is a problem (`regression`) or a measurement (`capability`),
and a default would un-ask the question. A missing or unrecognised value is a
load-time error, as is any unknown scenario field.

`src` is found by walking up from the scenario's own dir, looking inside a
`_fixtures` folder at each level: a file used by only one scenario sits in that
scenario's own `_fixtures` dir (e.g. `prompts/ask-answers/_fixtures`); a file
shared across a suite sits in the suite dir's `_fixtures` above it (e.g.
`prompts/_fixtures/hello.py` serves every prompt scenario). Nearest match wins,
so a scenario can shadow a shared fixture.

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
`file_contains config:agent/memory/MEMORY.md tabs`. All three take at most one
`*` path segment (`projects/*/OUT.md`), and all three reject a second star at
load time, naming the op and the path: the matcher expands exactly one, so a
mistyped two-star path can only ever match nothing, and without the load-time
check it would surface mid-run as an ordinary "nothing matches" failure that
reads like a real finding about the agent. `file_not_contains` fails if nothing
matches its path (a missing file or a zero-hit glob is not evidence the file is
clean, it's evidence the file was never written); use `path_not_exists <path>`
when the check really is "nothing should be there at all" — it passes only when
zero files *and* directories match.

### `mode = "loop"` scenarios

    id     = "loop-fixes-failing-test"
    mode   = "loop"                # default is "print" (the -p/--continue path)
    kind   = "regression"
    trials = 3
    task   = "test_calc.py is failing. Find the bug in calc.py and fix it."
    expect = [
      "file_not_contains work:calc.py return a - b",
      "transcript_contains ALL TESTS PASS",
    ]

    [loop]
    max_iterations = 3            # required (--loop is unbounded otherwise)
    run = "python3 test_calc.py"  # optional (--loop-run); output feeds the next
                                  # iteration

Drives a single `zerostack --loop --loop-max N [--loop-run CMD] <task>`
invocation instead of the per-turn `-p`/`--continue` loop, so `task` must be a
single turn (no array). `loop` is in zerostack's default features: no extra
build flag needed.

Two things loop mode gives up, both enforced at load time so a scenario can't
silently ship a footgun:

- **No session file**: grading evidence is
  `$ZS_DATA_DIR/loops/<uuid>/iter-NNNN.json` instead, which `transcript.rs`
  folds in as ordinary messages, so `final_contains`/`transcript_contains`/
  `file_*` all work unchanged. With no session there is no recorded prompt
  either, so a loop scenario's `prompt_source` stays the old derivation from
  what the harness seeded (`docs/evidence-and-reports.md`).
- **No tool-call evidence at all**: no session file means no `tool` records,
  and the iteration records carry only prompt/response text. `tool_called` /
  `tool_not_called` / `tool_called_after` / `tool_count` / `tool_arg_contains`
  / `no_tool_call_contains` / `tokens_under` are all rejected on a
  `mode = "loop"` scenario at load time: grade on `file_contains` /
  `transcript_contains` / `final_contains` instead.

### `security_mode` and `cli_args`

    security_mode = "read-only"            # optional, default "yolo"
    cli_args      = ["--quick-model", "fast"]  # optional, default []

`security_mode` selects the permission mode a scenario's zerostack invocation
launches with, mirroring zerostack's own flag names: `yolo` (default, `--yolo`)
| `standard` (no flag, it's what zerostack does when no permission flag is
passed) | `restrictive` (`--restrictive`) | `read-only` (`--read-only`) |
`guarded` (`--guarded`) | `accept-all` (`--accept-all`) |
`dangerously-skip-permissions` (`--dangerously-skip-permissions`). Omitting
the field keeps a scenario's exact current invocation, since `yolo` was the
harness's hardcoded behaviour before this field existed. An unrecognised value
is a load-time error, same as any other typo'd scenario field. In current
zerostack, `--accept-all` resolves to the same permission mode as `standard`
(upstream `startup.rs` maps `accept_all` to `Standard`; there is no
`AcceptAll` permission variant), so a scenario contrasting `accept-all`
against `standard` measures no enforcement difference today; the value exists
to mirror upstream flags verbatim.

`security_mode` is the *only* place a run's permission mode is declared, and
that is enforced rather than assumed. zerostack resolves the mode it actually
runs in from its config file and the command line together, and the config
outranks some of the flags — a config holding `yolo = true` beats a
`--read-only` on the command line — so a permission key sitting in a config the
run reads would decide the mode while the report still named the declared one.
Both routes into such a config are refused:

- A **`--target`** file carrying `yolo`, `accept_all`, `restrictive`, or
  `default_permission_mode` (the four keys upstream resolves a mode from) fails
  the preflight gate, naming the file and the key. Nothing runs and nothing is
  spent.
- A **`[[files]]` seed with a `config:` dest** whose fixture carries one of those
  keys fails scenario load, naming the seed and the key.

Presence is what is refused, not the value: `yolo = false` steers nothing today,
but it is still a launch's permission mode written in the wrong file, and the
fix is the same either way. Put the mode in `security_mode` and delete the key.

`cli_args` appends verbatim tokens after every harness-owned flag and
immediately before the turn message, in both the `-p` and `--loop` assembly
paths. A flag that takes a separate value is two entries, not one:

    cli_args = ["--quick-model", "fast"]

Every dash-prefixed token (with any `=value` suffix stripped) is checked at
load time against the flags the harness already owns (`-p`, `--loop`,
`--log-file`, `--continue`, `--load-prompt`, `--no-color`, `--pure-stdout`,
`--loop-max`, `--loop-run`, and the six permission-mode flags: `--yolo`,
`--restrictive`/`-R`, `--read-only`, `--guarded`, `--accept-all`,
`--dangerously-skip-permissions`), and a hit fails the load, naming the
scenario path and the offending token, before any trial spends money. Every
accepted spelling of a harness-owned flag is rejected, not just its canonical
form: `--print` (an alias of `-p`) and `-c` (an alias of `--continue`) are
denied the same as the flags they alias. A single-dash token is also scanned
character-wise, so a short cluster that smuggles an owned short flag inside it
(e.g. `-nR`, which smuggles `-R`) is denied even though `-nR` itself never
appears in the list. Permission-mode flags are denied here even though they'd
otherwise be harmless duplicates: `security_mode` is their one source of
truth, so `cli_args` can't smuggle a second, possibly conflicting, declaration
of the same thing.

`--api-key` (in either the `["--api-key", "sk-…"]` or the `--api-key=sk-…`
spelling) fails the load too, for a different reason: an argument vector is
readable by every process on the host — zerostack's own help for the flag says
so — and the by-hand repro command a timed-out trial prints copies `cli_args`
verbatim into a persisted report, so a key passed here leaks twice. Export the
provider's key variable instead; the harness passes the environment through.
The error names the flag and never the value, so nothing echoes the key back.

Positional (non-dash) entries are allowed, since a separate-value flag needs
one right after it (as in `["--quick-model", "fast"]` above), but a stray
positional collides with the turn message silently rather than loudly:
zerostack's positional argument is `message: Vec<String>`, and every
positional token is joined with spaces into one prompt. Any `cli_args` entry
that isn't dash-prefixed and isn't the value half of a preceding flag lands in
the argument vector as its own positional, ahead of the turn message that's
also a positional, and the two are joined into a single mutated message with
no error anywhere. The scenario then grades against a prompt nobody wrote
down, and nothing surfaces the mismatch. Getting the token order right, so a
non-dash entry sits strictly as the value immediately after its flag, is on
the scenario author.

The mirror image is a trailing value-taking flag with the value forgotten,
e.g. `cli_args = ["--quick-model"]` with nothing after it: zerostack's CLI
binds the turn message itself as that flag's value, so the scenario runs with
an empty message and no error at any layer.

`--no-session` works in `mode = "loop"` but breaks print mode: `run_print`
reads the session file back after the turn, so a print-mode scenario passing
`--no-session` fails with a missing session file.

A scenario can use `cli_args` to override what the target supplies (`--model`,
`--quick-model`, `--provider`); this is deliberately not denied. The
convention, same as `[[files]]` whole-file config seeds that override a
target's `config.toml`: a scenario that overrides target identity notes in a
TOML comment that it ignores `--target`. Overriding the *permission mode* that
way is the one exception, refused in both routes as described above.

## Evaluating another subsystem

A subsystem like `memory` lays out files zerostack itself decides the shape of
(e.g. `<config_dir>/agent/memory/MEMORY.md`): that layout knowledge is a
*snapshot* of zerostack internals, not something the harness core should know.
It's quarantined to one file, `crates/zseval/src/domains/<name>.rs`. The core
never names a specific subsystem: `Scenario::load`, `seed::apply`, and the
runner call exactly three dispatch functions: `domains::{validate, expand,
verify}` (`crates/zseval/src/domains/mod.rs`), and those three functions are
the *only* place "which domains exist" is listed. Adding eval support for
another subsystem is: one new `domains::` module, one match arm in each of the
three dispatch functions, plus one new optional field on `SeedSugar` *if* the
subsystem actually has something to seed (zero changes to
`scenario`/`seed`/`runner` otherwise).

Because the knowledge is a snapshot, every `domains::` module pairs its layout
knowledge with a runtime drift check `domains::verify` dispatches to after
driving the agent (see `domains::memory::verify`). If zerostack's actual layout
no longer matches what the module assumes, the trial grades **Indeterminate**
with a message naming the fix, never a silent Fail. This is what makes memory
evals resilient to zerostack iterating quickly: a scenario doesn't quietly
start "failing" just because an internal path moved, it stops being gradable
until someone updates the domain module.

`verify` normally runs because the scenario declared `[seed.memory]` sugar, but
a scenario that starts from an *empty* store (nothing to seed, only an
assertion that the agent wrote something new) has no sugar to trigger it. For
that case, opt in explicitly: `domains = ["memory"]` at the top level of
`scenario.toml` runs the same drift check with no seeding attached. An unknown
name in `domains = [...]` is a load-time error, same as any other typo'd
scenario field.

`domains::subagents` (`subagents/`) is the same pattern applied to a subsystem
with nothing to seed and, as of this writing, no reliable startup trace line
either: zerostack's `task` tool (subagent delegation) logs nothing at all,
unlike memory's `Mem::open()`. Its `verify` is a deliberate no-op rather than a
drift check; scenarios opt in with `domains = ["subagents"]` alone, and
vacuous-pass protection comes entirely from pairing
`tool_called`/`tool_not_called task` with a positive assert in the scenario
itself. See the module's own doc for the full investigation.

`domains::mcp` (`mcp/`) evaluates whether the agent uses an MCP-provided tool
when it should and leaves it alone when it shouldn't, via a dependency-free
`python3` stdio server fixture (`mcp/_fixtures/mock_mcp_server.py`, exposing
one tool, `lookup_ticket`). `[seed.mcp]` sugar rewrites the run's
already-seeded `config.toml` in place to add an `[mcp_servers.<name>]` table,
the one domain so far whose seeding isn't a file copy, since MCP server config
is a field inside `config.toml`, not a separate file. It also force-disables
zerostack's default-enabled "Exa Web Search" MCP server (`enable-exa-mcp =
false`), confirmed live to otherwise connect unconditionally and add a second,
real, network-backed tool alongside the mock one (see the module's own doc for
the exact trace evidence). `verify` greps turn zslogs for `Connected to MCP
server '{name}'` per seeded server, the same drift-check shape as
`memory::verify`. Requires zerostack's `mcp` feature (in `default` features as
of this writing).
