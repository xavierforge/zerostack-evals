## ADDED Requirements

### Requirement: Unknown scenario fields are load-time errors at every layer
Deserializing a `scenario.toml` SHALL reject unknown fields at all three layers: the top-level `Scenario` struct, the named nested structs (`FileSeed`, `LoopCfg`, `MemorySeed`, `NoteSeed`, `McpSeed`, `McpServerSeed`), and the table form of the untagged enums (`Task`/`Turn`). A rejected scenario fails `zseval list` and `run` at load, before any trial spends money. The load error SHALL carry the scenario's path.

The dangerous layer is the untagged one: before this change, a typo'd `new_session` key was silently dropped and the field defaulted to `false`, rewriting what `session-fresh-forgets` measures (isolation → continuation) while staying green. That is an assert-level false pass, worse than a lenient assert.

#### Scenario: Unknown top-level key fails
- **WHEN** a scenario.toml contains a top-level key the schema does not define (e.g. `trails = 3`)
- **THEN** loading fails naming the unknown field

#### Scenario: Unknown key in a nested table fails
- **WHEN** a `[[files]]` entry carries a key other than `src`/`dest`, or a `[loop]` table carries an undefined key
- **THEN** loading fails

#### Scenario: Typo'd turn field fails instead of silently defaulting
- **WHEN** a multi-turn task contains `{ msg = "...", new_sesion = true }`
- **THEN** loading fails rather than treating the turn as `new_session = false`

#### Scenario: The in-tree suite is clean
- **WHEN** all 42 in-tree scenarios are loaded with strict deserialization enabled
- **THEN** every one loads without an unknown-field error (audited 2026-07-27: zero unknown keys in-tree)

### Requirement: Untagged-enum load errors still locate the scenario
WHEN strict deserialization of an untagged enum fails, the error presented to the user SHALL include the failing scenario's directory path, even where serde's own message degrades to "did not match any variant" without naming the offending field.

#### Scenario: Error names the scenario
- **WHEN** a turn table fails to match any `Turn` variant
- **THEN** the load error identifies which scenario directory failed
