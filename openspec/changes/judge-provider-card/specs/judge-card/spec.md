# judge-card

The judge card is the committed TOML file (`--judge judges/<name>.toml`) that names which ruler grades a run. It carries inert data only.

## ADDED Requirements

### Requirement: A judge card carries exactly four required fields
A judge card MUST contain exactly `provider`, `model`, `price_in_usd_per_mtok`, and `price_out_usd_per_mtok`. `provider` MUST be one of the closed set `anthropic`, `openai`, `openrouter`, `gemini`. All fields are required; unknown fields MUST be rejected. There SHALL be no built-in default card: a card is loaded from a file or not at all.

#### Scenario: A valid card loads for every supported provider
- **WHEN** a card with all four fields is loaded with `provider` set to each of `anthropic`, `openai`, `openrouter`, `gemini`
- **THEN** loading succeeds and the parsed config carries the file's model and prices verbatim

#### Scenario: A missing field is a loud error
- **WHEN** a card omits any of the four fields
- **THEN** loading fails with an error naming the missing field, and the CLI exits 2

#### Scenario: A provider outside the closed set is a loud error
- **WHEN** a card names `provider = "someother"`
- **THEN** loading fails listing the supported providers, and the CLI exits 2

#### Scenario: An unknown field is a loud error
- **WHEN** a card contains a field not in the schema (e.g. a typo like `temperture`)
- **THEN** loading fails naming the unknown field, and the CLI exits 2

### Requirement: A committed card can never name a secret's destination
The judge config type MUST contain no field that can influence the network destination of a request or which environment variable is read. The removed legacy fields `api_url` and `api_key_env` MUST be rejected with a targeted error stating they were removed for security and that routing is derived from `provider`, not a generic unknown-field error.

#### Scenario: The verified legacy attack file is rejected with the security rationale
- **WHEN** a card containing valid `provider`/`model`/prices plus `api_url = "https://evil.example/v1/messages"` and `api_key_env = "GITHUB_TOKEN"` is loaded
- **THEN** loading fails before any network activity, the error names the removed field and states the security reason, and the CLI exits 2

#### Scenario: Each removed routing field is rejected on its own
- **WHEN** a card contains only `api_url`, or only `api_key_env`, alongside the four valid fields
- **THEN** loading fails with the same targeted removed-for-security error

#### Scenario: Key-shaped fields are rejected by the rule they break
- **WHEN** a card contains a field named like a secret (`api_key`, `key`, `token`, `secret`, or case variants)
- **THEN** loading fails with an error stating a judge card is committed and holds no secrets

### Requirement: Card values are validated beyond TOML types
`model` MUST be non-empty and MUST NOT contain whitespace or control characters. Both prices MUST be finite and non-negative.

#### Scenario: An empty or malformed model is a loud error
- **WHEN** a card has `model = ""` or a model containing whitespace or control characters
- **THEN** loading fails naming `model`, and the CLI exits 2

#### Scenario: A non-finite or negative price is a loud error
- **WHEN** a card has a negative, NaN, or non-numeric price
- **THEN** loading fails naming the offending price field, and the CLI exits 2
