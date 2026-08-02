# prompts-pack-run

## Purpose

`run` accepts a single custom prompt pack via `--prompts <dir>`, validating it at load time and seeding its top-level `*.md` files into every trial's `work:.zerostack/prompts/` before the scenario's own file placements, so a report can measure custom prompts with no zerostack recompile.

## Requirements

### Requirement: run accepts a single prompt pack
`zseval run` SHALL accept `--prompts <dir>`, naming a directory of zerostack prompt files to evaluate against. The flag SHALL be single-arity: giving it more than once is a usage error, unlike `--target`, which is repeatable. A run therefore evaluates exactly one pack, and comparing two packs is done by two runs joined with `zseval matrix`.

#### Scenario: One pack is accepted
- **WHEN** `run --prompts my-pack/` is invoked
- **THEN** the run proceeds against that pack

#### Scenario: Two packs is a usage error
- **WHEN** `run --prompts a/ --prompts b/` is invoked
- **THEN** the command exits 2 explaining that `--prompts` takes at most one pack and that two packs are compared by two runs plus `matrix`

### Requirement: A pack contains only files zerostack will read
zerostack reads only the top-level `*.md` files of a prompt directory, taking each file's stem as the prompt name, and never recurses. A pack SHALL therefore be validated at load time, before any trial spends money: a directory that does not exist or is not a directory, a directory containing no `*.md` file, and a directory containing any subdirectory or any non-`.md` entry SHALL each be a load-time error naming the offending entries. Silently skipping the entries zerostack cannot read would reproduce the very "the file is there but nothing loads it" failure this capability exists to prevent.

#### Scenario: A pack with a subdirectory is rejected
- **WHEN** a pack directory contains a subdirectory
- **THEN** the run exits 2 before any trial, naming the subdirectory and stating that zerostack reads only top-level `*.md`

#### Scenario: A pack with a non-markdown file is rejected
- **WHEN** a pack directory contains a file whose extension is not `.md`
- **THEN** the run exits 2 before any trial, naming that file

#### Scenario: An empty or missing pack is rejected
- **WHEN** the pack directory does not exist, is not a directory, or contains no `*.md` file
- **THEN** the run exits 2 before any trial

### Requirement: The pack is seeded into every trial
For every trial of every scenario, the pack's top-level `*.md` files SHALL be copied into that trial's isolated working directory at `work:.zerostack/prompts/`, which is the top layer of zerostack's prompt override chain and requires no recompile. Seeding SHALL be per trial, inside the same isolation every other seeded file already uses, so no trial can observe another's prompts.

#### Scenario: Every trial gets the pack
- **WHEN** a suite runs with `--prompts my-pack/`
- **THEN** each trial's working directory contains `.zerostack/prompts/` holding the pack's `*.md` files

#### Scenario: Without the flag nothing is seeded
- **WHEN** a suite runs without `--prompts`
- **THEN** no trial working directory contains a `.zerostack/prompts/` directory created by the harness

### Requirement: A scenario's own prompt seed wins over the pack
The pack SHALL be seeded before the scenario's own file placements, so a scenario that seeds a same-named file into `work:.zerostack/prompts/` overrides the pack for that scenario. This follows the precedent already set for the target config, which a scenario's `config:` placement may likewise override. The override SHALL NOT be silent: the scenario's recorded prompt source distinguishes it from the pack (see the `prompts-pack-identity` capability).

#### Scenario: A scenario seeding the same prompt name overrides the pack
- **WHEN** a scenario seeds `work:.zerostack/prompts/code.md` and the pack also provides `code.md`
- **THEN** the file present at trial time is the scenario's, and the scenario's recorded prompt source is `scenario`, not `pack`

#### Scenario: Different names coexist
- **WHEN** a scenario seeds a prompt name the pack does not provide
- **THEN** both the scenario's file and the pack's files are present

### Requirement: A pin the run's own pack shadows is skipped, not graded
A `prompt_recorded <name> built_in` assert pins the compiled-in default, and the pack is seeded into every trial, so a pack providing that same `<name>` replaces the very thing the pin watches; grading it would report the harness's own seeding as a product regression. WHEN a scenario resolves its prompt from the run's pack and one of its asserts pins that same name as `built_in`, the scenario SHALL be skipped before any trial runs: the run SHALL say so on stderr naming the scenario and the shadowed name, and the scenario SHALL be ungradable, not failed. The skip SHALL be exactly that narrow: a scenario seeding the pinned name itself SHALL grade normally (the shadowing is the scenario's own doing, not the run's pack), a pin on a name the scenario never resolves SHALL grade and fail honestly (an authoring error the pack does not excuse), and a `user_file` pin SHALL always grade.

#### Scenario: The shipped built-in pin is skipped under a pack that shadows it
- **WHEN** a run seeds a pack providing `code.md` and a scenario resolves `code` from that pack while asserting `prompt_recorded code built_in`
- **THEN** the scenario runs no trial, the skip line names the scenario and `code`, and the scenario is ungradable, not failed

#### Scenario: A scenario shadowing the built-in itself still grades
- **WHEN** the scenario's own seed supplies the pinned name, pack or no pack
- **THEN** the scenario is graded normally

#### Scenario: A skipped pin does not stop the suite
- **WHEN** one scenario is skipped for a shadowed pin
- **THEN** every other scenario still runs and the report still builds

### Requirement: --prompts is rejected under the mock backend
`--prompts` with `--backend mock` SHALL be a usage error. The mock backend replays canned artifacts and never constructs a zerostack invocation or seeds a run directory, so a pack could not affect its results; accepting the flag would produce a report advertising a pack that could not possibly have been read. This mirrors the existing rejection of `--target` under mock.

#### Scenario: Pack plus mock is a usage error
- **WHEN** `run --backend mock=<fixture> --prompts my-pack/` is invoked
- **THEN** the command exits 2 explaining that mock replays canned artifacts and cannot load a pack

### Requirement: The auto-generated run tag names the pack
WHEN a run uses `--prompts` and no explicit `--tag`, the auto-generated tag SHALL include the pack directory's name, so two runs that differ only by pack are distinguishable by their results directory names rather than only by timestamp.

#### Scenario: The pack name appears in the auto tag
- **WHEN** a run with `--prompts my-pack/` generates its own tag
- **THEN** the tag contains `my-pack` alongside the suite, provider/model, and timestamp segments

#### Scenario: An explicit tag is untouched
- **WHEN** a run passes `--tag stock` with a pack
- **THEN** the tag is exactly `stock`
</content>
