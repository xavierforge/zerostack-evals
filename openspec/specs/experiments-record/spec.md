# experiments-record

## Purpose

A committed record of dated matrix-run snapshots, distinct from the gitignored `results/`: `experiments/` holds only distilled markdown and small config, written once by hand and never regenerated.

## Requirements

### Requirement: experiments is a committed record directory
A new `experiments/` directory with a `README.md` SHALL hold dated markdown snapshots of matrix runs. It SHALL be under version control, unlike `results/`, which is gitignored, because it holds only distilled markdown and small config, never raw transcripts.

#### Scenario: The directory is committed with a README
- **WHEN** the change lands
- **THEN** `experiments/` and `experiments/README.md` exist and are tracked in git

### Requirement: A snapshot is written once and never regenerated
A snapshot SHALL be produced by hand via redirect (for example `zseval matrix results/<run>/*/report.json --markdown > experiments/<date>-<name>.md`), the same manual ritual as `baselines/`. A snapshot is a dated record, not a cache: it SHALL NOT be regenerated after the fact, and `experiments/README.md` SHALL state this.

#### Scenario: The README states the never-regenerate rule
- **WHEN** a reader opens `experiments/README.md`
- **THEN** it states that snapshots are dated records written once and are never regenerated

### Requirement: A snapshot embeds its provenance
A snapshot SHALL embed enough to reproduce and interpret it: the date; each target's provider/model and path; the backend; the trial count; the total cost; the judge file, `judge_hash`, and `judge_model` tri-state; and each scenario's content hash. Each target's full `target.toml` SHALL be embedded from the run-directory copy, not re-read from `targets/` at render time. WHEN a column's `target.toml` content is unavailable (a report detached from its run directory), the snapshot SHALL record that column's identity from the report and note that its content is not embedded.

#### Scenario: A snapshot embeds the target content from the run copy
- **WHEN** a snapshot is produced from a live run directory
- **THEN** each target's `target.toml` is embedded from the run-directory copy rather than from the current `targets/`

#### Scenario: A detached column degrades honestly
- **WHEN** a snapshot includes a column whose `target.toml` content is no longer available
- **THEN** it records that column's identity from the report and notes the content is not embedded
