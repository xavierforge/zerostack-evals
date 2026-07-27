# absence-asserts

## Purpose

Asserts that claim something is absent — `file_not_contains` and the new `path_not_exists` — pass only when the absence is actually verified, never as a side effect of nothing matching, so "the file was never created" stays distinguishable from "the file is clean".

## Requirements

### Requirement: file_not_contains fails when nothing matches
`file_not_contains <path> <needle>` SHALL fail when no file matches `<path>`, with a detail naming the pattern that had no match. It asserts two facts: the file exists, and its contents lack the needle. The former unconditional pass on zero hits made "the file was never created" indistinguishable from "the file is clean".

#### Scenario: No matching file fails
- **WHEN** `file_not_contains work:NOTES.md SPDX` is graded and no `NOTES.md` exists under the work root
- **THEN** the assert fails, and the detail names the unmatched pattern

#### Scenario: Clean file still passes
- **WHEN** the file exists and does not contain the needle
- **THEN** the assert passes, unchanged from before

#### Scenario: Offending file still fails
- **WHEN** the file exists and contains the needle
- **THEN** the assert fails naming the offending path, unchanged from before

### Requirement: path_not_exists passes only when nothing exists at the path
A new assert `path_not_exists <path>` SHALL pass if and only if zero filesystem entries match `<path>`. A directory counts as existing, the same as a file. On failure the detail SHALL name what it found.

#### Scenario: Absent path passes
- **WHEN** `path_not_exists work:debug.log` is graded and nothing named `debug.log` exists under the work root
- **THEN** the assert passes

#### Scenario: An existing file fails
- **WHEN** a file exists at the asserted path
- **THEN** the assert fails naming it

#### Scenario: An existing directory fails
- **WHEN** a directory exists at the asserted path
- **THEN** the assert fails naming it — a directory is not "nothing"

#### Scenario: Empty glob passes
- **WHEN** `path_not_exists data:sessions/*` is graded and `sessions/` is missing or empty
- **THEN** the assert passes

#### Scenario: Populated glob fails
- **WHEN** `data:sessions/*` matches one or more entries
- **THEN** the assert fails listing the matched entries

### Requirement: Absence asserts share the one assert path language
`path_not_exists` SHALL accept exactly the same path syntax as the existing `file_*` asserts: the `data:`/`config:`/`work:` root prefixes (default `data:`) and at most one `*` path segment. There SHALL be no per-assert exceptions to the path language. Existence matching counts files and directories; content-reading asserts (`file_contains`/`file_not_contains`) continue to read files only.

#### Scenario: Root prefixes resolve identically
- **WHEN** `path_not_exists config:agent/memory/MEMORY.md` is graded
- **THEN** the path resolves against the config root exactly as it would for `file_contains`

#### Scenario: Malformed patterns fail at load time
- **WHEN** a scenario declares a `path_not_exists` line with more than one `*` segment
- **THEN** loading fails, the same as any other malformed assert line
