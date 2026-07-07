//! Orchestration: scenarios × trials -> graded trials -> report on disk.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result};

use crate::backend::AgentBackend;
use crate::judge::{Judge, JudgeVerdict};
use crate::scenario::Scenario;
use crate::transcript::Transcript;
use crate::verdict::{Final, Report, ScenarioResult, TrialResult};

pub struct RunOptions {
    /// None = don't force a model; zerostack uses its configured default.
    pub model: Option<String>,
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
}

pub fn run_suite(
    scenarios: Vec<Scenario>,
    backend: &dyn AgentBackend,
    judge: &dyn Judge,
    opts: &RunOptions,
) -> Result<Report> {
    let mut results = Vec::new();
    let mut spent = 0.0_f64;

    for sc in &scenarios {
        // Check the cost cap once per scenario, so a scenario always runs its
        // full trial count or not at all — never a partial, misleading pass^k.
        if let Some(cap) = opts.max_total_usd {
            if spent >= cap {
                eprintln!("budget cap ${cap} reached; stopping before {}", sc.id);
                break;
            }
        }
        let trials = opts.trials_override.unwrap_or(sc.trials).max(1);
        let trial_results = run_trials_for_scenario(sc, backend, judge, opts, trials)?;
        for tr in &trial_results {
            spent += tr.cost_usd;
        }
        results.push(ScenarioResult::from_trials_with_hash(
            sc.id.clone(),
            sc.content_hash.clone(),
            trial_results,
        ));
    }

    let report = Report::build(
        opts.tag.clone(),
        crate::target::describe(opts.target.as_deref(), opts.model.as_deref()),
        backend.name().to_string(),
        opts.trials_override.unwrap_or(0),
        results,
    );
    // Everything for a run lives under results/<tag>/ — the report next to its
    // per-trial artifacts, so the results root never fills with loose files.
    let run_root = opts.results_root.join(&opts.tag);
    std::fs::create_dir_all(&run_root)?;
    let report_path = run_root.join("report.json");
    std::fs::write(&report_path, serde_json::to_vec_pretty(&report)?)
        .with_context(|| format!("write {}", report_path.display()))?;
    Ok(report)
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
    opts: &RunOptions,
    trials: usize,
) -> Result<Vec<TrialResult>> {
    let run_one = |trial: usize| -> Result<TrialResult> {
        let run_dir = opts
            .results_root
            .join(&opts.tag)
            .join(&sc.id)
            .join(format!("trial-{trial}"));
        std::fs::create_dir_all(&run_dir)?;
        let tr = run_trial(sc, backend, judge, opts, trial, &run_dir);
        // Persist per-trial for `explain`.
        std::fs::write(run_dir.join("trial.json"), serde_json::to_vec_pretty(&tr)?)?;
        Ok(tr)
    };

    if opts.jobs <= 1 {
        let mut out = Vec::with_capacity(trials);
        for trial in 0..trials {
            let tr = run_one(trial)?;
            print_trial_line(&sc.id, &tr);
            out.push(tr);
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
    print_trial_line(&sc.id, &first);
    if trials == 1 {
        return Ok(vec![first]);
    }

    let jobs = opts.jobs.min(trials - 1);
    let next = AtomicUsize::new(1);
    let outcome: Result<Vec<(usize, TrialResult)>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..jobs)
            .map(|_| {
                scope.spawn(|| -> Result<Vec<(usize, TrialResult)>> {
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
    for (_, tr) in all {
        print_trial_line(&sc.id, &tr);
        out.push(tr);
    }
    Ok(out)
}

fn run_trial(
    sc: &Scenario,
    backend: &dyn AgentBackend,
    judge: &dyn Judge,
    opts: &RunOptions,
    trial: usize,
    run_dir: &Path,
) -> TrialResult {
    // 1. Drive the agent. Backend errors = indeterminate (we never got a
    //    gradable transcript), not fail.
    let artifacts = match backend.run(sc, opts.model.as_deref(), run_dir) {
        Ok(a) => a,
        Err(e) => {
            return indeterminate(trial, run_dir, format!("backend: {e:#}"));
        }
    };
    grade_trial(sc, judge, opts.no_judge, trial, run_dir, &artifacts)
}

/// Re-grade an already-completed run_dir (produced by a prior `run` or a
/// captured `Mock` fixture) against `sc`'s *current* asserts/judge, without
/// driving the agent again. This is what backs `zseval regrade`: edit an
/// assert, re-score the same frozen evidence, see whether the new rule would
/// have passed — no API call, no new session. Persists the updated
/// `trial.json` next to the artifacts it graded, same as a normal run.
pub fn regrade(
    sc: &Scenario,
    judge: &dyn Judge,
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
    let tr = grade_trial(sc, judge, no_judge, trial, run_dir, &artifacts);
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
    judge: &dyn Judge,
    no_judge: bool,
    trial: usize,
    run_dir: &Path,
    artifacts: &crate::backend::RunArtifacts,
) -> TrialResult {
    let mut reasons = Vec::new();

    // 2. Assemble the gradable transcript: messages/tokens/cost from session
    // JSON plus tool calls from stdout (see Transcript::from_run and the
    // transcript.rs module doc for why both channels exist). Schema mismatch
    // = indeterminate.
    let transcript = match Transcript::from_run(artifacts) {
        Ok(t) => t,
        Err(e) => {
            return indeterminate(trial, run_dir, format!("transcript: {e:#}"));
        }
    };

    let roots = artifacts.roots();

    // 2b. A scenario that seeds a domain's files is only gradable if our
    // snapshot of that subsystem's layout still matches reality — a stale
    // snapshot must never be silently blamed on the agent. Each domain
    // derives its expectations from `roots` the same way `expand` seeded
    // them, so there's one computation to keep in sync, not two.
    let zslogs: Vec<PathBuf> = artifacts.turns.iter().map(|t| t.zslog.clone()).collect();
    if let Err(reason) = crate::domains::verify(sc, &roots, &zslogs) {
        return indeterminate(trial, run_dir, format!("domain drift: {reason}"));
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
    let mut judge_input_tokens = 0u64;
    let mut judge_output_tokens = 0u64;
    let mut judge_cost_usd = 0.0f64;
    let mut outcome = if all_pass { Final::Pass } else { Final::Fail };
    if outcome == Final::Pass {
        if let Some(rubric) = &sc.judge {
            if no_judge {
                reasons.push("judge skipped (--no-judge)".into());
            } else if !judge.available() {
                return TrialResult {
                    judge: None,
                    ..indeterminate(
                        trial,
                        run_dir,
                        "judge required but not available (is ANTHROPIC_API_KEY set?)".into(),
                    )
                };
            } else {
                match judge.judge(rubric, &transcript.render_for_judge(20_000), run_dir) {
                    Ok(o) => {
                        judge_verdict = Some(o.verdict);
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

    TrialResult {
        trial,
        outcome,
        reasons,
        asserts: assert_results,
        judge: judge_verdict,
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
        run_dir: run_dir.display().to_string(),
    }
}

fn indeterminate(trial: usize, run_dir: &Path, reason: String) -> TrialResult {
    TrialResult {
        trial,
        outcome: Final::Indeterminate,
        reasons: vec![reason],
        asserts: Vec::new(),
        judge: None,
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
        run_dir: run_dir.display().to_string(),
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
