# Configuration

zerostack reads an optional config file. It supports TOML, YAML and JSON
formats. The file is resolved by priority:

- If `ZS_CONFIG_DIR` is set: `$ZS_CONFIG_DIR/config.toml` (preferred), `config.yaml`/`config.yml`, or `config.json`
- Otherwise: `~/.config/zerostack/config.toml` (preferred), `config.yaml`/`.yml`, or `config.json`

All config keys are optional. CLI flags and their environment-backed values
take precedence where both exist.

Accepted top-level keys:

| Key                       | Type    | Description                                                                                                                                                                 |
| ------------------------- | ------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `provider`                | string  | Provider name. Built-ins are `openrouter`, `openai`, `anthropic`, `gemini`/`google`, and `ollama`; custom provider aliases are also accepted. Default: `openrouter`.        |
| `model`                   | string  | Model name. Default: `deepseek/deepseek-v4-flash`.                                                                                                                          |
| `max_tokens`              | integer | Maximum response tokens. Default: `16384`.                                                                                                                                  |
| `max_agent_turns`         | integer | Maximum agent turns per response. Default: `200`.                                                                                                                          |
