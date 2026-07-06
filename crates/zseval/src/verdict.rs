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

pub const REPORT_SCHEMA_VERSION: u32 = 1;

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
    pub asserts: Vec<AssertResultOwned>,
    pub judge: Option<JudgeVerdict>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub wall_secs: f64,
    /// Where the raw transcript(s) and stdout/stderr live, for `explain`.
    pub run_dir: String,
}

/// AssertResult mirror that also deserializes (compare reads reports back).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssertResultOwned {
    pub spec: String,
    pub pass: bool,
    pub detail: String,
}

impl From<AssertResult> for AssertResultOwned {
    fn from(a: AssertResult) -> Self {
        AssertResultOwned {
            spec: a.spec,
            pass: a.pass,
            detail: a.detail,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioResult {
    pub id: String,
    pub trials: Vec<TrialResult>,
    pub pass_at_k: f64,
    pub pass_hat_k: f64,
    pub indeterminate: usize,
}

impl ScenarioResult {
    pub fn from_trials(id: String, trials: Vec<TrialResult>) -> Self {
        let graded: Vec<&TrialResult> = trials
            .iter()
            .filter(|t| t.outcome != Final::Indeterminate)
            .collect();
        let indeterminate = trials.len() - graded.len();
        let any = graded.iter().any(|t| t.outcome == Final::Pass);
        let all = !graded.is_empty() && graded.iter().all(|t| t.outcome == Final::Pass);
        ScenarioResult {
            id,
            pass_at_k: if any { 1.0 } else { 0.0 },
            pass_hat_k: if all { 1.0 } else { 0.0 },
            indeterminate,
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

    pub fn trial_pass_rate(&self) -> f64 {
        let graded: Vec<&TrialResult> = self
            .trials
            .iter()
            .filter(|t| t.outcome != Final::Indeterminate)
            .collect();
        if graded.is_empty() {
            return 0.0;
        }
        graded.iter().filter(|t| t.outcome == Final::Pass).count() as f64 / graded.len() as f64
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
