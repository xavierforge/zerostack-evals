# prompts-pack-identity — delta

## MODIFIED Requirements

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

## REMOVED Requirements

### Requirement: The default prompt name is derived only where derivation holds
**Reason**: The recorded value no longer comes from derivation at all — the session readback is authoritative, and it stays valid even when a scenario seeds the effective config, so the "abandon derivation, record `unknown`" branch protected against a hazard that no longer exists.
**Migration**: The derivation logic itself survives in two narrower roles, specified in "The derivation survives as a cross-check" below: as a cross-check against the readback for session-backed scenarios, and as the recorded value for loop-mode scenarios only (upstream's loop path writes no session file, so there is nothing to read back).

## ADDED Requirements

### Requirement: The derivation survives as a cross-check
The seed-based derivation (scenario `prompt` field, else the target config's `default_prompt`, else zerostack's `code` fallback; layered scenario-over-pack-over-stock) SHALL still be computed for session-backed scenarios, and WHEN it disagrees with the readback-mapped value, the readback SHALL win and the run SHALL warn, naming both values. This is where a pack prompt whose bytes equal the built-in surfaces benignly: upstream classifies it `built_in` by content, the recorded source is `stock`, and the warning explains the disagreement instead of either value being silently wrong. For loop-mode scenarios, which produce no session file, the derivation SHALL remain the recorded value under the old rules, including recording `unknown` when the scenario seeds the effective config.

#### Scenario: Derivation disagreement warns but does not override
- **WHEN** the derivation says `pack` but the session records `{"name": "code", "source": "built_in"}` because the pack's `code.md` is byte-identical to the built-in
- **THEN** the scenario records source `stock` and the run warns, naming both the derived and read-back values

#### Scenario: A loop-mode scenario still derives
- **WHEN** a `mode = "loop"` scenario runs (no session file is written)
- **THEN** its prompt name and source are derived exactly as before, and a config-seeding loop scenario records `unknown`
