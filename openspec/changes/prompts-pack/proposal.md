## Why

Evaluating a custom prompt today means editing prompts inside the zerostack
checkout and rebuilding (README's "Iterating on prompts"). That puts the one
thing a user most wants to measure behind a compile step, and it leaves no
record of which prompts a report was produced with. zerostack already reads
`.zerostack/prompts/*.md` from its working directory as the top layer of its
prompt override chain, so the harness can seed a whole pack per trial and get
custom-prompt evaluation with no recompile at all.

Seeding files is the easy half. The hard half is that a seeded file is not a
loaded file: which prompt a trial actually loads is decided by the scenario's
`prompt` field (or the config default), so a pack whose names nobody calls sits
inert on disk while the report advertises it. And once a run can vary the pack,
`compare` and `matrix` gain a second thing that can move underneath a score,
which would otherwise read as a model regression.

## What Changes

- `zseval run --prompts <dir>` seeds the pack's top-level `*.md` into every
  trial's `work:.zerostack/prompts/`, before the scenario's own seeds, so a
  scenario that seeds a same-named prompt still wins.
- `--prompts` is single-arity, validated at load time, and rejected under
  `--backend mock`. A pack containing subdirectories or non-`.md` files, a
  missing directory, and an empty pack are all load-time errors: zerostack
  would silently ignore those files, which is the exact failure this change
  exists to prevent.
- `Report` records the pack's identity: relative path, content hash (contents
  and file names, never the pack's own path), and the sorted prompt names.
- `ScenarioResult` records `prompt_name` and `prompt_source`
  (`pack` / `scenario` / `stock` / `unknown`), so a report shows per scenario
  whether the pack was actually the prompt that loaded. A run whose pack was
  never loaded by any scenario warns.
- `compare` and `matrix` display pack identity (path plus short hash) on their
  identity lines, and mark a comparison in which more than one independent
  variable moved. Varying only the pack is a clean experiment and is not marked.
- The auto-generated run tag includes the pack directory name.
- An `examples/prompt-pack/` directory ships a marker pack and a scenario that
  asserts the marker reaches the model. It lives outside `scenarios/` because
  it is only meaningful with `--prompts`.
- README gains a `--prompts` section covering the two-run + `matrix` flow.

No **BREAKING** changes: every new field deserializes with a default, so
reports predating this change still load.

## Capabilities

### New Capabilities

- `prompts-pack-run`: the `--prompts` flag surface, pack validation, per-trial
  seeding, precedence against scenario seeds, and the run tag.
- `prompts-pack-identity`: what a run records about the pack and about which
  prompt each scenario actually loaded, including the derivation rules, the
  `unknown` state, and the never-loaded warning.
- `controlled-variables`: the rule that a comparison may vary exactly one
  independent variable, and how `compare` applies it to the pack.

### Modified Capabilities

- `matrix-render`: the column legend gains the pack identity, and a column mark
  is added for a table in which the pack moved alongside the target. This mark
  is not DRIFT, which stays reserved for a changed ruler.

## Non-goals

Deliberately out of scope, each with its reason:

- **The three false-pass fixes and `kind` marking** (ROADMAP v1 items 1): a
  separate change, independent of this one.
- **Regenerating the baseline** (ROADMAP v1 item 5): the schema will likely
  move again before v1 ships, so the baseline is regenerated once, later.
- **`zseval site`** (ROADMAP v1 item 6): downstream renderer, needs the
  coverage ledger first.
- **A mode-enforcement scenario**: blocked upstream — zerostack's `%%mode=`
  directive never took effect in the permission layer. The acceptance scenario
  here deliberately uses a non-permission marker.
- **Repeatable `--prompts`**: the correct shape is "at most one repeatable
  axis", not two, and it reworks the multi-target orchestration that only just
  landed. Recorded as a follow-up in design.md.
- **Exit-code semantics for a broken comparison**: `compare`'s exit code
  already ignores the existing `target_mismatch`, so this is a pre-existing gap
  and belongs with the CI gate (ROADMAP item 12), not here.

## Impact

- `crates/zseval/src/main.rs`: `--prompts` parsing, validation, usage errors,
  `auto_tag`, `compare` rendering.
- `crates/zseval/src/backend.rs`: `ZsCli` seeds the pack before `seed::apply`.
- `crates/zseval/src/verdict.rs`: new `Report` and `ScenarioResult` fields.
- `crates/zseval/src/runner.rs`: resolving each scenario's prompt name and
  source at run time.
- `crates/zseval/src/compare.rs`, `matrix.rs`: identity display and marking.
- `crates/zseval/tests/harness.rs`: offline coverage via a stub `--zs-bin`.
- New `examples/prompt-pack/`; `README.md`.
- No new crate dependencies.
- Depends on nothing; ROADMAP item 2 (recording zerostack's identity in the
  report) later sharpens `compare`'s variable count — see design.md.
