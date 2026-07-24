## ADDED Requirements

### Requirement: Report carries the pack's identity
`Report` SHALL record three facts about the pack a run evaluated:
`prompts_pack` (the pack directory, normalized by the same rule as
`judge_file`: working-directory-relative, forward-slashed, never absolute, so a
report copied into `baselines/` is not a map of someone's filesystem),
`prompts_hash` (the fingerprint defined below), and `prompts_names` (the sorted
prompt names the pack provides). A run without a pack SHALL record these as
empty. A path alone cannot pin a pack down, for exactly the reason
`ScenarioResult::content_hash` and `Report::judge_hash` exist: contents change
under a stable path.

#### Scenario: A pack run records all three fields
- **WHEN** a run uses `--prompts my-pack/` containing `code.md` and `review.md`
- **THEN** the report records the relative path, a hash, and `["code", "review"]`

#### Scenario: A run without a pack records empties
- **WHEN** a run uses no `--prompts`
- **THEN** the pack path, hash, and names are empty rather than absent-but-guessed

### Requirement: The pack fingerprint covers contents and names, not location
`prompts_hash` SHALL be computed over the pack's top-level `*.md` files sorted
by file name, folding each file's name and bytes into `util::fnv1a_hex`, the
same hashing approach scenarios and judge files already use. The pack
directory's own path SHALL NOT contribute, so moving an unchanged pack does not
change its fingerprint. File names SHALL contribute, because renaming a prompt
changes which built-in prompt it overrides and therefore changes behavior even
when no byte of content moved.

#### Scenario: Moving an unchanged pack keeps the hash
- **WHEN** the same files are evaluated from `my-pack/` and later from `packs/v2/`
- **THEN** the recorded hash is identical

#### Scenario: Renaming a prompt changes the hash
- **WHEN** a pack's `code.md` is renamed to `mycode.md` with identical bytes
- **THEN** the recorded hash differs

### Requirement: Each scenario records the prompt it actually loaded
Seeding a file is not loading it: which prompt a trial loads is decided by the
scenario's `prompt` field or the config default, so a pack whose names no
scenario calls sits inert while the report advertises it. `ScenarioResult`
SHALL therefore record `prompt_name` and `prompt_source`, resolved in this
order:

1. WHEN the prompt name cannot be determined (see the derivation requirement
   below), the source SHALL be `unknown`.
2. WHEN the scenario's own file placements provide that name under
   `work:.zerostack/prompts/`, the source SHALL be `scenario`.
3. WHEN the pack provides that name, the source SHALL be `pack`. This is exact
   rather than inferred: `.zerostack/prompts/` is the top layer of zerostack's
   override chain, so a name the pack provides is the content that loads.
4. Otherwise the source SHALL be `stock`, meaning zerostack's built-in prompt.

These are scenario-level facts, constant across a scenario's trials, and belong
beside `content_hash` rather than being repeated per trial.

#### Scenario: A scenario naming a prompt the pack provides
- **WHEN** a scenario declares `prompt = "code"` and the pack provides `code.md`
- **THEN** its recorded name is `code` and its source is `pack`

#### Scenario: A scenario naming a prompt the pack does not provide
- **WHEN** a scenario declares `prompt = "ask"` and the pack provides only `code.md`
- **THEN** its recorded name is `ask` and its source is `stock`

#### Scenario: A scenario that seeds its own prompt
- **WHEN** a scenario seeds `work:.zerostack/prompts/code.md` and the pack also provides `code.md`
- **THEN** its source is `scenario`

### Requirement: The default prompt name is derived only where derivation holds
A scenario that declares no `prompt` does not run without one: zerostack falls
back to the config's `default_prompt`, and to `code` when the config sets none.
The harness SHALL derive that name from the target config it seeds, so the
scenarios that declare no prompt are covered rather than blank. The derivation
SHALL be abandoned, recording `unknown`, WHEN the scenario places any file that
could move the effective config out from under it, namely a placement into the
run's config directory or into `work:.zerostack/config.toml`. In that case the
harness's own copy is no longer the last word, and reading it would record a
value that never took effect.

#### Scenario: A scenario without a prompt field resolves to the default
- **WHEN** a scenario declares no `prompt` and the target config sets no `default_prompt`
- **THEN** its recorded name is `code`, sourced by the same rules as a declared name

#### Scenario: A config-seeding scenario records unknown
- **WHEN** a scenario declares no `prompt` and places a file into the config directory or `work:.zerostack/config.toml`
- **THEN** its recorded source is `unknown` rather than a derived name

### Requirement: A pack that never loads is reported
A run SHALL report, on its own output, how many scenarios actually loaded a
prompt from the pack. WHEN a pack was given and no scenario resolved to source
`pack`, the run SHALL warn that the pack was seeded but never loaded, so a
report advertising a pack cannot be read as that pack's score when it is the
built-in prompts' score. This is a run-level check about whether the
independent variable was manipulated at all, distinct from the cross-run checks
in the `controlled-variables` capability.

#### Scenario: A pack no scenario calls warns
- **WHEN** a run uses a pack providing only `my-code.md` and no scenario resolves to that name
- **THEN** the run warns that the pack was seeded but never loaded

#### Scenario: A partially-used pack is visible per scenario
- **WHEN** a pack is loaded by some scenarios and not others
- **THEN** no run-level never-loaded warning fires, and each scenario's own recorded source distinguishes which were measured against the pack

### Requirement: Reports predating these fields load as unknown, not as stock
Every field added here SHALL deserialize with a default so a report written
before this change still loads, following the precedent of `judge_file`,
`target`, and `content_hash`. The default for `prompt_source` SHALL be
`unknown`, never `stock`: an older report's prompts were not observed, which is
not the same fact as a run that observably used the built-in prompts.
`REPORT_SCHEMA_VERSION` SHALL NOT be bumped and no code SHALL branch on it.

#### Scenario: An older report still loads
- **WHEN** a report written before these fields is deserialized
- **THEN** it loads with an empty pack identity rather than failing to parse

#### Scenario: Absent is not the same as stock
- **WHEN** an older report is loaded
- **THEN** its scenarios' prompt source is `unknown`, distinguishable from a new run that recorded `stock`
