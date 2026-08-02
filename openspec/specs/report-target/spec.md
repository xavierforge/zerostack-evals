# report-target

## Purpose

`Report` carries a `target` field as its column identity, path-normalized like `judge_file` and surviving detachment from its run directory, without gating any behavior on `schema_version`.

## Requirements

### Requirement: Report carries its target identity
`Report` SHALL gain a `target` field recording the target file, path-normalized the same way as `judge_file`: working-directory-relative, forward-slashed, never absolute (see the `report-paths` capability). This field is the report's column identity and SHALL survive the report being detached from its run directory, for example when copied into `baselines/`. The run-level `target.toml` holds the target's content; `Report.target` holds its identity.

#### Scenario: The target path is recorded normalized
- **WHEN** a run records its report with a `--target` under the working directory
- **THEN** `Report.target` is the working-directory-relative, forward-slashed path, never absolute

#### Scenario: An orphan report keeps its identity
- **WHEN** a report is copied away from its run directory
- **THEN** `Report.target` still names the target it was run against, so `matrix` can label the column without the run directory

### Requirement: The schema version is frozen, not gated on
`REPORT_SCHEMA_VERSION` SHALL be frozen at `1` and SHALL NOT be bumped for this change. No code SHALL branch on `schema_version`. The `target` field originally deserialized with a default so a report lacking it still loaded; `report-strict-read` has since retired that escape hatch: a missing `target` is a load error. Consumers that need a *non-empty* target (see `matrix-render`, `multi-target-run`) still check that on its own merits rather than gating on the version number.

#### Scenario: Nothing reads the version number
- **WHEN** a report is loaded
- **THEN** no code path makes a decision based on `schema_version`

#### Scenario: A report without the field still loads
- **WHEN** a report predating the `target` field is deserialized
- **THEN** it loads with an empty target rather than failing to parse
