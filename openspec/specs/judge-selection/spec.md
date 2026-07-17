# judge-selection

## Purpose

The CLI contract for choosing a judge: explicit, single, and mandatory when a suite needs grading.

## Requirements

### Requirement: A rubric suite requires an explicit judge decision
When the discovered suite contains at least one scenario with an LLM rubric (`judge` field set), the CLI MUST require exactly one of `--judge <file>` or `--no-judge`. When neither is given, `run` and `regrade` MUST exit 2 before any trial starts, with an error naming both options. There SHALL be no built-in default judge configuration.

#### Scenario: Rubric suite with neither flag fails fast
- **WHEN** `zseval run` targets a suite containing a rubric scenario and neither `--judge` nor `--no-judge` is given
- **THEN** the process exits 2 before any trial runs, and the error names `--judge` and `--no-judge`

#### Scenario: A suite without rubrics needs no judge decision
- **WHEN** `zseval run` targets a suite with no rubric scenarios and neither flag is given
- **THEN** the run proceeds normally and the report records no judge (`judge_file` empty)

#### Scenario: Explicit --no-judge on a rubric suite is honored
- **WHEN** `zseval run --no-judge` targets a suite containing rubric scenarios
- **THEN** the run proceeds, rubric grading is skipped with the recorded reason, and no judge key is required

### Requirement: The judge choice is single and unambiguous
`--judge` MUST be single-arity (given at most once) and MUST conflict with `--no-judge`.

#### Scenario: Two --judge flags are a usage error
- **WHEN** `--judge a.toml --judge b.toml` is given
- **THEN** the process exits 2 with a usage error

#### Scenario: --judge with --no-judge is a usage error
- **WHEN** both `--judge x.toml` and `--no-judge` are given
- **THEN** the process exits 2 with a usage error
