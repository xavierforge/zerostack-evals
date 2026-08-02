//! Orchestration: scenarios × trials -> graded trials -> report on disk.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result};

use crate::backend::AgentBackend;
use crate::judge::{Judge, JudgeVerdict};
use crate::prompts::PromptPack;
use crate::scenario::{Mode, Scenario};
use crate::transcript::{RecordedPrompt, Transcript};
use crate::verdict::{
    Final, JudgeFileRef, PromptSource, Report, ReportMeta, ScenarioResult, TrialResult,
};

pub struct RunOptions {
    /// The target config.toml seeded into each run (see `backend::ZsCli`) —
    /// kept here too so the report can record what was actually evaluated
    /// (`target::describe`), independent of whichever backend is in use.
    pub target: Option<PathBuf>,
    pub trials_override: Option<usize>,
    pub tag: String,
    pub no_judge: bool,
    pub results_root: PathBuf,
    pub max_total_usd: Option<f64>,
    /// How many trials of the *same* scenario to drive concurrently — trials
    /// are independent (each gets its own isolated run_dir), so this is pure
    /// wall-clock win with no change to grading. Trial 0 always runs solo
    /// first to warm the provider's prompt cache (see
    /// `run_trials_for_scenario`); only the remaining trials fan out.
    /// Scenarios themselves stay sequential: the cost-cap check below is
    /// scenario-granular ("a scenario runs its full trial count or not at
    /// all"), and interleaving scenarios too would only complicate that for
    /// no benefit, since a suite's wall time is dominated by
    /// trials-per-scenario, not scenario count. `1` (the default) reproduces
    /// the old strictly-sequential, live-printed behavior exactly.
    pub jobs: usize,
    /// The judge file this run was told to grade with (`--judge`), recorded
    /// in the report. `None` when none was named (`--no-judge`, or a suite
    /// with no rubric) — there is no file to name, and there is no built-in
    /// default to fall back to. Kept here (rather than read off the
    /// `Judge`) for the same reason as `target`: the report must describe the
    /// run regardless of which judge implementation is in use, including a
    /// test double.
    ///
    /// There is deliberately no matching `judge_model` option: which model
    /// graded is not something the caller gets to declare. It is read back
    /// from the judge's own responses (see `Report::judge_model`).
    ///
    /// Named `judge_file`, not `judge`, so it stops colliding with the
    /// `judge: &dyn Judge` argument at the same call site: this is the file
    /// naming a ruler, that is the ruler.
    pub judge_file: Option<PathBuf>,
    /// Whether this `run_suite` call is one of several targets evaluated by
    /// one `zseval run` invocation (target-matrix section 3/4). `false` (the
    /// default single-target shape) keeps today's flat `results/<tag>/`
    /// layout; `true` nests this target's report and trial dirs one level
    /// deeper, under `results/<tag>/<stem>/` (`stem` = `target`'s filename
    /// without extension — see `target::stem`), so N targets sharing one
    /// `--tag` don't collide on `sc.id`. Requires `target` to be `Some`: the
    /// stem has nothing to derive from otherwise.
    pub multi_target: bool,
}

/// Everything the grading half of a trial needs to know about the ruler: the
/// referee to ask, which file configured it (recorded on the trial), whether
/// to skip it, and where its request/response artifacts land.
struct Grading<'a> {
    judge: &'a dyn Judge,
    /// The judge file as it will be recorded — resolved once per suite rather
    /// than per trial, so every trial of a run records the same fingerprint.
    judge_file: &'a JudgeFileRef,
    no_judge: bool,
    /// Where the judge call leaves `judge-request.json` /
    /// `judge-response.json`. The trial's own run dir for a fresh run; a
    /// regrade points this at a subdirectory instead, so re-scoring with a
    /// second ruler never destroys the evidence of what graded the first time
    /// (see `regrade`).
    judge_artifacts_dir: &'a Path,
}

/// The results root for one target's report and trial dirs: flat
/// `results/<tag>/` for a single-target run, nested
/// `results/<tag>/<stem>/` when `multi_target` (so N targets sharing one
/// `--tag` don't collide on `sc.id`). Derived in exactly one place and
/// reused by both `run_suite` and the end-of-run summary printer, rather
/// than re-deriving the same formula at each site (design.md: compute once,
/// reuse). Requires `target` to be `Some` when `multi_target`: the stem has
/// nothing to derive from otherwise.
pub fn run_root(
    results_root: &Path,
    tag: &str,
    multi_target: bool,
    target: Option<&Path>,
) -> Result<PathBuf> {
    if multi_target {
        let target = target.ok_or_else(|| {
            anyhow::anyhow!("multi-target run requires --target to derive the results stem")
        })?;
        Ok(results_root.join(tag).join(crate::target::stem(target)))
    } else {
        Ok(results_root.join(tag))
    }
}

/// zerostack's own hardcoded prompt when a config sets no `default_prompt`: it
/// loads `code` (verified against zerostack v1.6.1, `src/config/startup.rs`).
/// Pinned here the same way `LoopCfg` pins the loop invariants it copied from
/// upstream, so a drift in that default is a one-line fix with a version to
/// check it against, rather than a value the harness silently mis-records.
const DEFAULT_PROMPT_FALLBACK: &str = "code";

/// Does this scenario's own file placements target the effective config,
/// making the harness's seeded copy no longer the last word on it? True when a
/// declared `[[files]]` overwrites the config file itself, `config:config.toml`
/// (the `config:` root that `backend::ZsCli` copies the target onto), or the
/// project-local `work:.zerostack/config.toml` that zerostack merges over the
/// base config. Any *other* `config:` seed (a memory file, an agent doc) leaves
/// `default_prompt` untouched, so it must not blind derivation. Domain sugar
/// (`[seed.memory]`, `[seed.mcp]`) is likewise not counted: memory seeds under
/// `config/agent/memory/` (data, not the config file) and mcp rewrites the
/// seeded `config.toml` in place without touching `default_prompt`, so neither
/// moves the derived default out from under us.
fn seeds_effective_config(sc: &Scenario) -> bool {
    sc.files.iter().any(|f| match f.dest.split_once(':') {
        Some(("config", rel)) => rel_components(rel) == ["config.toml"],
        Some(("work", rel)) => rel_components(rel) == [".zerostack", "config.toml"],
        _ => false,
    })
}

/// Split a seed-dest relative path into its components, dropping any `.`
/// segments, so `.zerostack/config.toml` and `./.zerostack/config.toml`
/// compare equal.
fn rel_components(rel: &str) -> Vec<&str> {
    rel.split('/')
        .filter(|c| !c.is_empty() && *c != ".")
        .collect()
}

/// Does this scenario seed its own file for prompt `name` into
/// `work:.zerostack/prompts/<name>.md`, the top layer of zerostack's override
/// chain? When it does, that file (not the pack's, and not the built-in) is
/// what loads — the pack was copied before `seed::apply`, so the scenario's
/// placement lands last.
fn scenario_seeds_prompt(sc: &Scenario, name: &str) -> bool {
    let want = [".zerostack", "prompts", &format!("{name}.md")];
    sc.files.iter().any(|f| match f.dest.split_once(':') {
        Some(("work", rel)) => rel_components(rel) == want,
        _ => false,
    })
}

/// The prompt identity one scenario records, plus whatever the run has to say
/// out loud while recording it. The warnings are returned rather than printed
/// so the mapping stays a pure function the tests can read: `run_suite` is the
/// one place that puts them on stderr.
struct PromptRecord {
    name: String,
    source: PromptSource,
    warnings: Vec<String>,
}

/// Derive which prompt a scenario would have loaded and from which layer,
/// given the run's pack and the `default_prompt` read off the target config.
///
/// Inferring from seeds is not observing, so this is no longer what a
/// session-backed scenario records (`record_prompt` reads that back off the
/// session, design D3): here it is the cross-check that warns when the two
/// disagree. It is still the recorded value for a `mode = "loop"` scenario,
/// which upstream's `run_headless_loop` leaves no session file for.
///
/// The prompt *name* comes first: a scenario's own `prompt` field if set,
/// otherwise the config's `default_prompt`, otherwise zerostack's `code`
/// fallback. A loop scenario abandons that derivation — recording `unknown`
/// with no name — when it seeds the effective config out from under the
/// harness's copy (`seeds_effective_config`), since the value read would be
/// one that never took effect and, with no session, nothing else can say
/// which prompt did. A session-backed scenario keeps deriving through that
/// case: its readback is the last word on what loaded whoever wrote the
/// config, so the derivation's job there is only to have a value to compare.
///
/// The *source* then answers which layer supplied that name: the scenario's
/// own seed wins (it lands last), then the pack, then the built-in `stock`.
fn derive_prompt(
    sc: &Scenario,
    pack: Option<&PromptPack>,
    config_default_prompt: Option<&str>,
) -> (String, PromptSource) {
    let name = match &sc.prompt {
        Some(p) => p.clone(),
        None => {
            if sc.mode == Mode::Loop && seeds_effective_config(sc) {
                return (String::new(), PromptSource::Unknown);
            }
            config_default_prompt
                .filter(|s| !s.is_empty())
                .unwrap_or(DEFAULT_PROMPT_FALLBACK)
                .to_string()
        }
    };
    let source = if scenario_seeds_prompt(sc, &name) {
        PromptSource::Scenario
    } else if pack.is_some_and(|p| p.names().iter().any(|n| *n == name)) {
        PromptSource::Pack
    } else {
        PromptSource::Stock
    };
    (name, source)
}

/// What one trial's session had to say about which prompt it loaded (design
/// D3). Three states, not two: a trial that produced no readable session
/// observed nothing, which is not the same evidence as a session that was read
/// and recorded no prompt. Conflating them makes a trial that timed out or
/// failed its schema check look like a binary too old to record prompts, and
/// makes it outvote the trials that did read one back.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PromptReadback {
    /// The trial's session recorded which prompt it loaded.
    Recorded(RecordedPrompt),
    /// The trial's session was read and carries no `prompt` field: an
    /// observed absence, so the binary under test is the finding.
    Absent,
    /// The trial never produced a readable session at all (a backend error, a
    /// transcript schema mismatch), so it observed nothing — its own failure
    /// is already reported on the trial, with its real reason.
    Unobserved,
}

/// What one scenario's trials agreed their sessions recorded (design D4).
/// `prompt_name`/`prompt_source` are scenario-level facts, so the trials have
/// to produce one answer between them; the ways they can fail to are kept
/// apart here because they have different fixes.
enum Reconciled {
    /// Every trial that produced a session read back the same prompt — the
    /// expected case, since the trials of one scenario are identically seeded
    /// and prompt resolution is deterministic.
    Agreed(RecordedPrompt),
    /// Every trial that produced a session read back no prompt at all: the
    /// binary under test predates upstream's prompt recording (PR #228), and
    /// the fix is a rebuild.
    Absent,
    /// The trials that produced sessions did not agree, including one
    /// recording a prompt where another recorded none. Nothing about identical
    /// seeds should produce two answers, so the disagreement is itself the
    /// finding.
    Split,
    /// No trial produced a readable session, so there was nothing to reconcile
    /// — the trials' own failures are the finding, and the binary's age is not
    /// in evidence either way.
    Unobserved,
}

/// Reconcile over only the trials that actually produced a readable session: a
/// trial that observed nothing is no party to what the others observed, so it
/// neither votes for an absence nor counts as disagreement.
fn reconcile(readbacks: &[PromptReadback]) -> Reconciled {
    let observed: Vec<&PromptReadback> = readbacks
        .iter()
        .filter(|r| **r != PromptReadback::Unobserved)
        .collect();
    let Some(first) = observed.first() else {
        return Reconciled::Unobserved;
    };
    if observed.iter().any(|r| r != first) {
        return Reconciled::Split;
    }
    match first {
        PromptReadback::Recorded(p) => Reconciled::Agreed(p.clone()),
        // Everything left agrees with `first`, and `Unobserved` was filtered
        // out of `observed`, so this is the observed absence.
        _ => Reconciled::Absent,
    }
}

/// The report's own spelling of a `PromptSource`, so a warning names the value
/// its reader will find in `report.json` rather than a Rust debug name.
fn source_label(source: PromptSource) -> &'static str {
    match source {
        PromptSource::Unknown => "unknown",
        PromptSource::Stock => "stock",
        PromptSource::Pack => "pack",
        PromptSource::Scenario => "scenario",
    }
}

/// Every distinct readback the trials produced, for the disagreement warning
/// to name: `<name>/<source>` in upstream's two-value vocabulary, or `none`
/// for a trial whose session recorded no prompt. Trials that produced no
/// session are left out, the same as in `reconcile` — they observed nothing,
/// so they are not among the answers being disagreed over.
fn describe_readbacks(readbacks: &[PromptReadback]) -> String {
    let mut seen: Vec<String> = readbacks
        .iter()
        .filter_map(|r| match r {
            PromptReadback::Recorded(p) => Some(format!("{}/{}", p.name, p.source)),
            PromptReadback::Absent => Some("none".to_string()),
            PromptReadback::Unobserved => None,
        })
        .collect();
    seen.sort();
    seen.dedup();
    seen.join(", ")
}

/// What one scenario records for its prompt identity, given every trial's
/// session readback (design D3/D4).
///
/// The readback is the value: which prompt zerostack loaded is something the
/// session observed, not something the harness can infer from what it seeded.
/// This maps upstream's two-value `source` (`built_in` / `user_file`) onto the
/// report's four-way `prompt_source` by asking which layer provided the name
/// that loaded — the scenario's own seed lands last and so wins over the pack,
/// and a user file from neither layer means the trial environment is not what
/// the harness planted, which is `unknown` plus a warning.
///
/// The derivation stays as a cross-check: when it disagrees, the readback wins
/// and the run warns naming both. That is where the known upstream edge
/// surfaces benignly — a pack prompt whose bytes equal the built-in is
/// classified `built_in` by upstream's content comparison, records `stock`,
/// and the warning explains why rather than either value being silently wrong.
fn record_prompt(
    sc: &Scenario,
    pack: Option<&PromptPack>,
    config_default_prompt: Option<&str>,
    readbacks: &[PromptReadback],
) -> PromptRecord {
    let (derived_name, derived_source) = derive_prompt(sc, pack, config_default_prompt);
    // A loop run writes no session (see `scenario::LoopCfg`), so its silence
    // is expected rather than evidence of a stale binary, and the derivation
    // is all there is to record.
    if sc.mode == Mode::Loop {
        return PromptRecord {
            name: derived_name,
            source: derived_source,
            warnings: Vec::new(),
        };
    }

    let mut warnings = Vec::new();
    let unknown = |warnings: Vec<String>| PromptRecord {
        name: String::new(),
        source: PromptSource::Unknown,
        warnings,
    };
    let readback = match reconcile(readbacks) {
        Reconciled::Agreed(p) => p,
        Reconciled::Absent => {
            warnings.push(format!(
                "scenario {}: no trial's session recorded which prompt it loaded, so its \
                 prompt is unknown — ZS_BIN likely predates prompt recording (zerostack \
                 PR #228); rebuild it from a mainline that carries it",
                sc.id
            ));
            return unknown(warnings);
        }
        Reconciled::Split => {
            warnings.push(format!(
                "scenario {}: its trials disagree on the prompt they recorded ({}), so its \
                 prompt is unknown — identically seeded trials resolving two prompts is \
                 itself evidence something is wrong",
                sc.id,
                describe_readbacks(readbacks)
            ));
            return unknown(warnings);
        }
        Reconciled::Unobserved => {
            warnings.push(format!(
                "scenario {}: no trial produced a readable session, so nothing could be read \
                 back and its prompt is unknown — every trial failed before it could observe \
                 one, each for the reason recorded on it",
                sc.id
            ));
            return unknown(warnings);
        }
    };

    let source = if readback.source == "built_in" {
        PromptSource::Stock
    } else if scenario_seeds_prompt(sc, &readback.name) {
        PromptSource::Scenario
    } else if pack.is_some_and(|p| p.names().iter().any(|n| *n == readback.name)) {
        PromptSource::Pack
    } else {
        warnings.push(format!(
            "scenario {}: its session loaded prompt '{}' from a user file that neither the \
             scenario's own placements nor the pack provide, so its source is unknown — the \
             trial environment is not what the harness planted",
            sc.id, readback.name
        ));
        PromptSource::Unknown
    };

    if (readback.name.as_str(), source) != (derived_name.as_str(), derived_source) {
        warnings.push(format!(
            "scenario {}: the harness's seeds derive prompt '{}' ({}), but its session \
             recorded '{}' ({}) — the session is what actually loaded, so that is what is \
             recorded",
            sc.id,
            derived_name,
            source_label(derived_source),
            readback.name,
            source_label(source),
        ));
    }

    PromptRecord {
        name: readback.name,
        source,
        warnings,
    }
}

pub fn run_suite(
    scenarios: &[Scenario],
    backend: &dyn AgentBackend,
    judge: &dyn Judge,
    opts: &RunOptions,
) -> Result<Report> {
    let mut results = Vec::new();
    let mut spent = 0.0_f64;

    // Capture which zerostack (or fixture) is about to produce this run's
    // evidence, before any trial spends anything. For `ZsCli` this runs
    // `--version` and hashes the binary; an unrunnable or stale binary aborts
    // here, so an identity-less report can never exist. Done once per suite,
    // never per trial. In a multi-target run each target's suite captures its
    // own identity, but the binary is shared across targets (only `--target`
    // differs), so every per-target report records the same value, and a broken
    // binary still aborts before the first target spends.
    let zs = backend.identity()?;

    // Resolved once: the file is read here, so every trial and the report all
    // record the same path and the same fingerprint of the same bytes.
    let judge_file = opts
        .judge_file
        .as_deref()
        .map(JudgeFileRef::of)
        .unwrap_or_default();

    // Everything for this target lives under one root: the report next to
    // its per-trial artifacts, so results/ never fills with loose files.
    // Derived once here and threaded down (rather than re-derived at the
    // trial-dir site) so the report and its trial dirs can never split
    // across two different roots — see design.md's "implementation trap".
    let run_root = run_root(
        &opts.results_root,
        &opts.tag,
        opts.multi_target,
        opts.target.as_deref(),
    )?;
    std::fs::create_dir_all(&run_root)?;

    // A clean, run-level copy of the target config — identity, not the
    // per-trial seed `ZsCli::run` writes into each trial's own config dir
    // (backend.rs:307), which a scenario's `config:` seed may then override.
    // This copy is what a detached `report.json` (e.g. one embedded in
    // `experiments/`) can point back to for what was actually evaluated.
    if let Some(target) = &opts.target {
        std::fs::copy(target, run_root.join("target.toml"))
            .with_context(|| format!("copy run-level target {}", target.display()))?;
    }

    // Resolved once for the whole suite: the pack every trial is seeded with,
    // and the `default_prompt` off the target config. The latter is read from
    // `opts.target` (the run-level identity copy), which every trial's seeded
    // config derives from — mcp's in-place rewrite adds `mcp_servers` but never
    // `default_prompt`, so this value is still the effective one there.
    let pack = backend.prompt_pack();
    let config_default_prompt = opts
        .target
        .as_deref()
        .and_then(crate::target::default_prompt);

    let mut budget_truncated = false;
    for sc in scenarios {
        // Check the cost cap once per scenario, so a scenario always runs its
        // full trial count or not at all — never a partial, misleading pass^k.
        if let Some(cap) = opts.max_total_usd {
            if spent >= cap {
                eprintln!("budget cap ${cap} reached; stopping before {}", sc.id);
                // A declared scenario is going unrun: record the fact on the
                // report so `matrix` can mark this column truncated rather
                // than guessing from its scenario count.
                budget_truncated = true;
                break;
            }
        }
        let trials = opts.trials_override.unwrap_or(sc.trials).max(1);
        let graded =
            run_trials_for_scenario(sc, backend, judge, &judge_file, opts, trials, &run_root)?;
        let (trial_results, readbacks): (Vec<TrialResult>, Vec<PromptReadback>) =
            graded.into_iter().map(|g| (g.result, g.prompt)).unzip();
        for tr in &trial_results {
            spent += tr.cost_usd;
        }
        // Record the scenario's kind verbatim on the result, so matrix and the
        // Day-2 site group from report JSON alone (scenario-kind spec / D4).
        let mut sr = ScenarioResult::from_trials_with_hash(
            sc.id.clone(),
            sc.kind,
            sc.content_hash.clone(),
            trial_results,
        );
        // Which prompt this scenario loaded is read back off its trials'
        // sessions and reconciled here, because it is one fact per scenario,
        // not per trial (design D4).
        let prompt = record_prompt(sc, pack, config_default_prompt.as_deref(), &readbacks);
        for warning in &prompt.warnings {
            eprintln!("{warning}");
        }
        sr.prompt_name = prompt.name;
        sr.prompt_source = prompt.source;
        results.push(sr);
    }

    // A pack seeded into every trial but resolved by none is inert: the
    // report's headline number is entirely the built-ins' score, not the
    // pack's, and nothing about the report itself says so — `prompts_pack`
    // is populated either way. Checked once, over every scenario this run
    // actually produced, rather than per scenario: a partial hit (some
    // `pack`, some not) is real signal and stays visible only in each
    // scenario's own `prompt_source` (design.md, "Record which prompt each
    // scenario loaded, not merely which names intersect").
    if let Some(pack) = pack {
        let loaded = results
            .iter()
            .any(|sr| sr.prompt_source == PromptSource::Pack);
        if !loaded {
            eprintln!(
                "prompts pack {} was seeded but never loaded: no scenario resolved a prompt \
                 from it, so this report reflects zerostack's built-in prompts, not the pack",
                pack.dir().display()
            );
        }
    }

    // Two different facts, from two different places. The judge file is what
    // the run was configured with; it holds whether or not the judge was ever
    // called. `judge_model` is what actually graded, so it can only come from
    // the trials themselves. Every distinct ruler is listed: on the rare
    // disagreement, picking one to stand for the rest would be the silent lie
    // this field exists to prevent.
    let all_trials = || results.iter().flat_map(|s| s.trials.iter());
    let judge_model = if all_trials().any(|t| t.judge_model.is_none()) {
        // Some trial could not name the ruler that graded it, so this run's
        // rulers cannot be listed: an unnamed one might be among them, and a
        // list that silently omitted it would read as complete.
        None
    } else {
        let mut served: Vec<String> = all_trials()
            .filter_map(|t| t.judge_model.clone())
            .filter(|m| !m.is_empty())
            .collect();
        served.sort();
        served.dedup();
        // Empty here is "nothing was graded", which every trial agreed on.
        Some(served)
    };
    // Pack identity is recorded from the backend, which is what actually
    // seeds the pack (`ZsCli::run`); `mock` and any packless run report empty
    // fields. Normalised through the same `record_path` rule as `target` and
    // `judge_file` so a report copied into `baselines/` names a relative pack,
    // never someone's absolute filesystem path.
    let (prompts_pack, prompts_hash, prompts_names) = match pack {
        Some(pack) => (
            crate::verdict::record_path(pack.dir()),
            pack.fingerprint(),
            pack.names().into_iter().map(String::from).collect(),
        ),
        None => (String::new(), String::new(), Vec::new()),
    };
    let report = Report::build(
        ReportMeta {
            tag: opts.tag.clone(),
            // `--target` is mandatory for `zs` and rejected for `mock`
            // (target-matrix section 2), so a mock run never has a target to
            // describe: its model is the fixed `"mock"` label, set here
            // rather than derived from an absent target.
            model: if backend.name() == "mock" {
                "mock".to_string()
            } else {
                crate::target::describe(opts.target.as_deref())
            },
            backend: backend.name().to_string(),
            trials: opts.trials_override.unwrap_or(0),
            judge_file: judge_file.path,
            judge_hash: judge_file.hash,
            judge_model,
            target: opts
                .target
                .as_deref()
                .map(crate::verdict::record_path)
                .unwrap_or_default(),
            budget_truncated,
            prompts_pack,
            prompts_hash,
            prompts_names,
            zs,
        },
        results,
    );
    let report_path = run_root.join("report.json");
    std::fs::write(&report_path, serde_json::to_vec_pretty(&report)?)
        .with_context(|| format!("write {}", report_path.display()))?;
    Ok(report)
}

/// One trial's graded result and the prompt its session recorded. The
/// readback rides beside `TrialResult` rather than on it: the prompt is a
/// scenario-level report field, reconciled across trials (`record_prompt`),
/// so hanging it off every trial would add a per-trial field the report has
/// no reader for.
struct GradedTrial {
    result: TrialResult,
    prompt: PromptReadback,
}

/// Run every trial of one scenario, then return results ordered by trial
/// index regardless of completion order. `jobs <= 1` keeps the exact old
/// path (sequential, printed as each trial finishes) since that's the
/// default; `jobs > 1` runs trial 0 solo first (to warm the provider's
/// prompt cache — see the comment at that step), then a bounded pool of
/// worker threads pulling the remaining trial indices off a shared counter —
/// trials are independent (their own run_dir, no shared state), so timing is
/// the only thing that changes, not grading.
fn run_trials_for_scenario(
    sc: &Scenario,
    backend: &dyn AgentBackend,
    judge: &dyn Judge,
    judge_file: &JudgeFileRef,
    opts: &RunOptions,
    trials: usize,
    run_root: &Path,
) -> Result<Vec<GradedTrial>> {
    let run_one = |trial: usize| -> Result<GradedTrial> {
        let run_dir = run_root.join(&sc.id).join(format!("trial-{trial}"));
        std::fs::create_dir_all(&run_dir)?;
        let grading = Grading {
            judge,
            judge_file,
            no_judge: opts.no_judge,
            judge_artifacts_dir: &run_dir,
        };
        let graded = run_trial(sc, backend, &grading, trial, &run_dir);
        // Persist per-trial for `explain`.
        std::fs::write(
            run_dir.join("trial.json"),
            serde_json::to_vec_pretty(&graded.result)?,
        )?;
        Ok(graded)
    };

    if opts.jobs <= 1 {
        let mut out = Vec::with_capacity(trials);
        for trial in 0..trials {
            let graded = run_one(trial)?;
            print_trial_line(&sc.id, &graded.result);
            out.push(graded);
        }
        return Ok(out);
    }

    // Warm the provider's prompt cache before fanning out: every trial of a
    // scenario opens with a byte-identical request (same tool definitions,
    // same system prompt, same task text), so racing all of them from a cold
    // start makes each one pay the cache-WRITE rate on that shared prefix —
    // on Anthropic, 1.25x the base input price, where a cache read is 0.1x.
    // Running trial 0 alone first turns the other trials' opening requests
    // into cache reads, at the wall-clock cost of one solo trial per
    // scenario. Grading is untouched: trials stay fully independent; this
    // only changes when they start.
    let first = run_one(0)?;
    print_trial_line(&sc.id, &first.result);
    if trials == 1 {
        return Ok(vec![first]);
    }

    let jobs = opts.jobs.min(trials - 1);
    let next = AtomicUsize::new(1);
    let outcome: Result<Vec<(usize, GradedTrial)>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..jobs)
            .map(|_| {
                scope.spawn(|| -> Result<Vec<(usize, GradedTrial)>> {
                    let mut mine = Vec::new();
                    loop {
                        let trial = next.fetch_add(1, Ordering::SeqCst);
                        if trial >= trials {
                            break;
                        }
                        mine.push((trial, run_one(trial)?));
                    }
                    Ok(mine)
                })
            })
            .collect();
        let mut all = Vec::with_capacity(trials - 1);
        for h in handles {
            match h.join() {
                Ok(Ok(mine)) => all.extend(mine),
                Ok(Err(e)) => return Err(e),
                Err(_) => anyhow::bail!("a trial-runner thread panicked"),
            }
        }
        Ok(all)
    });
    let mut all = outcome?;
    // Deterministic output regardless of which worker finished first.
    all.sort_by_key(|(trial, _)| *trial);
    let mut out = Vec::with_capacity(trials);
    out.push(first);
    for (_, graded) in all {
        print_trial_line(&sc.id, &graded.result);
        out.push(graded);
    }
    Ok(out)
}

fn run_trial(
    sc: &Scenario,
    backend: &dyn AgentBackend,
    grading: &Grading,
    trial: usize,
    run_dir: &Path,
) -> GradedTrial {
    // 1. Drive the agent. Backend errors = indeterminate (we never got a
    //    gradable transcript), not fail.
    let artifacts = match backend.run(sc, run_dir) {
        Ok(a) => a,
        Err(e) => {
            return ungraded(indeterminate(
                grading,
                trial,
                run_dir,
                format!("backend: {e:#}"),
            ));
        }
    };
    grade_trial(sc, grading, trial, run_dir, &artifacts)
}

/// A trial that never produced a readable transcript, so it observed nothing
/// about the prompt — there was no session to read one out of, which is not
/// the same as a session that recorded none (`PromptReadback`).
fn ungraded(result: TrialResult) -> GradedTrial {
    GradedTrial {
        result,
        prompt: PromptReadback::Unobserved,
    }
}

/// Re-grade an already-completed run_dir (produced by a prior `run` or a
/// captured `Mock` fixture) against `sc`'s *current* asserts/judge, without
/// driving the agent again. This is what backs `zseval regrade`: edit an
/// assert, re-score the same frozen evidence, see whether the new rule would
/// have passed — no API call, no new session. Persists the updated
/// `trial.json` next to the artifacts it graded, same as a normal run.
///
/// `judge_file` is the file naming the ruler `judge` was built from, recorded
/// on the returned trial. Passing it matters most here: `regrade --judge` is
/// the one command that swaps the ruler on an existing trial, so a regraded
/// `trial.json` that did not name its own judge would sit under a `report.json`
/// naming the previous one, with nothing on disk to tell them apart.
pub fn regrade(
    sc: &Scenario,
    judge: &dyn Judge,
    judge_file: Option<&Path>,
    no_judge: bool,
    trial: usize,
    run_dir: &Path,
) -> Result<TrialResult> {
    // Canonicalize, matching `ZsCli::run` — the zslog memory-drift check
    // compares an absolute path recorded by zerostack against one computed
    // here, so a caller passing a relative run_dir (e.g. `results/tag/...`
    // straight off the command line) must not make that comparison fail on
    // a path-form mismatch that has nothing to do with an actual drift.
    let run_dir = std::fs::canonicalize(run_dir).unwrap_or_else(|_| run_dir.to_path_buf());
    let run_dir = run_dir.as_path();
    let session_files = crate::backend::discover_session_files(&run_dir.join("data"));
    if session_files.is_empty() {
        anyhow::bail!(
            "no session file found under {}/data/sessions — is this a completed trial dir?",
            run_dir.display()
        );
    }
    let artifacts = crate::backend::RunArtifacts {
        session_files,
        turns: crate::backend::discover_turn_artifacts(run_dir),
        data_dir: run_dir.join("data"),
        config_dir: run_dir.join("config"),
        work_dir: run_dir.join("work"),
        wall_secs: 0.0,
    };
    // A regrade's judge call must not overwrite the previous judge's
    // request/response: those artifacts are the only evidence of what graded
    // this trial the first time, and destroying them is precisely the trace
    // this command exists to leave. Each regrade gets its own subdirectory,
    // stamped like a run folder (`util::compact_timestamp`) so successive
    // regrades cannot clobber each other either. Created only when a judge
    // will actually be called, so a `--no-judge` regrade leaves no empty dirs.
    let will_judge = sc.judge.is_some() && !no_judge;
    let judge_artifacts_dir = if will_judge {
        let dir = run_dir.join(format!("regrade-{}", crate::util::compact_timestamp()));
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        dir
    } else {
        run_dir.to_path_buf()
    };
    let judge_file = judge_file.map(JudgeFileRef::of).unwrap_or_default();
    let grading = Grading {
        judge,
        judge_file: &judge_file,
        no_judge,
        judge_artifacts_dir: &judge_artifacts_dir,
    };
    // The prompt readback is dropped here: it is a scenario-level fact
    // reconciled across a scenario's trials (`record_prompt`), and a regrade
    // re-scores exactly one trial dir, which is not a scenario.
    let tr = grade_trial(sc, &grading, trial, run_dir, &artifacts).result;
    std::fs::write(run_dir.join("trial.json"), serde_json::to_vec_pretty(&tr)?)
        .with_context(|| format!("write {}", run_dir.join("trial.json").display()))?;
    Ok(tr)
}

/// The grading half of a trial: transcript assembly, domain drift check,
/// deterministic asserts, budgets, judge. Shared by a fresh `run_trial` (agent
/// just ran) and `regrade` (agent artifacts already exist on disk) — the only
/// difference between the two is where `artifacts` comes from.
fn grade_trial(
    sc: &Scenario,
    grading: &Grading,
    trial: usize,
    run_dir: &Path,
    artifacts: &crate::backend::RunArtifacts,
) -> GradedTrial {
    let mut reasons = Vec::new();

    // 2. Assemble the gradable transcript from the trial's session JSON: the
    // one evidence channel, tool records and prompt readback included (see
    // Transcript::from_run and the transcript.rs module doc). Schema mismatch
    // = indeterminate.
    let transcript = match Transcript::from_run(artifacts) {
        Ok(t) => t,
        Err(e) => {
            return ungraded(indeterminate(
                grading,
                trial,
                run_dir,
                format!("transcript: {e:#}"),
            ));
        }
    };
    // Which prompt the session recorded is evidence about the environment,
    // not about how the trial graded, so it is carried out of every path
    // below — a trial that grades Indeterminate still observed it. The
    // transcript was assembled, so whatever it says is an observation:
    // `Absent` here means the session recorded no prompt, never that no
    // session was read (`PromptReadback`).
    let prompt = match transcript.prompt.clone() {
        Some(p) => PromptReadback::Recorded(p),
        None => PromptReadback::Absent,
    };

    let roots = artifacts.roots();

    // 2b. A scenario that seeds a domain's files is only gradable if our
    // snapshot of that subsystem's layout still matches reality — a stale
    // snapshot must never be silently blamed on the agent. Each domain
    // derives its expectations from `roots` the same way `expand` seeded
    // them, so there's one computation to keep in sync, not two.
    let zslogs: Vec<PathBuf> = artifacts.turns.iter().map(|t| t.zslog.clone()).collect();
    if let Err(reason) = crate::domains::verify(sc, &roots, &zslogs) {
        return GradedTrial {
            result: indeterminate(grading, trial, run_dir, format!("domain drift: {reason}")),
            prompt,
        };
    }

    // 3. Deterministic floor.
    let mut all_pass = true;
    let mut assert_results = Vec::new();
    for (line, a) in sc.expect.iter().zip(&sc.asserts) {
        let r = a.eval(&transcript, &roots);
        if !r.pass {
            all_pass = false;
            reasons.push(format!("assert failed: {} ({})", line, r.detail));
        }
        assert_results.push(r);
    }

    // 4. Budgets are graded negatives: the agent DID overspend.
    if let Some(cap) = sc.max_cost_usd {
        if transcript.cost_usd > cap {
            all_pass = false;
            reasons.push(format!(
                "budget exceeded: ${:.4} > ${cap:.4}",
                transcript.cost_usd
            ));
        }
    }
    if let Some(cap) = sc.max_total_tokens {
        if transcript.total_tokens() > cap {
            all_pass = false;
            reasons.push(format!(
                "token budget exceeded: {} > {cap}",
                transcript.total_tokens()
            ));
        }
    }

    // 5. Judge (only when the deterministic floor didn't already fail — a
    //    failed floor is a fail regardless of what the judge thinks).
    let mut judge_verdict: Option<JudgeVerdict> = None;
    // "Nothing graded this trial" until a judge call actually comes back: the
    // ruler that graded is a fact about what happened, not about what was
    // configured (see `TrialResult::judge_model` for the third state).
    let mut judge_model = Some(String::new());
    let mut judge_input_tokens = 0u64;
    let mut judge_output_tokens = 0u64;
    let mut judge_cost_usd = 0.0f64;
    let mut outcome = if all_pass { Final::Pass } else { Final::Fail };
    if outcome == Final::Pass {
        if let Some(rubric) = &sc.judge {
            if grading.no_judge {
                reasons.push("judge skipped (--no-judge)".into());
            } else if !grading.judge.available() {
                return GradedTrial {
                    result: TrialResult {
                        judge: None,
                        ..indeterminate(
                            grading,
                            trial,
                            run_dir,
                            format!(
                                "judge required but not available ({})",
                                grading.judge.unavailable_hint()
                            ),
                        )
                    },
                    prompt,
                };
            } else {
                match grading.judge.judge(
                    rubric,
                    &transcript.render_for_judge(20_000),
                    grading.judge_artifacts_dir,
                ) {
                    Ok(o) => {
                        judge_verdict = Some(o.verdict);
                        // A judge that answered without naming its model leaves
                        // the ruler unknown — it did grade, so calling that
                        // "nothing graded" would be false, and naming the
                        // configured model would report an intention as a fact.
                        judge_model = o.model.clone();
                        judge_input_tokens = o.input_tokens;
                        judge_output_tokens = o.output_tokens;
                        judge_cost_usd = o.cost_usd;
                        match o.verdict {
                            JudgeVerdict::Yes => {}
                            JudgeVerdict::No => {
                                outcome = Final::Fail;
                                reasons.push("judge: No".into());
                            }
                            JudgeVerdict::Unknown => {
                                outcome = Final::Indeterminate;
                                reasons.push(
                                    "judge: Unknown (rubric may need work — see vault note)".into(),
                                );
                            }
                        }
                    }
                    Err(e) => {
                        outcome = Final::Indeterminate;
                        reasons.push(format!("judge error: {e:#}"));
                    }
                }
            }
        }
    }

    let result = TrialResult {
        trial,
        outcome,
        reasons,
        asserts: assert_results,
        judge: judge_verdict,
        judge_file: grading.judge_file.path.clone(),
        judge_hash: grading.judge_file.hash.clone(),
        judge_model,
        input_tokens: transcript.input_tokens,
        output_tokens: transcript.output_tokens,
        judge_input_tokens,
        judge_output_tokens,
        // Judge spend is real API cost even though it grades the agent
        // rather than being incurred by it — must count against
        // --max-total-usd like everything else, or a judge-heavy suite could
        // blow the budget cap while looking like it stayed under it.
        cost_usd: transcript.cost_usd + judge_cost_usd,
        wall_secs: artifacts.wall_secs,
        tool_call_count: transcript.tool_calls.len(),
        // report-paths: recorded working-directory-relative, forward-slashed,
        // never absolute — see `verdict::record_path`.
        run_dir: crate::verdict::record_path(run_dir),
    };
    GradedTrial { result, prompt }
}

fn indeterminate(grading: &Grading, trial: usize, run_dir: &Path, reason: String) -> TrialResult {
    TrialResult {
        trial,
        outcome: Final::Indeterminate,
        reasons: vec![reason],
        asserts: Vec::new(),
        judge: None,
        // The configured ruler is recorded even here: it is a fact about how
        // this trial was set up, and it holds whether or not the judge was
        // ever reached.
        judge_file: grading.judge_file.path.clone(),
        judge_hash: grading.judge_file.hash.clone(),
        // Ungradable: no judge reached a verdict, so no ruler to name.
        judge_model: Some(String::new()),
        input_tokens: 0,
        output_tokens: 0,
        judge_input_tokens: 0,
        judge_output_tokens: 0,
        // Best-effort: a backend error (e.g. a timeout) can still leave
        // real spend behind in whatever session JSON completed turns wrote
        // before the failure — recovering it here means --max-total-usd
        // never undercounts a trial that grades Indeterminate.
        cost_usd: recover_cost_usd(run_dir),
        wall_secs: 0.0,
        tool_call_count: 0,
        // report-paths: same treatment as the success path above.
        run_dir: crate::verdict::record_path(run_dir),
    }
}

/// Sum `total_cost` out of every `*.json` under `run_dir/data/sessions/`,
/// tolerating missing directories and unparseable/partial files (a session
/// file can be mid-write when a timeout kills the agent) — this must never
/// itself fail or panic, since it runs on the error path.
fn recover_cost_usd(run_dir: &Path) -> f64 {
    crate::backend::discover_session_files(&run_dir.join("data"))
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .filter_map(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .filter_map(|v| v.get("total_cost").and_then(|c| c.as_f64()))
        .sum()
}

fn print_trial_line(id: &str, tr: &TrialResult) {
    let mark = match tr.outcome {
        Final::Pass => "PASS",
        Final::Fail => "FAIL",
        Final::Indeterminate => "????",
    };
    eprintln!(
        "[{mark}] {id} trial {} (${:.4}, {:.1}s){}",
        tr.trial,
        tr.cost_usd,
        tr.wall_secs,
        if tr.reasons.is_empty() {
            String::new()
        } else {
            format!(" — {}", tr.reasons.join("; "))
        }
    );
}

#[cfg(test)]
mod prompt_resolution_tests {
    use super::*;
    use crate::scenario::{FileSeed, Kind, Task};

    /// A minimal scenario carrying only the three things resolution reads: its
    /// `mode`, its `prompt` field and its `[[files]]` dests. Everything else
    /// is filler.
    fn scenario_in(mode: Mode, prompt: Option<&str>, dests: &[&str]) -> Scenario {
        Scenario {
            id: "t".into(),
            kind: Kind::Regression,
            prompt: prompt.map(String::from),
            trials: 1,
            mode,
            loop_cfg: None,
            task: Task::Single("do it".into()),
            expect: vec!["final_contains x".into()],
            judge: None,
            timeout_secs: 300,
            max_cost_usd: None,
            max_total_tokens: None,
            files: dests
                .iter()
                .map(|d| FileSeed {
                    src: PathBuf::from("_fixtures/x"),
                    dest: (*d).to_string(),
                })
                .collect(),
            seed: Default::default(),
            domains: vec![],
            dir: PathBuf::new(),
            asserts: vec![],
            content_hash: String::new(),
        }
    }

    /// The session-backed shape every scenario has unless it says `mode =
    /// "loop"`: a `-p` run, so there is a session file to read a prompt back
    /// out of.
    fn scenario(prompt: Option<&str>, dests: &[&str]) -> Scenario {
        scenario_in(Mode::Print, prompt, dests)
    }

    /// `mode = "loop"`: upstream's `run_headless_loop` writes no session, so
    /// this is the one shape with nothing to read back (design D3).
    fn loop_scenario(prompt: Option<&str>, dests: &[&str]) -> Scenario {
        scenario_in(Mode::Loop, prompt, dests)
    }

    /// One trial's session readback, in upstream's two-value vocabulary
    /// (`built_in` / `user_file`) — never this crate's four-value
    /// `PromptSource`.
    fn readback(name: &str, source: &str) -> PromptReadback {
        PromptReadback::Recorded(RecordedPrompt {
            name: name.into(),
            source: source.into(),
        })
    }

    /// A trial whose session was read and recorded no `prompt` field: an
    /// observed absence, which is what the PR #228 rebuild message is for.
    fn no_prompt() -> PromptReadback {
        PromptReadback::Absent
    }

    /// A trial that never produced a readable session (a backend error, a
    /// schema mismatch), so it observed nothing at all.
    fn no_session() -> PromptReadback {
        PromptReadback::Unobserved
    }

    /// A validated pack over the given `<name>.md` files, in a fresh temp dir.
    fn pack(test: &str, names: &[&str]) -> PromptPack {
        let dir =
            std::env::temp_dir().join(format!("zseval-resolve-{test}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        for n in names {
            std::fs::write(dir.join(format!("{n}.md")), format!("{n} body")).unwrap();
        }
        PromptPack::load(&dir).unwrap()
    }

    // 6.1: the four-value derivation order — now the cross-check for a
    // session-backed scenario, and still the recorded value for a loop one.

    #[test]
    fn declared_prompt_the_pack_provides_derives_pack() {
        let sc = scenario(Some("code"), &[]);
        let p = pack("pack-provides", &["code", "review"]);
        assert_eq!(
            derive_prompt(&sc, Some(&p), None),
            ("code".into(), PromptSource::Pack)
        );
    }

    #[test]
    fn declared_prompt_the_pack_lacks_derives_stock() {
        let sc = scenario(Some("ask"), &[]);
        let p = pack("pack-lacks", &["code"]);
        assert_eq!(
            derive_prompt(&sc, Some(&p), None),
            ("ask".into(), PromptSource::Stock)
        );
    }

    #[test]
    fn a_scenario_seeding_the_same_name_derives_scenario() {
        // The pack also provides `code`, but the scenario's own seed lands last
        // and wins.
        let sc = scenario(Some("code"), &["work:.zerostack/prompts/code.md"]);
        let p = pack("scenario-seed", &["code"]);
        assert_eq!(
            derive_prompt(&sc, Some(&p), None),
            ("code".into(), PromptSource::Scenario)
        );
    }

    // 6.2: the default-prompt derivation.

    #[test]
    fn no_prompt_and_no_config_default_derives_code() {
        // No pack provides `code`, so the derived name lands as `stock`.
        let sc = scenario(None, &[]);
        let p = pack("derive-code", &["review"]);
        assert_eq!(
            derive_prompt(&sc, Some(&p), None),
            ("code".into(), PromptSource::Stock)
        );
    }

    #[test]
    fn no_prompt_with_a_config_default_derives_that_name() {
        let sc = scenario(None, &[]);
        let p = pack("derive-configured", &["code"]);
        assert_eq!(
            derive_prompt(&sc, Some(&p), Some("review")),
            ("review".into(), PromptSource::Stock)
        );
    }

    // 6.3: the config-seeding guard abandons derivation — for a loop
    // scenario, whose derivation is still the recorded value. A session-backed
    // scenario has a readback that is the last word regardless of who wrote
    // the config, so the guard is gone there (design D3).

    #[test]
    fn a_loop_scenario_seeding_the_config_directory_derives_unknown() {
        let sc = loop_scenario(None, &["config:config.toml"]);
        assert_eq!(
            derive_prompt(&sc, None, Some("review")),
            (String::new(), PromptSource::Unknown)
        );
    }

    #[test]
    fn a_loop_scenario_seeding_a_non_config_file_under_config_still_derives() {
        // A `config:` seed that is *not* the config.toml (an agent doc, a
        // memory file) leaves `default_prompt` untouched, so derivation must
        // proceed rather than blanking the prompt to Unknown.
        let sc = loop_scenario(None, &["config:agent/instructions.md"]);
        assert_eq!(
            derive_prompt(&sc, None, Some("review")),
            ("review".into(), PromptSource::Stock)
        );
    }

    #[test]
    fn a_loop_scenario_seeding_work_zerostack_config_derives_unknown() {
        let sc = loop_scenario(None, &["work:.zerostack/config.toml"]);
        assert_eq!(
            derive_prompt(&sc, None, Some("review")),
            (String::new(), PromptSource::Unknown)
        );
    }

    #[test]
    fn a_declared_prompt_survives_a_config_seed() {
        // The guard only abandons *derivation*; an explicitly declared prompt
        // needs no config default, so a config seed does not blind it.
        let sc = loop_scenario(Some("ask"), &["config:config.toml"]);
        assert_eq!(
            derive_prompt(&sc, None, None),
            ("ask".into(), PromptSource::Stock)
        );
    }

    #[test]
    fn a_session_backed_scenario_seeding_the_config_still_derives() {
        // The deleted branch (design D3): the harness's seeded config is no
        // longer the last word, but the readback is, so derivation has no
        // reason to abandon itself here — it is only the cross-check now.
        let sc = scenario(None, &["config:config.toml"]);
        assert_eq!(
            derive_prompt(&sc, None, Some("review")),
            ("review".into(), PromptSource::Stock)
        );
    }

    // D3's four mapping arms: what the readback records, per scenario.

    /// prompts-pack-identity mapping 1, and "A scenario naming a prompt the
    /// pack does not provide".
    #[test]
    fn a_built_in_readback_records_stock() {
        let sc = scenario(Some("ask"), &[]);
        let p = pack("map-built-in", &["code"]);
        let got = record_prompt(&sc, Some(&p), None, &[readback("ask", "built_in")]);
        assert_eq!(
            (got.name.as_str(), got.source),
            ("ask", PromptSource::Stock)
        );
        assert!(got.warnings.is_empty(), "{:?}", got.warnings);
    }

    /// prompts-pack-identity mapping 2, "A scenario that seeds its own
    /// prompt": the scenario's placement lands last, so it is the content
    /// that loaded.
    #[test]
    fn a_user_file_readback_the_scenario_seeded_records_scenario() {
        let sc = scenario(Some("code"), &["work:.zerostack/prompts/code.md"]);
        let p = pack("map-scenario", &["code"]);
        let got = record_prompt(&sc, Some(&p), None, &[readback("code", "user_file")]);
        assert_eq!(
            (got.name.as_str(), got.source),
            ("code", PromptSource::Scenario)
        );
        assert!(got.warnings.is_empty(), "{:?}", got.warnings);
    }

    /// prompts-pack-identity mapping 3, "A scenario naming a prompt the pack
    /// provides".
    #[test]
    fn a_user_file_readback_the_pack_provides_records_pack() {
        let sc = scenario(Some("code"), &[]);
        let p = pack("map-pack", &["code"]);
        let got = record_prompt(&sc, Some(&p), None, &[readback("code", "user_file")]);
        assert_eq!(
            (got.name.as_str(), got.source),
            ("code", PromptSource::Pack)
        );
        assert!(got.warnings.is_empty(), "{:?}", got.warnings);
    }

    /// prompts-pack-identity mapping 4: a user file the harness never planted
    /// means the trial environment is not what the harness thinks it is.
    #[test]
    fn a_user_file_readback_nobody_planted_records_unknown_and_warns() {
        let sc = scenario(Some("code"), &[]);
        let p = pack("map-unplanted", &["code"]);
        let got = record_prompt(&sc, Some(&p), None, &[readback("rogue", "user_file")]);
        assert_eq!(
            (got.name.as_str(), got.source),
            ("rogue", PromptSource::Unknown)
        );
        assert!(
            got.warnings.iter().any(|w| w.contains("rogue")),
            "the warning must name the prompt nobody planted: {:?}",
            got.warnings
        );
    }

    /// prompts-pack-identity, "Derivation disagreement warns but does not
    /// override": upstream classifies a pack prompt whose bytes equal the
    /// built-in as `built_in` (its `source_of` compares content), so the
    /// derivation's `pack` and the readback's `stock` disagree benignly.
    #[test]
    fn a_pack_prompt_identical_to_the_built_in_records_stock_and_warns() {
        let sc = scenario(Some("code"), &[]);
        let p = pack("crosscheck-identical", &["code"]);
        let got = record_prompt(&sc, Some(&p), None, &[readback("code", "built_in")]);
        assert_eq!(
            (got.name.as_str(), got.source),
            ("code", PromptSource::Stock),
            "the readback wins over the derivation"
        );
        let warned = got.warnings.join(" | ");
        assert!(
            warned.contains("pack") && warned.contains("stock"),
            "the warning must name both the derived and the read-back value: {warned}"
        );
    }

    // D4: the two trials-to-scenario reconciliations, which carry different
    // messages because they have different fixes.

    #[test]
    fn trials_that_agree_record_what_they_agreed_on() {
        let sc = scenario(Some("code"), &[]);
        let p = pack("reconcile-agree", &["code"]);
        let got = record_prompt(
            &sc,
            Some(&p),
            None,
            &[readback("code", "user_file"), readback("code", "user_file")],
        );
        assert_eq!(
            (got.name.as_str(), got.source),
            ("code", PromptSource::Pack)
        );
        assert!(got.warnings.is_empty(), "{:?}", got.warnings);
    }

    #[test]
    fn trials_that_disagree_record_unknown_and_warn() {
        let sc = scenario(Some("code"), &[]);
        let p = pack("reconcile-split", &["code"]);
        let got = record_prompt(
            &sc,
            Some(&p),
            None,
            &[readback("code", "user_file"), readback("code", "built_in")],
        );
        assert_eq!((got.name.as_str(), got.source), ("", PromptSource::Unknown));
        let warned = got.warnings.join(" | ");
        assert!(
            warned.contains("disagree"),
            "identically-seeded trials disagreeing is itself the finding: {warned}"
        );
    }

    #[test]
    fn one_trial_missing_the_readback_is_a_disagreement_not_an_absence() {
        let sc = scenario(Some("code"), &[]);
        let p = pack("reconcile-mixed", &["code"]);
        let got = record_prompt(
            &sc,
            Some(&p),
            None,
            &[readback("code", "user_file"), no_prompt()],
        );
        assert_eq!((got.name.as_str(), got.source), ("", PromptSource::Unknown));
        let warned = got.warnings.join(" | ");
        assert!(
            warned.contains("disagree"),
            "one trial recording a prompt and another recording none is a \
             disagreement, not a stale binary: {warned}"
        );
        assert!(
            !warned.contains("ZS_BIN"),
            "the rebuild is not the fix when a trial did record a prompt: {warned}"
        );
    }

    /// prompts-pack-identity mapping 5, "A session without a recorded prompt
    /// records unknown, loudly" — the warning names the rebuild because that
    /// is the actual fix.
    #[test]
    fn every_session_lacking_the_prompt_records_unknown_and_names_the_rebuild() {
        let sc = scenario(Some("code"), &[]);
        let p = pack("reconcile-absent", &["code"]);
        let got = record_prompt(&sc, Some(&p), None, &[no_prompt(), no_prompt()]);
        assert_eq!((got.name.as_str(), got.source), ("", PromptSource::Unknown));
        let warned = got.warnings.join(" | ");
        assert!(
            warned.contains("ZS_BIN"),
            "the warning must name the ZS_BIN rebuild: {warned}"
        );
    }

    // A trial that produced nothing observed nothing, so it is not a party to
    // the reconciliation at all — an absence it never observed must neither
    // outvote the trials that did read a prompt back, nor be blamed on a
    // binary too old to record one.

    #[test]
    fn a_trial_that_produced_no_session_does_not_disagree_with_the_trials_that_read_one() {
        let sc = scenario(Some("code"), &[]);
        let p = pack("reconcile-unobserved-partial", &["code"]);
        let got = record_prompt(
            &sc,
            Some(&p),
            None,
            &[
                readback("code", "user_file"),
                no_session(),
                readback("code", "user_file"),
            ],
        );
        assert_eq!(
            (got.name.as_str(), got.source),
            ("code", PromptSource::Pack),
            "two trials read the same prompt back; the third read nothing at all"
        );
        assert!(
            got.warnings.is_empty(),
            "a trial that observed nothing is not a disagreement: {:?}",
            got.warnings
        );
    }

    #[test]
    fn no_trial_producing_a_session_records_unknown_and_blames_the_trials_not_the_binary() {
        let sc = scenario(Some("code"), &[]);
        let p = pack("reconcile-unobserved-all", &["code"]);
        let got = record_prompt(&sc, Some(&p), None, &[no_session(), no_session()]);
        assert_eq!((got.name.as_str(), got.source), ("", PromptSource::Unknown));
        let warned = got.warnings.join(" | ");
        assert!(
            warned.contains("no trial produced a readable session"),
            "the warning must say nothing could be read back because nothing ran: {warned}"
        );
        assert!(
            !warned.contains("#228") && !warned.contains("rebuild") && !warned.contains("ZS_BIN"),
            "the binary is not the finding when no trial got far enough to read it: {warned}"
        );
    }

    // A loop scenario has no session to read back, so it keeps the whole
    // derivation — including the config-seeding branch — and its silence is
    // not a missing-record alarm (design D3, Non-Goals).

    #[test]
    fn a_loop_scenario_records_its_derivation_not_the_absent_readback() {
        let sc = loop_scenario(None, &[]);
        let p = pack("loop-derive", &["code"]);
        let got = record_prompt(&sc, Some(&p), Some("review"), &[no_session()]);
        assert_eq!(
            (got.name.as_str(), got.source),
            ("review", PromptSource::Stock)
        );
        assert!(
            got.warnings.is_empty(),
            "a loop run writes no session, so an absent readback is expected, not a \
             stale binary: {:?}",
            got.warnings
        );
    }

    #[test]
    fn a_config_seeding_loop_scenario_still_records_unknown() {
        let sc = loop_scenario(None, &["work:.zerostack/config.toml"]);
        let got = record_prompt(&sc, None, Some("review"), &[no_session()]);
        assert_eq!((got.name.as_str(), got.source), ("", PromptSource::Unknown));
        assert!(got.warnings.is_empty(), "{:?}", got.warnings);
    }

    #[test]
    fn a_config_seeding_session_backed_scenario_records_its_readback() {
        // The branch this change deletes: the readback is the last word on
        // which prompt loaded, whoever wrote the config.
        let sc = scenario(None, &["work:.zerostack/config.toml"]);
        let got = record_prompt(&sc, None, Some("review"), &[readback("review", "built_in")]);
        assert_eq!(
            (got.name.as_str(), got.source),
            ("review", PromptSource::Stock)
        );
        assert!(got.warnings.is_empty(), "{:?}", got.warnings);
    }
}
