# prompt-pack example

A minimal `--prompts` pack, plus one scenario that proves the pack beat
zerostack's built-in prompt rather than merely sitting on disk next to it.

    examples/prompt-pack/
      pack/code.md        the pack: overrides zerostack's built-in `code` prompt
      scenario/scenario.toml   asserts the pack's marker, not the built-in's content

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

## How to run it

    cargo build -p zseval
    export ZS_BIN=/path/to/a/real/zerostack/build   # or --zs-bin <path>
    export ANTHROPIC_API_KEY=sk-ant-...             # or whatever targets/*.toml needs

    cargo run -p zseval -- run examples/prompt-pack/scenario \
      --target targets/anthropic.toml \
      --prompts examples/prompt-pack/pack \
      --tag prompt-pack-example \
      --results results/prompt-pack-example

No `--judge`/`--no-judge` is needed: the scenario has no `judge` rubric, only
the deterministic `final_contains` assert. Check the report for the pack's
identity and what the scenario resolved:

    grep -E '"prompts_pack"|"prompts_hash"' results/prompt-pack-example/*/report.json
    grep '"prompt_source"' results/prompt-pack-example/*/report.json

A passing trial's recorded `prompt_source` is `"pack"` and its final message
starts with `ZSEVAL-PROMPT-PACK-MARKER`.

## What a failure here can and cannot tell you

The assert rides on the model *obeying a formatting instruction* (put this
exact string on the first line of your reply), not on zerostack reporting
which prompt it loaded (it doesn't; see `prompts-pack-identity`'s design
notes). That means a failed marker check has two possible causes, and this
scenario alone cannot distinguish them:

1. The pack never reached the model (seeding broke, the wrong pack path was
   given, zerostack's own precedence changed).
2. The pack reached the model and it loaded `pack/code.md`, but the model
   didn't follow the first-line instruction to the letter.

Tell them apart with the evidence the harness *can* give you directly,
without relying on model behavior:

- The report's `prompt_source` for `prompt-pack-example-marker`: `"pack"`
  means the harness resolved this scenario's `code` to the pack's file by
  construction (scenario seed, then pack, then stock: see
  `prompts-pack-identity`), independent of whether the model obeyed anything.
  If it reads `"pack"`, cause 1 is ruled out: the disobedience is on the
  model, not the harness.
- The trial's `.zerostack/prompts/` listing (seeded fresh per trial;
  reproduce with `--zs-bin` per `.claude/skills/verify/SKILL.md` to inspect
  it without spending anything) shows whether `code.md` actually landed
  before the model ran at all.

In short: a disobedient model reads exactly like "the pack wasn't loaded" in
the scenario's pass/fail column alone. `prompt_source` is what separates the
two: read it before concluding the feature is broken.
