# report-paths

## Purpose

Paths recorded in reports are shareable: working-directory-relative, forward-slashed, never absolute.

## Requirements

### Requirement: Recorded run directories are relative
`TrialResult.run_dir` MUST be recorded relative to the working directory with forward slashes, falling back to the basename when the path cannot be made relative. Absolute local paths MUST NOT appear in reports intended for `baselines/`.

#### Scenario: A run under the working directory records a relative path
- **WHEN** a trial writes its result for a run directory under the working directory
- **THEN** the stored `run_dir` is relative, forward-slashed, and does not start with `/`

#### Scenario: Regrade resolves the relative path
- **WHEN** `regrade` reads a report whose `run_dir` is working-directory-relative
- **THEN** it locates the run directory correctly from the working directory

### Requirement: Recorded judge file paths are relative
The report's `judge_file` MUST be recorded working-directory-relative with forward slashes; a card outside the working directory records only its file name.

#### Scenario: A card outside the working directory leaks no path
- **WHEN** `--judge /somewhere/private/x.toml` names a card outside the working directory
- **THEN** the report records only `x.toml`
