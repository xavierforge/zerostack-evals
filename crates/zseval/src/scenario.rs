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

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

/// The folder a scenario's seed data lives in (see the module header). Its
/// contents are payloads the agent operates on rather than harness files, so
/// nothing in the tree walk reads them as anything else.
const FIXTURES_DIR: &str = "_fixtures";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub id: String,
    /// Whether a low score here is a problem (`regression` — a break should
    /// gate a prompts PR) or a measurement (`capability` — a tracked number).
    /// Required, with no default in either direction: a default would un-ask
    /// the very question this field exists to force the author to answer, "is
    /// a low score here a problem or a measurement?" A missing or unrecognized
    /// value is a load-time error.
    pub kind: Kind,
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
    /// Which of zerostack's permission modes this scenario's invocation
    /// launches with. Defaults to `yolo`, so a scenario that predates this
    /// field keeps its exact current argument list — see `backend::print_turn_args`
    /// and `backend::loop_args` for the mapping.
    ///
    /// The only place a run's permission mode is declared, and enforced as
    /// that rather than assumed: `cli_args` cannot carry a permission flag
    /// (see below), and no config file the run can reach may carry a
    /// permission key, because zerostack resolves the mode from its config
    /// ahead of some of the flags — see `target::permission_mode_keys`, the
    /// `config:` seed check in `load`, and `preflight::check_target_permission_keys`.
    #[serde(default)]
    pub security_mode: SecurityMode,
    /// Extra tokens appended to the assembled invocation verbatim and in
    /// order, after every harness-owned flag and immediately before the turn
    /// message, in both assembly paths — see `backend::print_turn_args` and
    /// `backend::loop_args`. A flag that takes a separate value is two
    /// entries (`["--quick-model", "fast"]`). Checked at load against the
    /// harness-owned flags in every spelling zerostack accepts, short
    /// clusters included (`backend::harness_owned_collision`);
    /// permission-mode flags are expressible only through `security_mode`,
    /// never here. Flags whose value is a secret are refused outright
    /// (`backend::secret_bearing_flag`): a token here reaches an argument
    /// vector the whole host can read, and a persisted timeout hint besides.
    #[serde(default)]
    pub cli_args: Vec<String>,
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

/// Is a low score on this scenario a problem or a measurement? The answer
/// decides whether a break gates a prompts PR (`Regression`) or is tracked as
/// a capability number (`Capability`). Deliberately two-valued and undefaulted
/// (see `Scenario::kind`): the closed set means adding a third kind must be a
/// loud schema decision, not a quiet new value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Regression,
    Capability,
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

/// zerostack's permission-mode surface, named to mirror its CLI flags
/// verbatim (`Standard` is the exception: zerostack has no `--standard`
/// flag, its Standard mode is what running with no permission flag at all
/// looks like). Defaults to `Yolo`, the mode every scenario launched with
/// before this field existed, so a scenario that doesn't declare it keeps
/// today's exact invocation. Mirroring upstream names rather than inventing
/// harness vocabulary keeps the mapping auditable and a new upstream mode a
/// one-line addition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SecurityMode {
    #[default]
    Yolo,
    Standard,
    Restrictive,
    ReadOnly,
    Guarded,
    AcceptAll,
    DangerouslySkipPermissions,
}

impl SecurityMode {
    /// Every variant, so a caller can walk every permission-flag spelling
    /// without restating it; not every entry is a distinct effective mode,
    /// since `accept-all` and `standard` resolve to the same one in current
    /// zerostack (see the README). `all_lists_every_variant` below keeps this
    /// list honest: a variant added to the enum and not to this list fails
    /// there, which is what lets the harness's drift test prove every mode's
    /// flag is denied in `cli_args`.
    pub const ALL: &'static [SecurityMode] = &[
        SecurityMode::Yolo,
        SecurityMode::Standard,
        SecurityMode::Restrictive,
        SecurityMode::ReadOnly,
        SecurityMode::Guarded,
        SecurityMode::AcceptAll,
        SecurityMode::DangerouslySkipPermissions,
    ];

    /// The permission flag this mode's invocation carries, or `None` for
    /// `Standard`, which is what zerostack's default looks like with no
    /// permission flag at all. Every other variant spells out
    /// `--<kebab-case-name>` by hand: the match is exhaustive, so a new
    /// upstream mode is a one-line addition the compiler asks for, and the
    /// harness's drift test then requires that new flag to appear in
    /// `backend::HARNESS_OWNED_FLAGS` as well.
    pub fn flag(self) -> Option<&'static str> {
        match self {
            SecurityMode::Yolo => Some("--yolo"),
            SecurityMode::Standard => None,
            SecurityMode::Restrictive => Some("--restrictive"),
            SecurityMode::ReadOnly => Some("--read-only"),
            SecurityMode::Guarded => Some("--guarded"),
            SecurityMode::AcceptAll => Some("--accept-all"),
            SecurityMode::DangerouslySkipPermissions => Some("--dangerously-skip-permissions"),
        }
    }
}

#[cfg(test)]
mod security_mode_tests {
    use super::SecurityMode;
    use serde::de::IntoDeserializer;
    use serde::Deserialize;

    /// `SecurityMode::ALL` is written out by hand, and everything that walks
    /// the permission surface (the harness's drift test over the launch
    /// flags, first of all) trusts it to be the whole surface. Rust has no
    /// way to enumerate an enum's variants, so the check goes through the one
    /// list that does grow on its own: the derived `Deserialize` names every
    /// variant it knows about when it rejects a value, so counting those
    /// names catches a variant added to the enum and not to `ALL`. Distinct
    /// entries are the other half: a list of the right length that repeats
    /// one variant and drops another would otherwise pass.
    #[test]
    fn all_lists_every_variant() {
        let err: serde::de::value::Error =
            SecurityMode::deserialize("nonesuch".into_deserializer()).unwrap_err();
        let err = err.to_string();
        let (_, listed) = err
            .split_once("expected one of")
            .unwrap_or_else(|| panic!("serde no longer names the variants it knows: {err}"));
        let named = listed.matches('`').count() / 2;
        assert_eq!(
            named,
            SecurityMode::ALL.len(),
            "SecurityMode::ALL is not the whole permission surface: {err}"
        );

        let mut distinct: Vec<usize> = SecurityMode::ALL.iter().map(|m| *m as usize).collect();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(distinct.len(), SecurityMode::ALL.len());
    }
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
///   - **No tool-call evidence at all.** Tool calls are read from a
///     session's structured `tool` records (see `transcript.rs`'s module
///     doc), and a loop run writes no session at all — so a
///     `tool_not_called` assert would pass vacuously against evidence that
///     could never have shown a call either way. `Scenario::load` rejects every
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
        ToolCalledAny => Some("tool_called_any"),
        ToolNotCalled(_) => Some("tool_not_called"),
        ToolCalledAfter { .. } => Some("tool_called_after"),
        ToolCount { .. } => Some("tool_count"),
        ToolArgContains { .. } => Some("tool_arg_contains"),
        NoToolCallContains(_) => Some("no_tool_call_contains"),
        TokensUnder(_) => Some("tokens_under"),
        // Loop mode writes no session file, so `Transcript.prompt` is always
        // `None` there — same evidence gap as the tool-call asserts above.
        PromptRecorded { .. } => Some("prompt_recorded"),
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
        // Every dash-prefixed token is checked against the harness-owned
        // flags before any trial spends money, in every spelling zerostack
        // accepts for one, including inside a short cluster (which reaches
        // the child split apart). Permission-mode flags are rejected here too:
        // `security_mode` is their one source of truth, so `cli_args` can't
        // smuggle a second, possibly conflicting, declaration of the same
        // thing.
        for token in &sc.cli_args {
            // A dash-prefixed token is trimmed before it's checked, so a
            // trailing-whitespace typo like `--yolo ` is caught as the
            // collision it is rather than sliding past an exact-string match.
            // A value token (e.g. "two words") is never dash-prefixed, so it's
            // left untouched here: internal whitespace in a value is legal.
            let trimmed = token.trim();
            let checked = if trimmed.starts_with('-') {
                if trimmed.chars().any(char::is_whitespace) {
                    bail!(
                        "{} ({}): cli_args token '{trimmed}' contains whitespace; a flag token \
                         must be a single shell word, so split the value into its own token",
                        sc.id,
                        path.display()
                    );
                }
                trimmed
            } else {
                token.as_str()
            };
            // Checked before the collision rule so the error can name the flag
            // without the value attached to it: this bail prints the flag, and
            // the token it came from may be `--api-key=<the secret>`.
            if let Some(flag) = crate::backend::secret_bearing_flag(checked) {
                bail!(
                    "{} ({}): cli_args token '{flag}' would put a secret on the command line, \
                     where every process on the host can read it and where this harness's own \
                     timeout hint would persist it into a report; the key belongs in the \
                     environment, never in cli_args",
                    sc.id,
                    path.display()
                );
            }
            if let Some(owned) = crate::backend::harness_owned_collision(checked) {
                bail!(
                    "{} ({}): cli_args token '{checked}' collides with the harness-owned flag \
                     '{owned}'; permission modes are set via security_mode, not cli_args",
                    sc.id,
                    path.display()
                );
            }
        }
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
            let src = sc
                .resolve_fixture(&f.src)
                .with_context(|| format!("{}: bad [[files]] src", sc.id))?;
            // A `config:` seed lands where zerostack reads its own config, and
            // zerostack resolves the launch's permission mode from there ahead
            // of some of the flags the harness passes — so a permission key in
            // a seeded config outranks this scenario's `security_mode` and the
            // trial measures a mode it never declared, with nothing to show for
            // it. Refused at load, so it costs a failed `list` and not a run.
            if f.dest.starts_with("config:") {
                let keys = crate::target::permission_mode_keys(&src);
                let subject = match keys.as_slice() {
                    [] => None,
                    [one] => Some(format!("the permission key `{one}`")),
                    many => Some(format!("the permission keys {}", many.join(", "))),
                };
                if let Some(subject) = subject {
                    bail!(
                        "{} ({}): the [[files]] seed '{}' places {subject} in the run's config \
                         root ({}) — zerostack resolves a launch's permission mode from its \
                         config file ahead of some command-line flags, so the seed would \
                         override the security_mode this scenario declares. Delete the key from \
                         the fixture; a launch's permission mode belongs in security_mode",
                        sc.id,
                        path.display(),
                        f.dest,
                        src.display(),
                    );
                }
            }
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
///
/// Every way of finding nothing is loud. The walk used to skip an unreadable
/// directory, a failed directory entry, and a `scenario.toml` nested under
/// another scenario, all three in silence, which meant a scenario could sit in
/// the tree while every count of the tree missed it. That was survivable while
/// the result only fed a run — you notice a suite that is short a case — and
/// stopped being survivable once `coverage.rs` started asking this function
/// which scenarios exist in order to answer whether the ledger accounts for all
/// of them. A silent skip there reports full coverage of a tree it never saw.
///
/// Nesting is refused rather than supported: a scenario directory is a leaf, so
/// a `scenario.toml` under one is a mistake with no sensible reading, and
/// naming it beats either ignoring it or inventing sub-scenario semantics. The
/// single-scenario shorthand below is exempt, since a caller naming one
/// directory is asking for that scenario and not for a survey of the tree.
///
/// A `_fixtures` folder is the one subtree the walk stays out of, and the
/// refusal above is why it has to. Fixtures are seed data copied into the
/// agent's working directory, so a `scenario.toml` there is a payload — the
/// file a "fix the syntax error in this config" task hands the agent — and
/// reading a payload as a nested scenario would take the whole tree down over
/// one fixture's file name.
///
/// Arriving at a directory twice is refused on the same grounds. The walk
/// follows symlinks, so a link pointing back at an ancestor grows the path one
/// component per lap and is walked until memory or the filesystem's path limit
/// gives out, by which point the error names neither the link nor the loop; and
/// two paths onto one directory count every scenario below it twice, which
/// surfaces as a duplicate id or a doubled run rather than as the link it came
/// from. So the canonical path of every directory walked is kept, and a second
/// arrival at one is named rather than skipped — a symlink into a directory the
/// walk has not seen is a legitimate way to assemble a tree and keeps working,
/// which is exactly what makes the repeat worth saying out loud.
pub fn discover(root: &Path) -> Result<Vec<Scenario>> {
    let mut out = Vec::new();
    if root.join("scenario.toml").is_file() {
        out.push(Scenario::load(root)?);
        return Ok(out);
    }
    // Every directory this walk has entered, by resolved path, so a link back
    // into the tree is a refusal instead of a lap. The root counts as walked
    // before the first entry is read: a link straight back at it is a cycle
    // like any other.
    let mut walked: HashSet<PathBuf> = HashSet::new();
    walked.insert(
        std::fs::canonicalize(root)
            .with_context(|| format!("resolve scenario directory {}", root.display()))?,
    );
    // Each entry is a directory to walk, paired with the scenario directory
    // containing it, if any. A scenario directory is still walked — that is what
    // makes a nested scenario findable rather than invisible.
    let mut stack: Vec<(PathBuf, Option<PathBuf>)> = vec![(root.to_path_buf(), None)];
    while let Some((dir, inside)) = stack.pop() {
        let entries = std::fs::read_dir(&dir)
            .with_context(|| format!("read scenario directory {}", dir.display()))?;
        for entry in entries {
            let entry = entry.with_context(|| format!("read an entry of {}", dir.display()))?;
            let p = entry.path();
            if !p.is_dir() {
                continue;
            }
            if p.file_name().is_some_and(|name| name == FIXTURES_DIR) {
                continue;
            }
            let real = std::fs::canonicalize(&p)
                .with_context(|| format!("resolve scenario directory {}", p.display()))?;
            if !walked.insert(real.clone()) {
                bail!(
                    "{} resolves to {}, which this walk has already entered — a link back at an \
                     ancestor is walked forever, and two paths onto one scenario count it twice, \
                     so the walk names the link here instead of continuing into either",
                    p.display(),
                    real.display()
                );
            }
            if p.join("scenario.toml").is_file() {
                if let Some(outer) = &inside {
                    bail!(
                        "{} holds a scenario.toml, and so does {}, which contains it — a scenario \
                         directory is a leaf, and a nested scenario would be counted by nobody",
                        p.display(),
                        outer.display()
                    );
                }
                out.push(Scenario::load(&p)?);
                stack.push((p.clone(), Some(p)));
            } else {
                stack.push((p, inside.clone()));
            }
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}
