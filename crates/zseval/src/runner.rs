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
    /// one `zseval run` invocation. `false` (the default single-target shape)
    /// keeps today's flat `results/<tag>/` layout; `true` nests this target's
    /// report and trial dirs one level deeper, under `results/<tag>/<stem>/`
    /// (`stem` = `target`'s filename without extension — see `target::stem`),
    /// so N targets sharing one `--tag` don't collide on `sc.id`. Requires
    /// `target` to be `Some`: the stem has nothing to derive from otherwise.
    pub multi_target: bool,
    /// The files and trees this run is *defined* by, watched for change while
    /// it runs (`crate::integrity`): the scenario tree, every target config,
    /// the judge file. A trial that escapes its work dir and edits one of
    /// these has silently rewritten the experiment, so the run stops, the
    /// scenario whose trials could have done it grades Indeterminate, and
    /// `run_suite` returns an error *after* writing the report of what ran.
    ///
    /// Empty (the default in the harness's own tests and every non-`run`
    /// caller) means no checking: a `regrade` drives no agent, so there is
    /// nothing that could write, and hashing a scenario tree per scenario for
    /// nothing is pure cost.
    pub integrity_roots: Vec<PathBuf>,
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
/// than re-deriving the same formula at each site. Requires `target` to be
/// `Some` when `multi_target`: the stem has nothing to derive from otherwise.
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

/// Has this run's pack replaced the very built-in a `prompt_recorded <name>
/// built_in` assert exists to watch? Returns the line to say so, `None` when
/// nothing collides.
///
/// The pin observes the compiled-in default, and `ZsCli::run` seeds the pack
/// into every trial of every scenario — including the ones that declare no
/// prompt of their own and so resolve `code` through the fallback. A pack
/// providing that same name therefore replaces what the pin watches, and
/// grading it would report the harness's own seeding as a product regression.
/// That is the same defect that made the tool-side pin stop naming a tool, so
/// the scenario is skipped instead: a regression pin is only evidence while it
/// watches a channel the harness itself did not supply.
///
/// Narrow on purpose: only a `built_in` pin, only against a name the pack
/// actually supplies, and only when this scenario *resolves* that same name —
/// `derive_prompt` answers that last part, and the pack has to be the layer
/// supplying it (a scenario seeding its own `<name>.md` shadows the built-in
/// itself, which is the scenario's own doing, not the run's pack). A scenario
/// pinning a name it never loads (declaring `prompt = "ask"` while asserting
/// `prompt_recorded code built_in`) is an authoring error the pack does not
/// excuse: it grades, and it fails honestly. The `user_file` mirror is
/// likewise left to grade — a pack-less run failing it is a run invoked
/// wrongly, and that failure is the honest signal.
///
/// Returned rather than printed for the same reason `PromptRecord`'s warnings
/// are: the decision stays a pure function the tests can read, and `run_suite`
/// is the one place that puts it on stderr.
fn shadowed_built_in_pin(
    sc: &Scenario,
    pack: Option<&PromptPack>,
    config_default_prompt: Option<&str>,
) -> Option<String> {
    let pack = pack?;
    let (resolved, resolved_source) = derive_prompt(sc, Some(pack), config_default_prompt);
    if resolved_source != PromptSource::Pack {
        return None;
    }
    let shadowed = sc.asserts.iter().find_map(|a| match a {
        crate::asserts::Assert::PromptRecorded { name, source }
            if source == "built_in" && *name == resolved =>
        {
            Some(name)
        }
        _ => None,
    })?;
    Some(format!(
        "scenario {}: the prompts pack provides '{shadowed}', the built-in its \
         `prompt_recorded {shadowed} built_in` assert exists to watch, so there is nothing \
         left for the pin to observe — no trial was run and the scenario is ungradable, not \
         failed",
        sc.id
    ))
}

/// What a run has to say at the end about a pack seeded into every trial but
/// resolved by no scenario: it is inert, the report's headline number is
/// entirely the built-ins' score rather than the pack's, and nothing about the
/// report itself says so — `prompts_pack` is populated either way. Decided
/// once, over every scenario the run actually produced, rather than per
/// scenario: a partial hit (some `pack`, some not) is real signal and stays
/// visible only in each scenario's own `prompt_source`, which records the
/// prompt that scenario actually loaded rather than merely which names the
/// pack and the scenarios have in common.
///
/// Read off what the scenarios *observed*, which is exactly the set with a
/// recorded `prompt_source`: a scenario skipped for pinning a built-in this
/// pack provides ran no trial, and one whose trials never produced a readable
/// session read nothing back, so both keep `Unknown` and neither is evidence
/// either way. When that set is empty the run genuinely observed nothing about
/// its pack and says so by saying nothing. When it is not, a scenario that ran
/// and resolved a built-in *is* the evidence, and a different scenario's skip
/// must not mute it — silencing the whole run on any skip would hide the one
/// case this warning exists for, since the shipped `prompt_recorded code
/// built_in` pin is skipped by every pack providing `code`. The skipped
/// scenarios are named in the same breath instead, so a reader who just saw
/// them skipped knows they were counted and not quietly ignored.
///
/// `skipped` carries the ids rather than a bare count so the line can name
/// them; it never changes whether the warning fires, only how it accounts for
/// itself.
fn unloaded_pack_warning(
    pack: &PromptPack,
    results: &[ScenarioResult],
    skipped: &[String],
) -> Option<String> {
    let observed: Vec<PromptSource> = results
        .iter()
        .map(|sr| sr.prompt_source)
        .filter(|s| *s != PromptSource::Unknown)
        .collect();
    if observed.is_empty() || observed.contains(&PromptSource::Pack) {
        return None;
    }
    let skipped_note = match skipped {
        [] => String::new(),
        [one] => format!(
            "; scenario {one} was skipped for pinning a built-in this pack provides, so it ran \
             no trial and resolved nothing either way"
        ),
        many => format!(
            "; scenarios {} were skipped for pinning a built-in this pack provides, so they ran \
             no trials and resolved nothing either way",
            many.join(", ")
        ),
    };
    Some(format!(
        "prompts pack {} was seeded but never loaded: no scenario that ran resolved a prompt \
         from it, so this report reflects zerostack's built-in prompts, not the pack{skipped_note}",
        pack.dir().display()
    ))
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
/// session): here it is the cross-check that warns when the two disagree. It
/// is still the recorded value for a `mode = "loop"` scenario, which
/// upstream's `run_headless_loop` leaves no session file for.
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

/// What one trial's session had to say about which prompt it loaded. Three
/// states, not two: a trial that produced no readable session observed
/// nothing, which is not the same evidence as a session that was read and
/// recorded no prompt. Conflating them makes a trial that timed out or failed
/// its schema check look like a binary too old to record prompts, and makes it
/// outvote the trials that did read one back.
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

/// What one scenario's trials agreed their sessions recorded.
/// `prompt_name`/`prompt_source` are scenario-level facts, so the trials have
/// to produce one answer between them; the ways they can fail to are kept
/// apart here because they have different fixes.
enum Reconciled {
    /// Every trial that produced a session read back the same prompt — the
    /// expected case, since the trials of one scenario are identically seeded
    /// and prompt resolution is deterministic.
    Agreed(RecordedPrompt),
    /// Every trial that produced a session read back no prompt at all:
    /// whatever wrote those sessions predates upstream's prompt recording (PR
    /// #228), and the fix is a newer one — a rebuilt `ZS_BIN` for a live run,
    /// freshly captured evidence for a fixture or a regrade.
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
/// session readback.
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
                 prompt is unknown — whatever produced those sessions predates prompt \
                 recording (zerostack PR #228): rebuild a live ZS_BIN from a mainline that \
                 carries it, or re-capture the mock fixture or run dir being graded from a \
                 binary that does",
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
    // across two different roots.
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

    // The run's own inputs as they were before the first trial could touch
    // anything. Empty roots snapshot to an empty map, which is how "no
    // checking" costs nothing rather than needing a branch of its own.
    let integrity_baseline = crate::integrity::Snapshot::of(&opts.integrity_roots)?;
    let mut integrity_drift: Vec<crate::integrity::Drift> = Vec::new();

    let mut budget_truncated = false;
    // Which scenarios were skipped because the pack shadowed the built-in
    // their pin watches: they observed nothing, so the end-of-run pack check
    // names them rather than counting them either way
    // (`unloaded_pack_warning`).
    let mut shadowed_skips: Vec<String> = Vec::new();
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
        // Checked here — after load, before this scenario's first trial — so
        // a pin the pack has already replaced costs nothing at all. The
        // scenario is still recorded, with no trials: an empty `trials` is
        // exactly the report's existing shape for "ungradable"
        // (`ScenarioResult::is_gradable`), and dropping the scenario would
        // instead make it vanish from a report that declared it.
        if let Some(warning) = shadowed_built_in_pin(sc, pack, config_default_prompt.as_deref()) {
            eprintln!("{warning}");
            shadowed_skips.push(sc.id.clone());
            // Nothing observed which prompt loaded, because nothing ran:
            // `prompt_name`/`prompt_source` stay at their empty/`Unknown`
            // defaults rather than recording a derivation as an observation.
            results.push(ScenarioResult::from_trials_with_hash(
                sc.id.clone(),
                sc.kind,
                sc.content_hash.clone(),
                Vec::new(),
            ));
            continue;
        }
        let trials = opts.trials_override.unwrap_or(sc.trials).max(1);
        let graded =
            run_trials_for_scenario(sc, backend, judge, &judge_file, opts, trials, &run_root)?;
        let (mut trial_results, readbacks): (Vec<TrialResult>, Vec<PromptReadback>) =
            graded.into_iter().map(|g| (g.result, g.prompt)).unzip();
        for tr in &trial_results {
            spent += tr.cost_usd;
        }

        // Re-check the run's own inputs now this scenario's trials are done.
        // The previous scenario's check was clean, so the window a drift is
        // attributable to is exactly this scenario: its trials are the only
        // thing that ran since. Checked here, before the results are folded
        // into a `ScenarioResult`, so the withdrawal reaches the trials
        // themselves rather than only the rates derived from them.
        integrity_drift =
            integrity_baseline.diff(&crate::integrity::Snapshot::of(&opts.integrity_roots)?);
        if !integrity_drift.is_empty() {
            report_input_drift(sc, &integrity_drift);
            let reason = format!(
                "input-integrity drift while this scenario ran: {}",
                crate::integrity::summarize(&integrity_drift, 10)
            );
            for tr in &mut trial_results {
                withdraw_verdict(tr, &reason);
                // The persisted copy is what `explain` and every later reader
                // sees, so a withdrawal that lived only in memory would leave
                // `trial.json` still claiming the verdict this run just took
                // back.
                let path = trial_dir(&run_root, &sc.id, tr.trial).join("trial.json");
                std::fs::write(&path, serde_json::to_vec_pretty(tr)?)
                    .with_context(|| format!("rewrite {}", path.display()))?;
            }
        }

        // Record the scenario's kind verbatim on the result, so `matrix` and
        // the site renderer can group by kind from report JSON alone, without
        // re-reading the scenario files.
        let mut sr = ScenarioResult::from_trials_with_hash(
            sc.id.clone(),
            sc.kind,
            sc.content_hash.clone(),
            trial_results,
        );
        // Which prompt this scenario loaded is read back off its trials'
        // sessions and reconciled here, because it is one fact per scenario,
        // not per trial.
        let prompt = record_prompt(sc, pack, config_default_prompt.as_deref(), &readbacks);
        for warning in &prompt.warnings {
            eprintln!("{warning}");
        }
        sr.prompt_name = prompt.name;
        sr.prompt_source = prompt.source;
        results.push(sr);

        // Launch no further scenario: the inputs the next one would be graded
        // against are the ones just shown to be moving. Scenario-granular for
        // the same reason the cost cap is — a scenario runs its full trial
        // count or not at all — and the report below is still assembled and
        // written for everything that did run.
        if !integrity_drift.is_empty() {
            break;
        }
    }

    if let Some(warning) = pack.and_then(|p| unloaded_pack_warning(p, &results, &shadowed_skips)) {
        eprintln!("{warning}");
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
            // `--target` is mandatory for `zs` and rejected for `mock`, so a
            // mock run never has a target to describe: its model is the fixed
            // `"mock"` label, set here rather than derived from an absent
            // target.
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

    // Only after the report is on disk: a run whose inputs moved still owes
    // the evidence of what it did run. Failing here is what makes the process
    // exit non-zero — the trials of the drift window are Indeterminate, but
    // earlier scenarios may all have passed, and a run that quietly returned
    // that as a clean report would be the failure this check exists to
    // prevent. It is a harness error, not a graded failure: nothing was
    // learned about the agent, the experiment was edited underneath it.
    if !integrity_drift.is_empty() {
        anyhow::bail!(
            "input-integrity drift: this run's own inputs changed while it ran ({}). The \
             affected scenario's trials are recorded indeterminate and no further scenario \
             was launched; restore the listed paths (`git status` in the harness checkout) \
             before trusting anything measured here. Report for what ran: {}",
            crate::integrity::summarize(&integrity_drift, 10),
            report_path.display()
        );
    }
    Ok(report)
}

/// Put a drift on the console in full: every path, with what happened to it.
/// Uncapped (unlike the reason recorded on each trial) because this prints
/// once, and half a list is not enough to go and check what an escaped agent
/// touched.
fn report_input_drift(sc: &Scenario, drift: &[crate::integrity::Drift]) {
    eprintln!(
        "input-integrity drift after scenario {}: files this run is defined by changed while \
         it ran, so its trials were graded against inputs nobody declared:",
        sc.id
    );
    for d in drift {
        eprintln!("  {d}");
    }
    eprintln!(
        "every trial of {} is marked indeterminate and the run stops here",
        sc.id
    );
}

/// Take back a trial's verdict after the fact: the evidence it graded is no
/// longer trustworthy, so the outcome becomes Indeterminate and `reason` joins
/// whatever the grading already recorded.
///
/// Unlike `indeterminate` — which builds a trial that never produced evidence
/// at all — everything else is kept: the asserts really did evaluate the way
/// they are recorded, the spend really did happen and must still count against
/// the budget, and a reader diagnosing the drift needs both to see what the
/// trial looked like before its inputs moved.
fn withdraw_verdict(tr: &mut TrialResult, reason: &str) {
    tr.outcome = Final::Indeterminate;
    tr.reasons.push(reason.to_string());
}

/// Where one trial's artifacts live: `<run_root>/<scenario id>/trial-N`,
/// holding its logs, its isolated roots, and the `trial.json` `explain` reads.
/// Derived in one place so the run that writes a trial and the input-drift
/// check that rewrites its `trial.json` can never disagree about which
/// directory that is.
fn trial_dir(run_root: &Path, scenario_id: &str, trial: usize) -> PathBuf {
    run_root.join(scenario_id).join(format!("trial-{trial}"))
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
        let run_dir = trial_dir(run_root, &sc.id, trial);
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

/// The grading decision: deterministic asserts AND the judge fold into one
/// `Final` here — every Pass/Fail/Indeterminate a report carries is decided
/// in this function.
///
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
        // Recorded working-directory-relative, forward-slashed and never
        // absolute — see `verdict::record_path`.
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
        // Same `verdict::record_path` treatment as the success path above.
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
            security_mode: Default::default(),
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
    /// this is the one shape with nothing to read back.
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

    // The four-value derivation order — now the cross-check for a
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

    // The default-prompt derivation.

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

    // The config-seeding guard abandons derivation — for a loop scenario,
    // whose derivation is still the recorded value. A session-backed scenario
    // has a readback that is the last word regardless of who wrote the config,
    // so the guard does not apply there.

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
        // The harness's seeded config is no longer the last word here, but the
        // readback is, so derivation has no reason to abandon itself — it is
        // only the cross-check.
        let sc = scenario(None, &["config:config.toml"]);
        assert_eq!(
            derive_prompt(&sc, None, Some("review")),
            ("review".into(), PromptSource::Stock)
        );
    }

    // The four mapping arms: what the readback records, per scenario.

    /// A scenario naming a prompt the pack does not provide loads the built-in
    /// and records `stock`.
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

    /// A scenario that seeds its own prompt records `scenario`, even when the
    /// pack supplies the same name: the scenario's placement lands last, so
    /// its content is what loaded.
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

    /// A scenario naming a prompt the pack provides, and seeding none of its
    /// own, records `pack`: the pack is the only layer that could have put the
    /// user file the session read there.
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

    /// A user file neither the scenario nor the pack planted records `unknown`
    /// and warns: the trial environment is not what the harness thinks it is.
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

    /// A derivation that disagrees with the readback warns but never overrides
    /// it: upstream classifies a pack prompt whose bytes equal the built-in as
    /// `built_in` (its `source_of` compares content), so the derivation's
    /// `pack` and the readback's `stock` disagree benignly.
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

    // The two trials-to-scenario reconciliations, which carry different
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

    /// A session without a recorded prompt records `unknown`, loudly — and the
    /// warning names the rebuild, because that is the actual fix.
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

    /// The same warning also serves `--backend mock=<fixture>` and a
    /// `regrade` over a captured run dir, neither of which has a live `ZS_BIN`
    /// to rebuild. It has to name the artifact whose `prompt` field is missing
    /// and offer the rebuild as one cause, without going vague on the live run
    /// it was written for.
    #[test]
    fn the_absent_readback_warning_names_the_sessions_not_only_a_stale_zs_bin() {
        let sc = scenario(Some("code"), &[]);
        let got = record_prompt(&sc, None, None, &[no_prompt()]);
        let warned = got.warnings.join(" | ");
        assert!(
            warned.contains("session"),
            "the artifact missing the field is the session: {warned}"
        );
        assert!(
            warned.contains("ZS_BIN"),
            "a live run against an old binary must still be told plainly what to do: {warned}"
        );
        assert!(
            warned.contains("fixture") || warned.contains("run dir"),
            "a mock fixture or a regraded run dir has no ZS_BIN to rebuild, so the rebuild \
             cannot be the only instruction: {warned}"
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
    // not a missing-record alarm.

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
        // No config-seeding guard applies here: the readback is the last word
        // on which prompt loaded, whoever wrote the config.
        let sc = scenario(None, &["work:.zerostack/config.toml"]);
        let got = record_prompt(&sc, None, Some("review"), &[readback("review", "built_in")]);
        assert_eq!(
            (got.name.as_str(), got.source),
            ("review", PromptSource::Stock)
        );
        assert!(got.warnings.is_empty(), "{:?}", got.warnings);
    }
}

/// What a run does with a `prompt_recorded <name> built_in` pin when its own
/// `--prompts` pack provides that same `<name>`. Driven through `run_suite`
/// rather than a helper, because the claim is about what the run *spends* —
/// the collision has to be caught before a trial runs, and only the whole
/// loop can show that.
#[cfg(test)]
mod shadowed_pin_tests {
    use super::*;
    use crate::asserts::Assert;
    use crate::backend::RunArtifacts;
    use crate::judge::{Judge, JudgeOutcome};
    use crate::scenario::{FileSeed, Kind, Task};
    use crate::verdict::{Final, Report, ZsIdentity};

    /// The shipped prompt pin's shape: no `prompt` of its own (so it resolves
    /// the target's default, else `code`) and one `prompt_recorded` assert.
    fn pinned(id: &str, expect: &str) -> Scenario {
        Scenario {
            id: id.into(),
            kind: Kind::Regression,
            prompt: None,
            trials: 1,
            mode: Mode::Print,
            loop_cfg: None,
            security_mode: Default::default(),
            task: Task::Single("In one sentence, what is the capital of France?".into()),
            expect: vec![expect.to_string()],
            judge: None,
            timeout_secs: 300,
            max_cost_usd: None,
            max_total_tokens: None,
            files: vec![],
            seed: Default::default(),
            domains: vec![],
            dir: PathBuf::new(),
            asserts: vec![Assert::parse(expect).unwrap()],
            content_hash: String::new(),
        }
    }

    /// The shipped prompt-channel pin, `scenarios/session/prompt-recorded-
    /// stock/`: the scenario this rule exists for.
    fn stock_pin() -> Scenario {
        pinned(
            "session-prompt-recorded-stock",
            "prompt_recorded code built_in",
        )
    }

    /// A validated pack over the given `<name>.md` files, in a fresh temp dir.
    fn pack(test: &str, names: &[&str]) -> PromptPack {
        let dir = std::env::temp_dir().join(format!("zseval-shadow-{test}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        for n in names {
            std::fs::write(dir.join(format!("{n}.md")), format!("{n} body")).unwrap();
        }
        PromptPack::load(&dir).unwrap()
    }

    /// A backend that spends nothing and writes one session recording the
    /// built-in of whichever prompt the scenario declares (`code` when it
    /// declares none, the same name the fallback resolves), counting every
    /// trial it is asked to drive — the count is how "no trial ran" is
    /// checked, since a skipped scenario and a scenario whose trials all
    /// failed both leave no graded trial. Honouring the declared name is what
    /// lets a pin on a name its scenario never resolves grade and fail here,
    /// rather than accidentally passing on a session that ignored it.
    struct StubBackend {
        pack: Option<PromptPack>,
        runs: AtomicUsize,
    }

    impl AgentBackend for StubBackend {
        fn name(&self) -> &str {
            "stub"
        }

        fn identity(&self) -> Result<ZsIdentity> {
            Ok(ZsIdentity {
                zs_version: "stub 0.0.0".into(),
                zs_bin_path: String::new(),
                zs_bin_sha256: String::new(),
                git_sha: None,
                features: None,
            })
        }

        fn prompt_pack(&self) -> Option<&PromptPack> {
            self.pack.as_ref()
        }

        fn run(&self, sc: &Scenario, run_dir: &Path) -> Result<RunArtifacts> {
            self.runs.fetch_add(1, Ordering::SeqCst);
            let data = run_dir.join("data");
            std::fs::create_dir_all(data.join("sessions"))?;
            let session = data.join("sessions").join("session.json");
            let loaded = sc.prompt.as_deref().unwrap_or(DEFAULT_PROMPT_FALLBACK);
            std::fs::write(
                &session,
                format!(
                    r#"{{"id":"stub","messages":[{{"role":"assistant","content":"Paris."}}],
                    "prompt":{{"name":"{loaded}","source":"built_in"}}}}"#
                ),
            )?;
            Ok(RunArtifacts {
                session_files: vec![session],
                turns: Vec::new(),
                data_dir: data,
                config_dir: run_dir.join("config"),
                work_dir: run_dir.join("work"),
                wall_secs: 0.0,
            })
        }
    }

    /// No scenario here declares a rubric, so the ruler is never reached.
    struct UnusedJudge;

    impl Judge for UnusedJudge {
        fn available(&self) -> bool {
            false
        }
        fn unavailable_hint(&self) -> String {
            "no judge in this test".into()
        }
        fn judge(&self, _rubric: &str, _evidence: &str, _dir: &Path) -> Result<JudgeOutcome> {
            unreachable!("no scenario in these tests declares a rubric")
        }
    }

    /// Drive a whole suite and report both the run's own findings and how many
    /// trials the backend was actually asked for.
    fn run(test: &str, scenarios: &[Scenario], pack: Option<PromptPack>) -> (Report, usize) {
        let results_root =
            std::env::temp_dir().join(format!("zseval-shadow-run-{test}-{}", std::process::id()));
        std::fs::remove_dir_all(&results_root).ok();
        let backend = StubBackend {
            pack,
            runs: AtomicUsize::new(0),
        };
        let opts = RunOptions {
            target: None,
            trials_override: None,
            tag: test.to_string(),
            no_judge: true,
            results_root,
            max_total_usd: None,
            jobs: 1,
            judge_file: None,
            multi_target: false,
            integrity_roots: Vec::new(),
        };
        let report = run_suite(scenarios, &backend, &UnusedJudge, &opts).unwrap();
        let runs = backend.runs.load(Ordering::SeqCst);
        (report, runs)
    }

    #[test]
    fn a_pack_providing_the_pinned_name_runs_no_trials_and_records_the_scenario_ungradable() {
        let (report, runs) = run(
            "shadowed",
            &[stock_pin()],
            Some(pack("shadowed", &["code"])),
        );
        assert_eq!(
            runs, 0,
            "the collision must be caught before any trial runs"
        );
        let sr = report
            .scenarios
            .iter()
            .find(|s| s.id == "session-prompt-recorded-stock")
            .expect("a skipped scenario still belongs on the report, not dropped from it");
        assert!(sr.trials.is_empty());
        assert!(
            !sr.is_gradable(),
            "the pin observed nothing, so it is ungradable — never a failure"
        );
        assert_eq!(report.summary.n_gradable, 0);
        assert_eq!(
            report.summary.indeterminate_trials, 0,
            "no trial was spent, so none was indeterminate either"
        );
    }

    #[test]
    fn the_skip_line_names_the_scenario_and_the_prompt_the_pack_shadowed() {
        let line = shadowed_built_in_pin(&stock_pin(), Some(&pack("skip-line", &["code"])), None)
            .expect("the pack provides the pinned name");
        assert!(
            line.contains("session-prompt-recorded-stock") && line.contains("'code'"),
            "a reader has to be told which scenario and which prompt: {line}"
        );
    }

    // The skip is gated on the scenario resolving the pinned name, not merely
    // asserting it: only a pin the pack really did take out of the picture is
    // ungradable, everything else grades.

    #[test]
    fn the_shipped_pin_shape_is_skipped_because_the_name_it_resolves_is_the_one_it_pins() {
        // No `prompt` of its own and no config default, so it resolves `code`
        // through the fallback — the very name this pack replaces.
        assert!(
            shadowed_built_in_pin(&stock_pin(), Some(&pack("shipped-shape", &["code"])), None)
                .is_some(),
            "the shipped pin is exactly what this rule exists for"
        );
    }

    #[test]
    fn a_config_default_steering_the_scenario_off_the_pinned_name_leaves_it_grading() {
        // The scenario declares no prompt, so the target's `default_prompt`
        // decides: it loads `review`, and the pack's `code` replaces nothing
        // this pin watches.
        assert_eq!(
            shadowed_built_in_pin(
                &stock_pin(),
                Some(&pack("config-default", &["code"])),
                Some("review")
            ),
            None,
            "the pack shadows a built-in this scenario never loads"
        );
    }

    #[test]
    fn a_pin_whose_scenario_seeds_that_prompt_itself_is_graded_not_skipped() {
        // The scenario's own placement lands after the pack's, so what
        // replaced the built-in here is the scenario, not the run's pack —
        // its pin failing is its own authoring to answer for.
        let mut sc = stock_pin();
        sc.files = vec![FileSeed {
            src: PathBuf::from("_fixtures/code.md"),
            dest: "work:.zerostack/prompts/code.md".into(),
        }];
        assert_eq!(
            shadowed_built_in_pin(&sc, Some(&pack("self-seeded", &["code"])), None),
            None,
            "the pack is not what took this built-in out of the picture"
        );
    }

    #[test]
    fn a_skipped_pin_does_not_stop_the_rest_of_the_suite() {
        let scenarios = [
            stock_pin(),
            pinned("session-says-paris", "final_contains Paris"),
        ];
        let (report, runs) = run("rest-of-suite", &scenarios, Some(pack("rest", &["code"])));
        assert_eq!(runs, 1, "only the shadowed scenario is skipped");
        assert_eq!(report.scenarios.len(), 2);
        assert!(!report.scenarios[0].is_gradable());
        assert_eq!(report.scenarios[1].trials[0].outcome, Final::Pass);
    }

    // The end-of-run "pack seeded but never loaded" check reads the same skip:
    // a scenario that ran no trials resolved no prompt, so it can no longer
    // contribute the `Pack` source that check looks for — but neither can it
    // speak for the scenarios that did run.

    /// A scenario result as a run records one: `source` is what its trials'
    /// sessions read back, and `Unknown` is what a scenario that observed
    /// nothing keeps — a skipped pin (no trial ran at all) among them.
    fn recorded(id: &str, source: PromptSource) -> ScenarioResult {
        let mut sr = ScenarioResult::from_trials(id.into(), Kind::Regression, Vec::new());
        sr.prompt_source = source;
        sr
    }

    /// The shipped pin, skipped: what a run of `scenarios/` with a pack
    /// providing `code` hands the end-of-run check every time.
    fn skipped_stock_pin() -> [String; 1] {
        ["session-prompt-recorded-stock".to_string()]
    }

    #[test]
    fn a_pack_no_scenario_that_ran_resolved_is_reported_unloaded_even_when_a_pin_was_skipped() {
        let p = pack("unloaded-with-skip", &["code"]);
        let results = [
            recorded("session-prompt-recorded-stock", PromptSource::Unknown),
            recorded("declares-ask", PromptSource::Stock),
        ];
        let warning = unloaded_pack_warning(&p, &results, &skipped_stock_pin()).expect(
            "a scenario ran and resolved a built-in, which is evidence about the pack that a \
             different scenario's skip cannot mute",
        );
        assert!(warning.contains("never loaded"), "{warning}");
        assert!(
            warning.contains("session-prompt-recorded-stock"),
            "the skipped scenario has to be accounted for in the same breath, not ignored: \
             {warning}"
        );
    }

    #[test]
    fn a_pack_another_scenario_resolved_is_not_reported_unloaded_when_a_pin_was_skipped() {
        let p = pack("loaded-with-skip", &["code"]);
        let results = [
            recorded("session-prompt-recorded-stock", PromptSource::Unknown),
            recorded("declares-code", PromptSource::Pack),
        ];
        assert_eq!(
            unloaded_pack_warning(&p, &results, &skipped_stock_pin()),
            None,
            "a scenario resolved a prompt from the pack, so it plainly loaded"
        );
    }

    #[test]
    fn a_run_whose_scenarios_were_all_skipped_claims_nothing_about_its_pack() {
        let p = pack("unloaded-all-skipped", &["code"]);
        let results = [recorded(
            "session-prompt-recorded-stock",
            PromptSource::Unknown,
        )];
        assert_eq!(
            unloaded_pack_warning(&p, &results, &skipped_stock_pin()),
            None,
            "nothing ran, so nothing was observed about the pack either way"
        );
    }

    #[test]
    fn a_scenario_that_observed_no_prompt_is_no_evidence_the_pack_went_unloaded() {
        let p = pack("unloaded-unobserved", &["code"]);
        let results = [recorded("every-trial-failed", PromptSource::Unknown)];
        assert_eq!(
            unloaded_pack_warning(&p, &results, &[]),
            None,
            "a scenario that ran but read back nothing observed no absence either"
        );
    }

    #[test]
    fn a_pack_no_scenario_resolved_is_still_reported_unloaded_when_nothing_was_skipped() {
        let p = pack("unloaded-plain", &["code"]);
        let results = [recorded("declares-ask", PromptSource::Stock)];
        let warning = unloaded_pack_warning(&p, &results, &[])
            .expect("a scenario ran, resolved a built-in, and nothing was skipped");
        assert!(warning.contains("never loaded"), "{warning}");
        assert!(
            !warning.contains("skipped"),
            "nothing was skipped, so there is nothing to account for: {warning}"
        );
    }

    #[test]
    fn a_pack_providing_only_other_names_leaves_the_built_in_pin_running() {
        let (report, runs) = run(
            "unrelated",
            &[stock_pin()],
            Some(pack("unrelated", &["review"])),
        );
        assert_eq!(runs, 1);
        assert_eq!(report.scenarios[0].trials.len(), 1);
        assert_eq!(report.scenarios[0].trials[0].outcome, Final::Pass);
    }

    #[test]
    fn a_run_with_no_pack_at_all_leaves_the_built_in_pin_running() {
        let (report, runs) = run("packless", &[stock_pin()], None);
        assert_eq!(runs, 1);
        assert_eq!(report.scenarios[0].trials.len(), 1);
        assert_eq!(report.scenarios[0].trials[0].outcome, Final::Pass);
    }

    #[test]
    fn a_pin_naming_a_prompt_its_scenario_never_resolves_is_graded_not_skipped() {
        // Declaring `prompt = "ask"` while pinning `code`'s built-in is an
        // authoring error: the scenario loads `ask`, so the pack's `code`
        // shadows nothing it watches. The honest answer is the failing grade,
        // not a silent ungradable.
        let mut sc = pinned("session-pin-mismatch", "prompt_recorded code built_in");
        sc.prompt = Some("ask".into());
        let (report, runs) = run("pin-mismatch", &[sc], Some(pack("pin-mismatch", &["code"])));
        assert_eq!(runs, 1, "the pack shadows nothing this scenario resolves");
        assert!(report.scenarios[0].is_gradable());
        assert_eq!(
            report.scenarios[0].trials[0].outcome,
            Final::Fail,
            "the session loaded 'ask', so a pin on 'code' fails on its own terms"
        );
    }

    #[test]
    fn a_user_file_pin_is_graded_not_skipped_when_the_pack_provides_its_name() {
        // The mirror case is deliberately uncovered: a `user_file` pin that
        // reads back the built-in means the pack never loaded, and that plain
        // failure is the honest signal.
        let sc = pinned("session-prompt-pack", "prompt_recorded code user_file");
        let (report, runs) = run("mirror", &[sc], Some(pack("mirror", &["code"])));
        assert_eq!(runs, 1, "the mirror case still spends its trials");
        assert!(report.scenarios[0].is_gradable());
        assert_eq!(report.scenarios[0].trials[0].outcome, Final::Fail);
    }
}

/// What a run does when the files it is *defined* by change while it runs —
/// the sandbox-escape case that motivated the check: a trial reaching outside
/// its work dir and editing the scenario tree it is being graded against.
/// Driven through `run_suite` rather than a helper, because every claim here
/// is about the whole loop: which scenarios get launched, what the trials that
/// already ran are rewritten to, and that the report still lands on disk.
#[cfg(test)]
mod input_drift_tests {
    use super::*;
    use crate::asserts::Assert;
    use crate::backend::RunArtifacts;
    use crate::judge::{Judge, JudgeOutcome};
    use crate::scenario::{Kind, Task};
    use crate::verdict::{Final, Report, ZsIdentity};
    use std::sync::Mutex;

    /// A backend that escapes: while driving one named scenario it writes into
    /// a watched input root, then produces the same passing session every
    /// scenario gets. It records every scenario it was asked to drive, which
    /// is how "no further scenario was launched" is checked.
    struct EscapingBackend {
        input_file: PathBuf,
        escapes_on: String,
        ran: Mutex<Vec<String>>,
    }

    impl AgentBackend for EscapingBackend {
        fn name(&self) -> &str {
            "escaping"
        }

        fn identity(&self) -> Result<ZsIdentity> {
            Ok(ZsIdentity {
                zs_version: "stub 0.0.0".into(),
                zs_bin_path: String::new(),
                zs_bin_sha256: String::new(),
                git_sha: None,
                features: None,
            })
        }

        fn run(&self, sc: &Scenario, run_dir: &Path) -> Result<RunArtifacts> {
            self.ran.lock().unwrap().push(sc.id.clone());
            if sc.id == self.escapes_on {
                std::fs::write(&self.input_file, format!("edited while {} ran\n", sc.id))?;
            }
            let data = run_dir.join("data");
            std::fs::create_dir_all(data.join("sessions"))?;
            let session = data.join("sessions").join("session.json");
            std::fs::write(
                &session,
                r#"{"id":"s","messages":[{"role":"assistant","content":"Paris."}]}"#,
            )?;
            Ok(RunArtifacts {
                session_files: vec![session],
                turns: Vec::new(),
                data_dir: data,
                config_dir: run_dir.join("config"),
                work_dir: run_dir.join("work"),
                wall_secs: 0.0,
            })
        }
    }

    /// No scenario here declares a rubric, so the ruler is never reached.
    struct UnusedJudge;

    impl Judge for UnusedJudge {
        fn available(&self) -> bool {
            false
        }
        fn unavailable_hint(&self) -> String {
            "no judge in this test".into()
        }
        fn judge(&self, _rubric: &str, _evidence: &str, _dir: &Path) -> Result<JudgeOutcome> {
            unreachable!("no scenario in these tests declares a rubric")
        }
    }

    /// A scenario the stub session passes: one deterministic assert, no judge.
    fn scenario(id: &str) -> Scenario {
        let expect = "final_contains Paris";
        Scenario {
            id: id.into(),
            kind: Kind::Regression,
            prompt: None,
            trials: 1,
            mode: Mode::Print,
            loop_cfg: None,
            security_mode: Default::default(),
            task: Task::Single("what is the capital of France?".into()),
            expect: vec![expect.into()],
            judge: None,
            timeout_secs: 300,
            max_cost_usd: None,
            max_total_tokens: None,
            files: vec![],
            seed: Default::default(),
            domains: vec![],
            dir: PathBuf::new(),
            asserts: vec![Assert::parse(expect).unwrap()],
            content_hash: String::new(),
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zseval-drift-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The whole halt, end to end: a suite of three scenarios where the second
    /// one's trials edit a shared fixture in the watched tree.
    #[test]
    fn a_scenario_whose_trials_edit_a_watched_input_ends_the_run_indeterminate() {
        let inputs = scratch("inputs");
        let fixture = inputs.join("_fixtures/shared.txt");
        std::fs::create_dir_all(fixture.parent().unwrap()).unwrap();
        std::fs::write(&fixture, "original\n").unwrap();
        let results_root = scratch("results");

        let backend = EscapingBackend {
            input_file: fixture.clone(),
            escapes_on: "drifts".into(),
            ran: Mutex::new(Vec::new()),
        };
        let opts = RunOptions {
            target: None,
            // Two trials, so the withdrawal has to reach every trial of the
            // window rather than only the one that did the writing.
            trials_override: Some(2),
            tag: "drift".into(),
            no_judge: true,
            results_root: results_root.clone(),
            max_total_usd: None,
            jobs: 1,
            judge_file: None,
            multi_target: false,
            integrity_roots: vec![inputs.clone()],
        };

        let scenarios = [scenario("clean"), scenario("drifts"), scenario("never-run")];
        let err = run_suite(&scenarios, &backend, &UnusedJudge, &opts)
            .expect_err("a run whose inputs moved must not return a clean report");
        let err = format!("{err:#}");
        assert!(
            err.contains("input-integrity drift") && err.contains("shared.txt"),
            "the error has to name what moved: {err}"
        );

        assert_eq!(
            *backend.ran.lock().unwrap(),
            vec!["clean", "clean", "drifts", "drifts"],
            "the scenario after the drift must never be launched"
        );

        // The report of what ran is still on disk — the run is aborted, not
        // erased.
        let report_path = results_root.join("drift/report.json");
        let report: Report = serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
        assert_eq!(
            report
                .scenarios
                .iter()
                .map(|s| s.id.as_str())
                .collect::<Vec<_>>(),
            vec!["clean", "drifts"],
        );
        assert!(
            report.scenarios[0]
                .trials
                .iter()
                .all(|t| t.outcome == Final::Pass),
            "the scenario before the drift window graded normally: {:?}",
            report.scenarios[0].trials
        );
        for tr in &report.scenarios[1].trials {
            assert_eq!(tr.outcome, Final::Indeterminate, "{tr:?}");
            assert!(
                tr.reasons
                    .iter()
                    .any(|r| r.contains("input-integrity drift") && r.contains("shared.txt")),
                "the withdrawn verdict has to say why, naming the path: {:?}",
                tr.reasons
            );
        }

        // …and the persisted per-trial copy `explain` reads agrees with it,
        // rather than still claiming the verdict this run took back.
        for trial in 0..2 {
            let path = results_root.join(format!("drift/drifts/trial-{trial}/trial.json"));
            let tr: TrialResult = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert_eq!(tr.outcome, Final::Indeterminate, "{}", path.display());
            assert!(
                tr.reasons
                    .iter()
                    .any(|r| r.contains("input-integrity drift")),
                "{:?}",
                tr.reasons
            );
        }

        std::fs::remove_dir_all(&inputs).ok();
        std::fs::remove_dir_all(&results_root).ok();
    }

    /// The check is per scenario, and a suite that never touches its inputs
    /// runs to the end: the barrier must not fire on the harness's own
    /// writes into the results tree, which is not an input.
    #[test]
    fn a_suite_that_leaves_its_inputs_alone_runs_to_the_end() {
        let inputs = scratch("clean-inputs");
        std::fs::write(inputs.join("scenario.toml"), "id = \"x\"\n").unwrap();
        let results_root = scratch("clean-results");

        let backend = EscapingBackend {
            input_file: inputs.join("never-written.txt"),
            escapes_on: "nothing-matches-this".into(),
            ran: Mutex::new(Vec::new()),
        };
        let opts = RunOptions {
            target: None,
            trials_override: Some(1),
            tag: "clean".into(),
            no_judge: true,
            results_root: results_root.clone(),
            max_total_usd: None,
            jobs: 1,
            judge_file: None,
            multi_target: false,
            integrity_roots: vec![inputs.clone()],
        };

        let scenarios = [scenario("one"), scenario("two")];
        let report = run_suite(&scenarios, &backend, &UnusedJudge, &opts).unwrap();
        assert_eq!(*backend.ran.lock().unwrap(), vec!["one", "two"]);
        assert!(report
            .scenarios
            .iter()
            .all(|s| s.trials.iter().all(|t| t.outcome == Final::Pass)));

        std::fs::remove_dir_all(&inputs).ok();
        std::fs::remove_dir_all(&results_root).ok();
    }
}
