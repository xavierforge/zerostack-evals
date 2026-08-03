# Judges

A **judge card** is the committed TOML file (`--judge judges/<name>.toml`) that
names which LLM grades the subjective (rubric) layer of a scenario:
`zseval run --judge <file>` (and `zseval regrade --judge <file>`) grades with
it, explicitly, committably, reproducibly. The card is inert data only: it
cannot name a network destination or an environment variable. That is the
whole point of its shape, and the rest of this file explains why.

```toml
provider = "anthropic"
model = "claude-sonnet-4-6"
price_in_usd_per_mtok = 3.0
price_out_usd_per_mtok = 15.0
```

Exactly four fields, all required, nothing else accepted:

| Field                     | Meaning                                                          |
| ------------------------- | ----------------------------------------------------------------- |
| `provider`                | One of the closed set `anthropic \| openai \| openrouter \| gemini` |
| `model`                   | The model id to request (see "What a run records" below for why the served model can differ) |
| `price_in_usd_per_mtok`   | Input price, USD per million tokens                              |
| `price_out_usd_per_mtok`  | Output price, USD per million tokens                             |

The prices are not decoration: judge calls are real API spend, and they land
in each trial's `cost_usd` and in `--max-total-usd`. A model swapped without
its prices would silently mis-report what every run cost. That is why they
live in the same file, and why a judge file has no optional fields: a
missing, misspelled, non-numeric, or negative value is an error (exit 2),
never a quiet fall back.

## No default judge

**There is no built-in judge.** A suite containing at least one judge-graded
scenario requires an explicit `--judge <file>` or `--no-judge`; giving neither
is a usage error (exit 2) before any trial runs. A suite with no judge-graded
scenarios needs neither flag. This is a deliberate break from an earlier
design where omitting `--judge` silently graded with a pinned default: the
experimenter now owns the experiment, in full, every time.

The decision is one of the four legs the run's preflight gate checks together
before anything is spent (see the README's "Quick start"). When it is missing,
the refusal names the judge-graded scenarios that forced the choice and lists
the `judges/*.toml` files sitting there right now as candidates — offered, not
picked, because which ruler grades a batch is what its scores mean. `zseval
list` prints the same count up front, so the choice arrives before the first
run rather than as the thing that stops it.

    export ANTHROPIC_API_KEY=sk-ant-...
    zseval run scenarios/prompts --judge judges/opus.toml --tag opus-judged

## The security invariant: routing is derived from `provider`, in code

A judge card names *which* provider's key is used; it can never say *where*
that key is sent or *which* environment variable holds it. Those two facts,
the base URL and the key's env var name, come from a `match` on the
`provider` enum, in code, reviewed like any other change, never from a field
a committed file can supply:

| `provider`   | Key env var           |
| ------------ | ---------------------- |
| `anthropic`  | `ANTHROPIC_API_KEY`     |
| `openai`     | `OPENAI_API_KEY`        |
| `openrouter` | `OPENROUTER_API_KEY`    |
| `gemini`     | `GEMINI_API_KEY`        |

Because of this, a card can at most send a provider's own key to that
provider's own official endpoint. Cross-pairing (provider X's key to
provider Y's endpoint, or any key to an arbitrary host) is not representable
in the schema. Availability follows the same variable: a judge configured
for one provider does not report itself ready just because some other
provider's key happens to be set.

### Why this replaced `api_url` + `api_key_env`

An earlier design let a judge file name an `api_url` plus the *name* of an
env var (`api_key_env`) to send that key to. That shape was a verified
exfiltration vector: any PR-introduced file could point an existing secret at
an arbitrary host, indistinguishable at a glance from a flaky judge.
Three escalating validation patches made the attack loud, not impossible,
which is why the fields were removed outright rather than fenced further.

**Loading a card that still names either field fails loudly, before any
network activity**, with an error stating the field was removed for security
and naming the four fields that remain. If a key was ever exposed through one
of these fields, rotate it (the error says so too). A field shaped like a
secret itself (`api_key`, `key`, `token`, `secret`, or a case variant) is
rejected the same way, naming the rule it breaks rather than serde's generic
"unknown field": a judge card is committed and holds no secrets, ever.

There is deliberately no hostname allowlist, and there never was one to lose:
a "is this really the official endpoint?" string check is exactly the kind of
thing that loses to a lookalike domain, so the fix is structural (no field
can name a destination) rather than a list that needs maintaining.

One environment-level routing input remains outside the card's control: the
HTTP stack honors the standard proxy variables (`HTTP_PROXY`, `HTTPS_PROXY`,
`ALL_PROXY`), so judge traffic can be routed through a proxy by the
environment that runs the harness. TLS bounds what such a proxy sees for
`https` endpoints (the destination host, not the headers or query strings
inside the tunnel), but the route itself is not recorded in any artifact.
Treat the proxy environment as part of the run's configuration when auditing
where a key could travel.

## Preflight: fail before any trial spends money

Before the first trial, whenever a judge will be used, two checks run:

1. **Key presence.** The configured provider's key env var must be set. If
   not, the process exits 2 naming the exact variable (with an `export` hint)
   and the `--no-judge` escape hatch.
2. **A live dry-run.** One probe call, in the exact shape a real judge call
   takes (same prompt template, same `max_tokens`, same temperature-fallback
   logic), must come back with a parseable verdict word. Any failure, such as
   bad auth, an unknown model, rejected parameters, or truncated/unparseable
   output, exits 2 relaying the provider's own error.

This catches a mistyped model name, an expired key, or a thinking model that
silently exhausts its token budget on `judge = "..."` before trial 1 runs,
rather than mid-suite. The probe's cost is real API spend but is **not**
recorded in the report and does not count toward `--max-total-usd`: it graded
nothing. `regrade` runs the same two checks before touching any stored
verdict.

## Shipped judges

| File | Model | In / out $ per Mtok |
| ---- | ----- | ------------------- |
| `sonnet.toml` | `claude-sonnet-4-6` | 3.0 / 15.0 |
| `opus.toml`   | `claude-opus-4-8`   | 5.0 / 25.0 |

Both target `provider = "anthropic"`. Model ids and prices are looked up at
[models.dev](https://models.dev); re-check both before bumping either file,
since a price update without the matching model swap (or vice versa) would
silently mis-report every run's cost.

## Changing the judge means re-calibrating

The judge is the ruler. Swapping it takes one flag, which makes it *easier*
to do and no less consequential: **a judge swap must be paired with
re-checking a batch of trials against human labels.** Two runs graded by
different models are not comparable just because both say "pass", because
the scores moved when the ruler moved, and without a human-labelled batch you
cannot tell that apart from the agent's behavior changing. Do not read a
pass-rate diff across a judge swap as a behavior diff.

`zseval regrade <scenario-dir> <trial-dir> --judge <file>` re-scores frozen
evidence with a different judge without driving the agent again: the cheap
way to see how far apart two rulers actually are on trials you have already
labelled.

A regrade rewrites `trial.json` in place, so it records the judge file that
produced the new verdict. Without that, a trial regraded by a second ruler
would sit under a `report.json` that still names the first, with nothing on
disk to tell them apart. The run's `report.json` is *not* rewritten (it
describes the run, and the run is not what changed), so a trial whose
`judge_file` differs from its report's is exactly what a regrade looks like.
The new judge's request/response go to a `regrade-<timestamp>/` subdirectory
of the trial dir rather than over the top of the previous judge's: those
artifacts are the only evidence of what graded the trial the first time, and
swapping the ruler must never destroy the trace it exists to leave.

Because the ruler must not move mid-measurement, `--judge` may be given **at
most once per run**, unlike `--target`, which is the axis a matrix varies.
The premise of a matrix is that everything except the target is held fixed;
a per-column judge would mean each column was measured with a different
ruler.

## What a run records

Every `report.json` records two different things, so a score can always be
traced back to what graded it:

- `"judge_file"` (+ `"judge_hash"`) is **configuration**: the judge file the
  run was told to use, `""` when none was named. The hash fingerprints the
  file's bytes, because a path is not an identity: a judge file's contents
  change under a stable path, so the path alone cannot say which ruler a past
  run actually held. (Scenarios record a `content_hash` for the same reason.)
  The path is recorded relative to the working directory, or as a bare file
  name if it lives outside it: a report is meant to be committed to
  `baselines/`, and must not carry your filesystem layout into git.
- `"judge_model"` is **execution**: the model(s) that actually graded, read
  back from the judge's own response, not from the card. The card only says
  which model was *asked for*; the API resolves names server-side, so a
  stale or aliased name can come back as a different ruler than the one on
  the file.

`"judge_model"` is a list with three states, and no two of them may be
confused:

| value | meaning |
| --- | --- |
| absent / `null` | **unknown**: the report predates the field, or a judge graded without naming the model that served it |
| `[]` | **nothing was graded**: `--no-judge`, no scenario carried a rubric, or every call failed |
| `["claude-opus-4-8"]` | these rulers graded (every distinct one, sorted) |

The `[]`/`null` split is the point. `[]` says "no ruler touched this", which
is honest; `null` says "we cannot tell". A baseline committed before these
fields existed *was* graded, so it must read as `null`: reporting it as `[]`
would state a falsehood about a real run, which is the one thing these
fields exist to prevent. Echoing the configured model back would do the same
in reverse, by reporting an intention as a fact.

Each trial's `trial.json` records the same facts for that trial alone
(`judge_file`, `judge_hash`, `judge_model` as a single value with the same
three states), next to the evidence it graded.

See the README's "What a report identifies", and
`docs/evidence-and-reports.md` for the field-by-field detail.
