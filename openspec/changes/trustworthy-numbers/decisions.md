# Decision ledger — trustworthy-numbers

Post-start rulings only, one per line, written when the ruling happens.
Pre-start decisions live in design.md's reconciliation list (35 entries) and
proposal.md's Non-Goals; they are not repeated here.

Standing constraints for implementers (pre-start, recorded in design/specs,
repeated here only because workers read this file first):

- A scenario that fails the new strict loading gets fixed; never add a
  whitelist or escape hatch (design D2).
- Label before evidence: a regression failing the new baseline is a finding,
  never a silent relabel to capability (scenario-kind spec, rule 3).

<!-- - <MM-DD> [caveat|reversal→pending|reversal→denied|reversal→accepted|included|tracked] <ruling> (source: <where>) -->

- 07-27 [caveat] ZS_BIN / --zs-bin must be a readable binary path (absolute, or relative with a directory), not a bare $PATH command name: identity capture hashes the file via `std::fs::read`, which does not search $PATH the way `Command::new` does. The error message and README were clarified; bare-name-on-$PATH is an accepted unsupported config, not a bug to fix. (source: preflight defect axis, backend.rs `ZsCli::identity`)
- 07-27 [reversal→denied] cross-check standards flagged util.rs's hand-rolled SHA-256 (~90 lines, backs `zs_bin_sha256`) as a "reinvented primitive"; "fixing" it means adding the `sha2` crate. The hand-roll is a deliberate, documented code-level choice (util.rs doc comment: "pure std, no dependency", matching the repo's `fnv1a_hex` / `civil_date_string` house style), not a formal D3 decision. Verified correct against NIST vectors / hashlib by three independent reviewers; SHA-256 is a frozen algorithm and the input is a trusted local file, so no security surface. User ruled keep (07-27): no security issue, `sha2` would only be a simpler comparison path and is not worth the dependency. (source: preflight cross-check standards axis)
- 07-27 [caveat] /vibe-diff sign-off page (preflight step 4) is a user-invoked skill (disable-model-invocation); this gate did not render it. Deferred to the user to run at sign-off. (source: preflight step 4)
- 07-27 [reversal→accepted] compare's warnings render in two class-based blocks, not D6's single block: incomparability warnings (target/pack/zs mismatch, whose message is "a diff here is not a regression check") print ABOVE the scenario table so they are seen before the scores; caveats (truncation, definition-changed, evidence, low-resolution) stay BELOW it. Reverses design D6 structural anchor 3 (single render block) only; the ADR core (`exit_code()` purity, warnings never move the exit code) is untouched. ADR 0001 anchor 3 + consequences and the controlled-variables spec's "Warnings never move the exit code" requirement updated to match; the all-warnings-lit invariant test is unaffected (it asserts exit code, not render position). (source: /vibe-diff sign-off, user ruling B)
