## REMOVED Requirements

### Requirement: compare counts the pack as a moved variable until the build is recorded
**Reason**: The premise is retired: reports now record the zerostack build identity (`zs_bin_sha256`, see `report-zs-identity`), so `compare` can observe whether the build actually moved instead of assuming it always did.
**Migration**: The behavior this requirement mandated is retained unchanged by "A pack difference stays marked pending calibration" below — only its justification changes. Build differences get their own warning ("compare warns when builds differ"). The `compare.rs` doc comment and design notes reading "compare treats the build as always moved, for now" are updated in the same change.

## ADDED Requirements

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

Enforcement is structural: an invariant test SHALL construct a comparison with every warning lit and assert its exit code equals the all-quiet value; warnings SHALL render through one block in fixed order.

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
