# prompt-pack example

A minimal `--prompts` pack, plus one scenario that proves the pack beat
zerostack's built-in prompt rather than merely sitting on disk next to it.

    examples/prompt-pack/
      pack/code.md        the pack: overrides zerostack's built-in `code` prompt
      scenario/scenario.toml   asserts the prompt identity the session recorded

The scenario's id is still `prompt-pack-example-marker`: it kept the name it
was born with, from the days when the evidence was a marker string the
prompt asked the model to echo. It no longer asserts a marker.

## Why this lives outside `scenarios/`

Every scenario under `scenarios/` is expected to pass with no flags beyond
`--target`, because that is the suite CI and `zseval list scenarios/` count
on. This scenario fails without `--prompts examples/prompt-pack/pack`: its
whole point is to only pass when a pack is loaded, so it cannot live inside
`scenarios/` without either changing the default suite's count or silently
never being run. Keeping it in `examples/` keeps `scenarios/` at its
documented count and keeps this one runnable on demand.

`pack/` also has to be its own directory rather than sitting beside the
scenario: `--prompts <dir>` requires the directory to contain nothing but
top-level `*.md` files (see `prompts-pack-run`'s pack-validation
requirement), and this directory has a `scenario.toml` and a `README.md` in
it.

## Copying this: a pack prompt must not copy a built-in

zerostack decides `built_in` vs `user_file` by content, not by which
directory the file was read from: a user file whose bytes are identical to
the built-in of that name is recorded as `built_in`. So a pack prompt that
starts life as a verbatim copy of `data/prompts/<name>.md` records
`built_in` even though the pack really did win, and an assert on the
recorded identity would then be asserting the wrong thing. `pack/code.md`
here is a few hundred bytes of its own text against a built-in `code` of
several thousand, nowhere near colliding. Keep any pack prompt you copy this
example into meaningfully different from the built-in it overrides.

## How to run it

    cargo build -p zseval
    export ZS_BIN=/path/to/a/real/zerostack/build   # or --zs-bin <path>
    export ANTHROPIC_API_KEY=sk-ant-...             # or whatever targets/*.toml needs

    cargo run -p zseval -- run examples/prompt-pack/scenario \
      --target targets/anthropic.toml \
      --prompts examples/prompt-pack/pack \
      --tag prompt-pack-example \
      --results results/prompt-pack-example

`ZS_BIN` has to be a build that records the prompt it loaded into its session
JSON (zerostack PR #228); against an older binary the session carries no
`prompt` field and the assert fails rather than passing vacuously. See the
repo README's "How it drives zerostack" for the build prerequisite.

No `--judge`/`--no-judge` is needed: the scenario has no `judge` rubric, only
the deterministic `prompt_recorded` assert. Check the report for the pack's
identity and what the scenario resolved:

    grep -E '"prompts_pack"|"prompts_hash"' results/prompt-pack-example/*/report.json
    grep '"prompt_source"' results/prompt-pack-example/*/report.json

A passing trial's session recorded `code` from a `user_file`, which is what
the assert grades; the report's own `prompt_source` for the scenario then
reads `"pack"`, the harness mapping that readback against what it seeded.

## What a failure here tells you

The assert reads zerostack's own record of the prompt that served the run, so
a failure is a statement about the run rather than about the model's writing.
It no longer rides on the model obeying a formatting instruction, which used
to make "the model dropped the marker" indistinguishable from "the pack never
loaded". The three failures it can report are different problems:

1. **Recorded `built_in` instead of `user_file`**: the built-in `code` won.
   Either the pack never reached zerostack's override layer (seeding broke,
   the wrong pack path was given, zerostack's own precedence changed), or the
   pack file's bytes match the built-in and were classified as one (see the
   copying rule above).
2. **Recorded a different name**: the run resolved some prompt other than
   `code`, so the scenario is not testing the override it means to. Check the
   scenario's `prompt` field and the target's `default_prompt`.
3. **Nothing recorded**: the session carries no `prompt` field at all, which
   means the binary under test predates PR #228. Rebuild `ZS_BIN`. This
   fails; it never passes silently.

Two further pieces of evidence, neither of which depends on model behavior:

- The report's `prompt_source` for `prompt-pack-example-marker`: `"pack"`
  means the recorded `user_file` matched the pack's own file list (scenario
  seed, then pack, then stock: see `prompts-pack-identity`). `"unknown"`
  means a `user_file` nobody in the harness claims to have seeded, which is
  worth reading the run dir over.
- The trial's `.zerostack/prompts/` listing (seeded fresh per trial;
  reproduce with `--zs-bin` per `.claude/skills/verify/SKILL.md` to inspect
  it without spending anything) shows whether `code.md` actually landed
  before the model ran at all.
