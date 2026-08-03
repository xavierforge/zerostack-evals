# prompts-pack-identity

## Purpose

`Report` and `ScenarioResult` record what a run's prompt pack actually was and which prompt each scenario actually loaded, so a report advertising a pack can be trusted or, when the pack never loaded, warned against.

## Requirements

### Requirement: Report carries the pack's identity
`Report` SHALL record three facts about the pack a run evaluated: `prompts_pack` (the pack directory, normalized by the same rule as `judge_file`: working-directory-relative, forward-slashed, never absolute, so a report copied into `baselines/` is not a map of someone's filesystem), `prompts_hash` (the fingerprint defined below), and `prompts_names` (the sorted prompt names the pack provides). A run without a pack SHALL record these as empty. A path alone cannot pin a pack down, for exactly the reason `ScenarioResult::content_hash` and `Report::judge_hash` exist: contents change under a stable path.

#### Scenario: A pack run records all three fields
- **WHEN** a run uses `--prompts my-pack/` containing `code.md` and `review.md`
- **THEN** the report records the relative path, a hash, and `["code", "review"]`

#### Scenario: A run without a pack records empties
- **WHEN** a run uses no `--prompts`
- **THEN** the pack path, hash, and names are empty rather than absent-but-guessed

### Requirement: The pack fingerprint covers contents and names, not location
`prompts_hash` SHALL be computed over the pack's top-level `*.md` files sorted by file name, folding each file's name and bytes into `util::fnv1a_hex`, the same hashing approach scenarios and judge files already use. The pack directory's own path SHALL NOT contribute, so moving an unchanged pack does not change its fingerprint. File names SHALL contribute, because renaming a prompt changes which built-in prompt it overrides and therefore changes behavior even when no byte of content moved.

#### Scenario: Moving an unchanged pack keeps the hash
- **WHEN** the same files are evaluated from `my-pack/` and later from `packs/v2/`
- **THEN** the recorded hash is identical

#### Scenario: Renaming a prompt changes the hash
- **WHEN** a pack's `code.md` is renamed to `mycode.md` with identical bytes
- **THEN** the recorded hash differs

### Requirement: Each scenario records the prompt it actually loaded
Seeding a file is not loading it — and inferring from seeds is not observing. `ScenarioResult` SHALL record `prompt_name` and `prompt_source` from the session's own recorded prompt (the `session-evidence` capability's readback), mapped as follows:

1. WHEN the readback's source is `built_in`, the recorded source SHALL be `stock`, and the name the readback's name.
2. WHEN the readback's source is `user_file` and the scenario's own file placements provide that name under `work:.zerostack/prompts/`, the source SHALL be `scenario` — the scenario's placement lands last, so it is the content that loaded.
3. WHEN the readback's source is `user_file` and the pack provides that name, the source SHALL be `pack`.
4. WHEN the readback's source is `user_file` and neither layer provides that name, the source SHALL be `unknown` and the run SHALL warn: a user file the harness did not plant means the trial environment is not what the harness thinks it is.
5. WHEN no session in the trial recorded a prompt at all (a `ZS_BIN` predating the readback), the source SHALL be `unknown` with an empty name, and the run SHALL warn, naming the `ZS_BIN` rebuild as the likely fix.

These are scenario-level facts, constant across a scenario's trials: each trial's readback is reconciled, and WHEN the trials of one scenario disagree, the scenario SHALL record `unknown` and the run SHALL warn — disagreement between identically-seeded trials is itself evidence something is wrong.

#### Scenario: A scenario naming a prompt the pack provides
- **WHEN** a scenario declares `prompt = "code"`, the pack provides `code.md`, and the session records `{"name": "code", "source": "user_file"}`
- **THEN** its recorded name is `code` and its source is `pack`

#### Scenario: A scenario naming a prompt the pack does not provide
- **WHEN** a scenario declares `prompt = "ask"`, the pack provides only `code.md`, and the session records `{"name": "ask", "source": "built_in"}`
- **THEN** its recorded name is `ask` and its source is `stock`

#### Scenario: A scenario that seeds its own prompt
- **WHEN** a scenario seeds `work:.zerostack/prompts/code.md`, the pack also provides `code.md`, and the session records `{"name": "code", "source": "user_file"}`
- **THEN** its source is `scenario`

#### Scenario: A session without a recorded prompt records unknown, loudly
- **WHEN** a trial's sessions carry no `prompt` field
- **THEN** the scenario records source `unknown` with an empty name, and the run warns that `ZS_BIN` likely predates prompt recording

### Requirement: The derivation survives as a cross-check
The seed-based derivation (scenario `prompt` field, else the target config's `default_prompt`, else zerostack's `code` fallback; layered scenario-over-pack-over-stock) SHALL still be computed for session-backed scenarios, and WHEN it disagrees with the readback-mapped value, the readback SHALL win and the run SHALL warn, naming both values. This is where a pack prompt whose bytes equal the built-in surfaces benignly: upstream classifies it `built_in` by content, the recorded source is `stock`, and the warning explains the disagreement instead of either value being silently wrong. For loop-mode scenarios, which produce no session file, the derivation SHALL remain the recorded value under the old rules, including recording `unknown` when the scenario seeds the effective config.

#### Scenario: Derivation disagreement warns but does not override
- **WHEN** the derivation says `pack` but the session records `{"name": "code", "source": "built_in"}` because the pack's `code.md` is byte-identical to the built-in
- **THEN** the scenario records source `stock` and the run warns, naming both the derived and read-back values

#### Scenario: A loop-mode scenario still derives
- **WHEN** a `mode = "loop"` scenario runs (no session file is written)
- **THEN** its prompt name and source are derived exactly as before, and a config-seeding loop scenario records `unknown`

### Requirement: A pack that never loads is reported
A run SHALL report, on its own output, how many scenarios actually loaded a prompt from the pack. WHEN a pack was given and no scenario resolved to source `pack`, the run SHALL warn that the pack was seeded but never loaded, so a report advertising a pack cannot be read as that pack's score when it is the built-in prompts' score. This is a run-level check about whether the independent variable was manipulated at all, distinct from the cross-run checks in the `controlled-variables` capability.

#### Scenario: A pack no scenario calls warns
- **WHEN** a run uses a pack providing only `my-code.md` and no scenario resolves to that name
- **THEN** the run warns that the pack was seeded but never loaded

#### Scenario: A partially-used pack is visible per scenario
- **WHEN** a pack is loaded by some scenarios and not others
- **THEN** no run-level never-loaded warning fires, and each scenario's own recorded source distinguishes which were measured against the pack

### Requirement: Unknown is a recorded state, not a read-side default
`prompt_source` SHALL keep `unknown` as a distinct recorded value, never conflated with `stock`: a run that could not observe which prompt a scenario loaded records `unknown`, which is not the same fact as observably using the built-ins. The read-side escape hatches this requirement originally mandated (every field added here deserializing with a default so a pre-field report still loads) are retired by `report-strict-read`: a report lacking any of these fields is a load error naming the field. `REPORT_SCHEMA_VERSION` SHALL NOT be bumped and no code SHALL branch on it.

#### Scenario: A pre-field report is a load error
- **WHEN** a report written before these fields is deserialized
- **THEN** loading fails naming the missing field (see `report-strict-read`), rather than loading with defaults

#### Scenario: Unobserved is not the same as stock
- **WHEN** a run cannot observe which prompt a scenario loaded (e.g. a zerostack build predating the session `prompt` record)
- **THEN** that scenario records `prompt_source: "unknown"`, distinguishable from a run that observably used the built-ins and recorded `stock`
</content>
