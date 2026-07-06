//! Parse a zerostack session JSON into a normalized `Transcript`.
//!
//! Contract (mirrors zerostack's session storage):
//! - Sessions persist to `{ZS_DATA_DIR}/sessions/{id}.json` as a whole
//!   `Session { id, messages, total_input_tokens, total_output_tokens,
//!   total_cost, ... }` with `SessionMessage { role, content }`.
//! - `role` is snake_case: user | assistant | system | tool_call |
//!   tool_result | subagent_tool_call.
//! - Tool-call content is `"{name}"` or `"{name} {summary...}"` — the tool
//!   name is always the first whitespace token. Full args are not persisted,
//!   so arg asserts match the summary string; outcome asserts (`file_*`)
//!   check the environment instead.
//!
//! A schema we cannot parse becomes an `Err`, which the runner maps to an
//! `Indeterminate` verdict — an unreadable transcript is not an agent
//! failure. If zerostack's schema changes, adapt this file.
//!
//! `tool_call`/`tool_result` roles above are only ever written by
//! zerostack's *interactive* TUI (`ui/event_handler.rs`) — the headless `-p`
//! path (`agent/runner.rs::run_print`) never calls `Session::add_tool_call`,
//! so a real session JSON from this harness's backend never contains one.
//! The only place tool calls surface in headless mode is stdout, and only
//! with `--pure-stdout`: `◈ {name} {summary}` for a call, `◈ {name}
//! result:` (followed by the tool's output) for its result. `backend::ZsCli`
//! always passes `--pure-stdout` and captures each turn's stdout to
//! `turn-N.stdout`; `tool_calls_from_stdout` below reconstructs `ToolCall`s
//! from that text. The mock backend has no stdout logs, so its fixtures keep
//! authoring tool calls directly in the session JSON as before — both paths
//! feed the same `Transcript.tool_calls`.

use std::path::Path;

use anyhow::{Context, Result};
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
}

#[derive(Debug, Clone, Deserialize)]
struct RawMessage {
    role: String,
    content: String,
}

#[derive(Debug, Clone)]
pub struct Msg {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct ToolCall {
    /// A monotonic position for ordering asserts (`tool_called_after`) to
    /// compare against — *not* an index into `messages`. Tool calls parsed
    /// from a session's own messages (`tool_call`/`subagent_tool_call` roles)
    /// do use their message index; ones reconstructed from a `turn-N.stdout`
    /// log (the real headless-mode path — see this module's doc) instead
    /// count up from `index_base` across that turn's `◈ ` markers, since
    /// there's no message list to index into. Both number spaces only need
    /// to preserve relative order among tool calls, which they do.
    pub index: usize,
    pub name: String,
    /// Human summary of args as zerostack rendered it (may be truncated).
    pub summary: String,
    pub subagent: bool,
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
    /// tokens, and cost from every session file, plus tool calls from every
    /// turn's `--pure-stdout` log. This is the one entry point the runner
    /// needs — it doesn't know or care that "evidence" currently comes from
    /// two different channels depending on the backend (session JSON for the
    /// mock backend's fixtures, stdout markers for a real `zerostack` run;
    /// see the module doc for why). A schema mismatch in any session file is
    /// an `Err`, which the caller maps to Indeterminate.
    pub fn from_run(artifacts: &crate::backend::RunArtifacts) -> Result<Transcript> {
        let mut t = Transcript::default();
        for f in &artifacts.session_files {
            t.absorb(parse_file(f)?);
        }
        for turn in &artifacts.turns {
            let base = t.tool_calls.len();
            t.tool_calls
                .extend(tool_calls_from_stdout_file(&turn.stdout, base));
        }
        Ok(t)
    }
}

/// Reconstruct `ToolCall`s from a captured `turn-N.stdout` log (see the
/// module doc for why this, and not session JSON, is the real source in
/// headless mode). `index_base` lets a caller keep indices monotonic across
/// several turns' logs, so `tool_called_after` orders correctly.
///
/// A `◈ {name} result:` line marks a tool's *result*, not another call, and
/// is skipped — its output lines don't start with `◈ ` so nothing else needs
/// filtering out. Caveat: this is a line-prefix match against zerostack's own
/// marker, not a structured format — a tool's *output* that happens to
/// contain a line starting with `◈ ` (e.g. `bash cat`-ing a file using that
/// character) would be misread as another call. Unlikely in practice and not
/// currently guarded against; this is the one channel that carries tool
/// calls at all in headless mode, so there's no independent source to
/// cross-check it against (see the module doc).
pub fn tool_calls_from_stdout(text: &str, index_base: usize) -> Vec<ToolCall> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("◈ ") else {
            continue;
        };
        let (name, tail) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        let tail = tail.trim();
        if tail == "result:" {
            continue;
        }
        out.push(ToolCall {
            index: index_base + out.len(),
            name: name.to_string(),
            summary: tail.to_string(),
            subagent: false,
        });
    }
    out
}

/// `tool_calls_from_stdout` over a `turn-N.stdout` file on disk. A missing or
/// unreadable file yields no tool calls rather than an error — every
/// scenario has a `.stdout` log only when driven by the real CLI backend
/// (the mock backend has none), so "no file" is an expected, silent no-op.
pub fn tool_calls_from_stdout_file(path: &Path, index_base: usize) -> Vec<ToolCall> {
    match std::fs::read_to_string(path) {
        Ok(text) => tool_calls_from_stdout(&text, index_base),
        Err(_) => Vec::new(),
    }
}

pub fn parse_file(path: &Path) -> Result<Transcript> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("read session {}", path.display()))?;
    parse_str(&text).with_context(|| format!("parse session {}", path.display()))
}

pub fn parse_str(text: &str) -> Result<Transcript> {
    let raw: RawSession = serde_json::from_str(text).context("session schema mismatch")?;
    let mut t = Transcript {
        input_tokens: raw.total_input_tokens,
        output_tokens: raw.total_output_tokens,
        cost_usd: raw.total_cost,
        estimated_tokens: raw.total_estimated_tokens,
        ..Default::default()
    };
    for (i, m) in raw.messages.iter().enumerate() {
        match m.role.as_str() {
            "tool_call" | "subagent_tool_call" => {
                let mut parts = m.content.splitn(2, char::is_whitespace);
                let name = parts.next().unwrap_or("").to_string();
                let summary = parts.next().unwrap_or("").trim().to_string();
                t.tool_calls.push(ToolCall {
                    index: i,
                    name,
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
mod stdout_tool_call_tests {
    use super::*;

    #[test]
    fn parses_call_and_skips_result_marker() {
        let text = "\n◈ bash python3 -c 'print(2+2)'\n◈ bash result:\n4\n\n**4**\n";
        let calls = tool_calls_from_stdout(text, 0);
        assert_eq!(calls.len(), 1, "{calls:?}");
        assert_eq!(calls[0].name, "bash");
        assert_eq!(calls[0].summary, "python3 -c 'print(2+2)'");
        assert_eq!(calls[0].index, 0);
    }

    #[test]
    fn handles_empty_summary() {
        // format_tool_args_summary can return "", leaving a trailing space:
        // `println!("\n◈ {} {}", name, "")` -> "◈ memory_search ".
        let text = "◈ memory_search \n◈ memory_search result:\nNo matches.\n";
        let calls = tool_calls_from_stdout(text, 0);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "memory_search");
        assert_eq!(calls[0].summary, "");
    }

    #[test]
    fn preserves_order_across_multiple_calls_and_respects_index_base() {
        let text = "◈ memory_search deploy\n◈ memory_search result:\nsnippet\n\
                    ◈ memory_read deploy-strategy\n◈ memory_read result:\nfull text\n";
        let calls = tool_calls_from_stdout(text, 5);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "memory_search");
        assert_eq!(calls[0].index, 5);
        assert_eq!(calls[1].name, "memory_read");
        assert_eq!(calls[1].index, 6);
        assert!(calls[0].index < calls[1].index);
    }

    #[test]
    fn missing_stdout_file_yields_no_calls_not_an_error() {
        let calls = tool_calls_from_stdout_file(Path::new("/no/such/file.stdout"), 0);
        assert!(calls.is_empty());
    }
}

#[cfg(test)]
mod from_run_tests {
    use super::*;
    use crate::backend::{RunArtifacts, TurnArtifacts};

    #[test]
    fn merges_session_messages_with_stdout_tool_calls() {
        let dir = std::env::temp_dir().join(format!("zseval-fromrun-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let session = dir.join("session.json");
        std::fs::write(
            &session,
            r#"{"id":"s","messages":[{"role":"user","content":"hi"},{"role":"assistant","content":"hello"}],"total_input_tokens":10,"total_output_tokens":5,"total_cost":0.01}"#,
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
        assert_eq!(t.final_assistant, "hello");
        assert_eq!(t.input_tokens, 10);
        assert_eq!(t.tool_calls.len(), 1, "{:?}", t.tool_calls);
        assert_eq!(t.tool_calls[0].name, "bash");
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
