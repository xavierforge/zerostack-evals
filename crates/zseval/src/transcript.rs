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
