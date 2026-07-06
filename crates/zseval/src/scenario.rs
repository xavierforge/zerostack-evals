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
pub struct Scenario {
    pub id: String,
    /// Named prompt to load (`zs --load-prompt <name>`). None = default prompt.
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default = "default_trials")]
    pub trials: usize,
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

/// Subsystem-specific seeding sugar, one optional field per `domains::`
/// module. Adding support for another subsystem = one new field here plus
/// its module — the harness core (`seed::apply`) stays generic, it just
/// expands whichever fields are `Some`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SeedSugar {
    #[serde(default)]
    pub memory: Option<crate::domains::memory::MemorySeed>,
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
#[serde(untagged)]
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
