# scenario-launch-flags

## ADDED Requirements

### Requirement: A scenario selects the permission mode its target launches with
`scenario.toml` SHALL accept a `security_mode` field whose values mirror zerostack's permission-mode flags verbatim: `yolo`, `standard`, `restrictive`, `read-only`, `guarded`, `accept-all`, `dangerously-skip-permissions`. The assembled invocation SHALL carry `--<value>` for every value except `standard`, which SHALL carry no permission flag. The field SHALL default to `yolo`, so a scenario that does not declare it launches with today's exact argument list. The mapping SHALL apply identically to the `-p` and `--loop` assembly paths and to every turn of a multi-turn scenario.

#### Scenario: Default preserves the current invocation
- **WHEN** a scenario.toml does not declare `security_mode`
- **THEN** the assembled arguments contain `--yolo`, byte-identical to the pre-change argument list

#### Scenario: Read-only mode reaches the argument list
- **WHEN** a scenario declares `security_mode = "read-only"`
- **THEN** the assembled arguments contain `--read-only` and no other permission flag

#### Scenario: Standard mode emits no permission flag
- **WHEN** a scenario declares `security_mode = "standard"`
- **THEN** the assembled arguments contain none of zerostack's six permission flags

#### Scenario: Loop assembly honors the same mode
- **WHEN** a `mode = "loop"` scenario declares a non-default `security_mode`
- **THEN** the `--loop` invocation carries the same permission flag the `-p` path would

#### Scenario: An unknown mode value is a load error
- **WHEN** a scenario declares `security_mode = "readonly"` (or any value outside the seven)
- **THEN** loading fails naming the field, before any trial runs

### Requirement: A scenario passes extra CLI arguments verbatim
`scenario.toml` SHALL accept a `cli_args` array of strings. The tokens SHALL be appended to the assembled invocation verbatim and in order, after every harness-owned argument (including conditional `--load-prompt`, `--continue`, and `--loop-*` arguments) and immediately before the turn message, in both the `-p` and `--loop` assembly paths. An absent or empty `cli_args` SHALL leave the invocation unchanged.

#### Scenario: A flag with a separate value rides as two tokens
- **WHEN** a scenario declares `cli_args = ["--quick-model", "fast"]`
- **THEN** the assembled arguments contain `--quick-model` followed by `fast`, placed after the harness-owned flags and before the turn message

#### Scenario: Ordering is stable across turns
- **WHEN** a multi-turn scenario with `cli_args` reaches a turn that adds `--continue`
- **THEN** the extra tokens still follow `--continue` and still precede that turn's message

### Requirement: Harness-owned flags are rejected at scenario load
Every dash-prefixed `cli_args` token, after stripping any `=value` suffix, SHALL be checked against the set of harness-owned flags: `--loop`, `--log-file`, `--load-prompt`, `--no-color`, `--pure-stdout`, `--loop-max`, `--loop-run`, the six permission-mode flags, and every spelling zerostack accepts for one of these: `-p`/`--print`, `-c`/`--continue`, `-R`/`--restrictive`. A token of the form `-<chars>` SHALL additionally be read as a possible short cluster, since zerostack's parser splits one (`-nR` arrives as `-n -R`): a cluster containing the short form of a harness-owned flag SHALL be rejected. A collision SHALL fail scenario load with an error naming the scenario path, the offending token, and the harness-owned flag it collides with, so both `list` and `run` refuse before any trial spends money. Permission-mode flags SHALL be expressible only through `security_mode`.

#### Scenario: Smuggled permission flag fails at load
- **WHEN** a scenario declares `cli_args = ["--yolo"]`
- **THEN** loading fails naming the scenario path and `--yolo`, and no trial runs

#### Scenario: An alias spelling is rejected like the flag itself
- **WHEN** a scenario declares `cli_args = ["--print"]` or `cli_args = ["-c"]`
- **THEN** loading fails naming the scenario path and the offending token

#### Scenario: A short cluster cannot smuggle a permission flag
- **WHEN** a scenario declares `cli_args = ["-nR"]`
- **THEN** loading fails naming the scenario path, `-nR`, and `-R`

#### Scenario: The equals form does not evade the denylist
- **WHEN** a scenario declares `cli_args = ["--log-file=/tmp/x"]`
- **THEN** loading fails naming `--log-file`

#### Scenario: Non-colliding flags load cleanly
- **WHEN** a scenario declares `cli_args = ["--no-context-files"]`
- **THEN** the scenario loads and the token reaches the assembled arguments

### Requirement: Launch flags are part of the scenario's identity
Because `content_hash` covers the raw TOML source, an edit to `security_mode` or `cli_args` SHALL change the scenario's `content_hash`, making a launch-flag edit ruler drift like any other scenario edit.

#### Scenario: Editing cli_args changes the hash
- **WHEN** a scenario's `cli_args` gains a token between two runs
- **THEN** the two runs record different `content_hash` values for that scenario

#### Scenario: Editing security_mode changes the hash
- **WHEN** a scenario's `security_mode` changes between two runs
- **THEN** the two runs record different `content_hash` values for that scenario
