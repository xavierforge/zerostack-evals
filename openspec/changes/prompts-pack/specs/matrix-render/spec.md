## MODIFIED Requirements

### Requirement: Column identity and the legend
A column SHALL be labelled in the header by its target stem. The full
provider/model, the target path, the judge tri-state (unknown / nothing-graded
/ the listed rulers), and the prompt pack identity (its path with a short
content hash, or a plain marker when the column used no pack) SHALL appear in
the legend rather than the header. A report that carries no target identity
SHALL NOT be a column: `matrix` SHALL exit 2 naming it.

#### Scenario: The header is the stem, the legend has the rest
- **WHEN** a report with target `targets/opus.toml` is a column
- **THEN** the header shows `opus` and the legend shows its full provider/model, path, judge tri-state, and pack identity

#### Scenario: A report without a target identity is rejected
- **WHEN** a report has no target identity
- **THEN** `matrix` exits 2 naming that report as one it cannot render as a column

#### Scenario: Two columns of the same pack path with different contents
- **WHEN** two columns record the same pack path but different pack hashes
- **THEN** their legend lines differ in the displayed short hash

## ADDED Requirements

### Requirement: A column is marked when the pack moved alongside the target
`matrix`'s own axis is the target, so columns differing only by target are the
table working as intended, and columns sharing a target and differing only by
pack are equally a clean single-variable comparison. WHEN the columns differ in
both target and pack identity, the affected column(s) SHALL be marked, because
no cell difference can be attributed to either. This mark SHALL be distinct
from DRIFT, which stays reserved for a changed ruler, and SHALL appear in the
legend beside the existing per-column marks. Like SPREAD and DRIFT it is a
display heuristic that says "look here", never a verdict about which column is
correct.

#### Scenario: Same target, different packs is not marked
- **WHEN** two columns share a target and differ only by pack
- **THEN** neither column is marked for multiple variables, and their packs are visible in the legend

#### Scenario: Different targets and different packs is marked
- **WHEN** columns differ in both target and pack identity
- **THEN** the affected columns are marked, and the mark is not DRIFT

#### Scenario: Different targets, one shared pack is not marked
- **WHEN** columns differ by target and all record the same pack identity
- **THEN** no column is marked for multiple variables
