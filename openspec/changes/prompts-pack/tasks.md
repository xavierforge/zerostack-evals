# Tasks: prompts-pack

Conventions: a section is the execution unit (one section per session, landing
exactly one commit) and every task names the execution evidence that lets its box
be ticked. Sections are vertical slices: each carries its own failing tests and
the implementation that turns them green. `depends` names the sections that must
be green first, and is the real ordering constraint; `parallel: yes` means the
section may run alongside its siblings once its dependencies are met.

## 1. Accept and validate a prompt pack [dispatch: sai-hu, depends: none, parallel: no, reason: introduces a new type and its interface (what a pack is, how it is loaded and fingerprinted); the error cases are specified but the module boundary and error wording are open choices]

- [x] 1.1 Write failing unit tests for pack loading: a directory of top-level `*.md` loads with sorted names; a missing path, a non-directory, an empty directory, a directory containing a subdirectory, and a directory containing a non-`.md` file each fail with an error naming the offending entry. Evidence: `cargo test -p zseval` output showing the new tests failing for the right reason before implementation.
- [x] 1.2 Write failing unit tests for the fingerprint: identical contents under two different directory paths hash equal; renaming `code.md` to `mycode.md` with identical bytes changes the hash; file enumeration order does not affect the hash. Evidence: the three failing on the missing function, not on unrelated compile errors.
- [x] 1.3 Implement the pack type in `crates/zseval/src/` — load, validate, expose the sorted prompt names, the file bytes for seeding, and the `util::fnv1a_hex` fingerprint over sorted `name\0bytes\0`. Evidence: tests from 1.1 and 1.2 green.
- [x] 1.4 Write failing CLI tests in `crates/zseval/tests/harness.rs`: `run --prompts a/ --prompts b/` exits 2 naming the single-arity rule; `run --backend mock=<fixture> --prompts <dir>` exits 2 explaining mock cannot load a pack; a pack with a subdirectory exits 2 before any trial runs.
- [x] 1.5 Wire `--prompts` into `cmd_run`'s argument parsing with the arity and mock checks, and validate the pack before any trial spends money. Evidence: tests from 1.4 green, plus a control command that exits 0 so "exit 2" proves something.
- [x] 1.6 Add `--prompts` to the `USAGE` constant in `main.rs`: the flag on the `zseval run` usage lines, plus its own prose paragraph in the same shape as the existing `--target` and `--judge` paragraphs, stating single-arity, the mock rejection, and that a pack holds top-level `*.md` only. Evidence: `$BIN --help` output showing both the usage line and the paragraph.
- [x] 1.7 Verify at the real CLI surface: `cargo build -p zseval` then drive each rejection by hand, capturing exit codes with `$BIN ... > /dev/null 2>&1; echo $?`. Evidence: pasted commands and exit codes.

## 2. Seed the pack into every trial, behind the scenario's own seeds [dispatch: too-te, depends: 1, parallel: no, reason: spec names the destination, the ordering, and the precedent; design names the exact insertion point in ZsCli::run]

- [x] 2.1 Write a failing harness test using a stub `--zs-bin` script (per `.claude/skills/verify/SKILL.md`) that records the contents of `.zerostack/prompts/` from its own working directory, asserting every pack file lands there for every trial.
- [x] 2.2 Write a failing harness test for precedence: a scenario seeding `work:.zerostack/prompts/<name>.md` while the pack provides the same name leaves the scenario's bytes in place, not the pack's.
- [x] 2.3 Write a failing harness test that without `--prompts`, no `.zerostack/prompts/` directory is created by the harness.
- [x] 2.4 Implement seeding in `ZsCli::run`, copying the pack's `*.md` into `work/.zerostack/prompts/` before `seed::apply` so scenario placements land last. Evidence: tests from 2.1-2.3 green.
- [x] 2.5 Verify by hand with a stub bin: run one scenario with a two-file pack and `cat` the stub's recorded listing. Evidence: pasted listing showing both files under the trial's working directory.

## 3. Name the pack in the auto-generated run tag [dispatch: too-te, depends: 1, parallel: yes, reason: one function, `auto_tag`, with the expected string shape stated in the spec]

- [x] 3.1 Write failing unit tests for `auto_tag`: with a pack and no explicit tag, the tag contains the pack directory name alongside suite, provider/model, and timestamp; with an explicit `--tag`, the tag is exactly what was passed.
- [x] 3.2 Write a failing unit test for the multi-target path: with `multi = true` (which already drops the provider/model segment, `main.rs:1023`), the pack segment is still present, since the pack is held fixed across targets and is what distinguishes one multi-target run from the next.
- [x] 3.3 Thread the pack name into `auto_tag` and its call sites in `main.rs`. Evidence: tests from 3.1-3.2 green.
- [x] 3.4 Verify at the CLI: run a scenario with `--prompts` and no `--tag` against a stub bin and `--results` a scratch dir. Evidence: the produced results directory name, showing the pack segment.

## 4. Record the pack's identity on the report [dispatch: too-te, depends: 1, parallel: yes, reason: three fields following the `judge_file` / `judge_hash` / `target` precedent exactly, including the path-normalization rule and serde defaults]

- [x] 4.1 Write failing tests: a run with a pack records the relative, forward-slashed path, the fingerprint, and the sorted prompt names; a pack outside the working directory records its bare name, never an absolute path.
- [x] 4.2 Write a failing test that a run without a pack records empty values for all three fields.
- [x] 4.3 Write a failing round-trip test that a report JSON predating these fields still deserializes, following the existing older-report test precedent in `verdict.rs`.
- [x] 4.4 Add `prompts_pack`, `prompts_hash`, and `prompts_names` to `Report` with `#[serde(default)]` and doc comments stating why each exists, and populate them in the run path. Evidence: tests from 4.1-4.3 green.
- [x] 4.5 Confirm `REPORT_SCHEMA_VERSION` is unchanged and no code branches on it. Evidence: `grep -n schema_version crates/zseval/src/` output.
- [x] 4.6 Verify at the CLI: run with a pack against a stub bin and inspect the report. Evidence: the three fields as they appear in the emitted `report.json`.

## 5. Give `ScenarioResult` a place to record its prompt [dispatch: too-te, depends: 4, parallel: no, reason: two fields and a four-value enum whose values, defaults, and doc obligations are fixed by the spec; the resolution that fills them is section 6]

- [x] 5.1 Write failing tests for the source type: it round-trips all four values (`pack`, `stock`, `scenario`, `unknown`) through serde, and an unrecognized value is a deserialization error rather than a silent `unknown`.
- [x] 5.2 Write a failing round-trip test that a `ScenarioResult` predating these fields deserializes with source `unknown`, and assert `unknown != stock` so the three-state honesty the design calls for is pinned by a test rather than a comment.
- [x] 5.3 Add `prompt_name` and `prompt_source` to `ScenarioResult` with `#[serde(default)]`, beside `content_hash`, with doc comments stating why they are scenario-level and not trial-level, and leave them written as empty/`unknown` by the run path for now. Evidence: tests from 5.1-5.2 green.
- [x] 5.4 Verify at the CLI: a stub-bin run emits both fields on every scenario with their default values. Evidence: one scenario's object from the emitted `report.json`.

## 6. Resolve which prompt each scenario actually loaded [dispatch: sai-hu, depends: 2, 5, parallel: no, reason: the four-value resolution and the derivation guard need a home and a shape; deciding where the name is resolved and how a config-seeding scenario is detected is a real design call]

- [x] 6.1 Write failing tests for the resolution order: a declared prompt the pack provides resolves `pack`; a declared prompt the pack lacks resolves `stock`; a scenario seeding the same name resolves `scenario`.
- [x] 6.2 Write failing tests for the default-prompt derivation: a scenario declaring no prompt with a target config setting no `default_prompt` resolves to `code`; the same with `default_prompt` set in the target resolves to that name. Read the value by extending `target::peek`'s `Peek` struct, which already parses the target config for provider and model.
- [x] 6.3 Write a failing test for the guard: a scenario placing a file into the config directory or `work:.zerostack/config.toml` resolves `unknown` rather than a derived name.
- [x] 6.4 Implement the resolution in the run path and populate the section 5 fields, pinning the `code` fallback with a comment naming the verified zerostack version (matching how `LoopCfg` documents its upstream verification). Evidence: tests from 6.1-6.3 green.
- [x] 6.5 Verify at the CLI against a stub bin: run a mixed set of scenarios with a one-prompt pack. Evidence: the per-scenario `prompt_name` / `prompt_source` values in the emitted report, showing at least two distinct sources.

## 7. Warn when a seeded pack was never loaded [dispatch: too-te, depends: 6, parallel: yes, reason: one condition over data section 6 already records, with the message shape stated in the spec]

- [ ] 7.1 Write a failing harness test: a run whose pack provides only names no scenario calls warns that the pack was seeded but never loaded.
- [ ] 7.2 Write a failing harness test: a run where some scenarios resolve `pack` and others `stock` emits no never-loaded warning.
- [ ] 7.3 Implement the run-level check and its message. Evidence: tests from 7.1-7.2 green.
- [ ] 7.4 Verify at the CLI: run the suite with a deliberately mis-named pack against a stub bin. Evidence: the warning text as printed.

## 8. Show the pack in compare and note a prompt A/B [dispatch: too-te, depends: 4, parallel: yes, reason: mirrors the existing `target_mismatch` note in shape, placement, and exit-code neutrality; the spec fixes when it fires]

- [ ] 8.1 Write a failing test that `compare`'s header line carries each side's pack identity as path plus short hash beside the existing tags and models, and a plain no-pack marker when a side used none.
- [ ] 8.2 Write failing tests for the note: comparing two reports whose pack identities differ prints a note that the diff is a prompt change, not a regression check; identical pack identities, and both sides packless, print nothing.
- [ ] 8.3 Write a failing test that the note does not change the exit code: a pack difference with no regression still exits 0.
- [ ] 8.4 Implement the header field and the note. Evidence: tests from 8.1-8.3 green.
- [ ] 8.5 Verify at the CLI: `$BIN compare <a>/report.json <b>/report.json` over two stub-bin runs differing only by pack. Evidence: pasted output and exit code.

## 9. Show the pack in matrix and mark a two-variable table [dispatch: too-te, depends: 4, parallel: yes, reason: follows the per-column `judge-drift` mechanism and legend layout already in `matrix.rs`; the spec states exactly when the mark fires and that it is not DRIFT]

- [ ] 9.1 Write a failing test that each column's legend line carries its pack identity as path plus short hash, and a plain marker when a column used no pack.
- [ ] 9.2 Write a failing test that two columns sharing a target and differing only by pack are not marked, and that their legend lines differ.
- [ ] 9.3 Write a failing test that columns differing in both target and pack are marked, with the mark distinct from DRIFT.
- [ ] 9.4 Write a failing test that columns differing by target with one shared pack are not marked.
- [ ] 9.5 Implement the legend field and the per-column mark, and add it to the legend caveat that labels these as display heuristics. Evidence: tests from 9.1-9.4 green.
- [ ] 9.6 Verify at the CLI: `$BIN matrix` over two stub-bin reports sharing a target with different packs, then over two with different targets and packs. Evidence: both rendered tables.

## 10. Ship the example pack and prove it offline [dispatch: too-te, depends: 6, parallel: no, reason: the pack, the scenario, and the README's content are all fixed by the design; the live run this example exists for is section 12]

- [ ] 10.1 Create `examples/prompt-pack/` containing a pack that overrides `code.md` with an instruction to emit a fixed marker string on the first line, and a scenario asserting the marker with `final_contains`. Keep it outside `scenarios/` so the default suite is unaffected.
- [ ] 10.2 Write the example's own README: how to run it, why it lives outside `scenarios/`, and that the assert rides on model obedience, so a disobedient model reads as "not loaded".
- [ ] 10.3 Verify offline that the scenario loads and the pack validates: `$BIN list examples/prompt-pack`, and a stub-bin run showing the pack files reaching the trial's working directory and the scenario recording `prompt_source = "pack"`. Evidence: both command outputs.
- [ ] 10.4 Verify that `zseval run` over `scenarios/` is unaffected by the example's existence. Evidence: scenario count from `$BIN list scenarios/` unchanged at 41.

## 11. Document --prompts in the README [dispatch: too-te, depends: 8, 9, 10, parallel: no, reason: one new section replacing existing prose, with the commands and caveats already fixed by the specs and design]

- [ ] 11.1 Replace the README's "Iterating on prompts" advice to edit the zerostack checkout and rebuild with the `--prompts` flow.
- [ ] 11.2 Document the two-run plus `matrix` shape, with short explicit tags (`--tag stock`, `--tag my-pack`) and a sentence saying why: `matrix` labels same-target columns by tag, and an auto tag is wide enough to break the table.
- [ ] 11.3 Document what a pack may contain (top-level `*.md` only), that a name not in the pack falls through to the built-in prompt, and that a pack shipping `code.md` also affects scenarios that declare no prompt.
- [ ] 11.4 Point at `examples/prompt-pack/` as the copy-paste starting point.
- [ ] 11.5 Verify every command in the new section runs as written against a stub bin. Evidence: each command with its exit code.

## 12. Live acceptance: the pack reaches the model [dispatch: main, depends: 10, 11, parallel: no, reason: needs a real ZS_BIN, an API key, and the user's judgment on whether the marker actually landed; it cannot be verified inside an agent without spending the user's money, so it is a human gate rather than a dispatchable section]

- [ ] 12.1 Run `examples/prompt-pack/` once with a real `ZS_BIN` and the sonnet judge, per the project's real-run setup. Evidence: the trial's final message showing the marker, plus the scenario's recorded `prompt_source = "pack"`.
- [ ] 12.2 If the marker is absent, record which of the two causes it was (the pack did not reach the model, or the model disobeyed the formatting instruction) before treating it as a defect. Evidence: the trial's transcript and the seeded `.zerostack/prompts/` listing, which separate the two.
