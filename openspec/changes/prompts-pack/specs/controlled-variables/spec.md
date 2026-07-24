## ADDED Requirements

### Requirement: A comparison varies exactly one independent variable
An eval comparison is a controlled experiment: the scores are the dependent
variable, and exactly one thing about the subject may deliberately vary.
Subject variables are the zerostack build, the target (provider and model), and
the prompt pack. WHEN two or more of them differ across the sides of a
comparison, the comparison SHALL be marked, because the score difference cannot
be attributed to any one of them. WHEN one or none differ, the comparison is
clean and SHALL NOT be marked.

Rulers are not subject variables and are not counted here: the scenario
definition (`content_hash`) and the judge (`judge_hash`) are the instruments
that produce the reading, so a change in either invalidates comparability
outright and is marked unconditionally by the existing rules. A mark SHALL
name what moved and SHALL NOT adjudicate which side is correct, consistent with
how DRIFT and the existing different-targets note already behave.

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
`compare` and `matrix` SHALL display each side's or column's pack identity as
its path together with a short form of its content hash, on the same identity
lines that already carry the model, target, and judge. Displaying the
fingerprint, not only the path, is what makes two runs of the same pack path
with different contents distinguishable by eye; without it, an invisible
difference would need a warning of its own.

#### Scenario: The identity line shows path and fingerprint
- **WHEN** a run used `--prompts my-pack/`
- **THEN** its identity line shows the pack path and a short hash, for example `prompts=my-pack#a3f1`

#### Scenario: Same path, different contents are distinguishable
- **WHEN** two runs used the same pack path but its contents changed between them
- **THEN** their identity lines differ in the displayed fingerprint

#### Scenario: No pack is shown as no pack
- **WHEN** a run used no pack
- **THEN** its identity line shows that plainly rather than omitting the field

### Requirement: compare counts the pack as a moved variable until the build is recorded
`compare`'s premise is that its two sides differ by a rebuilt zerostack, but the
report does not yet record any zerostack identity, so `compare` cannot observe
whether the build actually moved. Until it can, `compare` SHALL treat the build
as always moved, and SHALL therefore mark any comparison whose pack identity
differs, in the same shape and for the same reason as the existing
different-targets note: the diff is a prompt A/B, not a regression. The note
SHALL NOT change `compare`'s exit code, which continues to reflect regressions
only.

#### Scenario: A pack difference is noted
- **WHEN** `compare` is given a baseline and a candidate whose pack identities differ
- **THEN** it prints a note that the diff reflects a prompt change and is not a regression check

#### Scenario: The note does not move the exit code
- **WHEN** a pack difference is noted and no scenario regressed
- **THEN** `compare` still exits 0

#### Scenario: Identical packs are not noted
- **WHEN** both sides record the same pack path and hash, or neither used a pack
- **THEN** no pack note is printed
