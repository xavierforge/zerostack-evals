# scenario-launch-flags decisions

Post-start rulings only; pre-start decisions live in design.md's reconciliation list.

- 08-10 [included] denylist extended past the spec's literal token set: upstream alias spellings (`--print` for `-p`, `-c` for `--continue`) and single-dash short clusters (e.g. `-nR` smuggling `-R`) are now rejected too; spec.md requirement text updated to match (source: preflight defect review)
- 08-10 [included] drift-guard test added: every flag the assembly functions emit and every `SecurityMode` mapping must appear in `HARNESS_OWNED_FLAGS`, closing design.md's "the pair drifts together or the test fails" promise that shipped without its test (source: preflight cross-check, spec axis)
- 08-10 [included] timeout repro hint now carries the scenario's declared permission flag and cli_args instead of a hardcoded `--yolo`, so the printed command reproduces the run that hung (source: preflight defect review)
- 08-10 [caveat] a stray positional in `cli_args` is silently joined into the turn message (upstream `message: Vec<String>` joins with spaces); no load-time check can tell it from a flag value, so the README documents the real failure shape instead of the previously claimed loud clap rejection (source: preflight defect review)
- 08-10 [caveat] `--accept-all` resolves to Standard's permission mode in today's zerostack, so `accept-all` and `standard` scenarios measure the same enforcement; the value stays for verbatim upstream mirroring (D2) and the README notes the equivalence (source: preflight defect review)
- 08-10 [caveat] the unknown-`security_mode` load error names the field only through toml's source-line echo, not harness-authored text; the load-rejection test locks the behavior (source: preflight cross-check, spec axis)
- 08-10 [caveat] accepted smells, no action: the print/loop assembly spines stay two separate functions (argument order is the locked contract), the duplicated default-invocation test stays (tasks 1.1 and 2.1 each demand one), and adding an upstream mode remains enum + match arm + denylist edits (source: preflight standards axis)
