## Context

The suite answers "how well does zerostack do the things we test?" Nothing answers "what don't we test?" — the closest thing is `scenarios/PLAN.md`, which mixes three kinds of statement: facts about the harness (gap A blocks per-scenario CLI flags), facts about zerostack (headless hides subagent tool calls), and scheduling (wave 1, wave 2, gap letters). The first two age with the code; the third ages with the author's plans. Ten days after PLAN.md was written, upstream fixed five of its nine product findings, and both kinds of fact moved at once.

The v1 site (Day 2) renders three regions: identity, coverage, results. Identity and results come from a report. Coverage has no source today. This change creates it, and it is the last Day 1 item — after this, the numbers and their frame are both accountable.

Two existing decisions constrain the shape. `backend.rs` stores the `--version` banner verbatim and refuses to parse it, so upstream's banner shape never becomes a compatibility contract. The 2026-07-23 test-organization decision keeps a single integration test binary. Both hold here.

## Goals / Non-Goals

**Goals:**

- A denominator that can express "this area has no scenario at all" — the fact a suite-derived count structurally cannot state.
- Machine-checked where machine-checkable: every id the ledger cites exists, every scenario the tree holds is claimed, both enforced by `cargo test`.
- Honest where it cannot be checked: prose that names its subject in full, and one staleness signal (`audited_against`) that the renderer surfaces.
- A stable contract for `zseval site` to render against, written down before the renderer exists.

**Non-Goals:**

- Rendering, including the mismatch disclosure and per-claim "not in this run" marking. Day 2.
- Mechanized surface diffing against a zerostack checkout. Post-v1 ROADMAP item 9.
- Any CLI surface, README section, or run-path dependency on the ledger.
- Coverage percentages.

## Decisions

### D1: Four statuses, and `uncovered` rather than `planned`

`covered` / `uncovered` / `product-blocked` / `excluded`. The ROADMAP drafted the second as `planned` carrying a wave number and a gap letter. Both scheduling fields were cut (D2), and `planned` without them promises roughly forty things the ledger has no field to back. `uncovered` states the fact and nothing else; the optional `blocked_by` says whether a harness gap stands in the way.

*Alternative considered:* keep `planned` and keep `wave`. Rejected: it duplicates PLAN.md's scheduling into a second file that must then be re-planned twice.

### D2: Prose names its subject; no pointer into a planning document

`blocked_by = "The backend hardcodes -p --yolo per call; scenario.toml has no field to add or drop a flag."` — not `gap = "A"`. PLAN.md's gap letters and wave numbers are an index into a document whose whole purpose is to be reorganized. A sentence that stands on its own survives the reorganization; a letter silently starts pointing at something else.

*Alternative considered:* `gap = "A"` with the letter resolved at render time. Rejected: the ledger would break without a second file, and the letters have already been renumbered once.

### D3: The ledger says `area`, the code keeps `domain`

`domain` is taken: `scenario.toml`'s `domains = [...]` opts a scenario into a subsystem's seed sugar and post-run drift check, with `KNOWN_DOMAINS` in `domains/mod.rs` as the closed list. Three names — memory, subagents, mcp — would exist in both vocabularies meaning different things, and `domains/mod.rs`'s own doc says nothing outside that file names a specific domain. `area` costs one word and removes the collision permanently.

### D3a: `project-instructions` is its own area, not folded into `context-window`

The 2026-07-16 carve-out gave 12 areas; `mcp` and `loop` joined because they already had scenarios. `scenarios/context/` is the same case and was missed: its two scenarios (`context-follows-agents-md`, `context-no-overapply`) exercise `context::load()`'s upward `AGENTS.md` walk and the scoping of the rules it finds, a calibration pair on whether a rule scoped to `.py` leaks onto other files.

Folding them into `context-window` was the tempting shortcut, since the source directory is named `context`. It is wrong, and expensively so: `context-window` in PLAN.md means the model catalog and window sizing (`context-window-catalog-autodetect`, `context-window-unknown-model-fallback`), which no scenario touches. Merging would let two `AGENTS.md` scenarios report the context window as tested, which is the precise failure this ledger exists to prevent, in the ledger's own denominator. The two areas share a directory name and nothing else.

The alternative, leaving the pair unclaimed, contradicts the bidirectional drift check: every scenario under `scenario_roots` owes exactly one `covered` claim, so an unclaimed pair is a permanently red test.

The area set is therefore 15, with 8 areas holding `covered` claims and 7 at zero coverage.

### D4: `covered` is existence, not passing and not run membership

Three candidate meanings: a scenario exists; a scenario exists and passes; a scenario exists and ran in the report being rendered. The second conflates coverage with results — a 0% scenario is coverage, and often the most valuable kind, since it is reporting a real defect; `kind` already answers whether that 0% is a problem. The third makes the ledger stale after every partial run.

Consequence for the renderer: "covered but absent from this report" is computed at render time by intersecting claim ids with the report's scenario ids, and marked on the page. This matters immediately — `prompt-pack-example-marker` lives in `examples/prompt-pack/` and is not in the baseline suite — and generally, for any filtered run.

### D5: One covered claim per scenario id, ledger-wide

The alternative is to allow repeats and de-duplicate when counting. That moves a judgment ("which claim does this scenario actually back?") from authoring time to render time and resolves it silently. PLAN.md suite policy 3 already requires deliberate overlaps be counted once; the strict rule is that policy made enforceable. Overlaps that are worth recording go in `note`, which takes no coverage slot.

### D6: `audited_against` is compared by containment, and a mock report legitimately mismatches

`audit_matches(banner)` is `banner.contains(audited_against)`. No parse, no assumption about the banner's shape, no new coupling to upstream's release formatting. If upstream reshapes the banner, the worst case is a spurious mismatch line on the page — never a wrong version claim, never a blocked publish.

The version relationship is disclosed, never enforced: failing the page when the ledger is older than the binary means no results can be published for a new zerostack until 15 areas are re-audited, and that gate would be commented out within a release or two, exactly as `evals.yml`'s prompts-gate already is.

One consequence differs from the conversation that produced this design, and is deliberate: a mock-backend report records `mock` as its banner and therefore mismatches. Skipping the comparison for mock was the earlier instinct; disclosure is better. A page rendered from mock numbers must say so loudly, and "audited against 1.7.2, results from mock" is precisely the sentence a reader needs.

### D7: No coverage percentage anywhere, which is what licenses asymmetric granularity

`covered` claims are fine-grained (each names its scenarios); `uncovered` claims are coarse, because an area nobody has tested cannot be sliced with any confidence. That asymmetry is fine for a status list and fatal for a ratio: re-slicing the same facts moves the percentage, and nothing can detect it. The headline figure is instead "7 of 15 areas have no scenario at all", which no re-slicing moves.

### D8: Enforced by `cargo test`, exposed by no subcommand

The ledger is internal: it serves this repo's honesty and feeds one renderer. A `zseval coverage` subcommand would add a user-facing surface, a README section, and a help entry for something no user of the harness runs. CI already runs `cargo test --workspace` on every PR, which is the enforcement point the ROADMAP's future CI gate needs anyway.

*Alternative considered:* a subcommand, on the argument that the future CI gate wants a standalone exit code. Rejected: `cargo test` already has one.

### D9: `crates/zseval/src/coverage.rs`, in the core, not under `domains/`

The schema is backend-neutral — areas are strings, claims are strings, statuses are generic. What is zerostack-specific is the *content*, and that lives in `scenarios/coverage.toml`, which is data alongside the scenario tree. When the core is extracted into its own crate (ROADMAP mid-term), `coverage.rs` travels with the core and the TOML stays with the zerostack scenario set. `domains/` is for code that encodes a particular zerostack subsystem's layout; this is not that.

### D10: The drift test joins `crates/zseval/tests/harness.rs`

Not a second test binary: the 2026-07-23 decision is one integration binary, split into modules later (ROADMAP item 7). Schema rejection paths (missing evidence, unknown status, duplicate id) are unit tests inside `coverage.rs` with inline fixtures; the tree-vs-ledger test is the integration one, because its input is the real repo.

### D11: Array of tables with comment banners, not a map keyed by area

`[[areas]]` preserves file order for free, and file order is presentation order (safety boundaries first, not alphabetical). A map (`[areas.permission]`) would put the area name in every claim's header, which reads better while scrolling, but serde would sort or require an ordered-map dependency to preserve the curated order. A banner comment per area buys the same readability for nothing.

### D12: The prose half will rot, and `audited_against` is the only alarm

Roughly two thirds of the claims carry prose that no test can check, against a product that shipped three releases in a week. A `verified_on` date per claim was considered and rejected: it is the same staleness signal bought twice, and a copy-pasted date is worse than no date. `audited_against` is one field, already needed, and the renderer puts it beside the report's real version where a reader cannot miss the gap.

### D13: `excluded` entries are re-adjudicated, not copied

`excluded` is the only irreversible judgment in the file — it says "we will never test this". PLAN.md's do-not-build list already contains at least two entries whose stated reason expired when upstream implemented the feature (`tool-todo-toggle-off-not-registered`, `print-tools-allowlist-restricts-surface`). Every `excluded` reason is checked against ROADMAP_ZS.md's 2026-07-26 status before it is written, and anything not confidently adjudicable is written as `uncovered` instead. Under-claiming exclusion is cheap; over-claiming it silently deletes future coverage.

## Risks / Trade-offs

- **Prose rot outruns the audit** → `audited_against` is rendered beside the report's actual version, so a stale ledger announces itself; re-auditing is a per-release ritual until ROADMAP item 9 mechanizes the surface diff.
- **Every new scenario now costs a ledger edit, and a red test until it is made** → intended, and the fix is four lines. ROADMAP item 10 already assumes new scenarios are claimed into the ledger.
- **The marker scenario counts as coverage though the baseline never runs it** → `note` records that it is a proxy indicator (zerostack does not report which prompt it loaded), and D4's render-time marking shows it as absent from the run.
- **Day 2's renderer could invent its own semantics** → the four load-bearing rules are in the spec, and `audit_matches` ships here so the renderer calls it rather than re-deriving it.
- **Authoring error in `excluded`** → D13's adjudication rule, plus a review pass focused on the seven zero-coverage areas and every `excluded` entry.
- **Two files now describe coverage (PLAN.md and the ledger)** → the split is stated in both: PLAN.md keeps scheduling and the unbuilt-proposal inventory, the ledger keeps facts and cites no index that a re-plan can move.

## Open Questions

None blocking. The renderer's exact treatment of a mock-backend mismatch (D6) is Day 2's to settle; the ledger's side of the contract is fixed here.
