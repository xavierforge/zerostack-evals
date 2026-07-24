## ADDED Requirements

### Requirement: run accepts a single prompt pack
`zseval run` SHALL accept `--prompts <dir>`, naming a directory of zerostack
prompt files to evaluate against. The flag SHALL be single-arity: giving it
more than once is a usage error, unlike `--target`, which is repeatable. A run
therefore evaluates exactly one pack, and comparing two packs is done by two
runs joined with `zseval matrix`.

#### Scenario: One pack is accepted
- **WHEN** `run --prompts my-pack/` is invoked
- **THEN** the run proceeds against that pack

#### Scenario: Two packs is a usage error
- **WHEN** `run --prompts a/ --prompts b/` is invoked
- **THEN** the command exits 2 explaining that `--prompts` takes at most one pack and that two packs are compared by two runs plus `matrix`

### Requirement: A pack contains only files zerostack will read
zerostack reads only the top-level `*.md` files of a prompt directory, taking
each file's stem as the prompt name, and never recurses. A pack SHALL therefore
be validated at load time, before any trial spends money: a directory that does
not exist or is not a directory, a directory containing no `*.md` file, and a
directory containing any subdirectory or any non-`.md` entry SHALL each be a
load-time error naming the offending entries. Silently skipping the entries
zerostack cannot read would reproduce the very "the file is there but nothing
loads it" failure this capability exists to prevent.

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
For every trial of every scenario, the pack's top-level `*.md` files SHALL be
copied into that trial's isolated working directory at
`work:.zerostack/prompts/`, which is the top layer of zerostack's prompt
override chain and requires no recompile. Seeding SHALL be per trial, inside
the same isolation every other seeded file already uses, so no trial can
observe another's prompts.

#### Scenario: Every trial gets the pack
- **WHEN** a suite runs with `--prompts my-pack/`
- **THEN** each trial's working directory contains `.zerostack/prompts/` holding the pack's `*.md` files

#### Scenario: Without the flag nothing is seeded
- **WHEN** a suite runs without `--prompts`
- **THEN** no trial working directory contains a `.zerostack/prompts/` directory created by the harness

### Requirement: A scenario's own prompt seed wins over the pack
The pack SHALL be seeded before the scenario's own file placements, so a
scenario that seeds a same-named file into `work:.zerostack/prompts/` overrides
the pack for that scenario. This follows the precedent already set for the
target config, which a scenario's `config:` placement may likewise override.
The override SHALL NOT be silent: the scenario's recorded prompt source
distinguishes it from the pack (see the `prompts-pack-identity` capability).

#### Scenario: A scenario seeding the same prompt name overrides the pack
- **WHEN** a scenario seeds `work:.zerostack/prompts/code.md` and the pack also provides `code.md`
- **THEN** the file present at trial time is the scenario's, and the scenario's recorded prompt source is `scenario`, not `pack`

#### Scenario: Different names coexist
- **WHEN** a scenario seeds a prompt name the pack does not provide
- **THEN** both the scenario's file and the pack's files are present

### Requirement: --prompts is rejected under the mock backend
`--prompts` with `--backend mock` SHALL be a usage error. The mock backend
replays canned artifacts and never constructs a zerostack invocation or seeds a
run directory, so a pack could not affect its results; accepting the flag would
produce a report advertising a pack that could not possibly have been read.
This mirrors the existing rejection of `--target` under mock.

#### Scenario: Pack plus mock is a usage error
- **WHEN** `run --backend mock=<fixture> --prompts my-pack/` is invoked
- **THEN** the command exits 2 explaining that mock replays canned artifacts and cannot load a pack

### Requirement: The auto-generated run tag names the pack
WHEN a run uses `--prompts` and no explicit `--tag`, the auto-generated tag
SHALL include the pack directory's name, so two runs that differ only by pack
are distinguishable by their results directory names rather than only by
timestamp.

#### Scenario: The pack name appears in the auto tag
- **WHEN** a run with `--prompts my-pack/` generates its own tag
- **THEN** the tag contains `my-pack` alongside the suite, provider/model, and timestamp segments

#### Scenario: An explicit tag is untouched
- **WHEN** a run passes `--tag stock` with a pack
- **THEN** the tag is exactly `stock`
