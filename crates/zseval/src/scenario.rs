//! Scenario definition: one `scenario.toml` per directory.
//!
//! A scenario is flat data, not code, so cases can be added without
//! recompiling. It names a prompt to load, a task to give the agent, and the
//! behaviour to check:
//!
//!   id     = "ask-readonly-refuses-edit"
//!   prompt = "ask"                     # -> zs --load-prompt ask
//!   trials = 3
//!   task   = "prepend a line to hello.py"  # string, or array for multi-turn
//!   expect = [ "tool_not_called write" ]
//!   judge  = "..."                     # optional LLM rubric
//!   [[files]]                          # optional generic seeding
//!   src = "_fixtures/hello.py"         # resolved by walking up from the scenario dir
//!   dest = "work:hello.py"
//!
//! Fixtures live in a `_fixtures` folder: beside the scenario's own dir if used
//! by only that `scenario.toml`, or in a suite dir above if shared across it
//! (e.g. `scenarios/prompts/_fixtures/hello.py` serves every prompt scenario).

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub id: String,
    /// Named prompt to load (`zs --load-prompt <name>`). None = default prompt.
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default = "default_trials")]
    pub trials: usize,
    /// `print` (default) drives zerostack's per-turn `-p`/`--continue` path;
    /// `loop` drives a single `--loop` invocation instead — see `LoopCfg`.
    #[serde(default)]
    pub mode: Mode,
    /// Required iff `mode = "loop"`; a load-time error otherwise either way
    /// (missing when needed, present when not) — see `Scenario::load`.
    #[serde(default, rename = "loop")]
    pub loop_cfg: Option<LoopCfg>,
    pub task: Task,
    /// Deterministic floor: one assert per line, mini-DSL (see asserts.rs).
    #[serde(default)]
    pub expect: Vec<String>,
    /// Optional LLM judge rubric, judged over the final message + tool calls.
    #[serde(default)]
    pub judge: Option<String>,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub max_cost_usd: Option<f64>,
    #[serde(default)]
    pub max_total_tokens: Option<u64>,
    /// Generic file placements: `dest` is `data:…`, `config:…`, or `work:…`.
    #[serde(default)]
    pub files: Vec<FileSeed>,
    /// Subsystem-specific seeding sugar (`[seed.memory]`, …), expanded into
    /// generic placements by the matching `domains::` module.
    #[serde(default)]
    pub seed: SeedSugar,
    /// Explicit opt-in to a domain's post-run drift check (`domains::verify`)
    /// for a scenario that doesn't declare any `[seed.*]` sugar of its own —
    /// e.g. one that starts from an empty memory store and only asserts the
    /// agent wrote to it, so there's nothing to seed but the layout snapshot
    /// still needs guarding. `domains = ["memory"]`; unknown names are a
    /// load-time error (see `domains::validate`). Interpreting these names is
    /// entirely `domains::`' business — this field is just where the raw
    /// list is stored, same as `seed` above.
    #[serde(default)]
    pub domains: Vec<String>,
    /// Directory this scenario was loaded from (filled by the loader).
    #[serde(skip)]
    pub dir: PathBuf,
    /// `expect` parsed once at load time — the type carries the "every
    /// assert line is valid" invariant, so graders never re-parse. Same
    /// order as `expect` (which is kept for human-readable fail reasons).
    #[serde(skip)]
    pub asserts: Vec<crate::asserts::Assert>,
    /// FNV-1a hash of this scenario's raw TOML source, so `compare` can
    /// warn when a scenario's own definition changed between a baseline and
    /// a candidate run — see `util::fnv1a_hex`.
    #[serde(skip)]
    pub content_hash: String,
}

/// Which zerostack invocation shape drives this scenario. `Loop` trades away
/// tool-call and token-usage evidence (see `LoopCfg`'s doc) for the ability
/// to test multi-iteration autonomous behavior; `Scenario::load` enforces
/// that trade-off can't be silently ignored (rejects `tool_*`/`tokens_under`
/// asserts on a loop scenario).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Print,
    Loop,
}

/// `mode = "loop"` sugar: drives `zerostack --loop --loop-max
/// <max_iterations> [--loop-run <run>] <task>` as a single invocation,
/// instead of the per-turn `-p`/`--continue` loop `Mode::Print` uses.
///
/// Two consequences that shape what a loop scenario can assert on, both
/// verified against zerostack v1.6.1's `run_headless_loop` (`main.rs`) and
/// `extras/loop/*.rs`:
///   - **No session file.** `run_headless_loop` never calls `save_session`;
///     grading evidence instead comes from `$ZS_DATA_DIR/loops/<uuid>/
///     iter-NNNN.json` records (prompt/response/validation_output per
///     iteration — `extras/loop/transcript.rs::save_iteration`) plus the
///     filesystem. `transcript.rs`'s `from_run` folds these in as ordinary
///     messages; `final_contains`/`transcript_contains`/`file_*` all work
///     unchanged.
///   - **No tool-call evidence at all.** The loop's own `run_print` call
///     hardcodes `pure_stdout: false`, so the `◈ name ...` markers that are
///     the *only* tool-call channel in headless mode never appear —
///     regardless of what CLI flags the harness passes. A `tool_not_called`
///     assert would therefore pass vacuously against evidence that could
///     never have shown a call either way. `Scenario::load` rejects every
///     `tool_*` assert and `tokens_under` (no usage evidence either, same
///     root cause) on a loop scenario at load time instead of letting that
///     footgun ship.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoopCfg {
    /// `--loop-max`. Required (not optional) because `--loop` alone runs
    /// unbounded — every loop scenario must declare a hard ceiling.
    pub max_iterations: u32,
    /// `--loop-run`: a shell command run after each iteration, its output
    /// fed into the next iteration's prompt. The natural place for a
    /// "make the tests pass" scenario's pass/fail signal to show up in the
    /// graded transcript (via `transcript_contains`).
    #[serde(default)]
    pub run: Option<String>,
}

/// Subsystem-specific seeding sugar, one optional field per `domains::`
/// module. Adding support for another subsystem = one new field here plus
/// its module — the harness core (`seed::apply`) stays generic, it just
/// expands whichever fields are `Some`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SeedSugar {
    #[serde(default)]
    pub memory: Option<crate::domains::memory::MemorySeed>,
    #[serde(default)]
    pub mcp: Option<crate::domains::mcp::McpSeed>,
}

/// A task is one user message or a scripted sequence of them.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Task {
    Single(String),
    Multi(Vec<Turn>),
}

impl Task {
    pub fn turns(&self) -> Vec<Turn> {
        match self {
            Task::Single(s) => vec![Turn::Simple(s.clone())],
            Task::Multi(v) => v.clone(),
        }
    }
}

/// A user turn. Plain strings continue the current session; the table form can
/// force a fresh session (`new_session = true`) — the way to cut the context
/// cord for cross-session tests.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum Turn {
    Simple(String),
    Full {
        msg: String,
        #[serde(default)]
        new_session: bool,
    },
}

impl Turn {
    pub fn msg(&self) -> &str {
        match self {
            Turn::Simple(s) => s,
            Turn::Full { msg, .. } => msg,
        }
    }
    pub fn new_session(&self) -> bool {
        matches!(
            self,
            Turn::Full {
                new_session: true,
                ..
            }
        )
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileSeed {
    pub src: PathBuf,
    pub dest: String,
}

fn default_trials() -> usize {
    1
}
fn default_timeout() -> u64 {
    300
}

/// Which asserts depend on evidence loop mode never produces (see
/// `LoopCfg`'s doc) — `Some(name)` names the offending op for the load-time
/// error; everything else (file/final/transcript asserts) is fine, since
/// those grade the filesystem or the loop's own iteration records.
fn loop_incompatible_assert(a: &crate::asserts::Assert) -> Option<&'static str> {
    use crate::asserts::Assert::*;
    match a {
        ToolCalled(_) => Some("tool_called"),
        ToolNotCalled(_) => Some("tool_not_called"),
        ToolCalledAfter { .. } => Some("tool_called_after"),
        ToolCount { .. } => Some("tool_count"),
        ToolArgContains { .. } => Some("tool_arg_contains"),
        NoToolCallContains(_) => Some("no_tool_call_contains"),
        TokensUnder(_) => Some("tokens_under"),
        FinalContains(_)
        | FinalNotContains(_)
        | FinalMaxLines(_)
        | TranscriptContains(_)
        | TranscriptNotContains(_)
        | FileContains { .. }
        | FileNotContains { .. }
        | PathNotExists(_) => None,
    }
}

impl Scenario {
    pub fn load(dir: &Path) -> Result<Scenario> {
        let path = dir.join("scenario.toml");
        let text =
            std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let mut sc: Scenario =
            toml::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
        sc.dir = dir.to_path_buf();
        sc.content_hash = crate::util::fnv1a_hex(text.as_bytes());
        if sc.task.turns().is_empty() {
            bail!("{}: task must not be empty", sc.id);
        }
        if sc.expect.is_empty() && sc.judge.is_none() {
            bail!("{}: needs at least one expect assert or a judge", sc.id);
        }
        // Validate asserts, seed dests, and every fixture path at load time,
        // not mid-run: a typo'd fixture should fail `zseval list`/load, not
        // burn an API call before surfacing as an indeterminate.
        for line in &sc.expect {
            let a = crate::asserts::Assert::parse(line)
                .with_context(|| format!("{}: bad assert '{line}'", sc.id))?;
            sc.asserts.push(a);
        }
        match (sc.mode, &sc.loop_cfg) {
            (Mode::Loop, None) => {
                bail!("{}: mode = \"loop\" requires a [loop] table", sc.id)
            }
            (Mode::Print, Some(_)) => {
                bail!("{}: [loop] table is only valid with mode = \"loop\"", sc.id)
            }
            (Mode::Loop, Some(lc)) if lc.max_iterations == 0 => {
                bail!("{}: [loop].max_iterations must be >= 1", sc.id)
            }
            _ => {}
        }
        if sc.mode == Mode::Loop {
            if sc.task.turns().len() != 1 {
                bail!(
                    "{}: mode = \"loop\" supports exactly one task turn — loop mode has no \
                     multi-turn/--continue concept, it drives a single `--loop` invocation",
                    sc.id
                );
            }
            for (line, a) in sc.expect.iter().zip(&sc.asserts) {
                if let Some(op) = loop_incompatible_assert(a) {
                    bail!(
                        "{}: assert '{line}' uses '{op}', which mode = \"loop\" scenarios can't \
                         use — loop mode has no tool-call or token-usage evidence \
                         (run_headless_loop hardcodes pure_stdout=false and never saves a \
                         session); grade on file_contains/transcript_contains/final_contains \
                         instead",
                        sc.id
                    );
                }
            }
        }
        for f in &sc.files {
            let probe = std::path::Path::new(".");
            crate::seed::resolve_dest(
                &f.dest,
                &crate::backend::RunRoots {
                    data: probe,
                    config: probe,
                    work: probe,
                },
            )
            .with_context(|| format!("{}: bad seed dest '{}'", sc.id, f.dest))?;
            sc.resolve_fixture(&f.src)
                .with_context(|| format!("{}: bad [[files]] src", sc.id))?;
        }
        crate::domains::validate(&sc)?;
        Ok(sc)
    }

    /// Resolve a seed fixture by walking up from the scenario's own dir: a
    /// scenario-specific file lives in its own `_fixtures` dir, while a file
    /// shared across a suite lives in the suite dir's `_fixtures` above it.
    /// Nearest wins.
    pub fn resolve_fixture(&self, p: &Path) -> Result<PathBuf> {
        let mut dir = Some(self.dir.as_path());
        while let Some(d) = dir {
            let cand = d.join(p);
            if cand.is_file() {
                return Ok(cand);
            }
            dir = d.parent();
        }
        bail!(
            "{}: fixture '{}' not found walking up from {}",
            self.id,
            p.display(),
            self.dir.display()
        )
    }
}

/// Walk a path and collect every directory containing `scenario.toml`.
/// Accepts either a single scenario dir or a tree.
pub fn discover(root: &Path) -> Result<Vec<Scenario>> {
    let mut out = Vec::new();
    if root.join("scenario.toml").is_file() {
        out.push(Scenario::load(root)?);
        return Ok(out);
    }
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                if p.join("scenario.toml").is_file() {
                    out.push(Scenario::load(&p)?);
                } else {
                    stack.push(p);
                }
            }
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}
