//! Verdict model — a three-value verdict plus pass@k / pass^k.
//!
//! - `Pass`: every assert passed AND the judge (if any) said Yes.
//! - `Fail`: a graded negative — an assert failed, the judge said No, or a
//!   budget was exceeded.
//! - `Indeterminate`: we could not grade — backend crash, unreadable session
//!   schema, missing API key for a judge scenario, judge Unknown. A broken
//!   eval is not an agent failure, so a fully-ungradable scenario is excluded
//!   from the pass rates — never counted as a 0.
//!
//! pass@k = 1 if any trial passed (capability ceiling).
//! pass^k = 1 if all graded trials passed (stability floor).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::asserts::AssertResult;
use crate::judge::JudgeVerdict;
use crate::scenario::Kind;

/// Frozen at `1`: nothing reads this value (verified — only set at build and
/// asserted in tests), so bumping it is decorative. `#[serde(default)]` on
/// individual fields (e.g. `Report::target`) does the real backward-load
/// work; consumers check for a field's own presence on its own merits rather
/// than branching on this number.
pub const REPORT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Final {
    Pass,
    Fail,
    Indeterminate,
}

/// Which layer supplied a scenario's `prompt_name` — see
/// `ScenarioResult::prompt_source` for the resolution order (implemented in
/// section 6; this section only carries the type and its defaults).
///
/// `Unknown` is the `#[derive(Default)]` variant: it is what a report
/// predating this field reads as, and what the run path writes for now,
/// before section 6 fills in a real resolution. It is deliberately distinct
/// from `Stock` — an unobserved prompt is not the same fact as a run that
/// observably used zerostack's built-in prompts.
///
/// A value on the wire that is not one of these four names is a
/// deserialization error, not a silent `Unknown`: `#[serde(default)]` on the
/// *field* (see `ScenarioResult::prompt_source`) covers a *missing* field,
/// never a garbled one, so an unrecognized value fails loudly rather than
/// being misread as "we don't know".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PromptSource {
    #[default]
    Unknown,
    Stock,
    Pack,
    Scenario,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrialResult {
    pub trial: usize,
    pub outcome: Final,
    /// Human-readable reasons when not a clean pass.
    #[serde(default)]
    pub reasons: Vec<String>,
    #[serde(default)]
    pub asserts: Vec<AssertResult>,
    pub judge: Option<JudgeVerdict>,
    /// Configuration: the judge file this trial was graded with (`--judge`),
    /// `""` when none was named (there is no built-in default). Recorded per
    /// trial and not only per report because `regrade --judge` re-scores a
    /// single trial dir in place: without this, a trial regraded by a second
    /// ruler would sit under a `report.json` naming the first, and nothing on
    /// disk would say which of the two produced the verdict. Recorded the same
    /// way as `Report::judge_file` — see `JudgeFileRef`.
    #[serde(default)]
    pub judge_file: String,
    /// Fingerprint of the judge file's bytes, `None` when no judge file was
    /// named (or its bytes could not be read). Same reason
    /// `ScenarioResult::content_hash` exists: a path is not an identity, and a
    /// judge file's contents change under a stable path.
    #[serde(default)]
    pub judge_hash: Option<String>,
    /// Execution: the model that actually graded this trial, as the judge's
    /// own response reported it. Recorded per trial because that is where the
    /// evidence it graded lives; the report aggregates these (see
    /// `Report::judge_model`). Three distinct states:
    ///
    /// - `None` — unknown: this record predates the field, or the judge
    ///   answered without naming the model that served the call. Naming the
    ///   configured model instead would report an intention as a fact.
    /// - `Some("")` — nothing graded this trial (no rubric, `--no-judge`, no
    ///   key, or the call failed). No ruler to name, which is not the same as
    ///   not knowing which ruler it was.
    /// - `Some(model)` — `model` graded it.
    #[serde(default)]
    pub judge_model: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Judge-call token usage, tracked separately from the agent's own
    /// `input_tokens`/`output_tokens` above — it's eval overhead, not agent
    /// behavior, so it must never count against a scenario's
    /// `max_total_tokens`. Zero when no judge ran (or a test double reports
    /// nothing). Real API spend even so — see `judge::JudgeOutcome`.
    #[serde(default)]
    pub judge_input_tokens: u64,
    #[serde(default)]
    pub judge_output_tokens: u64,
    pub cost_usd: f64,
    pub wall_secs: f64,
    /// How many tool calls the transcript actually contained — the evidence
    /// count behind every `tool_called*` assert on this trial. Tracked
    /// separately from pass/fail because a `tool_not_called`-only scenario
    /// passes vacuously if the evidence channel itself breaks (exactly what
    /// happened before headless tool-call reconstruction existed): `compare`
    /// uses this to flag "evidence vanished" even when the pass rate didn't
    /// move.
    #[serde(default)]
    pub tool_call_count: usize,
    /// Where the raw transcript(s) and stdout/stderr live, for `explain`.
    pub run_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioResult {
    pub id: String,
    /// The scenario's `kind`, recorded verbatim: matrix and the Day-2 site
    /// group from report JSON alone, never by re-reading scenario.toml
    /// (scenario-kind spec / design D4). `#[serde(default)]` for the same
    /// read-tolerance reason as `content_hash`/`prompt_name` below — a report
    /// written before this field existed still loads (S7 removes the hatch).
    /// The run path always sets it from the scenario (see `runner.rs`), so a
    /// real report never leans on the default.
    #[serde(default = "default_result_kind")]
    pub kind: Kind,
    pub trials: Vec<TrialResult>,
    pub pass_at_k: f64,
    pub pass_hat_k: f64,
    pub indeterminate: usize,
    /// Sum of `tool_call_count` across all trials — see `TrialResult`'s doc.
    #[serde(default)]
    pub total_tool_calls: usize,
    /// `Scenario::content_hash` at run time — lets `compare` warn when a
    /// scenario's own definition changed between baseline and candidate.
    /// `#[serde(default)]` so an old committed baseline predating this
    /// field deserializes as `""`, which `compare` treats as "unknown, skip
    /// the check" rather than a false-positive warning on every scenario.
    #[serde(default)]
    pub content_hash: String,
    /// The prompt name this scenario actually loaded — `""` when unresolved.
    /// Scenario-level rather than trial-level: constant across a scenario's
    /// trials (unlike `TrialResult::judge_file`, which varies because
    /// `regrade --judge` can re-score one trial with a different ruler), so it
    /// sits beside `content_hash` instead. `#[serde(default)]` so a
    /// `ScenarioResult` predating this field deserializes as `""` rather than
    /// failing to parse. Populated by the resolution added in a later section;
    /// the run path writes `""` for now.
    #[serde(default)]
    pub prompt_name: String,
    /// Which layer supplied `prompt_name` — see `PromptSource`.
    /// `#[serde(default)]` so a `ScenarioResult` predating this field
    /// deserializes as `Unknown`, never `Stock`: an older report's prompt was
    /// never observed, which is not the same fact as a run that observably
    /// used the built-in prompts. Populated by the resolution added in a
    /// later section; the run path writes `Unknown` for now.
    #[serde(default)]
    pub prompt_source: PromptSource,
}

/// Read-tolerance default for `ScenarioResult::kind` (see its doc): only
/// reached by a report predating the field, or a test fixture that leaves it
/// unset. Every real run overwrites it from the scenario, so this value never
/// travels on a genuine report. Removed with the other legacy
/// `#[serde(default)]` hatches in S7.
fn default_result_kind() -> Kind {
    Kind::Regression
}

impl ScenarioResult {
    pub fn from_trials(id: String, trials: Vec<TrialResult>) -> Self {
        Self::from_trials_with_hash(id, String::new(), trials)
    }

    pub fn from_trials_with_hash(
        id: String,
        content_hash: String,
        trials: Vec<TrialResult>,
    ) -> Self {
        let graded: Vec<&TrialResult> = trials
            .iter()
            .filter(|t| t.outcome != Final::Indeterminate)
            .collect();
        let indeterminate = trials.len() - graded.len();
        let any = graded.iter().any(|t| t.outcome == Final::Pass);
        let all = !graded.is_empty() && graded.iter().all(|t| t.outcome == Final::Pass);
        let total_tool_calls = trials.iter().map(|t| t.tool_call_count).sum();
        ScenarioResult {
            id,
            kind: default_result_kind(),
            pass_at_k: if any { 1.0 } else { 0.0 },
            pass_hat_k: if all { 1.0 } else { 0.0 },
            indeterminate,
            total_tool_calls,
            content_hash,
            prompt_name: String::new(),
            prompt_source: PromptSource::default(),
            trials,
        }
    }

    /// A scenario is gradable if at least one trial produced a verdict; a
    /// fully-indeterminate scenario is excluded from pass rates and diffs.
    pub fn is_gradable(&self) -> bool {
        self.trials
            .iter()
            .any(|t| t.outcome != Final::Indeterminate)
    }

    /// How many trials actually produced a verdict — the pass-rate
    /// denominator, and the thing that sets the smallest possible nonzero
    /// step between two pass rates (`1 / n_graded_trials`). `compare` uses
    /// this to warn when a regression threshold is finer than any diff this
    /// scenario could actually produce (see AGENTS.md on trials and k).
    pub fn n_graded_trials(&self) -> usize {
        self.trials
            .iter()
            .filter(|t| t.outcome != Final::Indeterminate)
            .count()
    }

    pub fn trial_pass_rate(&self) -> f64 {
        let n = self.n_graded_trials();
        if n == 0 {
            return 0.0;
        }
        self.trials
            .iter()
            .filter(|t| t.outcome == Final::Pass)
            .count() as f64
            / n as f64
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub schema_version: u32,
    pub tag: String,
    pub model: String,
    pub backend: String,
    pub timestamp: String,
    pub trials: usize,
    /// Configuration: the judge file this run was told to grade with
    /// (`--judge`), `""` when none was named. Recorded because the judge is
    /// the ruler: two runs are only comparable if the same one measured them,
    /// and that fact has to survive the run. Always a working-directory-
    /// relative, forward-slashed path, never an absolute one — see
    /// `JudgeFileRef`.
    /// `#[serde(default)]` so a baseline committed before this field existed
    /// still loads, same precedent as `ScenarioResult::content_hash`.
    #[serde(default)]
    pub judge_file: String,
    /// Fingerprint of `judge_file`'s bytes, `None` when no judge file was
    /// named (or its bytes could not be read). The path alone cannot pin the
    /// ruler down: a judge file's contents change under a stable path, which
    /// is the same reason `ScenarioResult::content_hash` exists.
    #[serde(default)]
    pub judge_hash: Option<String>,
    /// Execution: the model(s) that actually graded, read back from the
    /// judge's own responses rather than from the config. `judge_file` and
    /// `judge_hash` say what was *asked for*; the API resolves model names
    /// server-side, so what answered is a separate fact. Three distinct
    /// states, none of which may be confused for another:
    ///
    /// - `None` — unknown. Either the report predates this field (a baseline
    ///   written before it was still graded by *something*, so claiming
    ///   "nothing" would state a falsehood about a real run), or some trial's
    ///   own ruler was unknown, which leaves the run's rulers unlistable.
    /// - `Some([])` — nothing was graded: `--no-judge`, no scenario carried a
    ///   rubric, or every call failed. The honest answer, where echoing the
    ///   configured model back would report an intention as a fact.
    /// - `Some([m, ...])` — these rulers graded, sorted and deduped. On the
    ///   rare disagreement (trials served by different models) every distinct
    ///   model is listed rather than one being picked to stand for the rest.
    #[serde(default)]
    pub judge_model: Option<Vec<String>>,
    /// The target this run evaluated against: column identity, not content.
    /// `target.toml` (the file's bytes) lives only in the run dir; this field
    /// is what travels with the report when it is copied elsewhere (e.g. into
    /// `baselines/`), so a detached report still names what it evaluated.
    /// Normalised through the same `record_path` rule as `JudgeFileRef::path`
    /// (working-directory-relative, forward-slashed, never absolute; reduced
    /// to a bare file name when the target lives outside the working
    /// directory). `#[serde(default)]` so a report written before this field
    /// existed still loads, as an empty target rather than an error.
    #[serde(default)]
    pub target: String,
    /// This run stopped early because it hit `--max-total-usd`: at least one
    /// declared scenario was never reached. Recorded as a fact on the report
    /// rather than inferred from a scenario count, so a consumer (`matrix`'s
    /// incomplete/`*` mark) can tell "the budget cut this short" apart from
    /// "this was simply a smaller suite" (a shorter baseline reached in full),
    /// which a bare count cannot distinguish. `#[serde(default)]` so a report
    /// written before this field existed still loads, as `false`.
    #[serde(default)]
    pub budget_truncated: bool,
    /// The prompt pack this run evaluated (`--prompts`), `""` when none was
    /// given. Recorded because a pack is a subject variable of the experiment:
    /// two runs are only a clean prompt comparison if this is what moved
    /// between them, and that fact has to survive the run. Normalised through
    /// the same `record_path` rule as `judge_file` and `target`
    /// (working-directory-relative, forward-slashed, never absolute; reduced to
    /// a bare directory name when the pack lives outside the working
    /// directory), so a report copied into `baselines/` is not a map of
    /// someone's filesystem. `#[serde(default)]` so a report written before
    /// this field existed still loads, as an empty pack.
    #[serde(default)]
    pub prompts_pack: String,
    /// Fingerprint of the pack's contents and names (`PromptPack::fingerprint`),
    /// `""` when no pack was given. The path alone cannot pin the pack down: a
    /// pack's files change under a stable path, the same reason `judge_hash`
    /// and `ScenarioResult::content_hash` exist. A moved-but-unchanged pack
    /// keeps this hash, so identity survives relocation.
    #[serde(default)]
    pub prompts_hash: String,
    /// The sorted prompt names the pack provides (file stems), `[]` when no
    /// pack was given. Recorded alongside the hash so "why did every scenario
    /// resolve `stock`" is answerable from the report alone: a pack whose names
    /// no scenario calls is visible here without re-reading the pack directory.
    #[serde(default)]
    pub prompts_names: Vec<String>,
    /// The zerostack build that produced this report, captured verbatim from
    /// `ZS_BIN --version`'s first line at run start (for `--backend mock`, the
    /// fixed `"mock"` label). Evidence for humans, deliberately *not* format-
    /// validated: the machine-comparable identity is `zs_bin_sha256`, so
    /// upstream's version-string shape never becomes a compatibility contract.
    ///
    /// Required, with **no** `#[serde(default)]`, unlike the fields above: this
    /// feature exists to make identity-less reports impossible, so a report
    /// that cannot name the zerostack that produced it must fail to load, not
    /// deserialize to an empty string. Capture failure aborts the run before
    /// any API spend rather than writing a defaulted value (design D3/D7).
    pub zs_version: String,
    /// The zerostack binary's path as run, normalised through `record_path`
    /// (working-directory-relative, forward-slashed, bare name when outside the
    /// working directory) so a report copied into `baselines/` is not a map of
    /// someone's filesystem — the same rule `target`/`judge_file`/`prompts_pack`
    /// follow. For `--backend mock`, the fixture path. Human-facing "where";
    /// the identity that `compare` diffs on is `zs_bin_sha256`.
    pub zs_bin_path: String,
    /// SHA-256 of the binary's file contents (for `--backend mock`, a content
    /// fingerprint of the fixture — see `AgentBackend::identity`). The
    /// machine-comparable build identity: two runs are only a controlled
    /// comparison if this matches, which is exactly what a same-version-
    /// different-binary incident (the motivating case) cannot fake. Required,
    /// no default, for the same reason as `zs_version`.
    pub zs_bin_sha256: String,
    /// The binary's embedded git sha, `null` today: the 1.7.x binary embeds
    /// none (no `build.rs`, no clap customization — live-tested). Stated as a
    /// fact of the current binary, not a runtime "record if present" branch;
    /// when upstream starts embedding one, this stops being unconditionally
    /// `null` and the capture in `AgentBackend::identity` fills it.
    pub git_sha: Option<String>,
    /// The build's enabled feature set, `null` today for the same reason as
    /// `git_sha`: the binary exposes none to capture.
    pub features: Option<Vec<String>>,
    pub scenarios: Vec<ScenarioResult>,
    pub summary: Summary,
}

/// What produced a run's evidence, captured once (never per trial). For a real
/// zerostack backend: the version string, the binary's path, and the SHA-256
/// of its contents. For `--backend mock`: `"mock"`, the fixture path, and a
/// content fingerprint of the fixture. Carried on `ReportMeta` and flattened
/// onto `Report` by `Report::build`; the capture logic lives on
/// `AgentBackend::identity` (it is the backend that knows what ran).
///
/// `git_sha`/`features` are `Option`, `None` today — the current binary embeds
/// neither. They are recorded as observed facts, not a runtime feature probe.
///
/// `Default` is the all-empty identity, used only by test fixtures that build a
/// `ReportMeta` without a live backend; a real run always fills every field or
/// aborts.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ZsIdentity {
    pub zs_version: String,
    pub zs_bin_path: String,
    pub zs_bin_sha256: String,
    pub git_sha: Option<String>,
    pub features: Option<Vec<String>>,
}

/// A `Summary`'s per-kind view: the same `n_scenarios`/`n_gradable`/
/// `pass_at_k`/`pass_hat_k` shape as the overall summary, computed over one
/// kind's scenarios only (design D5). `Summary` carries exactly two of these,
/// named `regression` and `capability` — not a map keyed by kind: the kind
/// enum is closed and two-valued, so adding a third kind must be a loud
/// schema decision (a new named field), never a quiet new map key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KindSummary {
    pub n_scenarios: usize,
    /// Scenarios of this kind with at least one graded trial (the pass-rate
    /// denominator for this kind).
    pub n_gradable: usize,
    pub pass_at_k: f64,
    pub pass_hat_k: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Summary {
    pub n_scenarios: usize,
    /// Scenarios with at least one graded trial (the pass-rate denominator).
    pub n_gradable: usize,
    pub pass_at_k: f64,
    pub pass_hat_k: f64,
    /// Scenarios where every trial was indeterminate (excluded from rates).
    pub indeterminate_scenarios: usize,
    pub indeterminate_trials: usize,
    pub total_cost_usd: f64,
    pub avg_wall_secs: f64,
    /// The same four numbers above, computed over regression scenarios only.
    /// The fields above stay the historical blended yardstick, unmoved by
    /// this — see design D5.
    pub regression: KindSummary,
    /// The same four numbers above, computed over capability scenarios only.
    pub capability: KindSummary,
}

/// How an artifact records the judge file it was graded with: where the file
/// was, and what it said. Both halves are needed — a path is not an identity,
/// since a judge file's contents change under a stable path (the precedent is
/// `ScenarioResult::content_hash`, which exists for exactly that reason about
/// scenarios).
///
/// `Default` is "no judge file was named".
#[derive(Debug, Clone, Default)]
pub struct JudgeFileRef {
    /// Relative to the working directory when the file lives under it, its
    /// bare file name otherwise; never an absolute path. A report is meant to
    /// be copied into `baselines/`, i.e. into git, so `--judge
    /// /Users/alice/private-client/judges/x.toml` must not turn a committed
    /// artifact into a map of someone's filesystem (the same leak `/results`
    /// is gitignored over). Forward slashes always, so the recorded value does
    /// not vary with the platform that wrote it.
    pub path: String,
    /// `util::fnv1a_hex` of the file's bytes — the same fingerprint a scenario
    /// uses for its own source, so there is one hashing approach here, not
    /// two. `None` when the bytes could not be read: an unknown fingerprint,
    /// never a guessed one.
    pub hash: Option<String>,
}

impl JudgeFileRef {
    pub fn of(path: &Path) -> Self {
        JudgeFileRef {
            path: record_path(path),
            hash: std::fs::read(path).ok().map(|b| crate::util::fnv1a_hex(&b)),
        }
    }
}

/// See `JudgeFileRef::path` for the rules this enforces. `pub(crate)` so
/// `runner.rs` can apply the same working-directory-relative, forward-slashed,
/// basename-fallback treatment to `TrialResult.run_dir` (report-paths) instead
/// of duplicating the logic.
pub(crate) fn record_path(path: &Path) -> String {
    let rel = match std::fs::canonicalize(path) {
        Ok(abs) => relative_to_cwd(&abs).unwrap_or_else(|| file_name_of(path)),
        // The path did not resolve (it may name a file that no longer exists).
        // One that was given relative is relative to the working directory by
        // construction and leaks nothing, so it stands as given; an absolute
        // one is reduced to its file name like any other.
        Err(_) if path.is_relative() => path.to_path_buf(),
        Err(_) => file_name_of(path),
    };
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

fn relative_to_cwd(abs: &Path) -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    if let Ok(rel) = abs.strip_prefix(&cwd) {
        return Some(rel.to_path_buf());
    }
    // A working directory reached through a symlink (macOS: /tmp ->
    // /private/tmp) still contains the file; compare resolved forms before
    // concluding the file lives somewhere else.
    let cwd = std::fs::canonicalize(&cwd).ok()?;
    abs.strip_prefix(&cwd).ok().map(|r| r.to_path_buf())
}

fn file_name_of(path: &Path) -> PathBuf {
    PathBuf::from(path.file_name().unwrap_or_default())
}

/// The run-level facts a report records that its scenarios cannot supply:
/// what was evaluated, and by which ruler.
///
/// Named fields rather than positional arguments because the values are
/// swappable neighbours of the same type: `tag`/`model`/`backend` are three
/// adjacent `String`s, and `judge_file`/`judge_model` are two more whose whole
/// design point is that they mean *different* things (configuration vs
/// execution). At a call site, only the names keep them apart.
///
/// `Default` records a run whose judge is unknown rather than one that claims
/// no judge ran: a caller that says nothing about the ruler has not thereby
/// established that there wasn't one.
#[derive(Debug, Clone, Default)]
pub struct ReportMeta {
    pub tag: String,
    pub model: String,
    pub backend: String,
    pub trials: usize,
    /// See `Report::judge_file`.
    pub judge_file: String,
    /// See `Report::judge_hash`.
    pub judge_hash: Option<String>,
    /// See `Report::judge_model` for the three states.
    pub judge_model: Option<Vec<String>>,
    /// See `Report::target`. Already normalised (`record_path`) by the
    /// caller; `Report::build` copies it through unchanged.
    pub target: String,
    /// See `Report::budget_truncated`. Set by `run_suite` when the cost cap
    /// stopped the run before a declared scenario.
    pub budget_truncated: bool,
    /// See `Report::prompts_pack`. Already normalised (`record_path`) by the
    /// caller; `Report::build` copies it through unchanged.
    pub prompts_pack: String,
    /// See `Report::prompts_hash`. Empty when no pack was evaluated.
    pub prompts_hash: String,
    /// See `Report::prompts_names`. Empty when no pack was evaluated.
    pub prompts_names: Vec<String>,
    /// The build (or fixture) that produced this run's evidence. Captured once
    /// per run by `run_suite` off `AgentBackend::identity` and flattened onto
    /// the report's `zs_version`/`zs_bin_path`/`zs_bin_sha256`/`git_sha`/
    /// `features` fields here.
    pub zs: ZsIdentity,
}

impl Report {
    pub fn build(meta: ReportMeta, scenarios: Vec<ScenarioResult>) -> Report {
        // Rates are averaged over gradable scenarios only; a fully-ungradable
        // scenario is a broken eval, not a 0.
        let gradable: Vec<&ScenarioResult> = scenarios.iter().filter(|s| s.is_gradable()).collect();
        let g = gradable.len().max(1) as f64;
        let pass_at_k = gradable.iter().map(|s| s.pass_at_k).sum::<f64>() / g;
        let pass_hat_k = gradable.iter().map(|s| s.pass_hat_k).sum::<f64>() / g;
        let indeterminate_scenarios = scenarios.iter().filter(|s| !s.is_gradable()).count();
        let indeterminate_trials = scenarios.iter().map(|s| s.indeterminate).sum();
        let all_trials: Vec<&TrialResult> =
            scenarios.iter().flat_map(|s| s.trials.iter()).collect();
        let total_cost_usd = all_trials.iter().map(|t| t.cost_usd).sum();
        let avg_wall_secs = if all_trials.is_empty() {
            0.0
        } else {
            all_trials.iter().map(|t| t.wall_secs).sum::<f64>() / all_trials.len() as f64
        };
        let regression = kind_summary(&scenarios, Kind::Regression);
        let capability = kind_summary(&scenarios, Kind::Capability);
        Report {
            schema_version: REPORT_SCHEMA_VERSION,
            tag: meta.tag,
            model: meta.model,
            backend: meta.backend,
            timestamp: now_iso(),
            trials: meta.trials,
            judge_file: meta.judge_file,
            judge_hash: meta.judge_hash,
            judge_model: meta.judge_model,
            target: meta.target,
            budget_truncated: meta.budget_truncated,
            prompts_pack: meta.prompts_pack,
            prompts_hash: meta.prompts_hash,
            prompts_names: meta.prompts_names,
            zs_version: meta.zs.zs_version,
            zs_bin_path: meta.zs.zs_bin_path,
            zs_bin_sha256: meta.zs.zs_bin_sha256,
            git_sha: meta.zs.git_sha,
            features: meta.zs.features,
            summary: Summary {
                n_scenarios: scenarios.len(),
                n_gradable: gradable.len(),
                pass_at_k: round4(pass_at_k),
                pass_hat_k: round4(pass_hat_k),
                indeterminate_scenarios,
                indeterminate_trials,
                total_cost_usd: round4(total_cost_usd),
                avg_wall_secs: round4(avg_wall_secs),
                regression,
                capability,
            },
            scenarios,
        }
    }

    /// The process exit code this report earns on its own, independent of
    /// `compare`. `2` (harness error) when scenarios were declared but not
    /// one was gradable — every trial indeterminate means the environment
    /// is broken (missing binary, bad target, expired key), and that must
    /// never look like a clean pass. `1` when any trial graded Fail. `0`
    /// otherwise, matching the CLI's documented exit-code contract.
    pub fn exit_code(&self) -> u8 {
        if !self.scenarios.is_empty() && self.summary.n_gradable == 0 {
            return 2;
        }
        let any_fail = self
            .scenarios
            .iter()
            .any(|s| s.trials.iter().any(|t| t.outcome == Final::Fail));
        if any_fail {
            1
        } else {
            0
        }
    }
}

fn round4(x: f64) -> f64 {
    (x * 10_000.0).round() / 10_000.0
}

/// `Summary::regression`/`Summary::capability`'s computation: the same
/// gradable-scenarios-only averaging `Report::build` does for the overall
/// numbers (design D5), filtered to one kind first. `n_gradable == 0` divides
/// by the same `.max(1)` guard as the overall computation, so an empty kind's
/// rates are `0.0` on the wire — the display-only `n/a` is
/// `print_run_report_summaries`'s job, not this function's.
fn kind_summary(scenarios: &[ScenarioResult], kind: Kind) -> KindSummary {
    let n_scenarios = scenarios.iter().filter(|s| s.kind == kind).count();
    let gradable: Vec<&ScenarioResult> = scenarios
        .iter()
        .filter(|s| s.kind == kind && s.is_gradable())
        .collect();
    let g = gradable.len().max(1) as f64;
    let pass_at_k = gradable.iter().map(|s| s.pass_at_k).sum::<f64>() / g;
    let pass_hat_k = gradable.iter().map(|s| s.pass_hat_k).sum::<f64>() / g;
    KindSummary {
        n_scenarios,
        n_gradable: gradable.len(),
        pass_at_k: round4(pass_at_k),
        pass_hat_k: round4(pass_hat_k),
    }
}

/// ISO-8601 UTC timestamp, pure std.
pub fn now_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    format!(
        "{}T{:02}:{:02}:{:02}Z",
        crate::util::civil_date_string(days),
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

#[cfg(test)]
mod exit_code_tests {
    use super::*;

    fn meta() -> ReportMeta {
        ReportMeta {
            tag: "t".into(),
            model: "m".into(),
            backend: "b".into(),
            trials: 1,
            ..Default::default()
        }
    }

    fn trial(outcome: Final) -> TrialResult {
        TrialResult {
            trial: 0,
            outcome,
            reasons: vec![],
            asserts: vec![],
            judge: None,
            judge_file: String::new(),
            judge_hash: None,
            judge_model: None,
            input_tokens: 0,
            output_tokens: 0,
            judge_input_tokens: 0,
            judge_output_tokens: 0,
            cost_usd: 0.0,
            wall_secs: 0.0,
            tool_call_count: 0,
            run_dir: String::new(),
        }
    }

    #[test]
    fn all_indeterminate_is_harness_error_not_a_clean_pass() {
        // Every trial ungradable (e.g. every run hit a missing API key) must
        // never look like exit 0 — that's exactly how a broken environment
        // hides behind a green CI check.
        let report = Report::build(
            meta(),
            vec![ScenarioResult::from_trials(
                "s".into(),
                vec![trial(Final::Indeterminate)],
            )],
        );
        assert_eq!(report.summary.n_gradable, 0);
        assert_eq!(report.exit_code(), 2);
    }

    #[test]
    fn no_scenarios_at_all_is_not_a_harness_error() {
        // An empty scenario set is a usage question (caught earlier by
        // `discover`), not this report's problem to flag.
        let report = Report::build(meta(), vec![]);
        assert_eq!(report.exit_code(), 0);
    }

    #[test]
    fn any_fail_is_exit_1() {
        let report = Report::build(
            meta(),
            vec![ScenarioResult::from_trials(
                "s".into(),
                vec![trial(Final::Fail)],
            )],
        );
        assert_eq!(report.exit_code(), 1);
    }

    #[test]
    fn all_pass_is_exit_0() {
        let report = Report::build(
            meta(),
            vec![ScenarioResult::from_trials(
                "s".into(),
                vec![trial(Final::Pass)],
            )],
        );
        assert_eq!(report.exit_code(), 0);
    }

    #[test]
    fn build_records_the_judge_file_its_hash_and_the_models_that_graded() {
        let report = Report::build(
            ReportMeta {
                judge_file: "judges/opus.toml".into(),
                judge_hash: Some("0123456789abcdef".into()),
                judge_model: Some(vec!["claude-opus-4-8".into()]),
                ..meta()
            },
            vec![],
        );
        assert_eq!(report.judge_file, "judges/opus.toml");
        assert_eq!(report.judge_hash.as_deref(), Some("0123456789abcdef"));
        assert_eq!(report.judge_model, Some(vec!["claude-opus-4-8".into()]));
    }

    /// A caller that says nothing about the ruler has not established that
    /// there wasn't one — the report must say "unknown", not "none".
    #[test]
    fn a_report_built_without_judge_meta_reads_as_unknown_not_as_ungraded() {
        let report = Report::build(meta(), vec![]);
        assert_eq!(report.judge_file, "");
        assert_eq!(report.judge_hash, None);
        assert_eq!(report.judge_model, None);
    }

    /// The three states of `judge_model` are the whole point of the field:
    /// each must survive a round trip as itself.
    #[test]
    fn the_three_judge_model_states_round_trip_distinctly() {
        let round_trip = |judge_model| {
            let report = Report::build(
                ReportMeta {
                    judge_model,
                    ..meta()
                },
                vec![],
            );
            let json = serde_json::to_string(&report).unwrap();
            serde_json::from_str::<Report>(&json).unwrap().judge_model
        };
        assert_eq!(round_trip(None), None, "unknown");
        assert_eq!(round_trip(Some(vec![])), Some(vec![]), "nothing graded");
        assert_eq!(
            round_trip(Some(vec!["m".to_string()])),
            Some(vec!["m".to_string()]),
            "m graded"
        );
    }

    #[test]
    fn a_fresh_report_carries_the_current_schema_version() {
        let report = Report::build(meta(), vec![]);
        assert_eq!(report.schema_version, 1);
    }

    /// Same precedent as `content_hash`: a baseline written before the *judge*
    /// fields existed must still load, as "unknown" rather than an error.
    /// Specifically it must not read as "nothing graded" — that baseline *was*
    /// graded (by the pinned default), so the one thing this report may not do
    /// is assert a falsehood about a real past run. It keeps the schema
    /// version it was written with; the serde defaults on the judge fields are
    /// what let it load. (The zerostack identity fields are required with no
    /// default, so the fixture carries them; that strictness is exercised by
    /// `a_report_json_lacking_zs_version_fails_to_load`.)
    #[test]
    fn a_baseline_predating_the_judge_fields_loads_as_unknown_not_as_ungraded() {
        let old = r#"{
            "schema_version": 2,
            "tag": "main",
            "model": "anthropic/claude-sonnet-4-6",
            "backend": "zs",
            "timestamp": "2026-07-01T00:00:00Z",
            "trials": 3,
            "zs_version": "zerostack 1.7.0",
            "zs_bin_path": "zerostack",
            "zs_bin_sha256": "0000000000000000000000000000000000000000000000000000000000000000",
            "git_sha": null,
            "features": null,
            "scenarios": [],
            "summary": {
                "n_scenarios": 0, "n_gradable": 0, "pass_at_k": 0.0, "pass_hat_k": 0.0,
                "indeterminate_scenarios": 0, "indeterminate_trials": 0,
                "total_cost_usd": 0.0, "avg_wall_secs": 0.0,
                "regression": {"n_scenarios": 0, "n_gradable": 0, "pass_at_k": 0.0, "pass_hat_k": 0.0},
                "capability": {"n_scenarios": 0, "n_gradable": 0, "pass_at_k": 0.0, "pass_hat_k": 0.0}
            }
        }"#;
        let report: Report = serde_json::from_str(old).unwrap();
        assert_eq!(
            report.schema_version, 2,
            "keeps the version it was written with"
        );
        assert_eq!(report.judge_file, "", "judge files did not exist yet");
        assert_eq!(report.judge_hash, None);
        assert_eq!(
            report.judge_model, None,
            "this run was graded — it must read as unknown, never as 'nothing graded'"
        );
    }

    /// A trial.json written before these fields existed is the same problem
    /// one level down: it was graded, so it may not claim it wasn't.
    #[test]
    fn a_trial_predating_the_judge_fields_loads_as_unknown_not_as_ungraded() {
        let old = r#"{
            "trial": 0,
            "outcome": "pass",
            "judge": "yes",
            "input_tokens": 1, "output_tokens": 2,
            "cost_usd": 0.1, "wall_secs": 1.0,
            "run_dir": "results/main/s/trial-0"
        }"#;
        let tr: TrialResult = serde_json::from_str(old).unwrap();
        assert_eq!(tr.judge, Some(JudgeVerdict::Yes));
        assert_eq!(tr.judge_file, "");
        assert_eq!(tr.judge_hash, None);
        assert_eq!(tr.judge_model, None);
    }

    #[test]
    fn mixed_gradable_and_indeterminate_scenarios_is_not_a_harness_error() {
        // Only a *fully* ungradable report is a harness error; a report where
        // some scenarios graded fine must still surface Fail/Pass normally.
        let report = Report::build(
            meta(),
            vec![
                ScenarioResult::from_trials("gradable".into(), vec![trial(Final::Pass)]),
                ScenarioResult::from_trials("broken".into(), vec![trial(Final::Indeterminate)]),
            ],
        );
        assert_eq!(report.exit_code(), 0);
    }
}

#[cfg(test)]
mod target_field_tests {
    use super::*;

    fn meta_with_target(target: String) -> ReportMeta {
        ReportMeta {
            tag: "t".into(),
            model: "m".into(),
            backend: "b".into(),
            trials: 1,
            target,
            ..Default::default()
        }
    }

    /// Column identity: a target under the working directory records as the
    /// relative path the caller would have typed, never the absolute one —
    /// same rule as `JudgeFileRef::path`, reused via `record_path`.
    #[test]
    fn a_target_under_cwd_records_a_relative_forward_slashed_path_never_starting_with_slash() {
        let cwd = std::env::current_dir().unwrap();
        let dir = cwd.join(format!("zseval-target-field-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("opus.toml");
        std::fs::write(&file, b"provider = \"anthropic\"\n").unwrap();

        let report = Report::build(meta_with_target(record_path(&file)), vec![]);
        let expected = format!("zseval-target-field-{}/opus.toml", std::process::id());
        assert_eq!(report.target, expected);
        assert!(!report.target.starts_with('/'), "{}", report.target);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A target outside the working directory must not leak the local
    /// filesystem layout into a committed report; only its file name
    /// survives.
    #[test]
    fn a_target_outside_cwd_records_only_its_file_name() {
        let dir =
            std::env::temp_dir().join(format!("zseval-target-field-out-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("private-target.toml");
        std::fs::write(&file, b"provider = \"anthropic\"\n").unwrap();

        let report = Report::build(meta_with_target(record_path(&file)), vec![]);
        assert_eq!(report.target, "private-target.toml");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The target field is recorded on the report itself, so a report
    /// serialized, moved away from its run dir (e.g. into `baselines/`), and
    /// deserialized elsewhere still names what it evaluated.
    #[test]
    fn a_report_copied_away_from_its_run_dir_still_names_its_target() {
        let cwd = std::env::current_dir().unwrap();
        let dir = cwd.join(format!("zseval-target-field-copy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("sonnet.toml");
        std::fs::write(&file, b"provider = \"anthropic\"\n").unwrap();

        let report = Report::build(meta_with_target(record_path(&file)), vec![]);
        let json = serde_json::to_string(&report).unwrap();

        // The run dir (and the target file within it) is gone, as if the
        // report alone had been copied into `baselines/`.
        std::fs::remove_dir_all(&dir).ok();

        let reloaded: Report = serde_json::from_str(&json).unwrap();
        assert_eq!(reloaded.target, report.target);
        assert!(!reloaded.target.is_empty());
    }

    /// Same precedent as `judge_file`/`content_hash`: a report written before
    /// the `target` field existed must still load, as an empty target rather
    /// than an error. (Identity fields are required, so the fixture carries
    /// them — see `a_report_json_lacking_zs_version_fails_to_load`.)
    #[test]
    fn a_report_json_lacking_the_field_deserialises_to_an_empty_target() {
        let old = r#"{
            "schema_version": 1,
            "tag": "main",
            "model": "anthropic/claude-sonnet-4-6",
            "backend": "zs",
            "timestamp": "2026-07-01T00:00:00Z",
            "trials": 3,
            "zs_version": "zerostack 1.7.0",
            "zs_bin_path": "zerostack",
            "zs_bin_sha256": "0000000000000000000000000000000000000000000000000000000000000000",
            "git_sha": null,
            "features": null,
            "scenarios": [],
            "summary": {
                "n_scenarios": 0, "n_gradable": 0, "pass_at_k": 0.0, "pass_hat_k": 0.0,
                "indeterminate_scenarios": 0, "indeterminate_trials": 0,
                "total_cost_usd": 0.0, "avg_wall_secs": 0.0,
                "regression": {"n_scenarios": 0, "n_gradable": 0, "pass_at_k": 0.0, "pass_hat_k": 0.0},
                "capability": {"n_scenarios": 0, "n_gradable": 0, "pass_at_k": 0.0, "pass_hat_k": 0.0}
            }
        }"#;
        let report: Report = serde_json::from_str(old).unwrap();
        assert_eq!(report.target, "");
    }
}

#[cfg(test)]
mod prompts_pack_field_tests {
    use super::*;

    fn meta() -> ReportMeta {
        ReportMeta {
            tag: "t".into(),
            model: "m".into(),
            backend: "b".into(),
            trials: 1,
            ..Default::default()
        }
    }

    /// prompts-pack 4.4: a run given a pack records its identity on the report.
    /// Path normalisation is `record_path`'s job (already covered by the
    /// target and judge-file tests); this pins that `Report::build` carries the
    /// three fields through from `ReportMeta`.
    #[test]
    fn build_carries_the_pack_identity_through() {
        let report = Report::build(
            ReportMeta {
                prompts_pack: "packs/my-pack".into(),
                prompts_hash: "deadbeef".into(),
                prompts_names: vec!["code".into(), "review".into()],
                ..meta()
            },
            vec![],
        );
        assert_eq!(report.prompts_pack, "packs/my-pack");
        assert_eq!(report.prompts_hash, "deadbeef");
        assert_eq!(report.prompts_names, vec!["code", "review"]);
    }

    /// prompts-pack 4.2: a run with no pack records the three fields empty,
    /// never absent-but-guessed.
    #[test]
    fn a_run_without_a_pack_records_empty_fields() {
        let report = Report::build(meta(), vec![]);
        assert_eq!(report.prompts_pack, "");
        assert_eq!(report.prompts_hash, "");
        assert!(report.prompts_names.is_empty());
    }

    /// prompts-pack 4.3: a report written before the *pack* fields existed
    /// still deserialises, as an empty pack rather than a parse error — same
    /// `#[serde(default)]` precedent as `target` and `judge_file`. (Identity
    /// fields are required, so the fixture carries them — see
    /// `a_report_json_lacking_zs_version_fails_to_load`.)
    #[test]
    fn a_report_json_predating_the_pack_fields_deserialises_to_empties() {
        let old = r#"{
            "schema_version": 1,
            "tag": "main",
            "model": "anthropic/claude-sonnet-4-6",
            "backend": "zs",
            "timestamp": "2026-07-01T00:00:00Z",
            "trials": 3,
            "zs_version": "zerostack 1.7.0",
            "zs_bin_path": "zerostack",
            "zs_bin_sha256": "0000000000000000000000000000000000000000000000000000000000000000",
            "git_sha": null,
            "features": null,
            "scenarios": [],
            "summary": {
                "n_scenarios": 0, "n_gradable": 0, "pass_at_k": 0.0, "pass_hat_k": 0.0,
                "indeterminate_scenarios": 0, "indeterminate_trials": 0,
                "total_cost_usd": 0.0, "avg_wall_secs": 0.0,
                "regression": {"n_scenarios": 0, "n_gradable": 0, "pass_at_k": 0.0, "pass_hat_k": 0.0},
                "capability": {"n_scenarios": 0, "n_gradable": 0, "pass_at_k": 0.0, "pass_hat_k": 0.0}
            }
        }"#;
        let report: Report = serde_json::from_str(old).unwrap();
        assert_eq!(report.prompts_pack, "");
        assert_eq!(report.prompts_hash, "");
        assert!(report.prompts_names.is_empty());
    }
}

#[cfg(test)]
mod zs_identity_field_tests {
    use super::*;

    fn meta_with_zs(zs: ZsIdentity) -> ReportMeta {
        ReportMeta {
            tag: "t".into(),
            model: "m".into(),
            backend: "zs".into(),
            trials: 1,
            zs,
            ..Default::default()
        }
    }

    /// trustworthy-numbers 3.1: `Report::build` flattens the captured identity
    /// onto the report's own fields, and every field survives a JSON round trip
    /// — including `git_sha`/`features` as `null`.
    #[test]
    fn build_flattens_the_identity_and_it_round_trips() {
        let report = Report::build(
            meta_with_zs(ZsIdentity {
                zs_version: "zerostack 1.7.2".into(),
                zs_bin_path: "zerostack".into(),
                zs_bin_sha256: "a".repeat(64),
                git_sha: None,
                features: None,
            }),
            vec![],
        );
        assert_eq!(report.zs_version, "zerostack 1.7.2");
        assert_eq!(report.zs_bin_path, "zerostack");
        assert_eq!(report.zs_bin_sha256, "a".repeat(64));
        assert_eq!(report.git_sha, None);
        assert_eq!(report.features, None);

        let json = serde_json::to_string(&report).unwrap();
        let back: Report = serde_json::from_str(&json).unwrap();
        assert_eq!(back.zs_version, "zerostack 1.7.2");
        assert_eq!(back.zs_bin_sha256, "a".repeat(64));
        assert_eq!(back.git_sha, None);
        assert_eq!(back.features, None);
    }

    /// trustworthy-numbers 3.1 (required, no default): a report JSON missing an
    /// identity field must fail to load, not deserialize to an empty string.
    /// This is the strictness the whole feature rests on — an identity-less
    /// report may not exist. (S7 extends this stance to the rest of the report
    /// family; here it is proven for the identity fields alone.)
    #[test]
    fn a_report_json_lacking_zs_version_fails_to_load() {
        let missing_version = r#"{
            "schema_version": 1,
            "tag": "main",
            "model": "anthropic/claude-sonnet-4-6",
            "backend": "zs",
            "timestamp": "2026-07-01T00:00:00Z",
            "trials": 3,
            "zs_bin_path": "zerostack",
            "zs_bin_sha256": "0000000000000000000000000000000000000000000000000000000000000000",
            "git_sha": null,
            "features": null,
            "scenarios": [],
            "summary": {
                "n_scenarios": 0, "n_gradable": 0, "pass_at_k": 0.0, "pass_hat_k": 0.0,
                "indeterminate_scenarios": 0, "indeterminate_trials": 0,
                "total_cost_usd": 0.0, "avg_wall_secs": 0.0,
                "regression": {"n_scenarios": 0, "n_gradable": 0, "pass_at_k": 0.0, "pass_hat_k": 0.0},
                "capability": {"n_scenarios": 0, "n_gradable": 0, "pass_at_k": 0.0, "pass_hat_k": 0.0}
            }
        }"#;
        let err = serde_json::from_str::<Report>(missing_version).unwrap_err();
        assert!(
            err.to_string().contains("zs_version"),
            "the load error should name the missing field: {err}"
        );
    }
}

#[cfg(test)]
mod prompt_field_tests {
    use super::*;

    /// prompts-pack 5.1: `PromptSource` round-trips all four spec values
    /// through serde exactly.
    #[test]
    fn prompt_source_round_trips_all_four_values() {
        let round_trip = |s: PromptSource| {
            let json = serde_json::to_string(&s).unwrap();
            serde_json::from_str::<PromptSource>(&json).unwrap()
        };
        assert_eq!(round_trip(PromptSource::Pack), PromptSource::Pack);
        assert_eq!(round_trip(PromptSource::Stock), PromptSource::Stock);
        assert_eq!(round_trip(PromptSource::Scenario), PromptSource::Scenario);
        assert_eq!(round_trip(PromptSource::Unknown), PromptSource::Unknown);
    }

    /// The wire values are exactly the spec's four names, lowercase.
    #[test]
    fn prompt_source_serializes_to_the_spec_named_lowercase_strings() {
        assert_eq!(
            serde_json::to_string(&PromptSource::Pack).unwrap(),
            "\"pack\""
        );
        assert_eq!(
            serde_json::to_string(&PromptSource::Stock).unwrap(),
            "\"stock\""
        );
        assert_eq!(
            serde_json::to_string(&PromptSource::Scenario).unwrap(),
            "\"scenario\""
        );
        assert_eq!(
            serde_json::to_string(&PromptSource::Unknown).unwrap(),
            "\"unknown\""
        );
    }

    /// prompts-pack 5.1: an unrecognized value must fail to deserialize
    /// rather than silently reading as `unknown` — `#[serde(default)]` on the
    /// *field* covers a missing field, not a garbled one, and the spec relies
    /// on that distinction holding.
    #[test]
    fn an_unrecognized_prompt_source_value_is_a_deserialization_error() {
        let err = serde_json::from_str::<PromptSource>("\"bogus\"");
        assert!(
            err.is_err(),
            "a garbled value must not silently read as unknown"
        );
    }

    /// prompts-pack 5.2: a `ScenarioResult` written before these fields
    /// existed must still deserialize, same `#[serde(default)]` precedent as
    /// `content_hash`, and its source must read as `unknown` rather than
    /// `stock`: an older report's prompt was never observed, which is not the
    /// same fact as a run that observably used the built-in prompts.
    #[test]
    fn a_scenario_result_predating_these_fields_deserializes_with_source_unknown() {
        let old = r#"{
            "id": "s",
            "trials": [],
            "pass_at_k": 0.0,
            "pass_hat_k": 0.0,
            "indeterminate": 0
        }"#;
        let sc: ScenarioResult = serde_json::from_str(old).unwrap();
        assert_eq!(sc.prompt_name, "");
        assert_eq!(sc.prompt_source, PromptSource::Unknown);
        assert_ne!(
            sc.prompt_source,
            PromptSource::Stock,
            "an older report's prompt was never observed, distinct from a run \
             that observably used the built-in prompts"
        );
    }
}

#[cfg(test)]
mod judge_file_ref_tests {
    use super::*;

    /// A judge file under the working directory records as the relative path
    /// the caller would have typed — never the absolute one, which would write
    /// the local filesystem layout into an artifact meant for `baselines/`.
    #[test]
    fn a_file_under_the_working_directory_records_relative_to_it() {
        let cwd = std::env::current_dir().unwrap();
        let dir = cwd.join(format!("zseval-jfr-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("opus.toml");
        std::fs::write(&file, b"model = \"claude-opus-4-8\"\n").unwrap();

        let by_abs = JudgeFileRef::of(&file);
        let expected = format!("zseval-jfr-{}/opus.toml", std::process::id());
        assert_eq!(by_abs.path, expected);
        assert!(!by_abs.path.starts_with('/'), "{}", by_abs.path);
        // The same file named relatively records identically: what is recorded
        // is the file, not the spelling it arrived in.
        assert_eq!(JudgeFileRef::of(Path::new(&expected)).path, expected);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// `--judge /Users/alice/private-client/judges/x.toml` must not leak that
    /// path into a committed report; only the file name survives.
    #[test]
    fn a_file_outside_the_working_directory_records_as_its_bare_name() {
        let dir = std::env::temp_dir().join(format!("zseval-jfr-out-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("private-judge.toml");
        std::fs::write(&file, b"model = \"x\"\n").unwrap();

        assert_eq!(JudgeFileRef::of(&file).path, "private-judge.toml");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A path is not an identity: the same path with different bytes behind it
    /// is a different ruler, and only the hash can say so.
    #[test]
    fn the_hash_tracks_the_bytes_not_the_path() {
        let dir = std::env::temp_dir().join(format!("zseval-jfr-hash-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("judge.toml");

        std::fs::write(&file, b"model = \"sonnet\"\n").unwrap();
        let first = JudgeFileRef::of(&file);
        std::fs::write(&file, b"model = \"opus\"\n").unwrap();
        let second = JudgeFileRef::of(&file);

        assert_eq!(first.path, second.path);
        assert_ne!(first.hash, second.hash);
        assert_eq!(
            first.hash,
            Some(crate::util::fnv1a_hex(b"model = \"sonnet\"\n")),
            "the same fingerprint a scenario uses for its own source"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// An unreadable judge file leaves the fingerprint unknown rather than
    /// guessed, and the default is "no judge file was named" throughout.
    #[test]
    fn an_unreadable_file_has_no_hash_and_the_default_names_nothing() {
        let missing = JudgeFileRef::of(Path::new("judges/does-not-exist.toml"));
        assert_eq!(missing.path, "judges/does-not-exist.toml");
        assert_eq!(missing.hash, None);

        let none = JudgeFileRef::default();
        assert_eq!(none.path, "");
        assert_eq!(none.hash, None);
    }
}

#[cfg(test)]
mod kind_summary_tests {
    use super::*;

    fn meta() -> ReportMeta {
        ReportMeta {
            tag: "t".into(),
            model: "m".into(),
            backend: "b".into(),
            trials: 1,
            ..Default::default()
        }
    }

    fn trial(outcome: Final) -> TrialResult {
        TrialResult {
            trial: 0,
            outcome,
            reasons: vec![],
            asserts: vec![],
            judge: None,
            judge_file: String::new(),
            judge_hash: None,
            judge_model: None,
            input_tokens: 0,
            output_tokens: 0,
            judge_input_tokens: 0,
            judge_output_tokens: 0,
            cost_usd: 0.0,
            wall_secs: 0.0,
            tool_call_count: 0,
            run_dir: String::new(),
        }
    }

    fn scenario(id: &str, kind: Kind, trials: Vec<TrialResult>) -> ScenarioResult {
        let mut sr = ScenarioResult::from_trials(id.into(), trials);
        sr.kind = kind;
        sr
    }

    /// trustworthy-numbers 5.1: `Summary::regression`/`Summary::capability`
    /// are computed over that kind's own scenarios only, independent of the
    /// other kind and of the unchanged, blended overall.
    #[test]
    fn per_kind_numbers_are_independent_of_each_other_and_of_overall() {
        let report = Report::build(
            meta(),
            vec![
                // regression: one scenario a clean pass (pass@k 1, pass^k 1),
                // one a clean fail (pass@k 0, pass^k 0) — averaged, 0.5/0.5.
                scenario("reg-pass", Kind::Regression, vec![trial(Final::Pass)]),
                scenario("reg-fail", Kind::Regression, vec![trial(Final::Fail)]),
                // capability: one clean pass — pass@k 1, pass^k 1.
                scenario("cap-pass", Kind::Capability, vec![trial(Final::Pass)]),
            ],
        );

        assert_eq!(report.summary.regression.n_scenarios, 2);
        assert_eq!(report.summary.regression.n_gradable, 2);
        assert_eq!(report.summary.regression.pass_at_k, 0.5);
        assert_eq!(report.summary.regression.pass_hat_k, 0.5);

        assert_eq!(report.summary.capability.n_scenarios, 1);
        assert_eq!(report.summary.capability.n_gradable, 1);
        assert_eq!(report.summary.capability.pass_at_k, 1.0);
        assert_eq!(report.summary.capability.pass_hat_k, 1.0);

        // The blended overall stays exactly what it always was: unaffected
        // by the per-kind split existing alongside it.
        assert_eq!(report.summary.n_scenarios, 3);
        assert_eq!(report.summary.n_gradable, 3);
        assert_eq!(report.summary.pass_at_k, round4((1.0 + 0.0 + 1.0) / 3.0));
        assert_eq!(report.summary.pass_hat_k, round4((1.0 + 0.0 + 1.0) / 3.0));
    }

    /// trustworthy-numbers 5.1: a fully-indeterminate scenario is excluded
    /// from its own kind's gradable count and rate — the same "never counted
    /// as a 0" rule `ScenarioResult::is_gradable` already enforces overall.
    #[test]
    fn a_fully_indeterminate_scenario_is_excluded_from_its_kinds_rate() {
        let report = Report::build(
            meta(),
            vec![
                scenario(
                    "cap-broken",
                    Kind::Capability,
                    vec![trial(Final::Indeterminate)],
                ),
                scenario("cap-pass", Kind::Capability, vec![trial(Final::Pass)]),
            ],
        );
        assert_eq!(report.summary.capability.n_scenarios, 2);
        assert_eq!(report.summary.capability.n_gradable, 1);
        assert_eq!(report.summary.capability.pass_at_k, 1.0);
    }

    /// trustworthy-numbers 5.1 / design D5: a kind with `n_gradable == 0`
    /// serializes its rates as `0.0` on the wire — matching the current
    /// overall behavior, never a third representation. (The `n/a` rendering
    /// is `print_run_report_summaries`'s display-only convention, not this
    /// struct's.)
    #[test]
    fn an_empty_kind_serializes_its_rates_as_zero() {
        let report = Report::build(
            meta(),
            vec![scenario(
                "reg-pass",
                Kind::Regression,
                vec![trial(Final::Pass)],
            )],
        );
        assert_eq!(report.summary.capability.n_scenarios, 0);
        assert_eq!(report.summary.capability.n_gradable, 0);
        assert_eq!(report.summary.capability.pass_at_k, 0.0);
        assert_eq!(report.summary.capability.pass_hat_k, 0.0);

        let json = serde_json::to_value(&report.summary.capability).unwrap();
        assert_eq!(json["pass_at_k"], serde_json::json!(0.0));
        assert_eq!(json["pass_hat_k"], serde_json::json!(0.0));
    }

    /// trustworthy-numbers 5.1: grouping by kind is a render-time concern
    /// only — the `scenarios` array on the report itself keeps discovery
    /// order regardless of how kinds interleave.
    #[test]
    fn scenarios_array_order_is_unchanged_by_kind() {
        let report = Report::build(
            meta(),
            vec![
                scenario("cap-1", Kind::Capability, vec![trial(Final::Pass)]),
                scenario("reg-1", Kind::Regression, vec![trial(Final::Pass)]),
                scenario("cap-2", Kind::Capability, vec![trial(Final::Pass)]),
            ],
        );
        let ids: Vec<&str> = report.scenarios.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["cap-1", "reg-1", "cap-2"]);
    }
}
