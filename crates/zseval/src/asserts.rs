//! Deterministic assert mini-DSL — the deterministic floor of every scenario.
//!
//! One assert per line. The op is the first token; needle args consume the
//! rest of the line verbatim, so needles may contain spaces.
//!
//!   tool_called <name>
//!   tool_not_called <name>
//!   tool_called_after <later> <earlier>       # order in the transcript
//!   tool_count <name> <op> <n>                # op: < <= == >= >
//!   tool_arg_contains <name> <needle...>      # matches the summary string
//!   no_tool_call_contains <needle...>         # injection canary guard
//!   final_contains <needle...>
//!   final_not_contains <needle...>
//!   final_max_lines <n>                       # conciseness (non-empty lines)
//!   transcript_contains <needle...>
//!   transcript_not_contains <needle...>
//!   tokens_under <n>
//!   file_contains <path> <needle...>       # outcome check
//!   file_not_contains <path> <needle...>   # supports one *
//!
//! `file_*` grade the environment, not the transcript — an on-disk effect only
//! counts if the bytes are there. Paths allow a single `*` path segment, e.g.
//! `projects/*/OUT.md`. `<path>` is rooted at the run's throwaway `ZS_DATA_DIR`
//! by default; prefix it with `config:` or `work:` to check the isolated
//! config dir or working dir instead (e.g. `config:agent/memory/MEMORY.md`).

use std::path::Path;

use anyhow::{bail, Result};

use crate::seed::SeedCtx;
use crate::transcript::Transcript;

#[derive(Debug, Clone, PartialEq)]
pub enum Assert {
    ToolCalled(String),
    ToolNotCalled(String),
    ToolCalledAfter { later: String, earlier: String },
    ToolCount { tool: String, op: CmpOp, n: usize },
    ToolArgContains { tool: String, needle: String },
    NoToolCallContains(String),
    FinalContains(String),
    FinalNotContains(String),
    FinalMaxLines(usize),
    TranscriptContains(String),
    TranscriptNotContains(String),
    TokensUnder(u64),
    FileContains { path: String, needle: String },
    FileNotContains { path: String, needle: String },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CmpOp {
    Lt,
    Le,
    Eq,
    Ge,
    Gt,
}

impl CmpOp {
    fn parse(s: &str) -> Result<CmpOp> {
        Ok(match s {
            "<" => CmpOp::Lt,
            "<=" => CmpOp::Le,
            "==" => CmpOp::Eq,
            ">=" => CmpOp::Ge,
            ">" => CmpOp::Gt,
            _ => bail!("bad comparison op '{s}'"),
        })
    }
    fn eval(&self, a: usize, b: usize) -> bool {
        match self {
            CmpOp::Lt => a < b,
            CmpOp::Le => a <= b,
            CmpOp::Eq => a == b,
            CmpOp::Ge => a >= b,
            CmpOp::Gt => a > b,
        }
    }
    fn sym(&self) -> &'static str {
        match self {
            CmpOp::Lt => "<",
            CmpOp::Le => "<=",
            CmpOp::Eq => "==",
            CmpOp::Ge => ">=",
            CmpOp::Gt => ">",
        }
    }
}

fn split1(rest: &str) -> Result<(String, String)> {
    match rest.trim().split_once(char::is_whitespace) {
        Some((a, b)) if !b.trim().is_empty() => Ok((a.to_string(), b.trim().to_string())),
        _ => bail!("expected two arguments, got '{rest}'"),
    }
}

impl Assert {
    pub fn parse(line: &str) -> Result<Assert> {
        let line = line.trim();
        let (op, rest) = line
            .split_once(char::is_whitespace)
            .map(|(a, b)| (a, b.trim()))
            .unwrap_or((line, ""));
        if rest.is_empty() {
            bail!("assert '{op}' needs arguments");
        }
        Ok(match op {
            "tool_called" => Assert::ToolCalled(rest.to_string()),
            "tool_not_called" => Assert::ToolNotCalled(rest.to_string()),
            "tool_called_after" => {
                let (later, earlier) = split1(rest)?;
                Assert::ToolCalledAfter { later, earlier }
            }
            "tool_count" => {
                let mut it = rest.split_whitespace();
                let tool = it.next().unwrap_or("").to_string();
                let op = CmpOp::parse(it.next().unwrap_or(""))?;
                let n: usize = it
                    .next()
                    .unwrap_or("")
                    .parse()
                    .map_err(|_| anyhow::anyhow!("tool_count: bad number in '{rest}'"))?;
                Assert::ToolCount { tool, op, n }
            }
            "tool_arg_contains" => {
                let (tool, needle) = split1(rest)?;
                Assert::ToolArgContains { tool, needle }
            }
            "no_tool_call_contains" => Assert::NoToolCallContains(rest.to_string()),
            "final_contains" => Assert::FinalContains(rest.to_string()),
            "final_not_contains" => Assert::FinalNotContains(rest.to_string()),
            "final_max_lines" => Assert::FinalMaxLines(
                rest.parse()
                    .map_err(|_| anyhow::anyhow!("final_max_lines: bad number '{rest}'"))?,
            ),
            "transcript_contains" => Assert::TranscriptContains(rest.to_string()),
            "transcript_not_contains" => Assert::TranscriptNotContains(rest.to_string()),
            "tokens_under" => Assert::TokensUnder(
                rest.parse()
                    .map_err(|_| anyhow::anyhow!("tokens_under: bad number '{rest}'"))?,
            ),
            "file_contains" => {
                let (path, needle) = split1(rest)?;
                Assert::FileContains { path, needle }
            }
            "file_not_contains" => {
                let (path, needle) = split1(rest)?;
                Assert::FileNotContains { path, needle }
            }
            other => bail!("unknown assert op '{other}'"),
        })
    }

    /// `roots` gives `file_*` asserts the run's three isolated dirs
    /// (data/config/work) to resolve a possibly-prefixed path against.
    pub fn eval(&self, t: &Transcript, roots: &SeedCtx) -> AssertResult {
        let (pass, detail) = match self {
            Assert::ToolCalled(name) => {
                let hit = t.tool_calls.iter().any(|c| &c.name == name);
                (hit, format!("tool '{name}' called: {hit}"))
            }
            Assert::ToolNotCalled(name) => {
                let n = t.tool_calls.iter().filter(|c| &c.name == name).count();
                (n == 0, format!("tool '{name}' called {n} time(s)"))
            }
            Assert::ToolCalledAfter { later, earlier } => {
                let first_earlier = t.tool_calls.iter().find(|c| &c.name == earlier);
                let later_hit = match first_earlier {
                    Some(e) => t
                        .tool_calls
                        .iter()
                        .any(|c| &c.name == later && c.index > e.index),
                    None => false,
                };
                (
                    later_hit,
                    format!("'{later}' after '{earlier}': {later_hit}"),
                )
            }
            Assert::ToolCount { tool, op, n } => {
                let count = t.tool_calls.iter().filter(|c| &c.name == tool).count();
                (
                    op.eval(count, *n),
                    format!("count('{tool}')={count}, want {} {n}", op.sym()),
                )
            }
            Assert::ToolArgContains { tool, needle } => {
                let hit = t
                    .tool_calls
                    .iter()
                    .any(|c| &c.name == tool && c.summary.contains(needle.as_str()));
                (hit, format!("'{tool}' summary contains '{needle}': {hit}"))
            }
            Assert::NoToolCallContains(needle) => {
                let offender = t.tool_calls.iter().find(|c| {
                    c.name.contains(needle.as_str()) || c.summary.contains(needle.as_str())
                });
                match offender {
                    Some(c) => (false, format!("tool call matched canary: {} {}", c.name, c.summary)),
                    None => (true, format!("no tool call contains '{needle}'")),
                }
            }
            Assert::FinalContains(n) => {
                let hit = t.final_assistant.contains(n.as_str());
                (hit, format!("final contains '{n}': {hit}"))
            }
            Assert::FinalNotContains(n) => {
                let hit = t.final_assistant.contains(n.as_str());
                (!hit, format!("final contains '{n}': {hit}"))
            }
            Assert::FinalMaxLines(max) => {
                let lines = t
                    .final_assistant
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .count();
                (lines <= *max, format!("final non-empty lines {lines} <= {max}"))
            }
            Assert::TranscriptContains(n) => {
                let hit = t.messages.iter().any(|m| m.content.contains(n.as_str()));
                (hit, format!("transcript contains '{n}': {hit}"))
            }
            Assert::TranscriptNotContains(n) => {
                let hit = t.messages.iter().any(|m| m.content.contains(n.as_str()));
                (!hit, format!("transcript contains '{n}': {hit}"))
            }
            Assert::TokensUnder(limit) => {
                let total = t.total_tokens();
                (total < *limit, format!("total tokens {total} < {limit}"))
            }
            Assert::FileContains { path, needle } => match read_glob(roots, path) {
                Ok(contents) => {
                    let hit = contents.iter().any(|(_, c)| c.contains(needle.as_str()));
                    (hit, format!("some '{path}' contains '{needle}': {hit}"))
                }
                Err(e) => (false, format!("file_contains '{path}': {e}")),
            },
            Assert::FileNotContains { path, needle } => match read_glob(roots, path) {
                Ok(contents) => {
                    let offender = contents.iter().find(|(_, c)| c.contains(needle.as_str()));
                    match offender {
                        Some((p, _)) => (false, format!("'{p}' contains '{needle}'")),
                        None => (true, format!("no '{path}' contains '{needle}'")),
                    }
                }
                // Missing files trivially don't contain the needle.
                Err(_) => (true, format!("no files matched '{path}'")),
            },
        };
        AssertResult {
            spec: self.spec(),
            pass,
            detail,
        }
    }

    fn spec(&self) -> String {
        format!("{self:?}")
    }
}

/// Split a `file_*` path into the run-root it targets and the path relative
/// to that root. No prefix defaults to `data:`, matching every scenario
/// written before `config:`/`work:` prefixes existed.
fn resolve_root<'a>(pattern: &'a str, roots: &SeedCtx<'a>) -> (&'a Path, &'a str) {
    match pattern.split_once(':') {
        Some(("data", rest)) => (roots.data, rest),
        Some(("config", rest)) => (roots.config, rest),
        Some(("work", rest)) => (roots.work, rest),
        _ => (roots.data, pattern),
    }
}

/// Read files under `root/pattern` where pattern may contain at most one `*`
/// path segment (e.g. `agent/memory/projects/*/notes/foo.md`).
fn read_glob(roots: &SeedCtx, pattern: &str) -> Result<Vec<(String, String)>> {
    let (root, pattern) = resolve_root(pattern, roots);
    let mut out = Vec::new();
    if let Some(star_pos) = pattern.find('*') {
        let (prefix, suffix) = pattern.split_at(star_pos);
        let suffix = suffix.trim_start_matches('*').trim_start_matches('/');
        let prefix_dir = root.join(prefix.trim_end_matches('/'));
        let entries = std::fs::read_dir(&prefix_dir)
            .map_err(|e| anyhow::anyhow!("{}: {e}", prefix_dir.display()))?;
        for entry in entries.flatten() {
            let candidate = if suffix.is_empty() {
                entry.path()
            } else {
                entry.path().join(suffix)
            };
            if candidate.is_file() {
                if let Ok(c) = std::fs::read_to_string(&candidate) {
                    out.push((candidate.display().to_string(), c));
                }
            }
        }
    } else {
        let p = root.join(pattern);
        let c = std::fs::read_to_string(&p)
            .map_err(|e| anyhow::anyhow!("{}: {e}", p.display()))?;
        out.push((p.display().to_string(), c));
    }
    if out.is_empty() {
        bail!("no files matched '{pattern}'");
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AssertResult {
    pub spec: String,
    pub pass: bool,
    pub detail: String,
}
