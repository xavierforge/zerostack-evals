# Decisions: coverage-ledger

Post-start rulings for this change, each dated, tagged, and sourced.
`design.md`'s Non-Goals and D1-D13 are the pre-start record and are never
edited here; this file holds what was decided after implementation began.

Created 2026-07-29 by the preflight gate, which is the first run that produced
rulings for this change. Nothing before that date was reconstructed.

- 07-29 [included] `audited_against` and `scenario_roots` are now rejected when empty, a load-time rule the spec's list did not carry: `audit_matches` is containment, and `"".contains()` matches every banner, so an empty value would report the ledger as audited against a mock report instead of mismatching it. That is a wrong version claim, the one outcome D6 says must never happen (source: preflight step 3, defect axis)

- 07-29 [caveat] The three `product-blocked` claims citing `issue #227` and `issue #229` are true against `audited_against = "1.7.2"`, but ROADMAP_ZS.md's 07-27 update records both as implemented upstream and filed as PR #228 / #230, with the zseval-side work explicitly waiting on those merging into the fork. The claims are correct as written and stay; they need re-adjudication at the next audit, when `audited_against` moves past 1.7.2 (source: preflight step 5, main session)

- 07-29 [caveat] `zs` carries three citation shapes: a GitHub issue (`issue #227`), an upstream roadmap work item (`sandbox hardening`), and a source marker (`TODO(loop-history)`). All five values resolve to real tracking entries; upstream tracks these things in three different places, and D2 prefers a string that names its subject over an index a re-plan can move. Not normalised on purpose (source: preflight step 3, standards and spec axes)

- 07-29 [caveat] The file-order half of the spec's "the ledger declares all 15 areas" scenario is pinned only on the inline fixture; the integration test sorts before asserting. Deliberate: spec.md line 10 and D11 make file order presentation order, so re-sequencing the page must stay free. Only the set is pinned (source: preflight step 3, spec axis)

- 07-29 [reversal→denied] The standards axis asks that `scenarios/coverage.toml` gain an entry in README's Layout block (README.md:23-37). This contradicts proposal.md's "No new CLI subcommand and no README section" and D8. The same reviewer noted `scenarios/PLAN.md` is absent from that block too, so no established rule is breached either way. Put to the author, who ruled the same day: keep the Non-Goal, no README entry (source: preflight step 3, standards axis)

- 07-29 [tracked] `scenario::discover`'s three silent skips (an unreadable directory, a failed `DirEntry`, and no descent into a directory that already holds `scenario.toml`) mean the "every scenario is claimed" direction can under-enumerate the tree and still report clean. Pre-existing `discover` behaviour that this change newly makes load-bearing for a correctness guarantee; no nested scenario exists today. Registered in ROADMAP.md rather than fixed here, because the fix touches `scenario.rs`, which proposal.md's Impact section puts out of scope (source: preflight step 3, defect axis)

- 07-29 [tracked] `check_ids` tests membership, not multiplicity, so two scenarios sharing one id, or overlapping `scenario_roots`, are both satisfied by a single covered claim and the check passes. Nothing in the repo rejects duplicate scenario ids today. Registered in ROADMAP.md together with the `discover` uniqueness check it wants (source: preflight step 3, defect axis)

- 07-29 [tracked] The schema accepts duplicate area names and an explicit `claims = []`, each of which loads clean while making an area account for nothing. The pinned-area-set test catches the duplicate for the real file only. Registered in ROADMAP.md (source: preflight step 3, defect axis)

- 07-29 [tracked] `Ledger`'s fields are `pub` and `check_unique_ids` runs only inside `parse`, so a hand-constructed `Ledger` (the likely shape of a Day-2 renderer fixture) can carry a duplicate id unchecked. D5's invariant is load-path-only. Registered in ROADMAP.md against Day 2 (source: preflight step 3, spec axis)
