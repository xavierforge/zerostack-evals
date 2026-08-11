# verify: scenario-launch-flags

Verdict: pass — all five changed surfaces observed live at the CLI (2026-08-11, argv-recording stub via `--zs-bin`, no API key spent).

## What was driven

Built `zseval` (debug), ran real `zseval run` invocations against a stub that logs its own argv. Scratch scenarios, `--results` in scratch, `--no-judge`, dummy `ANTHROPIC_API_KEY` (preflight-only; the stub never reaches the API).

1. **Default unchanged.** Scenario declaring neither field launches
   `-p --yolo --no-color --pure-stdout --log-file <path> --load-prompt ask <task>`
   — `--yolo` survives as the default, right after the shape flag. Exit 0.
2. **`security_mode = "standard"`** emits no permission flag at all; argv otherwise identical. Exit 0.
3. **Splice ordering.** `security_mode = "restrictive"` + `cli_args = ["--max-turns", "3", "--session-note", "two words"]` yields
   `-p --restrictive --no-color --pure-stdout --log-file <path> --load-prompt ask --max-turns 3 --session-note two words <task>`
   — mode flag in the permission slot, cli_args after every harness-owned flag, immediately before the turn message. Exit 0.
4. **Denylist at load time, exit 2, nothing spawned** (argv log stays empty):
   - `=value` form: `--log-file=/tmp/zs verify.log` → "collides with the harness-owned flag '--log-file'", names scenario id + toml path + token.
   - short cluster: `-nR` → "collides with the harness-owned flag '-R'".
5. **Timeout repro hint** (`timeout_secs = 2`, sleeping stub): hint prints the scenario's declared mode and shell-quoted cli_args
   (`... -p --restrictive --no-color --session-note 'two words' --log-level debug 'ping'`), not a hardcoded `--yolo`; the trial goes indeterminate, not fail.

## Post-review guard probes (added after the adversarial review fixes)

6. **`--api-key` refusal**: `cli_args = ["--api-key=sk-super-secret-value"]` fails load, exit 2; the error names the flag only and the secret value appears nowhere in the output.
7. **Target permission-key refusal**: a target file carrying `yolo = true` fails the preflight gate, exit 2, naming the file and the key, before anything runs; the unmodified target still passes (exit 0).

## Controls

- Valid run (probe 1) exit 0 alongside every exit-2 error probe, so exit 2 is meaningful.
- Preflight gates observed en route: missing `ANTHROPIC_API_KEY` and a `--version`-silent stub each block the run with exit 2 before any trial spends.
