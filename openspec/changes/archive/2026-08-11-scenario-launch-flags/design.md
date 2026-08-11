# scenario-launch-flags design

## Context

Every zerostack invocation is assembled in two places: `run_print` (`backend.rs:574-590`, the `-p` path) and `run_loop` (`backend.rs:653-667`, the `--loop` path). Both hardcode `--yolo --no-color` (plus `--pure-stdout` on the print path), then conditionally add `--load-prompt`, `--continue`, `--loop-max`, `--loop-run`, and finally the turn message as the positional argument. `scenario.toml` deserializes into a strict schema (`scenario.rs:32-99`, `deny_unknown_fields` at every layer per the scenario-strict-load spec) and its `content_hash` is an FNV-1a over the raw TOML bytes (`scenario.rs:274`), so any new field automatically participates in drift detection.

zerostack's permission surface is six mutually exclusive CLI flags (`cli.rs:103-127`): `--restrictive`/`-R`, `--read-only`, `--guarded`, `--accept-all`, `--yolo`, `--dangerously-skip-permissions`, with no flag meaning Standard mode. Removing `--yolo` is safe in headless: since upstream PR #212, an Ask resolution with no interactive consumer returns "Permission denied (non-interactive mode)" instead of hanging, and the harness makes no assumption that tool calls succeed.

## Goals / Non-Goals

**Goals:**

- A scenario can select any of zerostack's permission modes, with `yolo` as the default so every existing scenario keeps its exact current invocation.
- A scenario can pass extra CLI flags to zerostack without the harness enumerating them in advance.
- Misuse fails at scenario load, before any trial spends money, consistent with the run-prerequisite philosophy.

**Non-Goals:**

- No `[seed.config]` merge mechanics (Gap B) and no new assert kinds (Gap C); this change only covers the launch argument list.
- No per-turn argument variation: the declared mode and args apply to every turn of a scenario identically.
- No sugar beyond `security_mode`; low-frequency flags (`--no-context-files`, `--no-tools`, ...) ride `cli_args` until usage shows a sugar field is worth it.

## Decisions

### D1: Two fields, not one

`security_mode` (semantic enum) plus `cli_args` (raw escape hatch), per the PLAN.md Gap A sketch and user ruling. The sugar field exists because permission mode is the high-frequency case and because it is the one case that must *remove* a hardcoded flag, which an append-only `cli_args` cannot express. The escape hatch exists so the next flag-shaped need does not reopen this gap.

### D2: `security_mode` values mirror zerostack flag names

Serde enum, kebab-case: `yolo` (default) | `standard` | `restrictive` | `read-only` | `guarded` | `accept-all` | `dangerously-skip-permissions`. Mapping is mechanical: `standard` emits no flag, every other value emits `--<value>`. Mirroring upstream names verbatim keeps the mapping auditable and means a new upstream mode is a one-line addition. Rejected alternative: harness-invented names (`ask`, `deny`), which would drift from upstream vocabulary.

### D3: `cli_args` splice point and shape

Spliced after all harness-owned flags (including conditional `--load-prompt`/`--continue`/`--loop-*`) and immediately before the turn message, identically in both assembly paths. Entries are verbatim tokens; a flag taking a separate value is two entries (`["--quick-model", "fast"]`). Positional (non-dash) entries are allowed because separate-value flags need them; the README documents that a stray positional will collide with the turn message.

### D4: Load-time denylist, permission flags single-source

At scenario load, every dash-prefixed `cli_args` token (with any `=value` suffix stripped) is checked against the harness-owned set: `-p`, `--loop`, `--log-file`, `--continue`, `--load-prompt`, `--no-color`, `--pure-stdout`, `--loop-max`, `--loop-run`, and all six permission flags plus `-R`. A hit fails the load naming the scenario path and the offending token, so `list` and `run` both refuse before spend. Permission flags are banned from `cli_args` even though `security_mode` could tolerate redundancy: one source of truth, no ambiguity about which declaration won.

### D5: Assembly extracted into pure functions for testing

`run_print`/`run_loop` argument construction moves into pure functions returning the argument vector, which the spawn sites consume. Tests assert on the vector (mode mapping, splice ordering, loop parity) without spawning anything, the same assembly-layer testing pattern the zerostack sandbox work established. Explore confirmed no existing test asserts spawn args, so this creates the seam.

### D6: Target-overriding flags are allowed but must be declared in the scenario

`cli_args` can carry flags that override what the target supplies (`--model`, `--quick-model`, `--provider`). These are deliberately not denied: planned scenarios (provider-quick-model-cli-flag) exist to test exactly that surface. The convention, same as Gap B's whole-file config seeds: a scenario that overrides target identity notes in its TOML comment that it ignores `--target`.

## Risks / Trade-offs

- [Non-yolo modes change transcript texture: denials appear where tool results used to] → Expected and desired; graders for the new permission suite assert on the denial text. Existing scenarios are untouched because the default stays `yolo`.
- [A future harness flag joins the hardcoded list but not the denylist] → The denylist and the assembly functions live next to each other after D5; the assembly test enumerates harness-owned flags in one place so the pair drifts together or the test fails.
- [A stray positional in `cli_args` displaces the turn message] → Documented in scenarios/README.md; scenario authors are in-repo and the failure is loud (clap rejects a second positional).
- [coverage.toml claims about the hardcoded list go stale] → Rewriting them is an in-scope task; the coverage drift check enforces it.

## Decision reconciliation

- Two fields, sugar + escape hatch (user ruling 2026-08-10) -> D1
- `security_mode` value set and default `yolo` -> D2
- `cli_args` appended after harness flags, before message, both paths -> D3
- Load-time denylist, fail before spend -> D4
- Permission flags banned from `cli_args`, `security_mode` is single source -> D4
- content_hash automatic via raw-TOML hash -> Context (fact, no code decision needed)
- `deny_unknown_fields` retained -> Context / Goals
- Assembly-layer pure-function tests -> D5
- Target-overriding flags allowed with declared convention -> D6
