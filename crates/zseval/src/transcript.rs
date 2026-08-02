//! Parse a zerostack session JSON into a normalized `Transcript`.
//!
//! Contract (mirrors zerostack's session storage):
//! - Sessions persist to `{ZS_DATA_DIR}/sessions/{id}.json` as a whole
//!   `Session { id, messages, total_input_tokens, total_output_tokens,
//!   total_cost, ... }` with `SessionMessage { role, content, tool }`.
//! - `role` is snake_case: user | assistant | system | tool_call |
//!   tool_result | subagent_tool_call.
//! - A `tool_call` / `tool_result` / `subagent_tool_call` message carries a
//!   structured `tool` record beside its display `content`: `{ id, name,
//!   args }` for a call, `{ call_id, name, truncated, full_output_path }`
//!   for a result, `{ parent_call_id, name, args }` for a subagent's call.
//!   The `role` already discriminates the three, so `RawToolRecord` below
//!   mirrors them as one tolerant struct instead of an untagged enum.
//! - `content` is only the human display summary (zerostack's
//!   `ui::utils::format_tool_call_summary`, truncated for display); the
//!   record's `args` carries the complete value. Arg asserts still match the
//!   summary string; outcome asserts (`file_*`) check the environment.
//!
//! The session JSON is the one evidence channel for tool calls, and it is
//! the same one for both backends: headless `-p` runs persist these records
//! unconditionally (zerostack PR #230), so a real run and a mock fixture
//! feed `Transcript.tool_calls` identically. `backend::ZsCli` still passes
//! `--pure-stdout` and still tees each turn to `turn-N.stdout`, but those
//! `◈ {name} {summary}` marker lines are a human-facing debugging artifact
//! only. Nothing here parses them, and nothing should: they are a
//! line-prefix convention that a tool's own output can forge.
//!
//! A schema we cannot parse becomes an `Err`, which the runner maps to an
//! `Indeterminate` verdict — an unreadable transcript is not an agent
//! failure. A tool-call-role message carrying no `tool` record is exactly
//! that case: it means the binary under test predates PR #230, which has to
//! be visible rather than gradable as "the agent called no tools". If
//! zerostack's schema changes, adapt this file.

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
struct RawSession {
    #[allow(dead_code)]
    id: String,
    messages: Vec<RawMessage>,
    #[serde(default)]
    total_input_tokens: u64,
    #[serde(default)]
    total_output_tokens: u64,
    #[serde(default)]
    total_cost: f64,
    /// zerostack's own rough token estimate (word/char heuristic, not
    /// provider-reported usage) — the only token count that's ever nonzero
    /// in headless mode today (see `Transcript::total_tokens`).
    #[serde(default)]
    total_estimated_tokens: u64,
    /// The prompt active when the session was last saved (upstream's PR
    /// #228, `session::Session::prompt`). Absent on sessions predating that
    /// field, which must parse as `None`, not an error (design D3).
    #[serde(default)]
    prompt: Option<RawPromptRef>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawMessage {
    role: String,
    content: String,
    #[serde(default)]
    tool: Option<RawToolRecord>,
}

/// Mirror of upstream's `session::ToolRecord`, flattened. Upstream models
/// the three shapes as an untagged enum; here the message `role` already
/// says which one it is, so mirroring the enum would buy nothing and would
/// break outright the day upstream adds a field to any variant. Only `name`
/// is required, and only `name` is consumed today — the rest is mirrored so
/// the shape stays documented at the point of use.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
struct RawToolRecord {
    /// Present on all three shapes.
    name: String,
    /// `Call`.
    id: Option<u64>,
    /// `Result`.
    call_id: Option<u64>,
    /// `SubagentCall` — deliberately has no `id` upstream, which is what
    /// keeps its JSON from also satisfying `Call`.
    parent_call_id: Option<u64>,
    /// `Call` / `SubagentCall`: the complete, untruncated arguments.
    args: Option<serde_json::Value>,
    /// `Result`.
    truncated: Option<bool>,
    /// `Result`: where an over-threshold tool output was spilled.
    full_output_path: Option<String>,
}

/// Mirror of upstream's `session::PromptRef`. `source` stays a raw `String`
/// here rather than upstream's `PromptSource` enum, so an unrecognized value
/// can be its own schema-`Err` step in `parse_str` (naming the session
/// file), instead of the whole session failing to deserialize with a bare
/// serde message (design D3).
#[derive(Debug, Clone, Deserialize)]
struct RawPromptRef {
    name: String,
    source: String,
}

/// One `$ZS_DATA_DIR/loops/<session-id>/iter-NNNN.json` record — the
/// grading evidence for `mode = "loop"` scenarios, since `run_headless_loop`
/// never calls `save_session` (see `scenario::LoopCfg`'s doc). Shape mirrors
/// `extras/loop/transcript.rs::save_iteration` in zerostack.
#[derive(Debug, Clone, Deserialize)]
struct RawLoopIteration {
    iteration: u32,
    #[allow(dead_code)]
    timestamp: String,
    prompt: String,
    response: String,
    #[serde(default)]
    validation_output: Option<String>,
    #[allow(dead_code)]
    summary: String,
}

#[derive(Debug, Clone)]
pub struct Msg {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct ToolCall {
    /// A monotonic position for ordering asserts (`tool_called_after`) to
    /// compare against: the call's index in its own session's message list,
    /// shifted by `absorb` when several sessions are concatenated. Only the
    /// relative order among tool calls matters, so gaps (every non-tool
    /// message leaves one) are fine.
    pub index: usize,
    pub name: String,
    /// Human summary of args as zerostack rendered it (may be truncated).
    pub summary: String,
    pub subagent: bool,
}

/// The session's recorded prompt provenance (mirrors upstream's
/// `session::PromptRef`, design D3): which prompt shaped the trial, and
/// whether its content was the compiled-in default or a user file.
/// `source` is upstream's own two-value vocabulary, `"built_in"` or
/// `"user_file"`, already validated by `parse_str` — deliberately not this
/// crate's four-value `verdict::PromptSource` report field. Vocabulary note
/// (tasks.md): the bare word "source" never stands in for either on its
/// own, so this type spells out that it is the readback, not the mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedPrompt {
    pub name: String,
    pub source: String,
}

#[derive(Debug, Clone, Default)]
pub struct Transcript {
    pub messages: Vec<Msg>,
    pub tool_calls: Vec<ToolCall>,
    pub final_assistant: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    /// zerostack's rough estimate, summed across sessions — see
    /// `total_tokens`'s fallback.
    pub estimated_tokens: u64,
    /// The session's recorded prompt readback (design D3). `None` when the
    /// session predates zerostack's PR #228, or the trial recorded no
    /// prompt at all. `absorb` applies last-wins, matching both upstream's
    /// own last-write-wins rule and `final_assistant`'s existing one.
    pub prompt: Option<RecordedPrompt>,
}

impl Transcript {
    /// Merge another session into this one (multi-session scenarios produce
    /// several session files; asserts run over the concatenation).
    pub fn absorb(&mut self, other: Transcript) {
        let base = self.messages.len();
        self.messages.extend(other.messages);
        for mut tc in other.tool_calls {
            tc.index += base;
            self.tool_calls.push(tc);
        }
        if !other.final_assistant.is_empty() {
            self.final_assistant = other.final_assistant;
        }
        if other.prompt.is_some() {
            self.prompt = other.prompt;
        }
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cost_usd += other.cost_usd;
        self.estimated_tokens += other.estimated_tokens;
    }

    /// Real (input + output) usage when the provider/session reports it;
    /// falls back to zerostack's own rough estimate when it doesn't. As of
    /// 2026-07-06, zerostack's headless `-p` path never populates real usage
    /// into the session at all (a gap independent of the tool-call one this
    /// harness works around elsewhere) — `total_estimated_tokens` is the only
    /// nonzero count available, so `tokens_under`/`max_total_tokens` would be
    /// silent no-ops without this fallback. Real usage always wins when
    /// present, since it's the true number, not an estimate.
    pub fn total_tokens(&self) -> u64 {
        let real = self.input_tokens + self.output_tokens;
        if real > 0 {
            real
        } else {
            self.estimated_tokens
        }
    }

    /// Compact text form fed to the LLM judge.
    pub fn render_for_judge(&self, max_chars: usize) -> String {
        let mut out = String::new();
        out.push_str("## Tool calls (in order)\n");
        for tc in &self.tool_calls {
            out.push_str(&format!(
                "- {}{}\n",
                if tc.subagent { "[subagent] " } else { "" },
                if tc.summary.is_empty() {
                    tc.name.clone()
                } else {
                    format!("{} {}", tc.name, tc.summary)
                }
            ));
        }
        out.push_str("\n## Final assistant message\n");
        out.push_str(&self.final_assistant);
        if out.len() > max_chars {
            out.truncate(max_chars);
            out.push_str("\n[...truncated...]");
        }
        out
    }

    /// Build the complete, gradable `Transcript` for one trial: messages,
    /// tool calls, tokens, and cost from every session file, plus any loop
    /// iteration records. This is the one entry point the runner needs, and
    /// there is one evidence channel behind it for both backends alike — the
    /// session JSON. `artifacts.turns` is deliberately not read here: a
    /// turn's `turn-N.stdout` is a debugging artifact, not evidence (see the
    /// module doc). A schema mismatch in any session file is an `Err`, which
    /// the caller maps to Indeterminate.
    pub fn from_run(artifacts: &crate::backend::RunArtifacts) -> Result<Transcript> {
        let mut t = Transcript::default();
        for f in &artifacts.session_files {
            t.absorb(parse_file(f)?);
        }
        t.absorb(loop_transcript(&artifacts.data_dir)?);
        Ok(t)
    }
}

/// One `mode = "loop"` iteration, exposed for `explain` to print alongside
/// (loop scenarios have no session file to dump instead — see
/// `scenario::LoopCfg`'s doc).
#[derive(Debug, Clone)]
pub struct LoopIteration {
    pub iteration: u32,
    pub prompt: String,
    pub response: String,
    pub validation_output: Option<String>,
}

/// Read every `$ZS_DATA_DIR/loops/*/iter-*.json` record, sorted by the
/// record's own `iteration` field (not the filename — the filename's
/// zero-padding, `iter-0001.json`, happens to sort correctly today, but the
/// field is the source of truth). Empty, not an error, when the `loops`
/// directory doesn't exist at all — the ordinary case for every
/// `mode = "print"` scenario and the mock backend's non-loop fixtures. An
/// unreadable/malformed iter file that *does* exist is an `Err`, the same
/// rule session files already follow (-> Indeterminate at the runner).
pub fn read_loop_iterations(data_dir: &Path) -> Result<Vec<LoopIteration>> {
    let loops_dir = data_dir.join("loops");
    let session_dirs = match std::fs::read_dir(&loops_dir) {
        Ok(rd) => rd,
        Err(_) => return Ok(Vec::new()),
    };

    let mut iter_files = Vec::new();
    for e in session_dirs.flatten() {
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        let Ok(inner) = std::fs::read_dir(&p) else {
            continue;
        };
        for f in inner.flatten() {
            let fp = f.path();
            if fp.extension().map(|x| x == "json").unwrap_or(false) {
                iter_files.push(fp);
            }
        }
    }
    if iter_files.is_empty() {
        return Ok(Vec::new());
    }

    let mut records = Vec::with_capacity(iter_files.len());
    for f in &iter_files {
        let text = std::fs::read_to_string(f).with_context(|| format!("read {}", f.display()))?;
        let rec: RawLoopIteration =
            serde_json::from_str(&text).with_context(|| format!("parse {}", f.display()))?;
        records.push(LoopIteration {
            iteration: rec.iteration,
            prompt: rec.prompt,
            response: rec.response,
            validation_output: rec.validation_output,
        });
    }
    records.sort_by_key(|r| r.iteration);
    Ok(records)
}

/// Fold loop iteration records into a `Transcript` — the only evidence a
/// `mode = "loop"` scenario produces (see `scenario::LoopCfg`'s doc). Per
/// iteration: a `user` message (the built loop prompt), an `assistant`
/// message (the response, also becoming `final_assistant` — the *last*
/// iteration wins, same "last one absorbed wins" rule `absorb` uses
/// elsewhere), and — when present — a `tool_result`-role message carrying
/// `--loop-run`'s output, so `transcript_contains` can grade a validation
/// command's pass/fail text without it polluting `final_assistant`.
fn loop_transcript(data_dir: &Path) -> Result<Transcript> {
    let mut t = Transcript::default();
    for r in read_loop_iterations(data_dir)? {
        t.messages.push(Msg {
            role: "user".to_string(),
            content: r.prompt,
        });
        t.messages.push(Msg {
            role: "assistant".to_string(),
            content: r.response.clone(),
        });
        t.final_assistant = r.response;
        if let Some(v) = r.validation_output {
            t.messages.push(Msg {
                role: "tool_result".to_string(),
                content: v,
            });
        }
    }
    Ok(t)
}

pub fn parse_file(path: &Path) -> Result<Transcript> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("read session {}", path.display()))?;
    parse_str(&text).with_context(|| format!("parse session {}", path.display()))
}

pub fn parse_str(text: &str) -> Result<Transcript> {
    let raw: RawSession = serde_json::from_str(text).context("session schema mismatch")?;
    // Readback is the value (design D3): an absent `prompt` is `None`, but a
    // present one with an unrecognized `source` is a schema `Err` — upstream
    // vocabulary drift stops the run rather than being guessed around.
    let prompt = raw
        .prompt
        .map(|p| match p.source.as_str() {
            "built_in" | "user_file" => Ok(RecordedPrompt {
                name: p.name,
                source: p.source,
            }),
            other => Err(anyhow!(
                "session recorded prompt \"{}\" with unrecognized source '{other}' \
                 (expected 'built_in' or 'user_file')",
                p.name
            )),
        })
        .transpose()?;
    let mut t = Transcript {
        input_tokens: raw.total_input_tokens,
        output_tokens: raw.total_output_tokens,
        cost_usd: raw.total_cost,
        estimated_tokens: raw.total_estimated_tokens,
        prompt,
        ..Default::default()
    };
    for (i, m) in raw.messages.iter().enumerate() {
        match m.role.as_str() {
            "tool_call" | "subagent_tool_call" => {
                let record = m.tool.as_ref().ok_or_else(|| {
                    anyhow!(
                        "message {i} has role '{}' but no structured `tool` record — \
                         the binary that wrote this session predates zerostack's \
                         structured tool records (PR #230); rebuild ZS_BIN",
                        m.role
                    )
                })?;
                // The name is the record's, never the content's leading
                // token (design D1); the summary stays the content minus
                // that token, which is what `tool_arg_contains` matches.
                let summary = m
                    .content
                    .split_once(char::is_whitespace)
                    .map_or("", |(_, tail)| tail.trim())
                    .to_string();
                t.tool_calls.push(ToolCall {
                    index: i,
                    name: record.name.clone(),
                    summary,
                    subagent: m.role == "subagent_tool_call",
                });
            }
            "assistant" => t.final_assistant = m.content.clone(),
            _ => {}
        }
        t.messages.push(Msg {
            role: m.role.clone(),
            content: m.content.clone(),
        });
    }
    Ok(t)
}

#[cfg(test)]
mod tool_record_tests {
    use super::*;

    /// spec `session-evidence`, "A structured tool record becomes a tool
    /// call".
    #[test]
    fn a_structured_record_becomes_a_tool_call() {
        let t = parse_str(
            r#"{"id":"s","messages":[
                {"role":"user","content":"list the files"},
                {"role":"tool_call","content":"bash ls -la","tool":{"id":3,"name":"bash","args":{"command":"ls -la"}}}
            ]}"#,
        )
        .unwrap();
        assert_eq!(t.tool_calls.len(), 1, "{:?}", t.tool_calls);
        assert_eq!(t.tool_calls[0].name, "bash");
        assert_eq!(t.tool_calls[0].summary, "ls -la");
        assert!(!t.tool_calls[0].subagent);
    }

    /// The record, not the display text, is the authority for the name.
    /// Upstream always renders `content` as `"{name} {summary}"`, so a real
    /// session never diverges — the divergence here is synthetic, and it is
    /// what pins that `name` is read from `tool.name` rather than tokenized
    /// off `content` (design D1).
    #[test]
    fn the_name_comes_from_the_record_not_the_content_token() {
        let t = parse_str(
            r#"{"id":"s","messages":[
                {"role":"tool_call","content":"display-token deploy-strategy","tool":{"id":0,"name":"memory_read","args":{"name":"deploy-strategy"}}}
            ]}"#,
        )
        .unwrap();
        assert_eq!(t.tool_calls[0].name, "memory_read");
        assert_eq!(
            t.tool_calls[0].summary, "deploy-strategy",
            "the summary is still the content minus its leading token, so \
             tool_arg_contains keeps its meaning"
        );
    }

    /// spec `session-evidence`, "A subagent record is a subagent call".
    #[test]
    fn a_subagent_record_is_a_subagent_call() {
        let t = parse_str(
            r#"{"id":"s","messages":[
                {"role":"tool_call","content":"task investigate","tool":{"id":0,"name":"task","args":{"prompts":["investigate"]}}},
                {"role":"subagent_tool_call","content":"read src/main.rs","tool":{"parent_call_id":0,"name":"read","args":{"path":"src/main.rs"}}}
            ]}"#,
        )
        .unwrap();
        assert_eq!(t.tool_calls.len(), 2, "{:?}", t.tool_calls);
        assert_eq!(t.tool_calls[1].name, "read");
        assert!(t.tool_calls[1].subagent);
        assert!(!t.tool_calls[0].subagent);
    }

    /// A `tool_result`-role message carries a record too, but it is a
    /// message, not a call (design D1).
    #[test]
    fn a_tool_result_record_is_not_a_call() {
        let t = parse_str(
            r#"{"id":"s","messages":[
                {"role":"tool_call","content":"bash ls","tool":{"id":0,"name":"bash","args":{"command":"ls"}}},
                {"role":"tool_result","content":"bash:\nfile.txt","tool":{"call_id":0,"name":"bash","truncated":false,"full_output_path":null}}
            ]}"#,
        )
        .unwrap();
        assert_eq!(t.tool_calls.len(), 1, "{:?}", t.tool_calls);
        assert_eq!(t.messages.len(), 2);
    }

    /// spec `session-evidence`, "A tool-call-role message without a tool
    /// record is a schema error" — the rule that makes a pre-#230 `ZS_BIN`
    /// visible instead of silently gradable.
    #[test]
    fn a_tool_call_role_message_without_a_record_is_a_schema_error() {
        for role in ["tool_call", "subagent_tool_call"] {
            let text =
                format!(r#"{{"id":"s","messages":[{{"role":"{role}","content":"bash ls"}}]}}"#);
            assert!(
                parse_str(&text).is_err(),
                "a {role} message with no tool record must not parse"
            );
        }
    }

    #[test]
    fn the_missing_record_error_names_the_session_file() {
        let dir = std::env::temp_dir().join(format!("zseval-norecord-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let session = dir.join("no-record.json");
        std::fs::write(
            &session,
            r#"{"id":"s","messages":[{"role":"tool_call","content":"bash ls"}]}"#,
        )
        .unwrap();
        let err = parse_file(&session).unwrap_err();
        let rendered = format!("{err:#}");
        assert!(
            rendered.contains("no-record.json"),
            "the error must name the session file: {rendered}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod prompt_readback_tests {
    use super::*;

    /// spec `session-evidence`, "A recorded prompt is exposed".
    #[test]
    fn a_recorded_prompt_is_exposed() {
        let t =
            parse_str(r#"{"id":"s","messages":[],"prompt":{"name":"code","source":"built_in"}}"#)
                .unwrap();
        let prompt = t.prompt.expect("prompt readback must be present");
        assert_eq!(prompt.name, "code");
        assert_eq!(prompt.source, "built_in");
    }

    /// spec `session-evidence`, "An absent prompt field is exposed as
    /// absent" — a session predating PR #228 must parse, not error.
    #[test]
    fn an_absent_prompt_field_is_exposed_as_absent() {
        let t = parse_str(r#"{"id":"s","messages":[]}"#).unwrap();
        assert!(t.prompt.is_none());
    }

    /// spec `session-evidence`, "An unrecognized source is a schema error".
    #[test]
    fn an_unrecognized_source_is_a_schema_error() {
        let dir = std::env::temp_dir().join(format!("zseval-badsource-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let session = dir.join("bad-source.json");
        std::fs::write(
            &session,
            r#"{"id":"s","messages":[],"prompt":{"name":"code","source":"global_file"}}"#,
        )
        .unwrap();
        let err = parse_file(&session).unwrap_err();
        let rendered = format!("{err:#}");
        assert!(
            rendered.contains("bad-source.json"),
            "the error must name the session file: {rendered}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// design D3: `absorb` applies last-wins, matching `final_assistant`'s
    /// existing rule — a later session with no `prompt` field must not
    /// erase an earlier session's readback, but a later session that does
    /// carry one overrides.
    #[test]
    fn absorb_applies_last_wins_but_keeps_earlier_when_later_has_none() {
        let mut t =
            parse_str(r#"{"id":"a","messages":[],"prompt":{"name":"ask","source":"built_in"}}"#)
                .unwrap();

        let with_no_prompt = parse_str(r#"{"id":"b","messages":[]}"#).unwrap();
        t.absorb(with_no_prompt);
        assert_eq!(
            t.prompt.as_ref().map(|p| p.name.as_str()),
            Some("ask"),
            "a later session with no prompt field must not erase the earlier readback"
        );

        let switched =
            parse_str(r#"{"id":"c","messages":[],"prompt":{"name":"code","source":"user_file"}}"#)
                .unwrap();
        t.absorb(switched);
        let prompt = t.prompt.as_ref().expect("prompt must still be present");
        assert_eq!(prompt.name, "code");
        assert_eq!(prompt.source, "user_file");
    }
}

#[cfg(test)]
mod from_run_tests {
    use super::*;
    use crate::backend::{RunArtifacts, TurnArtifacts};

    #[test]
    fn collects_messages_tokens_and_tool_records_from_the_session() {
        let dir = std::env::temp_dir().join(format!("zseval-fromrun-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let session = dir.join("session.json");
        std::fs::write(
            &session,
            r#"{"id":"s","messages":[
                {"role":"user","content":"hi"},
                {"role":"tool_call","content":"bash ls","tool":{"id":0,"name":"bash","args":{"command":"ls"}}},
                {"role":"tool_result","content":"bash:\nfile.txt","tool":{"call_id":0,"name":"bash","truncated":false,"full_output_path":null}},
                {"role":"assistant","content":"hello"}
            ],"total_input_tokens":10,"total_output_tokens":5,"total_cost":0.01}"#,
        )
        .unwrap();

        let artifacts = RunArtifacts {
            session_files: vec![session],
            turns: Vec::new(),
            data_dir: dir.clone(),
            config_dir: dir.clone(),
            work_dir: dir.clone(),
            wall_secs: 0.0,
        };

        let t = Transcript::from_run(&artifacts).unwrap();
        assert_eq!(t.final_assistant, "hello");
        assert_eq!(t.input_tokens, 10);
        assert_eq!(t.tool_calls.len(), 1, "{:?}", t.tool_calls);
        assert_eq!(t.tool_calls[0].name, "bash");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// spec `session-evidence`, "Stdout markers are not evidence" — the
    /// test that makes D1's deletion observable rather than assumed.
    #[test]
    fn stdout_markers_are_not_evidence() {
        let dir = std::env::temp_dir().join(format!("zseval-nomarkers-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let session = dir.join("session.json");
        std::fs::write(
            &session,
            r#"{"id":"s","messages":[{"role":"user","content":"hi"},{"role":"assistant","content":"hello"}]}"#,
        )
        .unwrap();
        let stdout = dir.join("turn-0.stdout");
        std::fs::write(&stdout, "◈ bash ls\n◈ bash result:\nfile.txt\n").unwrap();

        let artifacts = RunArtifacts {
            session_files: vec![session],
            turns: vec![TurnArtifacts {
                stdout,
                stderr: dir.join("turn-0.stderr"),
                zslog: dir.join("turn-0.zslog"),
            }],
            data_dir: dir.clone(),
            config_dir: dir.clone(),
            work_dir: dir.clone(),
            wall_secs: 0.0,
        };

        let t = Transcript::from_run(&artifacts).unwrap();
        assert!(
            t.tool_calls.is_empty(),
            "the stdout log is a diagnostic artifact, not evidence: {:?}",
            t.tool_calls
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn propagates_session_schema_error_as_err() {
        let dir = std::env::temp_dir().join(format!("zseval-fromrun-err-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let session = dir.join("bad.json");
        std::fs::write(&session, "{\"totally\":\"different\"}").unwrap();
        let artifacts = RunArtifacts {
            session_files: vec![session],
            turns: Vec::new(),
            data_dir: dir.clone(),
            config_dir: dir.clone(),
            work_dir: dir.clone(),
            wall_secs: 0.0,
        };
        assert!(Transcript::from_run(&artifacts).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod token_fallback_tests {
    use super::*;

    #[test]
    fn real_usage_wins_when_present() {
        let t = parse_str(
            r#"{"id":"a","messages":[],"total_input_tokens":100,"total_output_tokens":50,"total_estimated_tokens":9999}"#,
        )
        .unwrap();
        assert_eq!(
            t.total_tokens(),
            150,
            "real usage must not be shadowed by the estimate"
        );
    }

    #[test]
    fn falls_back_to_estimate_when_real_usage_is_zero() {
        // Headless mode as of 2026-07-06: real usage is always 0, but
        // zerostack's own estimate is populated — tokens_under/
        // max_total_tokens would be a silent no-op without this fallback.
        let t = parse_str(r#"{"id":"a","messages":[],"total_estimated_tokens":37}"#).unwrap();
        assert_eq!(t.total_tokens(), 37);
    }

    #[test]
    fn zero_is_zero_when_neither_is_reported() {
        let t = parse_str(r#"{"id":"a","messages":[]}"#).unwrap();
        assert_eq!(t.total_tokens(), 0);
    }

    #[test]
    fn absorb_sums_estimated_tokens_across_sessions() {
        let mut t = parse_str(r#"{"id":"a","messages":[],"total_estimated_tokens":10}"#).unwrap();
        let u = parse_str(r#"{"id":"b","messages":[],"total_estimated_tokens":15}"#).unwrap();
        t.absorb(u);
        assert_eq!(t.total_tokens(), 25);
    }
}
