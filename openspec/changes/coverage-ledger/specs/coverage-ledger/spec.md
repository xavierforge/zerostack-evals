## ADDED Requirements

### Requirement: The ledger's denominator is zerostack's functional surface
`scenarios/coverage.toml` SHALL enumerate 14 areas of zerostack's functional surface as an ordered array of tables, each area carrying a `name`, a human `title`, and one or more claims. The area set is exactly: `permission`, `sandbox`, `tool-use`, `print-mode`, `session`, `memory`, `subagents`, `prompts`, `providers`, `hooks`, `context-window`, `worktree`, `mcp`, `loop` — the 12 carved out in `scenarios/PLAN.md` on 2026-07-16, plus `mcp` and `loop`, which already have scenarios. The denominator is the product's surface, never the set of scenarios that happen to exist: an area with no scenario at all is the ledger's most important row, and a suite-derived denominator cannot express it.

The ledger SHALL say `area`, never `domain`. `domain` is already load-bearing and differently defined in `scenario.toml`'s `domains = [...]` and `domains/mod.rs`'s `KNOWN_DOMAINS` (seed sugar and drift-check opt-in), and three names — memory, subagents, mcp — exist in both vocabularies meaning different things.

File order SHALL be presentation order; the loader preserves it and adds no sorting.

#### Scenario: The ledger declares all 14 areas
- **WHEN** `scenarios/coverage.toml` is loaded
- **THEN** it yields 14 areas whose names are exactly the set above, in file order

#### Scenario: Unknown keys are a load error
- **WHEN** a ledger carries an unknown key at the top level, on an area, or on a claim
- **THEN** loading fails naming the unknown key

#### Scenario: Header fields are required
- **WHEN** a ledger omits `audited_against` or `scenario_roots`
- **THEN** loading fails naming the missing field

### Requirement: Four claim statuses, each with its own required evidence
Every claim SHALL carry a `claim` string and a `status` of exactly one of `covered`, `uncovered`, `product-blocked`, `excluded`. Each status carries the evidence that status owes, and a wrong combination SHALL be a load-time error rather than a silently ignored field:

- `covered` requires a non-empty `scenarios` array of scenario ids.
- `uncovered` takes an optional `blocked_by`: present means a harness gap blocks the claim and the sentence names it in full; absent means the claim is buildable today and simply unbuilt. The presence or absence of the field is itself the fact being recorded.
- `product-blocked` requires a one-line `reason` describing the hole on the zerostack side, and takes an optional `zs` pointer.
- `excluded` requires a one-line `reason` for never testing this.

Any claim MAY carry a free-text `note`. Prose fields (`blocked_by`, `reason`, `note`) name their subject in full and SHALL NOT reference numbering that lives in a planning document: gap letters and wave numbers are re-planned regularly, and a ledger of facts cannot point at a moving index.

There is deliberately no `planned` status and no wave or schedule field. Scheduling lives in `scenarios/PLAN.md`; the ledger records only what is and is not measured today.

#### Scenario: Covered without scenarios fails to load
- **WHEN** a claim declares `status = "covered"` with no `scenarios` array, or an empty one
- **THEN** loading fails naming the claim

#### Scenario: Excluded without a reason fails to load
- **WHEN** a claim declares `status = "excluded"` with no `reason`
- **THEN** loading fails naming the claim

#### Scenario: Evidence belonging to another status fails to load
- **WHEN** a claim declares `status = "uncovered"` together with a `scenarios` array
- **THEN** loading fails naming the unexpected field

#### Scenario: An unknown status fails to load
- **WHEN** a claim declares `status = "planned"`
- **THEN** loading fails naming the invalid status

#### Scenario: Uncovered records blockage by presence
- **WHEN** one uncovered claim carries `blocked_by` and another omits it
- **THEN** both load, and the loaded claims differ in exactly that field

### Requirement: `covered` is an existence claim
`covered` SHALL mean "a scenario in this repo tests this claim" and nothing more. It does not assert that the scenario passes: a scenario scoring 0% is still coverage, and whether a low score is a problem or a measurement is `kind`'s question, already answered per scenario. It does not assert that the scenario ran in any particular report: run membership is a property of a report, computed by whoever renders one, and recording it in the ledger would make the ledger stale on every run.

The ledger SHALL therefore carry no pass rate, no score, no run reference, and no timestamp per claim.

#### Scenario: A failing scenario still counts as coverage
- **WHEN** a claim's scenario scored 0% in the latest report
- **THEN** the claim's status in the ledger is unchanged

#### Scenario: The ledger holds no per-claim run data
- **WHEN** any claim is loaded
- **THEN** it exposes no field derived from a report

### Requirement: A scenario id backs at most one covered claim
Across the whole ledger, a scenario id SHALL appear in at most one `covered` claim. Deliberate calibration overlaps — scenarios probing the same mechanism at different layers, which `scenarios/PLAN.md` suite policy 3 requires be counted once — are recorded as prose in `note` and take no coverage slot. A scenario that genuinely supports two claims is assigned to one of them by the author.

This makes per-area coverage counts mean exactly one thing: a reader seeing the same id under three claims cannot tell three units of coverage from one unit cited three times, and a renderer that silently de-duplicates hides the author's unmade decision.

#### Scenario: A duplicate id is rejected
- **WHEN** a ledger lists the same scenario id under two covered claims, in the same area or in different areas
- **THEN** the check fails naming the id and both claims

### Requirement: The ledger and the scenario tree are checked in both directions
`scenario_roots` SHALL name the trees that hold scenarios (`scenarios` and `examples/prompt-pack`), and the drift check SHALL enforce both directions against them:

- Every scenario id referenced by a `covered` claim exists as a loadable scenario under those roots. A renamed or deleted scenario leaves a dead reference, and a ledger citing evidence that no longer exists is worse than one citing none.
- Every scenario under those roots is referenced by exactly one `covered` claim. An unclaimed scenario means the ledger has quietly fallen behind the suite, which is the failure mode that makes a coverage page lie.

Failures SHALL name every offending id, in both directions, in one report rather than one per run.

#### Scenario: A dead reference fails the check
- **WHEN** the ledger cites a scenario id that exists under no root
- **THEN** the check fails naming that id

#### Scenario: An unclaimed scenario fails the check
- **WHEN** a new scenario is added to the tree and no claim cites it
- **THEN** the check fails naming that id

#### Scenario: The in-tree suite passes both directions
- **WHEN** the check runs against the repo's own `scenarios/` and `examples/prompt-pack/`
- **THEN** it passes, with every in-tree scenario claimed exactly once

### Requirement: The check is enforced by the test suite, not by a CLI surface
The drift check SHALL run as part of `cargo test --workspace` against the repo's real scenario tree, and the change SHALL add no `zseval` subcommand, flag, or README section for it. The ledger serves this repo's own honesty; its only renderer is `zseval site`. CI already runs `cargo test --workspace` on every pull request, so enforcement needs no workflow change.

No run path reads the ledger: a malformed ledger SHALL NOT be able to fail a `zseval run`.

#### Scenario: Drift turns the test suite red
- **WHEN** the ledger and the scenario tree disagree in either direction
- **THEN** `cargo test --workspace` fails

#### Scenario: A malformed ledger does not break a run
- **WHEN** `scenarios/coverage.toml` is malformed
- **THEN** `zseval run` over the same tree is unaffected

### Requirement: `audited_against` is compared by containment, never by parsing
`audited_against` SHALL record the zerostack version string the ledger's judgments were made against, and the loader SHALL expose a containment test that answers whether that string appears in a recorded `--version` banner. It SHALL NOT parse the banner. `backend.rs` stores the banner's first line verbatim precisely so upstream's banner shape never becomes a compatibility contract, and `zs_bin_sha256` is already the machine-comparable identity; a version parser here would sign that contract on the ledger's behalf.

The failure direction is deliberate: if upstream reshapes its banner, the worst outcome is a spurious mismatch notice, never a wrong version claim and never a blocked publish.

#### Scenario: Matching version is contained in the banner
- **WHEN** the ledger records `1.7.2` and a report's banner is `zerostack 1.7.2`
- **THEN** the containment test reports a match

#### Scenario: A newer zerostack does not match
- **WHEN** the ledger records `1.7.2` and a report's banner is `zerostack 1.7.4`
- **THEN** the containment test reports a mismatch

#### Scenario: Mock identity does not match
- **WHEN** the ledger records `1.7.2` and a report's banner is `mock`
- **THEN** the containment test reports a mismatch
