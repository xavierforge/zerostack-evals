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

Swap targets to evaluate a different provider/model, then `compare` the reports:

    zseval run scenarios/prompts --target targets/anthropic.toml --tag a --trials 3
    zseval run scenarios/prompts --target targets/openrouter.toml --tag b --trials 3
    zseval compare results/a.json results/b.json

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
