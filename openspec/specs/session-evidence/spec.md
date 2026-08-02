# session-evidence

## Purpose

The session JSON zerostack writes is the harness's evidence channel: how tool records (including subagent calls) and prompt provenance are read from it, what a schema mismatch does, and the regression scenarios that pin the channel against upstream drift. It replaces the stdout `◈`-marker reconstruction that existed while headless zerostack recorded neither.

## Requirements

### Requirement: Tool calls are read from session tool records only

The transcript SHALL source `ToolCall`s exclusively from session JSON messages whose role is `tool_call` or `subagent_tool_call`, reading the tool's name from the message's structured `tool` record (`tool.name`), never by tokenizing the message content. Captured stdout SHALL NOT be parsed for tool-call evidence; `turn-N.stdout` logs remain diagnostic artifacts only. A `tool_call`- or `subagent_tool_call`-role message that lacks a `tool` record SHALL be a parse error, which the runner maps to an Indeterminate verdict — the same rule as any other schema mismatch, and the rule that makes a pre-#230 `ZS_BIN` visible instead of silently gradable. `subagent_tool_call`-role records SHALL mark the resulting call as a subagent call. `tool_result`-role messages remain messages, not calls.

#### Scenario: A structured tool record becomes a tool call
- **WHEN** a session message has role `tool_call`, content `bash ls -la`, and `tool: {"id": 3, "name": "bash", "args": {...}}`
- **THEN** the transcript carries one tool call named `bash` whose summary is the content minus the leading name token

#### Scenario: A tool-call-role message without a tool record is a schema error
- **WHEN** a session message has role `tool_call` but no `tool` field
- **THEN** parsing fails with an error naming the session file, and the trial grades Indeterminate

#### Scenario: Stdout markers are not evidence
- **WHEN** a trial's `turn-N.stdout` contains `◈ bash ls` lines but its session JSON contains no tool records
- **THEN** the transcript contains no tool calls

#### Scenario: A subagent record is a subagent call
- **WHEN** a session message has role `subagent_tool_call` and `tool: {"parent_call_id": 3, "name": "read", "args": {...}}`
- **THEN** the transcript carries a tool call named `read` marked as a subagent call

### Requirement: Prompt provenance is read from the session

The transcript SHALL expose the session's recorded `prompt: { name, source }` where `source` is `built_in` or `user_file`. When a trial produces multiple session files, the last session's recorded prompt SHALL win, matching upstream's own last-write-wins rule. A session without the `prompt` field SHALL parse successfully with no provenance (the downstream consequence — `unknown` plus a warning — is prompts-pack-identity's concern), because old mock fixtures are regenerated but a stale real binary must degrade loudly-but-gradably rather than fail every trial's parse. A `prompt.source` string other than `built_in` or `user_file` SHALL be a parse error: upstream vocabulary drift stops the run rather than being guessed around.

#### Scenario: A recorded prompt is exposed
- **WHEN** a session JSON contains `"prompt": {"name": "code", "source": "built_in"}`
- **THEN** the transcript carries that name and source verbatim

#### Scenario: An absent prompt field is exposed as absent
- **WHEN** a session JSON predating the prompt field is parsed
- **THEN** parsing succeeds and the transcript carries no prompt provenance

#### Scenario: An unrecognized source is a schema error
- **WHEN** a session JSON contains `"prompt": {"name": "code", "source": "global_file"}`
- **THEN** parsing fails with an error naming the session file

### Requirement: A scenario can assert the recorded prompt

The scenario assert vocabulary SHALL include `prompt_recorded <name> <built_in|user_file>`, graded against the trial transcript's raw readback in upstream's two-value vocabulary — not the report's four-way mapping — because the assert's job is to pin the evidence channel itself, not the harness's interpretation of it. The assert SHALL fail when the session recorded no prompt at all.

#### Scenario: Asserting the stock prompt on a bare run
- **WHEN** a scenario declares `prompt_recorded code built_in` and the trial's session records `{"name": "code", "source": "built_in"}`
- **THEN** the assert passes

#### Scenario: A missing prompt record fails the assert
- **WHEN** a scenario declares `prompt_recorded code built_in` and the trial's session has no `prompt` field
- **THEN** the assert fails rather than passing vacuously

### Requirement: Regression scenarios pin both channels

The shipped suite SHALL include, under `scenarios/session/` and registered in the coverage ledger, one scenario whose passing proves headless sessions carry tool records (a trivial tool task asserted with `tool_called_any`) and one whose passing proves sessions carry the prompt field (a bare run asserted with `prompt_recorded code built_in`). The tool-side pin SHALL NOT name a specific tool: which tool the model picks to satisfy the task is not itself a claim this scenario makes, only that some tool call was recorded. These exist so an upstream regression of either channel fails a named scenario instead of silently emptying every report's evidence.

A pin that asserts `prompt_recorded <name> built_in` observes the compiled-in default, so a run whose `--prompts` pack provides that same `<name>` has replaced the very thing the pin exists to watch: the harness seeds the pack into every trial of every scenario, including scenarios that declare no prompt of their own. The run SHALL detect that collision before spending anything on the scenario's trials, SHALL record the scenario as ungradable with no trials run, and SHALL say on stderr which prompt name the pack shadowed. It SHALL NOT grade the pin as a failure: a red regression scenario there would report the harness's own seeding as a product regression, which is the same defect — a pin making a claim about something other than the evidence channel — that made the tool-side pin stop naming a tool. The mirror case is deliberately not covered: a `prompt_recorded <name> user_file` assert with no pack in play is a run invoked wrongly, and its failure is the honest signal that the pack never loaded.

#### Scenario: A pack shadowing the built-in makes the prompt pin ungradable
- **WHEN** a run passes `--prompts <dir>` where `<dir>` provides `code.md`, and the suite contains the prompt regression scenario asserting `prompt_recorded code built_in`
- **THEN** that scenario runs no trials, is recorded as ungradable rather than failed, and the run names the shadowed prompt on stderr

#### Scenario: A pack that shadows nothing leaves the pin alone
- **WHEN** a run passes `--prompts <dir>` where `<dir>` provides only names the prompt regression scenario does not resolve
- **THEN** that scenario runs its trials and grades normally

#### Scenario: The tool-record channel regression is detectable
- **WHEN** a `ZS_BIN` that stops writing tool records runs the tool-record regression scenario
- **THEN** the trial grades Indeterminate or fails its `tool_called_any` assert, rather than passing

#### Scenario: The tool-side pin does not name a tool
- **WHEN** the tool-record regression scenario's task is satisfiable by more than one tool and the model answers it with a tool other than the one the scenario's author had in mind
- **THEN** its `tool_called_any` assert still passes, because the assert names no tool

#### Scenario: The prompt-record channel regression is detectable
- **WHEN** a `ZS_BIN` that stops writing the prompt field runs the prompt regression scenario
- **THEN** the trial fails its `prompt_recorded` assert
