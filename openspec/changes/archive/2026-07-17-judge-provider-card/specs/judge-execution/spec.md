# judge-execution

How a judge call runs: provider routing, request policy, resilience, verdict parsing, and faithful recording.

## ADDED Requirements

### Requirement: Routing is derived from the provider enum in code
For each provider the endpoint MUST be the provider's official default base URL and the API key MUST be read from the provider's fixed environment variable (`anthropic` -> `ANTHROPIC_API_KEY`, `openai` -> `OPENAI_API_KEY`, `openrouter` -> `OPENROUTER_API_KEY`, `gemini` -> `GEMINI_API_KEY`). The key MUST never leave the process: no subprocess, no argv, no temp file. OpenAI requests use the Responses API.

#### Scenario: A key only ever goes to its own provider
- **WHEN** a card names `provider = "openrouter"`
- **THEN** the request is sent to OpenRouter's official endpoint using only `OPENROUTER_API_KEY`, regardless of any other keys present in the environment

#### Scenario: No secret ever appears in a process argument list
- **WHEN** a judge call executes
- **THEN** no child process is spawned for transport and no environment secret appears in any argv

### Requirement: Temperature 0 is requested, with a recorded fallback
Judge requests MUST set temperature 0. When the provider rejects the temperature parameter, the call MUST be retried once without it and the omission MUST be recorded in the request artifact. Model-name lists MUST NOT be used to decide this; the fallback is error-driven.

#### Scenario: Default request carries temperature 0
- **WHEN** a judge call is built
- **THEN** the request sets temperature 0 and `judge-request.json` records it

#### Scenario: A provider rejecting temperature degrades loudly-recorded
- **WHEN** the provider returns an error indicating the temperature parameter is unsupported
- **THEN** the call is retried once without temperature, succeeds if the provider accepts, and `judge-request.json` records that temperature was omitted because the provider rejected it

### Requirement: The token budget is a fixed constant sized for thinking models
Judge requests MUST set a fixed `max_tokens` of 1024. The budget is not a card field.

#### Scenario: Requests carry the fixed budget
- **WHEN** a judge call is built for any provider
- **THEN** the request sets max_tokens 1024

### Requirement: Transient failures are retried once
A judge call failing with a clearly transient error (rate limit, 5xx, transport) MUST be retried exactly once after a short fixed backoff. Errors not clearly transient (including auth and unknown-model errors) MUST NOT be retried. A second failure yields Indeterminate for the trial, preserving evidence for regrade.

#### Scenario: A single transient blip does not cost a verdict
- **WHEN** the first attempt fails with a rate-limit or 5xx error and the retry succeeds
- **THEN** the trial records the retried verdict normally

#### Scenario: Persistent failure falls back to Indeterminate
- **WHEN** both the attempt and its single retry fail
- **THEN** the trial is Indeterminate with the judge error as reason, and its evidence remains regradeable

### Requirement: Verdict parsing is word-based and conservative
The judge's visible text MUST be parsed by its first alphanumeric word, case-insensitively: `yes` -> Yes, `no` -> No, anything else -> Unknown.

#### Scenario: Hedged answers read as Unknown
- **WHEN** the judge answers "Not sure" or an empty string
- **THEN** the verdict is Unknown, not No

### Requirement: The run records what actually graded, at the configured price
`judge_model` MUST be read from the provider's raw response (the served model), never echoed from the card; a response without it records an unknown ruler. Judge token usage MUST be priced at the card's prices into the trial's `cost_usd`, counted against `--max-total-usd`. Artifacts MUST include a reconstructed request record (`judge-request.json`) and the serialized raw provider response (`judge-response.json`).

#### Scenario: The served model is fact, not declaration
- **WHEN** the provider serves a different concrete model than the card requested
- **THEN** the report's `judge_model` records the served name from the raw response

#### Scenario: Judge spend lands in the budget cap
- **WHEN** a judge call reports input and output token usage
- **THEN** the trial's `cost_usd` includes usage priced at the card's per-mtok prices
