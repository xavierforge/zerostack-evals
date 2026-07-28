## ADDED Requirements

### Requirement: Report-family JSON is read as strictly as it is written
Deserializing report-family JSON (`Report`, `ScenarioResult`, `TrialResult`) SHALL fail on a missing field, with an error naming it. The legacy `#[serde(default)]` escape hatches — added field by field so pre-field committed baselines could still load — are removed along with the only artifact that needed them (`baselines/main.json`, superseded: it measured pre-fix zerostack and Day 2 regenerates against v1.7.2).

The identity fields make the strict stance load-bearing rather than aesthetic: after hard-fail capture, no runtime path produces an identity-less report, so a read-side default would manufacture a state reachable only by reading foreign files — and the build-mismatch comparison would need a third truth value to absorb it. Fields whose default *value* is also a legitimate runtime value (e.g. `judge_file = ""` meaning "no judge named") keep that value's meaning; only the deserialization escape hatch goes.

#### Scenario: A missing field is a load error
- **WHEN** `compare` or `matrix` is given a report JSON lacking any schema field (e.g. the 07-21 baseline, which predates the identity fields)
- **THEN** loading fails with an error naming the missing field, rather than silently defaulting

#### Scenario: Current reports round-trip
- **WHEN** a report written by the current binary is read back by `compare` or `matrix`
- **THEN** it loads without error — serialization always emits every field

### Requirement: compare stops special-casing the unknown hash
`compare`'s scenario-definition check SHALL treat differing `content_hash` values as changed, by plain inequality. The empty-hash skip branch ("old baseline predating this field — unknown, skip") loses its referent once no pre-field artifact can load; current runs always record a non-empty hash.

#### Scenario: Definition drift still warns
- **WHEN** a shared scenario's `content_hash` differs between sides
- **THEN** the definition-changed warning names it, unchanged

### Requirement: judge_model has two live states, not three
With strict reading, `judge_model`'s "absent" state becomes a load error; the remaining live states are `null` (**unknown** — the fact was not observed) and a list (`[]` = nothing was graded; non-empty = the rulers). README wording SHALL match: absent is an error, only `null` means unknown.

#### Scenario: Absent judge_model is rejected
- **WHEN** a report JSON lacks the `judge_model` key entirely
- **THEN** loading fails

#### Scenario: Null and empty stay distinct
- **WHEN** a report records `"judge_model": null` versus `"judge_model": []`
- **THEN** downstream rendering distinguishes unknown from nothing-graded, unchanged
