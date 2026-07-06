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
    /// Index into `messages`, so ordering asserts can compare positions.
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
    }

    pub fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens
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
}

/// Reconstruct `ToolCall`s from a captured `turn-N.stdout` log (see the
/// module doc for why this, and not session JSON, is the real source in
/// headless mode). `index_base` lets a caller keep indices monotonic across
/// several turns' logs, so `tool_called_after` orders correctly.
///
/// A `◈ {name} result:` line marks a tool's *result*, not another call, and
/// is skipped — its output lines don't start with `◈ ` so nothing else needs
/// filtering out.
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
