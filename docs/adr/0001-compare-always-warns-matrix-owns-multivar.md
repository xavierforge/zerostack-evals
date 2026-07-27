# 1. compare always warns; matrix owns MULTI-VAR

## Status

Accepted (2026-07-27).

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
against. Two more warnings then arrived — budget truncation and a zerostack
build mismatch (`zs_mismatch`), the latter newly possible once every report
records `zs_bin_sha256` — which would have been a sixth and seventh
restatement of the same rule. The maintenance debt was never any one
warning's behavior; it was that the rule existed once per warning instead of
once.

## Decision

**`compare`'s exit code answers only the gate question — 0 clean, 1
regression, 2 nothing comparable — as a pure function of the comparison rows.
Every fact that weakens or invalidates that answer is a warning, uniformly,
with no exceptions and no per-warning escalation flags.**

This is now enforced structurally, not by convention:

1. **`exit_code()` stays a pure function of `rows`/`errored`/`regressions`.**
   Its signature admits no warning input. The acceptance criterion when the
   two new warnings were added was that its body stayed untouched — verified
   by diff, not by inspection.
2. **An invariant test constructs a `Comparison` with every warning kind lit**
   (definition changed, evidence, low resolution, target mismatch, pack
   mismatch, build mismatch, truncation) **and asserts the exit code equals
   the all-quiet value.** A future warning extends this one test, never adds
   a new convention to remember.
3. **Warnings render in two fixed-order blocks, split by class.**
   *Incomparability* warnings (different target, prompt pack, or zerostack
   build) print **above** the scenario table, because each one inverts how the
   whole table should be read — "a pass-rate diff here is not a regression
   check" has to be seen before the scores, not several lines after them.
   *Caveat* warnings (budget truncation, changed scenario definition, vanished
   evidence, low resolution) print **below** the table: the comparison is
   valid, they only qualify how strong it is. Adding a warning is: classify it
   into one of the two blocks and add one list entry, never deciding where in
   `print_human` a new `if` belongs. Both blocks are read by nothing in
   `exit_code()`.

The two new warnings, under the same policy as every existing one:

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
  to `Comparison`, compute it in `compare()`, classify it as incomparability
  (above the table) or caveat (below) and add one entry to that block, add one
  assertion to the invariant test. It is never: decide whether this warning
  should move the exit code (the policy already answers that — no), and never
  write a new "never affects `exit_code`" comment, because the invariant test
  is the enforcement, not the comment. The one judgment a new warning does
  carry is which of the two blocks it belongs in, and that is a bounded,
  principled call (is the comparison invalid, or merely weaker than it looks?),
  not the per-warning escalation policy this ADR abolished.
- `exit_code()`'s purity is now a tested property, not a habit. A change that
  makes `exit_code()` read a warning field breaks the invariant test, by
  design.
- The exit-3 reservation means a future PR proposing "warnings should fail CI
  too" has a named place to land (one predicate inside `exit_code()`) instead
  of reopening the per-warning question.
