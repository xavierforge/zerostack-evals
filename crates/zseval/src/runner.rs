//! Orchestration: scenarios × trials -> graded trials -> report on disk.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::asserts::Assert;
use crate::backend::AgentBackend;
use crate::backend::RunRoots;
use crate::judge::{self, JudgeVerdict};
use crate::scenario::Scenario;
use crate::transcript::Transcript;
use crate::verdict::{Final, Report, ScenarioResult, TrialResult};

pub struct RunOptions {
    /// None = don't force a model; zerostack uses its configured default.
    pub model: Option<String>,
    pub trials_override: Option<usize>,
    pub tag: String,
    pub no_judge: bool,
    pub results_root: PathBuf,
    pub max_total_usd: Option<f64>,
}

pub fn run_suite(
    scenarios: Vec<Scenario>,
    backend: &dyn AgentBackend,
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
        let mut trial_results = Vec::new();
        for trial in 0..trials {
            let run_dir = opts
                .results_root
                .join(&opts.tag)
                .join(&sc.id)
                .join(format!("trial-{trial}"));
            std::fs::create_dir_all(&run_dir)?;
            let tr = run_trial(sc, backend, opts, trial, &run_dir);
            spent += tr.cost_usd;
            // Persist per-trial for `explain`.
            std::fs::write(run_dir.join("trial.json"), serde_json::to_vec_pretty(&tr)?)?;
            print_trial_line(&sc.id, &tr);
            trial_results.push(tr);
        }
        results.push(ScenarioResult::from_trials(sc.id.clone(), trial_results));
    }

    let report = Report::build(
        opts.tag.clone(),
        opts.model
            .clone()
            .unwrap_or_else(|| "provider-default".into()),
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

fn run_trial(
    sc: &Scenario,
    backend: &dyn AgentBackend,
    opts: &RunOptions,
    trial: usize,
    run_dir: &Path,
) -> TrialResult {
    let mut reasons = Vec::new();

    // 1. Drive the agent. Backend errors = indeterminate (we never got a
    //    gradable transcript), not fail.
    let artifacts = match backend.run(sc, opts.model.as_deref(), run_dir) {
        Ok(a) => a,
        Err(e) => {
            return indeterminate(trial, run_dir, format!("backend: {e:#}"));
        }
    };

    // 2. Parse + merge session transcripts. Schema mismatch = indeterminate.
    let mut transcript = Transcript::default();
    for f in &artifacts.session_files {
        match crate::transcript::parse_file(f) {
            Ok(t) => transcript.absorb(t),
            Err(e) => {
                return indeterminate(trial, run_dir, format!("transcript: {e:#}"));
            }
        }
    }

    // 2a. In headless mode zerostack's session JSON never records tool calls
    // (see transcript.rs's module doc) — the only place they surface is the
    // `--pure-stdout` markers in each turn's captured stdout. Reconstruct
    // them here, in turn order, so `tool_called*` asserts have real evidence
    // for a real (non-mock) run. The mock backend writes no stdout logs, so
    // this is a no-op there and mock fixtures keep sourcing tool_calls from
    // session JSON as before.
    let mut stdout_logs: Vec<PathBuf> = std::fs::read_dir(run_dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
            name.starts_with("turn-") && name.ends_with(".stdout")
        })
        .collect();
    stdout_logs.sort();
    for log in &stdout_logs {
        let base = transcript.tool_calls.len();
        transcript
            .tool_calls
            .extend(crate::transcript::tool_calls_from_stdout_file(log, base));
    }

    let roots = RunRoots {
        data: &artifacts.data_dir,
        config: &artifacts.config_dir,
        work: &artifacts.work_dir,
    };

    // 2b. A scenario that seeds memory is only gradable if our snapshot of
    // zerostack's memory layout still matches reality — a stale snapshot must
    // never be silently blamed on the agent.
    if sc.seed.memory.is_some() {
        let expected_root = artifacts.config_dir.join("agent").join("memory");
        let expected_project = crate::domains::memory::project_slug(&artifacts.work_dir);
        if let Err(reason) =
            crate::domains::memory::verify_layout(run_dir, &expected_root, &expected_project)
        {
            return indeterminate(trial, run_dir, format!("memory layout drift: {reason}"));
        }
    }

    // 3. Deterministic floor.
    let mut all_pass = true;
    let mut assert_results = Vec::new();
    for line in &sc.expect {
        // Parse already validated at load time.
        let a = Assert::parse(line).expect("validated at load");
        let r = a.eval(&transcript, &roots);
        if !r.pass {
            all_pass = false;
            reasons.push(format!("assert failed: {} ({})", line, r.detail));
        }
        assert_results.push(r.into());
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
    let mut outcome = if all_pass { Final::Pass } else { Final::Fail };
    if outcome == Final::Pass {
        if let Some(rubric) = &sc.judge {
            if opts.no_judge {
                reasons.push("judge skipped (--no-judge)".into());
            } else if !judge::have_api_key() {
                return TrialResult {
                    judge: None,
                    ..indeterminate(
                        trial,
                        run_dir,
                        "judge required but ANTHROPIC_API_KEY unset".into(),
                    )
                };
            } else {
                match judge::judge(rubric, &transcript.render_for_judge(20_000), run_dir) {
                    Ok(v) => {
                        judge_verdict = Some(v);
                        match v {
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
        cost_usd: transcript.cost_usd,
        wall_secs: artifacts.wall_secs,
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
        cost_usd: 0.0,
        wall_secs: 0.0,
        run_dir: run_dir.display().to_string(),
    }
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
