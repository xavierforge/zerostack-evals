# Targets

A **target** is a zerostack `config.toml` that declares which provider + model a
run evaluates against. `zseval run --target <file>` seeds it into each run's
isolated `ZS_CONFIG_DIR`, so zerostack reads it through its normal config path —
explicit, committable, reproducible. No "whatever your machine happens to be
configured with".

**Keys never go in a target file.** Put the API key in an env var; zseval passes
the shell environment through to zerostack. Committing a target is safe because
it holds no secrets.

    export ANTHROPIC_API_KEY=sk-ant-...
    zseval run scenarios/prompts --target targets/anthropic.toml --tag anthropic

`--target` is repeatable: give it more than once to evaluate every target
against the same suite in one invocation, under one shared `--max-total-usd`,
and get a scenario x target table on stderr when it finishes:

    zseval run scenarios/prompts --target targets/anthropic.toml \
      --target targets/openrouter.toml --tag matrix-1 --trials 3

Reuse the resulting reports (or a committed baseline) as columns later,
without spending anything, via the pure renderer:

    zseval matrix results/matrix-1/anthropic/report.json \
      results/matrix-1/openrouter/report.json

`compare` stays pairwise, for the narrower question of whether to migrate from
one target to another (see `README.md`'s "What a report identifies").

## Built-in providers

zerostack ships providers `openai`, `anthropic`, `gemini`, `ollama`,
`openrouter`. A minimal target names the provider and a model valid for it:

```toml
provider = "anthropic"
model = "claude-sonnet-4-6"
```

The key is read from that provider's env var (`ANTHROPIC_API_KEY`,
`OPENAI_API_KEY`, `OPENROUTER_API_KEY`, …).

## Custom / self-hosted providers

Point at any OpenAI-compatible endpoint without putting the key in the file —
reference an env var with `api_key_env`:

```toml
provider = "mylocal"
model = "some-model-id"

[custom_providers.mylocal]
provider_type = "openai"
base_url = "http://localhost:11434/v1"
api_key_env = "MYLOCAL_API_KEY"
```
