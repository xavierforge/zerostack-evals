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
