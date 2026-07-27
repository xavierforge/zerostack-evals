## MODIFIED Requirements

### Requirement: The footer is recomputed over the common scenario set
The footer (pass@k, pass^k, total cost) SHALL be computed over the scenarios present and gradable in every column, so columns are compared on the same denominator. WHEN the common set is smaller than some column's own set, the table SHALL note which scenarios were excluded from the footer.

The footer SHALL render three metric groups — regression, capability, and overall, in that order — each computed over the common gradable set filtered to that kind (overall over the whole common set, unchanged in definition from before). A kind with no gradable scenario in the common set renders its rates as `n/a`, the same convention as a report's own summary. The grouping adds footer rows only, never width: the fixed-width budget (48 + 12N) is untouched.

#### Scenario: Different scenario sets use the intersection
- **WHEN** two columns ran different scenario sets
- **THEN** the footer is computed over their common gradable scenarios and the excluded scenarios are listed

#### Scenario: Identical suites match each report's own summary
- **WHEN** all columns ran the same suite
- **THEN** the footer equals each column's own report summary and nothing is excluded

#### Scenario: Footer groups by kind
- **WHEN** the common set contains both kinds
- **THEN** the footer shows regression pass@k/pass^k, capability pass@k/pass^k, and overall pass@k/pass^k as separate row groups, regression first, overall last

#### Scenario: A kind absent from the common set is n/a
- **WHEN** the common gradable set contains no capability scenario
- **THEN** the capability footer rows render `n/a` for every column

## ADDED Requirements

### Requirement: Rows are grouped by kind
The scenario rows SHALL render in two sections — regression first, then capability — with a section marker between them, reading each scenario's `kind` from the report rows (never from scenario.toml). Within a section, row order follows the existing ordering. JSON output carries `kind` per row and keeps its existing flat array shape; the sectioning is a rendering concern.

#### Scenario: Two sections in kind order
- **WHEN** a matrix renders reports containing both kinds
- **THEN** regression rows appear first under a regression marker, capability rows after under a capability marker

#### Scenario: JSON stays flat
- **WHEN** `matrix --json` renders the same reports
- **THEN** rows form one array, each row carrying its `kind`, with no section nesting
