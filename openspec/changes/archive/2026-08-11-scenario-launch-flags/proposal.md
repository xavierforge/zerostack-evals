# scenario-launch-flags

## Why

`backend.rs` hardcodes the same argument list for every zerostack invocation, including `--yolo`, which auto-approves the permission layer. No scenario can reach zerostack's standard, restrictive, read-only, guarded, or accept-all permission modes, and no scenario can pass any extra CLI flag. This is the single largest obstacle to permission-domain coverage (scenarios/PLAN.md Gap A, recorded as an untested-obstacle claim in coverage.toml), and it bites exactly now: upstream has made the permission layer genuinely testable (headless Ask is fail-closed since PR #212, `%%mode=` readonly is real enforcement), and the Wave 1 permission suite cannot be built without it.

## What Changes

- New `scenario.toml` field `security_mode`: semantic sugar over zerostack's permission-mode flags. Values: `yolo` (default, preserves current behavior for every existing scenario), `standard` (no permission flag), `restrictive`, `read-only`, `guarded`, `accept-all`, `dangerously-skip-permissions`. The harness stops unconditionally hardcoding `--yolo`; it becomes the default value of this field.
- New `scenario.toml` field `cli_args = [...]`: extra arguments appended after harness-owned flags and before the user message, in both the `-p` and `--loop` assembly paths.
- Load-time denylist for `cli_args`: entries that collide with flags the harness owns (permission-mode flags, `-p`/`--loop`, `--log-file`, `--continue`, `--load-prompt`, `--no-color`, `--pure-stdout`, `--loop-max`, `--loop-run`) fail scenario load with a named path and flag, before any trial spends money. Permission flags have exactly one source of truth: `security_mode`.
- Both fields participate in `content_hash` automatically (the hash covers the raw TOML source), so a launch-flag edit is ruler drift like any other scenario edit.
- `deny_unknown_fields` stays; the new fields are added to the strict schema.

**Non-goals**: no `[seed.config]` merge (Gap B), no new assert kinds (Gap C), no per-turn argument variation, no sugar fields beyond `security_mode`. This change covers only the launch argument list.

## Capabilities

### New Capabilities

- `scenario-launch-flags`: a scenario declares the permission mode and extra CLI arguments its zerostack invocation launches with; the harness validates the declaration at load and assembles it identically in `-p` and `--loop` modes.

### Modified Capabilities

<!-- none: scenario-strict-load's unknown-field requirements are untouched; the
     new fields join the existing strict schema, and the denylist is a new
     requirement owned by the new capability -->

## Impact

- `crates/zseval/src/scenario.rs`: two new fields, denylist validation at load.
- `crates/zseval/src/backend.rs`: `run_print` and `run_loop` assembly read `security_mode` instead of hardcoding `--yolo`, then splice `cli_args`.
- `crates/zseval/tests/harness.rs`: load-rejection tests plus assembly-level tests for mode mapping and arg ordering.
- `scenarios/README.md`: document both fields.
- `scenarios/coverage.toml`: rewrite the claims that say the argument list is unreachable per scenario (they become stale the moment this lands); the coverage drift check keeps this honest.
- No change to targets, judge, report, or evidence readback surfaces.
