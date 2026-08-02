//! Deterministic assert mini-DSL — the deterministic floor of every scenario.
//!
//! One assert per line. The op is the first token; needle args consume the
//! rest of the line verbatim, so needles may contain spaces.
//!
//!   tool_called <name>
//!   tool_called_any                            # any tool call recorded, name-agnostic (design D6, section 10);
//!                                                 # no argument, and fails rather than passing vacuously
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
//!   prompt_recorded <name> <built_in|user_file>  # session's raw readback (design D6);
//!                                                 # not this crate's four-way prompt_source, and
//!                                                 # fails rather than passing when nothing was recorded
//!   file_contains <path> <needle...>       # outcome check; fails if nothing matches
//!   file_not_contains <path> <needle...>   # supports one *; fails if nothing matches
//!   path_not_exists <path>                 # supports one *; dirs count as existing
//!
//! `file_*`/`path_not_exists` grade the environment, not the transcript — an
//! on-disk effect only counts if the bytes (or the path itself) are there.
//! Paths allow a single `*` path segment, e.g. `projects/*/OUT.md`. `<path>`
//! is rooted at the run's throwaway `ZS_DATA_DIR` by default; prefix it with
//! `config:` or `work:` to check the isolated config dir or working dir
//! instead (e.g. `config:agent/memory/MEMORY.md`). `file_contains`/
//! `file_not_contains` read files only and fail when zero files match (a
//! missing file or an empty/missing glob dir is not evidence either way);
//! `path_not_exists` is the complement — it passes exactly when zero
//! filesystem entries (files or directories) match.

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

use crate::backend::RunRoots;
use crate::transcript::Transcript;

#[derive(Debug, Clone, PartialEq)]
pub enum Assert {
    ToolCalled(String),
    /// Any tool call, whatever its name (design D6, section 10): a
    /// name-agnostic evidence-channel pin, deliberately not `tool_count *
    /// >= 1`, since `*` in a name slot would then have to mean something
    /// for `tool_not_called`/`tool_arg_contains` too, and it does not.
    ToolCalledAny,
    ToolNotCalled(String),
    ToolCalledAfter {
        later: String,
        earlier: String,
    },
    ToolCount {
        tool: String,
        op: CmpOp,
        n: usize,
    },
    ToolArgContains {
        tool: String,
        needle: String,
    },
    NoToolCallContains(String),
    FinalContains(String),
    FinalNotContains(String),
    FinalMaxLines(usize),
    TranscriptContains(String),
    TranscriptNotContains(String),
    TokensUnder(u64),
    PromptRecorded {
        name: String,
        source: String,
    },
    FileContains {
        path: String,
        needle: String,
    },
    FileNotContains {
        path: String,
        needle: String,
    },
    PathNotExists(String),
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
        // `tool_called_any` is the one op that takes no argument; every
        // other op needs the `rest` this guard requires.
        if rest.is_empty() && op != "tool_called_any" {
            bail!("assert '{op}' needs arguments");
        }
        Ok(match op {
            "tool_called" => Assert::ToolCalled(rest.to_string()),
            "tool_called_any" => {
                if !rest.is_empty() {
                    bail!("tool_called_any takes no argument, got '{rest}'");
                }
                Assert::ToolCalledAny
            }
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
            "prompt_recorded" => {
                let (name, source) = split1(rest)?;
                if source != "built_in" && source != "user_file" {
                    bail!("prompt_recorded: bad source '{source}', want 'built_in' or 'user_file'");
                }
                Assert::PromptRecorded { name, source }
            }
            "file_contains" => {
                let (path, needle) = split1(rest)?;
                Assert::FileContains { path, needle }
            }
            "file_not_contains" => {
                let (path, needle) = split1(rest)?;
                Assert::FileNotContains { path, needle }
            }
            "path_not_exists" => {
                let path = rest.to_string();
                let stars = path.matches('*').count();
                if stars > 1 {
                    bail!(
                        "path_not_exists: at most one '*' path segment allowed, got {stars} in '{path}'"
                    );
                }
                Assert::PathNotExists(path)
            }
            other => bail!("unknown assert op '{other}'"),
        })
    }

    /// `roots` gives `file_*` asserts the run's three isolated dirs
    /// (data/config/work) to resolve a possibly-prefixed path against.
    pub fn eval(&self, t: &Transcript, roots: &RunRoots) -> AssertResult {
        let (pass, detail) = match self {
            Assert::ToolCalled(name) => {
                let hit = t.tool_calls.iter().any(|c| &c.name == name);
                (hit, format!("tool '{name}' called: {hit}"))
            }
            // Name-agnostic: fails on an empty transcript rather than
            // passing vacuously, same posture as `PromptRecorded`'s `None`
            // arm below.
            Assert::ToolCalledAny => {
                let n = t.tool_calls.len();
                (n > 0, format!("any tool called: {n} recorded"))
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
                    Some(c) => (
                        false,
                        format!("tool call matched canary: {} {}", c.name, c.summary),
                    ),
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
                (
                    lines <= *max,
                    format!("final non-empty lines {lines} <= {max}"),
                )
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
            // Grades the session's raw readback, not this crate's four-way
            // `verdict::PromptSource` (design D6). `None` fails rather than
            // passing vacuously: a trial whose transcript recorded no
            // prompt has no evidence to grade, which is not the same as
            // agreement (this repo's `absence-asserts` posture).
            Assert::PromptRecorded { name, source } => match &t.prompt {
                Some(p) => {
                    let hit = &p.name == name && &p.source == source;
                    (
                        hit,
                        format!(
                            "prompt recorded name='{}' source='{}', want name='{name}' source='{source}': {hit}",
                            p.name, p.source
                        ),
                    )
                }
                None => (
                    false,
                    format!("no prompt recorded, want name='{name}' source='{source}'"),
                ),
            },
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
                // Zero matches asserts nothing: the file existing (and being
                // clean) is part of what this op claims, symmetric with
                // `file_contains`'s own Err arm below.
                Err(e) => (false, format!("file_not_contains '{path}': {e}")),
            },
            Assert::PathNotExists(path) => {
                let hits = glob_hits(roots, path);
                if hits.is_empty() {
                    (true, format!("nothing matches '{path}'"))
                } else {
                    let found: Vec<String> = hits.iter().map(|p| p.display().to_string()).collect();
                    (false, format!("'{path}' exists: {}", found.join(", ")))
                }
            }
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
fn resolve_root<'a>(pattern: &'a str, roots: &RunRoots<'a>) -> (&'a Path, &'a str) {
    match pattern.split_once(':') {
        Some(("data", rest)) => (roots.data, rest),
        Some(("config", rest)) => (roots.config, rest),
        Some(("work", rest)) => (roots.work, rest),
        _ => (roots.data, pattern),
    }
}

/// List every filesystem entry — file or directory — that `pattern` matches
/// under its resolved root, as full paths. Pattern may contain at most one
/// `*` path segment (e.g. `agent/memory/projects/*/notes/foo.md`); the
/// two-or-more-stars case is rejected earlier, at parse time, for
/// `path_not_exists` (see `Assert::parse`).
///
/// Never errors on "nothing matched": a missing single path, a missing glob
/// directory, and an existing-but-empty glob directory all fold into an
/// empty `Vec` — this is the one matcher both the content asserts
/// (`file_contains`/`file_not_contains`, via `read_glob` below) and the
/// existence assert (`path_not_exists`) share, and they disagree on what an
/// empty result means, so the matcher itself stays neutral.
fn glob_hits(roots: &RunRoots, pattern: &str) -> Vec<PathBuf> {
    let (root, pattern) = resolve_root(pattern, roots);
    let mut out = Vec::new();
    if let Some(star_pos) = pattern.find('*') {
        let (prefix, suffix) = pattern.split_at(star_pos);
        let suffix = suffix.trim_start_matches('*').trim_start_matches('/');
        let prefix_dir = root.join(prefix.trim_end_matches('/'));
        if let Ok(entries) = std::fs::read_dir(&prefix_dir) {
            for entry in entries.flatten() {
                let candidate = if suffix.is_empty() {
                    entry.path()
                } else {
                    entry.path().join(suffix)
                };
                if candidate.exists() {
                    out.push(candidate);
                }
            }
        }
    } else {
        let p = root.join(pattern);
        if p.exists() {
            out.push(p);
        }
    }
    out
}

/// Read files under `root/pattern` where pattern may contain at most one `*`
/// path segment. Filters `glob_hits` down to files and reads their contents;
/// zero matching files (missing path, empty/missing glob dir, or hits that
/// were directories only) is an error — the content asserts have no
/// evidence to grade in that case.
fn read_glob(roots: &RunRoots, pattern: &str) -> Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    for candidate in glob_hits(roots, pattern) {
        if candidate.is_file() {
            if let Ok(c) = std::fs::read_to_string(&candidate) {
                out.push((candidate.display().to_string(), c));
            }
        }
    }
    if out.is_empty() {
        bail!("no files matched '{pattern}'");
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AssertResult {
    pub spec: String,
    pub pass: bool,
    pub detail: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::ToolCall;
    use crate::transcript::{RecordedPrompt, Transcript};

    /// A fresh empty directory named after the test, so parallel tests in
    /// this process never share one.
    fn tmp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("zseval-asserts-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A `RunRoots` where all three roots are the same dir — these tests
    /// only care about the matcher/assert logic, not root separation
    /// (already covered in `tests/harness.rs`).
    fn flat_roots(dir: &Path) -> RunRoots<'_> {
        RunRoots {
            data: dir,
            config: dir,
            work: dir,
        }
    }

    #[test]
    fn file_not_contains_fails_on_missing_file() {
        let dir = tmp("fnc-missing-file");
        let t = Transcript::default();
        let roots = flat_roots(&dir);
        let r = Assert::parse("file_not_contains missing.md needle")
            .unwrap()
            .eval(&t, &roots);
        assert!(!r.pass, "{}", r.detail);
        assert!(r.detail.contains("missing.md"), "{}", r.detail);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn file_not_contains_fails_on_zero_hit_glob() {
        let dir = tmp("fnc-zero-hit-glob");
        // `projects/` doesn't exist at all under this root.
        let t = Transcript::default();
        let roots = flat_roots(&dir);
        let r = Assert::parse("file_not_contains projects/*/NOTES.md needle")
            .unwrap()
            .eval(&t, &roots);
        assert!(!r.pass, "{}", r.detail);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn path_not_exists_passes_on_absent_path() {
        let dir = tmp("pne-absent");
        let t = Transcript::default();
        let roots = flat_roots(&dir);
        let r = Assert::parse("path_not_exists debug.log")
            .unwrap()
            .eval(&t, &roots);
        assert!(r.pass, "{}", r.detail);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn path_not_exists_fails_on_existing_file() {
        let dir = tmp("pne-file");
        std::fs::write(dir.join("debug.log"), "x").unwrap();
        let t = Transcript::default();
        let roots = flat_roots(&dir);
        let r = Assert::parse("path_not_exists debug.log")
            .unwrap()
            .eval(&t, &roots);
        assert!(!r.pass, "{}", r.detail);
        assert!(r.detail.contains("debug.log"), "{}", r.detail);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn path_not_exists_fails_on_existing_directory() {
        let dir = tmp("pne-dir");
        std::fs::create_dir_all(dir.join("sessions")).unwrap();
        let t = Transcript::default();
        let roots = flat_roots(&dir);
        let r = Assert::parse("path_not_exists sessions")
            .unwrap()
            .eval(&t, &roots);
        assert!(!r.pass, "{}", r.detail);
        assert!(r.detail.contains("sessions"), "{}", r.detail);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn path_not_exists_passes_on_empty_or_missing_glob_dir() {
        let dir = tmp("pne-glob-empty");
        let t = Transcript::default();
        let roots = flat_roots(&dir);
        // Missing entirely.
        let r = Assert::parse("path_not_exists sessions/*")
            .unwrap()
            .eval(&t, &roots);
        assert!(r.pass, "{}", r.detail);
        // Exists but empty.
        std::fs::create_dir_all(dir.join("sessions")).unwrap();
        let r = Assert::parse("path_not_exists sessions/*")
            .unwrap()
            .eval(&t, &roots);
        assert!(r.pass, "{}", r.detail);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn path_not_exists_fails_on_populated_glob_naming_hits() {
        let dir = tmp("pne-glob-populated");
        std::fs::create_dir_all(dir.join("sessions")).unwrap();
        std::fs::write(dir.join("sessions/a.json"), "x").unwrap();
        let t = Transcript::default();
        let roots = flat_roots(&dir);
        let r = Assert::parse("path_not_exists sessions/*")
            .unwrap()
            .eval(&t, &roots);
        assert!(!r.pass, "{}", r.detail);
        assert!(r.detail.contains("a.json"), "{}", r.detail);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn path_not_exists_rejects_two_star_segments_at_parse_time() {
        // Distinguish "rejected because of the star-count rule" from
        // "rejected because the op doesn't exist at all": the real
        // validation error names the offending character.
        let err = Assert::parse("path_not_exists projects/*/notes/*").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains('*'), "{msg}");
    }

    // spec `session-evidence`, "A scenario can assert the recorded prompt"
    // (design D6): `prompt_recorded <name> <built_in|user_file>` grades
    // against the transcript's raw readback, upstream's own two-value
    // vocabulary, not this crate's four-way `prompt_source`.

    /// spec scenario "Asserting the stock prompt on a bare run".
    #[test]
    fn prompt_recorded_passes_when_the_readback_matches() {
        let dir = tmp("prompt-recorded-match");
        let t = Transcript {
            prompt: Some(RecordedPrompt {
                name: "code".to_string(),
                source: "built_in".to_string(),
            }),
            ..Default::default()
        };
        let roots = flat_roots(&dir);
        let r = Assert::parse("prompt_recorded code built_in")
            .unwrap()
            .eval(&t, &roots);
        assert!(r.pass, "{}", r.detail);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn prompt_recorded_fails_on_a_name_mismatch_naming_both_sides() {
        let dir = tmp("prompt-recorded-name-mismatch");
        let t = Transcript {
            prompt: Some(RecordedPrompt {
                name: "ask".to_string(),
                source: "built_in".to_string(),
            }),
            ..Default::default()
        };
        let roots = flat_roots(&dir);
        let r = Assert::parse("prompt_recorded code built_in")
            .unwrap()
            .eval(&t, &roots);
        assert!(!r.pass, "{}", r.detail);
        assert!(r.detail.contains("ask"), "{}", r.detail);
        assert!(r.detail.contains("code"), "{}", r.detail);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn prompt_recorded_fails_on_a_source_mismatch_naming_both_sides() {
        let dir = tmp("prompt-recorded-source-mismatch");
        let t = Transcript {
            prompt: Some(RecordedPrompt {
                name: "code".to_string(),
                source: "user_file".to_string(),
            }),
            ..Default::default()
        };
        let roots = flat_roots(&dir);
        let r = Assert::parse("prompt_recorded code built_in")
            .unwrap()
            .eval(&t, &roots);
        assert!(!r.pass, "{}", r.detail);
        assert!(r.detail.contains("user_file"), "{}", r.detail);
        assert!(r.detail.contains("built_in"), "{}", r.detail);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// spec scenario "A missing prompt record fails the assert": a
    /// transcript carrying no readback must fail, never pass vacuously
    /// (this repo's `absence-asserts` posture).
    #[test]
    fn prompt_recorded_fails_rather_than_passing_vacuously_when_no_prompt_was_recorded() {
        let dir = tmp("prompt-recorded-absent");
        let t = Transcript::default();
        let roots = flat_roots(&dir);
        let r = Assert::parse("prompt_recorded code built_in")
            .unwrap()
            .eval(&t, &roots);
        assert!(!r.pass, "{}", r.detail);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Upstream's `prompt.source` vocabulary is closed to `built_in` and
    /// `user_file` (`transcript.rs`); a typo'd operand here must be rejected
    /// at parse time rather than loading clean and then failing every trial
    /// forever, same posture as the neighbouring arms (`CmpOp::parse`,
    /// `tool_called_any`'s trailing-token check).
    #[test]
    fn prompt_recorded_rejects_a_source_outside_the_closed_vocabulary() {
        let err = Assert::parse("prompt_recorded code builtin").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("built_in"), "{msg}");
        assert!(msg.contains("user_file"), "{msg}");
    }

    #[test]
    fn prompt_recorded_accepts_both_spellings_in_the_closed_vocabulary() {
        Assert::parse("prompt_recorded code built_in").unwrap();
        Assert::parse("prompt_recorded code user_file").unwrap();
    }

    // spec `session-evidence`, "Regression scenarios pin both channels"
    // (section 10, opened from 9.2's first live run): `tool_called_any`
    // proves a headless session carries tool records without naming which
    // tool the model picked, since which tool it picks is not itself a
    // claim any fixture here makes.

    fn tool_call(name: &str) -> ToolCall {
        ToolCall {
            index: 0,
            name: name.to_string(),
            summary: String::new(),
            subagent: false,
        }
    }

    #[test]
    fn tool_called_any_passes_on_one_tool_call() {
        let dir = tmp("tool-called-any-one");
        let t = Transcript {
            tool_calls: vec![tool_call("bash")],
            ..Default::default()
        };
        let roots = flat_roots(&dir);
        let r = Assert::parse("tool_called_any").unwrap().eval(&t, &roots);
        assert!(r.pass, "{}", r.detail);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tool_called_any_passes_on_several_tool_calls() {
        let dir = tmp("tool-called-any-several");
        let t = Transcript {
            tool_calls: vec![tool_call("bash"), tool_call("read"), tool_call("write")],
            ..Default::default()
        };
        let roots = flat_roots(&dir);
        let r = Assert::parse("tool_called_any").unwrap().eval(&t, &roots);
        assert!(r.pass, "{}", r.detail);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A transcript with no tool calls must fail this assert, never pass
    /// vacuously (this repo's `absence-asserts` posture, matching
    /// `prompt_recorded`'s rule above).
    #[test]
    fn tool_called_any_fails_rather_than_passing_vacuously_on_no_tool_calls() {
        let dir = tmp("tool-called-any-empty");
        let t = Transcript::default();
        let roots = flat_roots(&dir);
        let r = Assert::parse("tool_called_any").unwrap().eval(&t, &roots);
        assert!(!r.pass, "{}", r.detail);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tool_called_any_rejects_a_trailing_argument() {
        let err = Assert::parse("tool_called_any bash").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("tool_called_any"), "{msg}");
    }
}
