# 1. compare always warns; matrix owns MULTI-VAR

## Status

Accepted (2026-07-27, `trustworthy-numbers` change, section S6).

## Context

`zseval compare`'s exit code is the loop-engineering API: CI reads it to
decide red or green (`0` clean, `1` regression, `2` nothing comparable —
see `compare.rs`'s module doc). Over several changes, `compare` grew warnings
one at a time, each carrying its own ad hoc justification for why it must not
touch the exit code: `target_mismatch` and `pack_mismatch` each got a
"never affects `exit_code`" comment; `evidence_warnings`, `definition_changed`,
and `low_resolution` each repeated the pattern; the design note for
`pack_mismatch` read "compare treats the build as always moved, for now"
specifically because no report yet recorded a build identity to check
against. This section (`trustworthy-numbers` S6) adds two more warnings —
budget truncation and a zerostack build mismatch (`zs_mismatch`), now
possible because `report-zs-identity` (S3) put `zs_bin_sha256` on every
report — which would have been a sixth and seventh restatement of the same
rule. The maintenance debt was never any one warning's behavior; it was that
the rule existed once per warning instead of once.

## Decision

**`compare`'s exit code answers only the gate question — 0 clean, 1
regression, 2 nothing comparable — as a pure function of the comparison rows.
Every fact that weakens or invalidates that answer is a warning, uniformly,
with no exceptions and no per-warning escalation flags.**

This is now enforced structurally, not by convention:

1. **`exit_code()` stays a pure function of `rows`/`errored`/`regressions`.**
   Its signature admits no warning input. This section's acceptance criterion
   was that its body is untouched by adding the two new warnings — verified
   by diff, not by inspection.
2. **An invariant test constructs a `Comparison` with every warning kind lit**
   (definition changed, evidence, low resolution, target mismatch, pack
   mismatch, build mismatch, truncation) **and asserts the exit code equals
   the all-quiet value.** A future warning extends this one test, never adds
   a new convention to remember.
3. **Warnings render through one block, in one fixed order.** Adding a
   warning is adding a list entry to that block, not deciding where in
   `print_human` a new `if` belongs.

The two new warnings this section adds, under the same policy as every
existing one:

- **Budget truncation**: warns when either side (or both) recorded
  `budget_truncated`, naming the side(s). The truncated side's missing
  scenarios already surface in `added`/`removed`; the warning supplies the
  *why* — that side's denominator is smaller than it looks.
- **`zs_mismatch`**: warns when `zs_bin_sha256` differs between baseline and
  candidate, naming both identities (version string + short hash). Same
  version string with a different hash still warns — that is exactly the
  07-26 stale-binary incident (a binary printing `zerostack 1.7.1` one
  version behind its checkout) this warning exists to catch mechanically
  instead of by luck. This retires `controlled-variables`' former "compare
  treats the build as always moved, for now": the build is now an observed
  variable like target and pack, not an assumed one.

`pack_mismatch` itself is unchanged by this: it still marks every pack
difference unconditionally, but no longer because the build is *unknown* —
now because relaxing it (same-build pack difference = clean single-variable
experiment) is a deliberate Non-Goal pending Day-2 calibration data, not a
capability gap.

**Reserved, not built**: a future exit code `3` ("experiment invalid") as a
single aggregate predicate over the warning set, evaluated inside
`exit_code()`. This is deliberately not built now — there is no consumer
until a CI gate exists that wants to treat "too many comparability threats"
as its own red, distinct from "no regressions found." When that consumer
exists, it gets one predicate in one place, not a per-warning escalation
flag threaded back through years of warnings that were designed not to need
one.

`matrix` is explicitly out of this ADR's scope for MULTI-VAR: `matrix`
already owns its own multi-variable mark (`Column::multi_variable`,
rendered as `MULTI-VAR` in the legend) for comparing N targets side by side,
which is a different question from `compare`'s two-sided regression gate.
This ADR governs `compare`'s warnings only; `matrix`'s columnar view keeps
its own mark under its own rules.

## Consequences

- Adding a comparability warning to `compare` in the future is: add a field
  to `Comparison`, compute it in `compare()`, add one entry to the fixed-order
  render block, add one assertion to the invariant test. It is never: decide
  whether this warning should move the exit code (the policy already answers
  that — no), and never write a new "never affects `exit_code`" comment,
  because the invariant test is the enforcement, not the comment.
- `exit_code()`'s purity is now a tested property, not a habit. A change that
  makes `exit_code()` read a warning field breaks the invariant test, by
  design.
- The exit-3 reservation means a future PR proposing "warnings should fail CI
  too" has a named place to land (one predicate inside `exit_code()`) instead
  of reopening the per-warning question.
