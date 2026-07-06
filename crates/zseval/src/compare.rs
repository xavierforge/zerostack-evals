//! Compare two reports: per-scenario trial pass-rate diff, regression gate.
//!
//! Exit-code contract (also the loop-engineering API — see AGENTS.md):
//!   0 = no regression
//!   1 = regression beyond threshold (CI goes red)
//! Scenario sets may differ; added/removed scenarios are listed but only
//! shared scenarios can regress.

use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::verdict::Report;

#[derive(Debug, Serialize)]
pub struct Comparison {
    pub baseline_tag: String,
    pub candidate_tag: String,
    pub rows: Vec<Row>,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub regressions: Vec<String>,
    /// Shared scenarios that were ungradable on one side (excluded from
    /// regression — an infra/eval problem, not an agent regression).
    pub errored: Vec<String>,
    /// Shared, gradable scenarios where tool-call evidence dropped to zero
    /// on the candidate despite the baseline having some. This is the
    /// tripwire for the exact failure mode that let every `tool_called`
    /// assert pass vacuously for months: a `tool_not_called`-only scenario
    /// can look like a clean pass even when the evidence channel itself is
    /// broken, since there's nothing to contradict it. Not counted as a
    /// regression (the pass rate may not have moved at all) — surfaced
    /// separately so it doesn't hide in a stable-looking diff.
    pub evidence_warnings: Vec<String>,
    pub summary_base_pass_hat_k: f64,
    pub summary_cand_pass_hat_k: f64,
    pub summary_base_cost: f64,
    pub summary_cand_cost: f64,
}

#[derive(Debug, Serialize)]
pub struct Row {
    pub id: String,
    pub base_rate: f64,
    pub cand_rate: f64,
    pub diff: f64,
    pub regression: bool,
}

pub fn load_report(path: &Path) -> Result<Report> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let r: Report =
        serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
    Ok(r)
}

pub fn compare(base: &Report, cand: &Report, threshold: f64) -> Comparison {
    let mut rows = Vec::new();
    let mut regressions = Vec::new();
    let mut errored = Vec::new();
    let mut evidence_warnings = Vec::new();
    let mut added = Vec::new();
    let mut removed = Vec::new();

    for b in &base.scenarios {
        match cand.scenarios.iter().find(|c| c.id == b.id) {
            Some(c) => {
                // If either side has no graded trial, we can't compare pass
                // rates — treat as errored, not a regression.
                if !b.is_gradable() || !c.is_gradable() {
                    errored.push(b.id.clone());
                    continue;
                }
                if b.total_tool_calls > 0 && c.total_tool_calls == 0 {
                    evidence_warnings.push(b.id.clone());
                }
                let base_rate = b.trial_pass_rate();
                let cand_rate = c.trial_pass_rate();
                let diff = cand_rate - base_rate;
                let regression = diff < -threshold;
                if regression {
                    regressions.push(b.id.clone());
                }
                rows.push(Row {
                    id: b.id.clone(),
                    base_rate,
                    cand_rate,
                    diff,
                    regression,
                });
            }
            None => removed.push(b.id.clone()),
        }
    }
    for c in &cand.scenarios {
        if !base.scenarios.iter().any(|b| b.id == c.id) {
            added.push(c.id.clone());
        }
    }

    Comparison {
        baseline_tag: base.tag.clone(),
        candidate_tag: cand.tag.clone(),
        rows,
        added,
        removed,
        regressions,
        errored,
        evidence_warnings,
        summary_base_pass_hat_k: base.summary.pass_hat_k,
        summary_cand_pass_hat_k: cand.summary.pass_hat_k,
        summary_base_cost: base.summary.total_cost_usd,
        summary_cand_cost: cand.summary.total_cost_usd,
    }
}

impl Comparison {
    /// This comparison's own exit code, independent of the CLI's other
    /// concerns. `2` (harness error) when scenarios existed on both sides
    /// but not one shared scenario was comparable — every shared id landed
    /// in `errored`, meaning the candidate (or baseline) environment is
    /// broken, which must never look like "no regressions" (exit 0). `1`
    /// when at least one shared scenario regressed. `0` otherwise.
    pub fn exit_code(&self) -> u8 {
        let shared = self.rows.len() + self.errored.len();
        if shared > 0 && self.rows.is_empty() {
            return 2;
        }
        if self.regressions.is_empty() {
            0
        } else {
            1
        }
    }
}

pub fn print_human(c: &Comparison) {
    println!(
        "baseline : {}    candidate: {}",
        c.baseline_tag, c.candidate_tag
    );
    println!("{:<48}{:>8}{:>8}{:>8}", "scenario", "base", "cand", "diff");
    for r in &c.rows {
        println!(
            "{:<48}{:>8.3}{:>8.3}{:>+8.3}{}",
            r.id,
            r.base_rate,
            r.cand_rate,
            r.diff,
            if r.regression { "  <- REGRESSION" } else { "" }
        );
    }
    println!(
        "{:<48}{:>8.3}{:>8.3}",
        "pass^k (overall)", c.summary_base_pass_hat_k, c.summary_cand_pass_hat_k
    );
    println!(
        "{:<48}{:>8.3}{:>8.3}",
        "total cost usd", c.summary_base_cost, c.summary_cand_cost
    );
    for id in &c.added {
        println!("+ new scenario: {id}");
    }
    for id in &c.removed {
        println!("- missing scenario: {id}");
    }
    for id in &c.errored {
        println!("? ungradable (eval/infra, not a regression): {id}");
    }
    if !c.evidence_warnings.is_empty() {
        println!(
            "\n⚠ tool-call evidence dropped to zero (the pass rate above may look \
             fine while the evidence channel itself is broken — see AGENTS.md):"
        );
        for id in &c.evidence_warnings {
            println!("  {id}");
        }
    }
    if !c.regressions.is_empty() {
        println!(
            "\nregressed scenarios (run `zseval explain <trial-dir>` to see the \
             failed asserts):"
        );
        for id in &c.regressions {
            println!("  {id}");
        }
    }
}

#[cfg(test)]
mod exit_code_tests {
    use super::*;
    use crate::verdict::{Final, Report, ScenarioResult, TrialResult};

    fn trial(outcome: Final, tool_call_count: usize) -> TrialResult {
        TrialResult {
            trial: 0,
            outcome,
            reasons: vec![],
            asserts: vec![],
            judge: None,
            input_tokens: 0,
            output_tokens: 0,
            cost_usd: 0.0,
            wall_secs: 0.0,
            tool_call_count,
            run_dir: String::new(),
        }
    }

    fn report(scenarios: Vec<ScenarioResult>) -> Report {
        Report::build("t".into(), "m".into(), "b".into(), 1, scenarios)
    }

    #[test]
    fn all_shared_scenarios_errored_is_harness_error() {
        // Every shared scenario ungradable on one side (e.g. the candidate
        // binary crashed on every scenario) must not look like a clean "no
        // regressions" — that's exit 2, same class of failure as a fully
        // indeterminate `run`.
        let base = report(vec![ScenarioResult::from_trials(
            "s".into(),
            vec![trial(Final::Pass, 0)],
        )]);
        let cand = report(vec![ScenarioResult::from_trials(
            "s".into(),
            vec![trial(Final::Indeterminate, 0)],
        )]);
        let c = compare(&base, &cand, 0.05);
        assert!(c.rows.is_empty());
        assert_eq!(c.errored, vec!["s".to_string()]);
        assert_eq!(c.exit_code(), 2);
    }

    #[test]
    fn no_shared_scenarios_at_all_is_not_a_harness_error() {
        // Base and candidate cover disjoint scenario sets (e.g. comparing
        // two different suites) — nothing to compare, but that's a usage
        // question, not an environment failure.
        let base = report(vec![ScenarioResult::from_trials(
            "only-in-base".into(),
            vec![trial(Final::Pass, 0)],
        )]);
        let cand = report(vec![ScenarioResult::from_trials(
            "only-in-cand".into(),
            vec![trial(Final::Pass, 0)],
        )]);
        let c = compare(&base, &cand, 0.05);
        assert_eq!(c.exit_code(), 0);
    }

    #[test]
    fn regression_is_exit_1() {
        let base = report(vec![ScenarioResult::from_trials(
            "s".into(),
            vec![trial(Final::Pass, 0)],
        )]);
        let cand = report(vec![ScenarioResult::from_trials(
            "s".into(),
            vec![trial(Final::Fail, 0)],
        )]);
        let c = compare(&base, &cand, 0.05);
        assert_eq!(c.exit_code(), 1);
    }

    #[test]
    fn stable_comparison_is_exit_0() {
        let base = report(vec![ScenarioResult::from_trials(
            "s".into(),
            vec![trial(Final::Pass, 0)],
        )]);
        let cand = report(vec![ScenarioResult::from_trials(
            "s".into(),
            vec![trial(Final::Pass, 0)],
        )]);
        let c = compare(&base, &cand, 0.05);
        assert_eq!(c.exit_code(), 0);
    }

    #[test]
    fn some_errored_but_some_comparable_is_not_a_harness_error() {
        // Only a *total* wipeout (zero comparable shared scenarios) is a
        // harness error; partial errored scenarios are already excluded
        // from regression by `compare` itself.
        let base = report(vec![
            ScenarioResult::from_trials("ok".into(), vec![trial(Final::Pass, 0)]),
            ScenarioResult::from_trials("broken".into(), vec![trial(Final::Pass, 0)]),
        ]);
        let cand = report(vec![
            ScenarioResult::from_trials("ok".into(), vec![trial(Final::Pass, 0)]),
            ScenarioResult::from_trials("broken".into(), vec![trial(Final::Indeterminate, 0)]),
        ]);
        let c = compare(&base, &cand, 0.05);
        assert_eq!(c.errored, vec!["broken".to_string()]);
        assert_eq!(c.rows.len(), 1);
        assert_eq!(c.exit_code(), 0);
    }
}
