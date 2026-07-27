# controlled-variables

## Purpose

An eval comparison is a controlled experiment: `compare` and `matrix` mark a comparison whenever more than one subject variable (zerostack build, target, or prompt pack) moved between its sides, warn when a build differs or a side was budget-truncated, and display each side's pack and build identity, so a score difference is never silently attributed to the wrong cause. Warnings never move the exit code, which answers only the regression-gate question.

## Requirements

### Requirement: A comparison varies exactly one independent variable
An eval comparison is a controlled experiment: the scores are the dependent variable, and exactly one thing about the subject may deliberately vary. Subject variables are the zerostack build, the target (provider and model), and the prompt pack. WHEN two or more of them differ across the sides of a comparison, the comparison SHALL be marked, because the score difference cannot be attributed to any one of them. WHEN one or none differ, the comparison is clean and SHALL NOT be marked.

Rulers are not subject variables and are not counted here: the scenario definition (`content_hash`) and the judge (`judge_hash`) are the instruments that produce the reading, so a change in either invalidates comparability outright and is marked unconditionally by the existing rules. A mark SHALL name what moved and SHALL NOT adjudicate which side is correct, consistent with how DRIFT and the existing different-targets note already behave.

#### Scenario: Only the pack differs
- **WHEN** two runs share a target and differ only in their pack
- **THEN** the comparison is not marked for multiple variables: varying the pack is the experiment

#### Scenario: Target and pack both differ
- **WHEN** two runs differ in both target and pack
- **THEN** the comparison is marked, naming both as having moved

#### Scenario: A changed ruler is marked regardless
- **WHEN** the judge or a scenario definition differs
- **THEN** the existing ruler marks apply independently of the variable count

### Requirement: Pack identity is displayed wherever it can differ
`compare` and `matrix` SHALL display each side's or column's pack identity as its path together with a short form of its content hash, on the same identity lines that already carry the model, target, and judge. Displaying the fingerprint, not only the path, is what makes two runs of the same pack path with different contents distinguishable by eye; without it, an invisible difference would need a warning of its own.

#### Scenario: The identity line shows path and fingerprint
- **WHEN** a run used `--prompts my-pack/`
- **THEN** its identity line shows the pack path and a short hash, for example `prompts=my-pack#a3f1`

#### Scenario: Same path, different contents are distinguishable
- **WHEN** two runs used the same pack path but its contents changed between them
- **THEN** their identity lines differ in the displayed fingerprint

#### Scenario: No pack is shown as no pack
- **WHEN** a run used no pack
- **THEN** its identity line shows that plainly rather than omitting the field

### Requirement: compare warns when builds differ
WHEN the two sides of a comparison record different `zs_bin_sha256` values, `compare` SHALL warn, naming both identities (version string + short hash). Same version strings with different hashes SHALL still warn — the hash is the identity, the version string is the label. The warning SHALL NOT change the exit code.

#### Scenario: Different builds are named
- **WHEN** baseline and candidate record different `zs_bin_sha256`
- **THEN** a warning names both sides' version and short hash

#### Scenario: Same version, different build still warns
- **WHEN** both sides print `zerostack 1.7.1` but the binaries' hashes differ
- **THEN** the build-mismatch warning fires — this is the 07-26 stale-binary incident, now caught mechanically

#### Scenario: Identical builds are quiet
- **WHEN** both sides record the same `zs_bin_sha256`
- **THEN** no build warning is printed

### Requirement: A pack difference stays marked pending calibration
`compare` SHALL keep marking any comparison whose pack identity differs, in the same shape as before: the diff is a prompt A/B, not a regression check. Retained deliberately even though the build is now observable: relaxing the conservative treatment (pack-difference-with-same-build = a clean single-variable experiment) is a Non-Goal until reviewed against Day-2 baseline data. The note SHALL NOT change the exit code.

#### Scenario: A pack difference is noted
- **WHEN** `compare` is given a baseline and a candidate whose pack identities differ
- **THEN** it prints a note that the diff reflects a prompt change and is not a regression check

#### Scenario: Identical packs are not noted
- **WHEN** both sides record the same pack path and hash, or neither used a pack
- **THEN** no pack note is printed

### Requirement: compare warns when a side was budget-truncated
WHEN either side of a comparison recorded `budget_truncated`, `compare` SHALL warn, naming which side (or both). The truncated side's missing scenarios already surface in the added/removed lists; the warning supplies the cause — "this side's denominator is smaller than you think". The warning SHALL NOT change the exit code: with no regressions `compare` exits 0 with the warning on stderr; with regressions it exits 1 with both.

#### Scenario: Truncated candidate warns
- **WHEN** the candidate report has `budget_truncated = true` and no scenario regressed
- **THEN** `compare` exits 0 and warns that the candidate was budget-truncated

#### Scenario: Truncation and regressions coexist
- **WHEN** the baseline was truncated and regressions are found
- **THEN** `compare` exits 1, listing the regressions and the truncation warning

### Requirement: Warnings never move the exit code
`compare`'s exit code SHALL answer only the gate question — 0 clean, 1 regressions found, 2 nothing comparable — as a pure function of the comparison rows. Every fact that weakens or invalidates that answer is a warning, uniformly, with no exceptions and no per-warning escalation flags. This policy is recorded as the repo's first ADR ("compare always warns; matrix owns MULTI-VAR"), which also reserves a future exit code 3 ("experiment invalid") as a single aggregate predicate over the warning set inside the exit-code function — to be built only when the CI gate creates a consumer.

Enforcement is structural: an invariant test SHALL construct a comparison with every warning lit and assert its exit code equals the all-quiet value. Warnings SHALL render in fixed order, split by class into two blocks: incomparability warnings (different target, prompt pack, or zerostack build — a pass-rate diff there is not a regression check) above the scenario table, and caveat warnings (truncation, changed definition, vanished evidence, low resolution — the comparison is valid but weaker than it looks) below it. Neither block is read by the exit-code function.

#### Scenario: All warnings lit, exit code unmoved
- **WHEN** a comparison carries every warning kind at once (definition changed, evidence, low resolution, target mismatch, pack mismatch, build mismatch, truncation) and no regressions
- **THEN** the exit code is 0, identical to the same comparison with no warnings

#### Scenario: A future warning inherits the policy
- **WHEN** a new comparability warning is added (e.g. the planned judge-mismatch warning)
- **THEN** it joins the invariant test and the single render block, and needs no exit-code decision of its own

### Requirement: Build identity is displayed wherever it can differ
`compare` and `matrix` SHALL display each side's or column's zerostack identity — the version string plus a short form of `zs_bin_sha256` — on the same identity lines that already carry the model, target, judge, and pack identity, in the pack identity line's shape.

#### Scenario: The identity line shows version and fingerprint
- **WHEN** a run's report records `zs_version = "zerostack 1.7.2"` and its binary hash
- **THEN** its identity line shows the version and a short hash, e.g. `zs=zerostack 1.7.2#b41c`

#### Scenario: Mock columns are identified as mock
- **WHEN** a matrix column comes from a mock-backend report
- **THEN** its legend line shows `mock` and the fixture's short fingerprint
</content>
