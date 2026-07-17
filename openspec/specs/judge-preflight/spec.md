# judge-preflight

## Purpose

Fail-fast checks that run before any trial spends money, whenever a judge will be used.

## Requirements

### Requirement: A missing key fails before the first trial
When a rubric suite will be graded with `--judge`, the configured provider's key environment variable MUST be checked before any trial starts. If unset or empty, the process MUST exit 2 with an error naming the exact variable (e.g. an `export OPENROUTER_API_KEY=...` hint) and the `--no-judge` escape hatch.

#### Scenario: Unset key is a pre-run usage error
- **WHEN** `zseval run --judge card.toml` targets a rubric suite and the card's provider key variable is unset
- **THEN** the process exits 2 before any trial runs, naming the exact environment variable and `--no-judge`

### Requirement: A live dry-run proves the judge chain end to end
Before the first trial, one probe call MUST be made in the real judge shape: the same prompt template, the same fixed max_tokens, and the same temperature-fallback logic, and it MUST return a parseable verdict word. Any failure (bad auth, unknown model, rejected parameters, truncated or unparseable output, network) MUST exit 2 relaying the underlying provider error. The probe's cost is not recorded in the report.

#### Scenario: An invalid key surfaces as a real provider error before spending
- **WHEN** the key variable is set to an invalid value
- **THEN** the dry-run fails, the process exits 2 relaying the provider's authentication error, and no trial has run

#### Scenario: A mistyped model name is caught up front
- **WHEN** the card names a model the provider does not serve
- **THEN** the dry-run fails and the process exits 2 relaying the provider's error, before any trial

#### Scenario: A judge that cannot produce a parseable verdict is rejected up front
- **WHEN** the dry-run returns output with no parseable verdict word (e.g. thinking exhausted the budget)
- **THEN** the process exits 2 explaining the judge produced no verdict

### Requirement: Regrade preflights the same way
`regrade` with `--judge` MUST run the same presence check and dry-run before re-judging any trial.

#### Scenario: Regrade with a broken judge changes nothing
- **WHEN** `zseval regrade --judge card.toml` is invoked and preflight fails
- **THEN** the process exits 2 and no trial's stored verdict or artifacts have been modified
