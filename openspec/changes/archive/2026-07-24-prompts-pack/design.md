## Context

zerostack merges prompts from four layers into one `HashMap<name, content>`
(`src/context/prompts.rs:16-33`): embedded built-ins, `$ZS_DATA_DIR/prompts`,
`./data/prompts`, then `./.zerostack/prompts`. Later layers replace earlier
ones **per file name**, wholesale: a pack providing `code.md` displaces the
built-in `code` entirely and leaves the other fifteen built-ins untouched. Only
the top level is read, only `*.md`, never recursively, and a file's stem is its
prompt name (`src/context/mod.rs:25-41`).

The harness already gives each trial five fresh directories and points
zerostack at them (`backend.rs:294-300`), copies the target config into
`config/config.toml`, and then applies the scenario's own placements
(`backend.rs:306-311`). `.zerostack/prompts/` sits under the working directory,
so seeding a pack is one more copy into an already-isolated tree.

Two facts shape everything below:

- **A scenario without a `prompt` field still loads a prompt.** zerostack falls
  back to the config's `default_prompt`, and to `code` when unset
  (`startup.rs:619`). Ten of the forty-one scenarios (all of `memory/`, `mcp/`,
  `session/`, and `loop/`) are in this position, so a pack shipping `code.md`
  changes scenarios that have nothing to do with prompts.
- **zerostack does not report which prompt it loaded.** `current_prompt_name`
  lives only in memory; the session JSON has no such field
  (`src/session/mod.rs:61-91`) and nothing logs it. The harness cannot read the
  fact back, so the loaded prompt must be derived from what the harness itself
  placed, or left unknown.

## Goals / Non-Goals

**Goals:**

- Evaluate a custom prompt pack with no zerostack recompile.
- Make "the pack was seeded but nothing loaded it" impossible to mistake for
  "the pack scored 0.878".
- Keep `compare` and `matrix` honest about how many things moved between runs.

**Non-Goals:** as listed in `proposal.md`. Two deserve their reasoning here
rather than there:

- **Repeatable `--prompts`.** Its correct shape is not "both flags repeatable"
  but "at most one repeatable axis": `--target a --target b --prompts p` and
  `--target a --prompts p1 --prompts p2` are experiments, while repeating both
  is an N x M grid whose columns this change's own rules would mark as
  uninterpretable. That shape encodes the one-variable rule into the CLI and is
  the right follow-up; it reworks `cmd_run`'s orchestration loop, `auto_tag`,
  and `disambiguate_labels`, which is more than this change should carry.
- **An exit code for a broken comparison.** `compare`'s exit code already
  ignores the existing `target_mismatch`: a comparison whose premise failed can
  still exit 0. Fixing that changes the CI contract and belongs with the CI
  gate (ROADMAP item 12), where "stable" finally gets a written definition.

## Decisions

### Record which prompt each scenario loaded, not merely which names intersect

The cheap defense against a dead pack is a set comparison: warn when the pack's
names and the suite's invoked names do not intersect at all. That only catches
total misses. The realistic failure is partial: a pack of two prompts against a
suite of forty-one, where eleven scenarios load the pack and thirty load
built-ins, and the headline number is mostly the built-ins' score.

So each `ScenarioResult` records `prompt_name` and `prompt_source`. The
never-loaded warning falls out as the case where no scenario resolved to
`pack`, and the partial case is visible per scenario. Cost is two fields.

*Alternative rejected:* recording what zerostack actually loaded, which is the
strictly better fact. It requires zerostack to report the loaded prompt name
and source layer, which it does not. That gap is filed as an upstream issue,
and this design is what the harness can honestly say without it.

### The fields are scenario-level, not trial-level

`prompt_name` and `prompt_source` are constant across a scenario's trials. They
sit beside `content_hash` in `ScenarioResult`, not beside `judge_file` in
`TrialResult`. The judge fields live per trial for a concrete reason (`regrade
--judge` rescoring one trial dir in place, so two trials of one scenario really
can have different rulers); no such operation exists for prompts, so per trial
would just be the same string copied three times, and both consumers (matrix
rows, compare's per-scenario pass) already work at scenario level.

### Derive the default prompt name, but abandon derivation where it stops holding

Ten scenarios declare no prompt. Leaving them blank would blind the feature on
exactly the group most exposed to a `code.md` override. The harness therefore
reads `default_prompt` from the target config it seeds and falls back to `code`,
zerostack's own hardcoded default.

The derivation rests on the harness's copy being the last word on the config,
which stops being true if a scenario places a file into the config directory or
into `work:.zerostack/config.toml` (zerostack picks the first existing of four
candidate names, then merges the project-local file over it,
`config/load.rs:33-55,376-416`). In that case the harness records `unknown`
rather than a value that never took effect. No scenario does this today, so
the branch is currently unreachable; it costs one condition and keeps the
record from ever stating an intention as a fact.

*Alternative rejected:* reproducing zerostack's config resolution in the harness
(pick-first-of-four, then merge the local override). About thirty lines and no
new dependency for TOML and JSON, but YAML would need `serde_yaml` added, so it
would not actually eliminate `unknown` — and it would freeze a copy of an
upstream precedence order whose drift fails in precisely the way this project
guards against, by confidently recording a stale value. Its marginal value
today is zero. Whoever writes the first config-seeding scenario will hit the
blank field and can then implement it with a real use case in hand.

### The scenario's own seed wins over the pack

The pack is copied before `seed::apply`, so a scenario seeding the same prompt
name overrides it. This follows the precedent already written for the target
config, and it keeps the planned
`prompt-project-layer-overrides-user-and-embedded` scenario
(`scenarios/PLAN.md:102`) meaningful whether or not a pack is in play — under
the opposite order, an unrelated pack would silently gut a scenario whose whole
purpose is testing the override chain.

Silence is what made this dangerous, and `prompt_source = "scenario"` removes
it: the harness holds both file lists, so it can say which one won.

*Alternative rejected:* erroring on a name collision. More explicit, but it
makes `--prompts` unusable across the whole suite because one unrelated
scenario happens to use a colliding name.

### A pack contains only what zerostack reads, enforced at load time

Subdirectories and non-`.md` entries are load-time errors naming the offenders.
Skipping them silently would recreate the "the file is there, nothing reads it"
failure one level up, and the natural mental model ("I drop my folder in") is
exactly the one that produces a subdirectory. `Scenario::load` sets the
precedent by rejecting loop-incompatible asserts rather than letting the
footgun ship. Copying everything is worse still: files that cannot affect
behavior would enter the fingerprint, so two behaviorally identical packs would
compare as different.

A `README.md` inside a pack is accepted and becomes an unused prompt named
`readme`. That is harmless and not worth a special case.

### Fingerprint over sorted name-and-bytes, excluding the pack's own path

`prompts_hash` folds each top-level `*.md`, sorted by file name, as
`name\0bytes\0` into `util::fnv1a_hex` — the same hash scenarios and judge files
already use, so there is one hashing approach in the repo rather than two.
Sorting removes filesystem enumeration order. Names count because a rename
changes which built-in is overridden. The pack directory's path does not count,
so an unchanged pack that moved keeps its identity.

`prompts_names` is recorded too. It is 16 short strings against a 145 KB
report, and it makes "why is every scenario `stock`" diagnosable from the
report alone.

### One independent variable, derived rather than asserted

The rule for marking is: rulers (scenario definition, judge) invalidate a
reading and are marked unconditionally; subject variables (build, target, pack)
may vary by one, and two or more moving is marked. This reproduces every
existing behavior without an exception — `compare`'s different-targets note is
"the axis is the version and the model moved too", `matrix` not marking
different targets is "that is its axis", and both DRIFT flavors are rulers.

The consequence for packs: `matrix` columns sharing a target and differing only
by pack are a clean experiment and are not marked, which is the flagship use
case; columns differing in both are marked.

*Alternative rejected:* marking any pack difference, mirroring `judge-drift`'s
unconditional treatment. It would fire on every ordinary use of the feature, and a
mark that is always on is a mark nobody reads.

### Display the fingerprint, so "invisible difference" needs no special rule

An earlier draft carried a third rule: same path with a different hash always
warns, because two identity lines reading `prompts=my-pack` cannot be told
apart. Showing the short hash (`prompts=my-pack#a3f1`) makes the difference
visible and deletes the rule. The fingerprint is being recorded anyway, so
printing it costs nothing.

### compare treats the build as always moved, for now

`compare`'s premise is that its two sides differ by a rebuilt zerostack, but no
report records a zerostack identity, so it cannot count that variable. Until
ROADMAP item 2 lands, `compare` assumes the build moved and therefore notes any
pack difference as a second variable — conservative, in the same shape as the
existing different-targets note, and not a new rule. **When item 2 lands, the
line to revisit is `compare`'s variable count: with the build observable,
"build unchanged, pack changed" becomes a single-variable comparison and should
stop being noted.**

### New fields default rather than requiring a regenerated baseline

Every new field deserializes with a default, following the six existing
precedents. This is not a backward-compatibility mechanism — no version
branching, no migration — it is how "this report predates the field" is
encoded, the same three-state honesty `judge_model` already uses. It matters
because ROADMAP sequences the baseline regeneration (item 5) after this change,
so required fields would break `compare` against `baselines/main.json` in the
interim, and the schema is expected to move again before v1 ships.

The default for `prompt_source` is `unknown`, never `stock`: an old report's
prompts were never observed, which is a different fact from a run that
observably used built-ins.

### Acceptance splits into an offline layer and one live scenario

A stub executable passed as `--zs-bin` drives the real `ZsCli` with no API key
and no zerostack build (`.claude/skills/verify/SKILL.md`), so seeding,
precedence, recorded fields, source resolution, marking, and every usage error
are covered offline in `tests/harness`. The mock backend cannot serve here: it
never constructs a zerostack invocation, which is also why `--prompts` under
mock is rejected.

Exactly one claim needs a live run: the pack reaches the model. That is
`examples/prompt-pack/` — a pack overriding `code.md` with a marker instruction
and a scenario asserting the marker with `final_contains`. It overrides rather
than adds a new name because adding one would only prove zerostack reads the
directory, while overriding proves the pack beats the built-in, which is the
feature's selling point. It lives outside `scenarios/` because it fails without
`--prompts`, and it doubles as the copy-paste example README needs.

Its limit is stated in its own README: the assert rides on the model obeying a
formatting instruction, so a disobedient model reads as "not loaded". Direct
evidence needs the upstream fix.

## Risks / Trade-offs

- **The live acceptance is a proxy, not proof** → stated in the example's
  README, and the upstream gap (zerostack reporting the loaded prompt name and
  layer) is filed in ROADMAP item 7's issue list.
- **The `code` fallback is copied from upstream and can drift** → pinned with a
  comment naming the verified zerostack version, matching how `LoopCfg`
  documents its own upstream verification.
- **A pack shipping `code.md` silently changes memory/mcp/session/loop
  scenarios** → not a defect but real behavior; `prompt_source` makes it
  visible per scenario, and README says so plainly.
- **`matrix` column headers use the full tag when two columns share a target,
  and the auto tag is long enough to break the fixed-width table**
  (`matrix.rs:273-279`, `NUM_COL = 12`) → pre-existing, exposed by this
  workflow; not fixed here, but README's example passes short explicit tags
  (`--tag stock`, `--tag my-pack`).
- **A conservative note fires on `compare` runs where only the pack moved** →
  accepted until ROADMAP item 2 makes the build observable; the revisit point
  is named above.

## Migration Plan

None. New fields default, no report is rewritten, and the baseline is
regenerated later under ROADMAP item 5.

## Open Questions

None outstanding. The deferred items (repeatable `--prompts`, the comparison
exit code, `compare`'s variable count after ROADMAP item 2) are decisions
already made to defer, each with its revisit condition recorded above.

## Decision reconciliation

| Decision locked in discussion | Landing section |
| --- | --- |
| Per-scenario recorded prompt name and source, not a name-set intersection | Record which prompt each scenario loaded, not merely which names intersect |
| Fields on `ScenarioResult`, not `TrialResult` | The fields are scenario-level, not trial-level |
| Derive the default prompt name; record `unknown` when a scenario seeds config | Derive the default prompt name, but abandon derivation where it stops holding |
| Scenario seed beats the pack; add `scenario` to the source values | The scenario's own seed wins over the pack |
| Subdirectories, non-`.md`, missing dir, empty pack, duplicate flag, mock: all errors | A pack contains only what zerostack reads, enforced at load time; `prompts-pack-run` spec |
| Hash over sorted name plus bytes, path excluded; record `prompts_names` | Fingerprint over sorted name-and-bytes, excluding the pack's own path |
| Marking derives from "one independent variable", not a bespoke intent rule | One independent variable, derived rather than asserted |
| Display path plus short hash, deleting the invisible-difference rule | Display the fingerprint, so "invisible difference" needs no special rule |
| `compare` exit code untouched this change | Goals / Non-Goals; `controlled-variables` spec |
| `--prompts` single-arity in v1; repeatable recorded as a follow-up | Goals / Non-Goals |
| Offline stub-bin tests plus one live `examples/prompt-pack/` scenario | Acceptance splits into an offline layer and one live scenario |
| `serde` defaults, `prompt_source` defaults to `unknown`, no baseline regeneration | New fields default rather than requiring a regenerated baseline |
| Auto tag includes the pack name | `prompts-pack-run` spec (no separate design section: mechanical) |
| Revisit point recorded for ROADMAP item 2 | compare treats the build as always moved, for now |
