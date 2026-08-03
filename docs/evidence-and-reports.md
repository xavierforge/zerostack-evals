# Evidence and reports

The reference half of the README's "How it drives zerostack" and "What a report
identifies": where a verdict's evidence comes from, what a `report.json`
records about the run that produced it, and every mark `compare` and `matrix`
put on a comparison they cannot fully vouch for.

## The evidence channel

**The session JSON is the evidence channel, and the only one.** A headless turn
persists structured `tool` records for every call it made (subagent calls
included) and a `prompt: { name, source }` record naming the prompt file it
actually loaded and whether that file was built into the binary or came from
disk. Both are read straight out of the session: `tool_called` and friends
grade the records, and each scenario's reported `prompt_source` starts from the
recorded `source` rather than from an inference about what the harness seeded.

The `turn-N.stdout` capture (`--pure-stdout`, `◈ {name} {summary}` marker
lines) is still taken and still worth reading while debugging, but nothing
grades it: a `◈ ` sequence inside a tool's own output is indistinguishable from
a real marker line, so parsing it risks seeing tool calls that never happened.

**So `ZS_BIN` has a floor.** It must be an `--all-features` build from a
zerostack mainline carrying PR #230 (tool records in headless session JSON) and
PR #228 (the prompt record). Against an older binary this is loud rather than
quiet: a tool-call message with no record makes the transcript unreadable, so
the scenario grades *indeterminate*, and a missing `prompt` record makes the
run warn and report `prompt_source: "unknown"` (and fails any
`prompt_recorded` assert). Neither degrades into "the agent called no tools",
which is the failure mode this design exists to prevent.

If zerostack's session schema changes, adapt `crates/zseval/src/transcript.rs`;
an unreadable schema grades as *indeterminate*, never as an agent failure.

### Replaying evidence without a build

`--backend mock=<path>` takes either a single captured session JSON file or a
*directory* shaped like a captured trial dir (`data/sessions/*.json` plus
`turn-N.{stdout,stderr,zslog}`), replayed exactly as a real run produced it.
Either form grades off the same evidence a live run does, because tool calls
and prompt identity are read from the session JSON; the turn logs a directory
fixture carries are along for the ride, not graded. A mock run drives no binary
and calls no provider, so it rejects `--target` and `--prompts`, and its
identity fields record the fixture rather than a binary.

### Loop mode has no session

A `mode = "loop"` scenario drives one `zerostack --loop` invocation, and
`run_headless_loop` never calls `save_session`. Grading evidence is
`$ZS_DATA_DIR/loops/<uuid>/iter-NNNN.json` instead (prompt / response /
validation_output per iteration), which `transcript.rs` folds in as ordinary
messages, so `final_contains` / `transcript_contains` / `file_*` all work
unchanged and `zseval explain` dumps them too. Two consequences: there is no
recorded prompt, so a loop scenario's `prompt_source` falls back to the old
derivation from what the harness seeded; and there are no `tool` records at
all, so every tool assert is rejected at load time. The authoring rules are in
`scenarios/README.md`.

## What a report identifies

Every `report.json` records what it evaluated against (`"model"`:
`"<provider>/<model>"`, resolved from `--target`'s config.toml (mandatory for
`--backend zs`) or `"mock"` for `--backend mock`, which rejects `--target`;
also `"target"`, the target file's own path, so a report carries its column
identity even once copied away from its run directory), what graded it
(`"judge_file"`, `"judge_hash"` and `"judge_model"`, below), and, per scenario,
a content hash of that scenario's `scenario.toml`.

### Which zerostack produced it

- **`"zs_version"`** is the first line of `ZS_BIN --version`, recorded
  verbatim. It is evidence for a human, deliberately not parsed or
  format-checked: the moment the harness validated the banner's shape, that
  shape would become a compatibility contract with upstream. For
  `--backend mock` it is the fixed string `"mock"`.
- **`"zs_bin_sha256"`** is the SHA-256 of the binary's contents (for
  `--backend mock`, a content fingerprint of the fixture). This is the
  machine-comparable identity: two runs are a controlled comparison only if
  this matches, which is exactly what a same-version-but-rebuilt binary cannot
  fake (the incident that motivated recording it: a `--version` that read
  `1.7.1` while the checkout had already moved on). `"zs_bin_path"` records
  where the binary lived, normalised the same way as `"target"` so a committed
  report is not a map of someone's filesystem. Because the hash is read from
  the file directly, `ZS_BIN` / `--zs-bin` must name the binary by path
  (absolute, or relative with a directory such as `./zerostack`) — a bare
  command name that only resolves via `$PATH` cannot be read this way and fails
  the run.
- **`"git_sha"`** and **`"features"`** are `null` today: the 1.7.x binary
  embeds neither. They are recorded as observed facts of the current binary,
  not a runtime "if the binary happens to expose it" branch, so when upstream
  starts embedding them the field simply stops being `null`.

Identity is captured once, at run start, before any trial spends anything. If
`ZS_BIN --version` cannot run, exits non-zero, or prints nothing, the run
**aborts** naming the binary rather than writing a report that cannot say what
produced it: an identity-less report is the very thing this record exists to
prevent, so there is no default or fallback value. A `--zs-bin` passed
alongside `--backend mock` is ignored, because identity records the source of
the evidence (the fixture), not an unused binary.

### Which ruler graded it

The judge fields exist because the judge is the ruler: a run graded by a
different model is not comparable to one graded by the old one just because
both say "pass", so which ruler was used has to survive the run. They record
two different kinds of fact:

- **`"judge_file"` (+ `"judge_hash"`) is configuration**: the judge file
  `--judge` named, `""` when none was. It holds whether or not the judge was
  ever called. `"judge_hash"` fingerprints the file's bytes because a path is
  not an identity: a judge file's contents change under a stable path, the same
  reason a scenario records a `content_hash`. The path is recorded relative to
  the working directory (a bare file name if it lives outside it, forward
  slashes always): a report is meant to be copied into `baselines/`, i.e. into
  git, and `--judge /Users/alice/private-client/judges/x.toml` must not put
  that in a committed artifact.
- **`"judge_model"` is execution**: the model(s) that actually graded, read
  back from the judge's own response, not from the judge file. The file says
  what was *asked for*; the API resolves model names server-side and can serve
  something else. It is a list of every distinct ruler that answered, so no
  consumer has to take a string apart, and no ruler has to stand in for
  another. The field is required — **absent** fails the report's load, naming
  the field, rather than reading in as any of the three states below: `null` is
  **unknown**, `[]` is **nothing was graded** (`--no-judge`, no scenario
  carried a rubric, or every call failed), and `["..."]` names the rulers. `[]`
  and `null` are deliberately different answers (see `judges/README.md`). Each
  trial's own `trial.json` records the same three facts for that trial alone.

Swapping the judge should be paired with re-checking a batch against human
labels (see `judges/README.md`). Report-family JSON is read as strictly as it
is written: every field, judge fields included, must be present or the whole
report fails to load naming what's missing — there is no read-tolerance default
that lets an older, incomplete report quietly stand in as "unknown".

## What `compare` warns about

`zseval compare` uses the model and hash fields. Every fact that weakens or
invalidates its answer is a **warning**, uniformly: the exit code stays a pure
function of the comparison rows (0 clean, 1 regressions, 2 nothing comparable),
with no per-warning escalation flag. That policy is the repo's first ADR
(`adr/0001-compare-always-warns-matrix-owns-multivar.md`) and an invariant test
enforces it, so adding a warning is never an exit-code decision. Warnings
render in two blocks.

*Incomparability*, above the scenario table, because each one inverts how the
whole table should be read:

- **Different targets**: comparing a baseline evaluated against one
  provider/model to a candidate evaluated against another prints a warning
  (still runs the diff; that's the migration-gate use case, deciding whether to
  switch targets) so a regression check never silently becomes an
  apples-to-oranges comparison. For a side-by-side view of more than two
  targets, use `zseval matrix` instead.
- **Different prompt packs**: the diff is a prompt A/B, not a regression check.
  Every pack difference is marked, deliberately: treating "same build,
  different pack" as a clean single-variable experiment stays a non-goal until
  it has been reviewed against real baseline data.
- **Different zerostack builds**: differing `zs_bin_sha256` warns, naming both
  sides as version string plus short hash. Identical version strings with
  different hashes still warn: the hash is the identity, the banner is only a
  label. This is the 2026-07-26 stale-binary incident (a binary printing
  `zerostack 1.7.1` while its checkout had moved on) caught mechanically
  instead of by luck.

*Caveats*, below the table, where the comparison is valid but weaker than it
looks:

- **Changed scenario definition**: if a shared scenario's `scenario.toml`
  differs between baseline and candidate, `compare` warns instead of quietly
  diffing two different tests under the same id (see AGENTS.md's guardrail on
  not moving the ruler while measuring). Plain inequality on `content_hash`:
  every current run records one, so there is no longer an "unknown, skip the
  check" case to carve out.
- **A budget-truncated side**: the cap stopped that side early, so its
  denominator is smaller than it looks. The missing scenarios already show up
  in the added/removed lists; this warning supplies the cause.
- **Tool-call evidence dropped to zero**: a `tool_not_called`-only scenario
  passes vacuously when the evidence channel itself breaks, so the pass rate
  above the warning may be meaningless.
- **Threshold finer than the trial count can resolve**: at 3 trials the pass
  rate only moves in steps of 1/3, so a nominal `--threshold 0.05` behaves like
  zero tolerance. Raise `--trials` or the threshold rather than ignore it.

## What `matrix` marks

`matrix` is a pure renderer: no API calls, nothing written to disk. Rather than
present a diff across changed conditions as a clean comparison, it marks what
it cannot vouch for:

- **SPREAD** on a row whose columns genuinely disagree about a scenario.
- **DRIFT** on a row whose scenario definition (`content_hash`) differs between
  columns, and **judge-drift** on a column graded by a different ruler than the
  others.
- **MULTI-VAR** on a column where two or more of the three subject variables
  (target, prompt pack, zerostack build) moved relative to another column: the
  score difference cannot be attributed to any one of them. The build counts as
  a subject variable alongside the other two, because every report records
  `zs_bin_sha256`, so "same version banner, different binary" is an observed
  fact rather than an assumption.
- **incomplete** on a column the budget cap cut short.

SPREAD, DRIFT and MULTI-VAR are display heuristics saying "look here", not
statistical or authoritative claims, and the rendered legend says so. Rulers
are not subject variables: a moved judge or scenario definition invalidates
comparability outright and is marked on its own, which is why it never feeds
the MULTI-VAR count.
