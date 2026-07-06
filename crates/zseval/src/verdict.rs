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

use serde::{Deserialize, Serialize};

use crate::asserts::AssertResult;
use crate::judge::JudgeVerdict;

pub const REPORT_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Final {
    Pass,
    Fail,
    Indeterminate,
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
            pass_at_k: if any { 1.0 } else { 0.0 },
            pass_hat_k: if all { 1.0 } else { 0.0 },
            indeterminate,
            total_tool_calls,
            content_hash,
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
    pub scenarios: Vec<ScenarioResult>,
    pub summary: Summary,
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
}

impl Report {
    pub fn build(
        tag: String,
        model: String,
        backend: String,
        trials: usize,
        scenarios: Vec<ScenarioResult>,
    ) -> Report {
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
        Report {
            schema_version: REPORT_SCHEMA_VERSION,
            tag,
            model,
            backend,
            timestamp: now_iso(),
            trials,
            summary: Summary {
                n_scenarios: scenarios.len(),
                n_gradable: gradable.len(),
                pass_at_k: round4(pass_at_k),
                pass_hat_k: round4(pass_hat_k),
                indeterminate_scenarios,
                indeterminate_trials,
                total_cost_usd: round4(total_cost_usd),
                avg_wall_secs: round4(avg_wall_secs),
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

    fn trial(outcome: Final) -> TrialResult {
        TrialResult {
            trial: 0,
            outcome,
            reasons: vec![],
            asserts: vec![],
            judge: None,
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
            "t".into(),
            "m".into(),
            "b".into(),
            1,
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
        let report = Report::build("t".into(), "m".into(), "b".into(), 1, vec![]);
        assert_eq!(report.exit_code(), 0);
    }

    #[test]
    fn any_fail_is_exit_1() {
        let report = Report::build(
            "t".into(),
            "m".into(),
            "b".into(),
            1,
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
            "t".into(),
            "m".into(),
            "b".into(),
            1,
            vec![ScenarioResult::from_trials(
                "s".into(),
                vec![trial(Final::Pass)],
            )],
        );
        assert_eq!(report.exit_code(), 0);
    }

    #[test]
    fn mixed_gradable_and_indeterminate_scenarios_is_not_a_harness_error() {
        // Only a *fully* ungradable report is a harness error; a report where
        // some scenarios graded fine must still surface Fail/Pass normally.
        let report = Report::build(
            "t".into(),
            "m".into(),
            "b".into(),
            1,
            vec![
                ScenarioResult::from_trials("gradable".into(), vec![trial(Final::Pass)]),
                ScenarioResult::from_trials("broken".into(), vec![trial(Final::Indeterminate)]),
            ],
        );
        assert_eq!(report.exit_code(), 0);
    }
}
