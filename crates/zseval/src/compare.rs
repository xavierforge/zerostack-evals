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
    /// Shared, gradable scenarios whose `content_hash` differs between
    /// baseline and candidate — the scenario's own definition changed, so a
    /// pass-rate diff compares two different tests under the same id. Not a
    /// regression signal by itself; surfaced so a moved ruler is visible
    /// instead of silently blamed on the agent (see AGENTS.md's guardrail).
    pub definition_changed: Vec<String>,
    /// Shared, gradable scenarios where `threshold` is finer than the
    /// smallest nonzero step this many graded trials can produce
    /// (`1 / n_graded_trials`) — e.g. 3 trials can only move in steps of
    /// 0.333, so a threshold of 0.05 means *any single trial flipping* is
    /// enough to call it a regression, regardless of the threshold's
    /// nominal value. Not wrong, but easy to mistake for real statistical
    /// tolerance; surfaced so that mismatch is visible instead of silently
    /// producing a noisier gate than the threshold implies.
    pub low_resolution: Vec<String>,
    /// Whether baseline and candidate were evaluated against a different
    /// provider/model (`Report.model`) — comparing across targets isn't
    /// inherently wrong (that's the A/B use case), but it must never be
    /// silent when the caller expected an apples-to-apples regression check.
    pub target_mismatch: bool,
    pub base_model: String,
    pub cand_model: String,
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
    let mut definition_changed = Vec::new();
    let mut low_resolution = Vec::new();
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
                // Empty hash = an old baseline predating this field, or a
                // hand-built ScenarioResult in a test — "unknown", not "the
                // same", but also not evidence of a real change, so skip.
                if !b.content_hash.is_empty()
                    && !c.content_hash.is_empty()
                    && b.content_hash != c.content_hash
                {
                    definition_changed.push(b.id.clone());
                }
                let base_rate = b.trial_pass_rate();
                let cand_rate = c.trial_pass_rate();
                let diff = cand_rate - base_rate;
                let regression = diff < -threshold;
                if regression {
                    regressions.push(b.id.clone());
                }
                // The smallest nonzero step candidate's own pass rate can
                // move by (1/n) — if that's already coarser than the
                // threshold, one trial flipping is indistinguishable from a
                // real regression at this k, no matter the threshold's
                // nominal value.
                let step = 1.0 / c.n_graded_trials().max(1) as f64;
                if step > threshold {
                    low_resolution.push(b.id.clone());
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
        definition_changed,
        low_resolution,
        target_mismatch: base.model != cand.model,
        base_model: base.model.clone(),
        cand_model: cand.model.clone(),
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
        "baseline : {} ({})    candidate: {} ({})",
        c.baseline_tag, c.base_model, c.candidate_tag, c.cand_model
    );
    if c.target_mismatch {
        println!(
            "⚠ comparing different targets — baseline was evaluated against \
             '{}', candidate against '{}'. A pass-rate diff here is an A/B \
             comparison, not a regression check.",
            c.base_model, c.cand_model
        );
    }
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
    if !c.definition_changed.is_empty() {
        println!(
            "\n⚠ scenario definition changed between baseline and candidate (the \
             diff above compares two different tests under the same id — see \
             AGENTS.md's guardrail on not moving the ruler):"
        );
        for id in &c.definition_changed {
            println!("  {id}");
        }
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
    if !c.low_resolution.is_empty() {
        println!(
            "\n⚠ threshold is finer than these scenarios' trial count can resolve \
             (any single trial flipping outcome is enough to call it a regression \
             here, regardless of the threshold's nominal value — raise --trials or \
             loosen --threshold if that's not what you want):"
        );
        for id in &c.low_resolution {
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
            judge_input_tokens: 0,
            judge_output_tokens: 0,
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
    fn warns_when_threshold_is_finer_than_trial_count_can_resolve() {
        // 3 graded trials can only move in steps of 1/3 ≈ 0.333 — a default
        // 0.05 threshold means any single trial flipping already counts as
        // a regression, which is worth flagging as not real statistical
        // headroom, independent of whether this comparison actually
        // regressed.
        let base = report(vec![ScenarioResult::from_trials(
            "s".into(),
            vec![trial(Final::Pass, 0), trial(Final::Pass, 0), trial(Final::Pass, 0)],
        )]);
        let cand = report(vec![ScenarioResult::from_trials(
            "s".into(),
            vec![trial(Final::Pass, 0), trial(Final::Pass, 0), trial(Final::Pass, 0)],
        )]);
        let c = compare(&base, &cand, 0.05);
        assert_eq!(c.low_resolution, vec!["s".to_string()]);
    }

    #[test]
    fn no_low_resolution_warning_when_threshold_fits_the_trial_count() {
        // Same 3 trials, but a threshold of 0.4 is coarser than the 0.333
        // step 3 trials can produce — no warning needed, the gate is
        // already tighter than what one flipped trial could trigger.
        let base = report(vec![ScenarioResult::from_trials(
            "s".into(),
            vec![trial(Final::Pass, 0), trial(Final::Pass, 0), trial(Final::Pass, 0)],
        )]);
        let cand = report(vec![ScenarioResult::from_trials(
            "s".into(),
            vec![trial(Final::Pass, 0), trial(Final::Pass, 0), trial(Final::Pass, 0)],
        )]);
        let c = compare(&base, &cand, 0.4);
        assert!(c.low_resolution.is_empty());
    }

    #[test]
    fn warns_when_shared_scenario_definition_changed() {
        // A scenario's own definition changing between baseline and
        // candidate is exactly "measuring yourself with a ruler you moved"
        // (AGENTS.md) — make it visible instead of silently comparing two
        // different tests under the same id.
        let mut base_result = ScenarioResult::from_trials("s".into(), vec![trial(Final::Pass, 0)]);
        base_result.content_hash = "aaaa".into();
        let mut cand_result = ScenarioResult::from_trials("s".into(), vec![trial(Final::Pass, 0)]);
        cand_result.content_hash = "bbbb".into();
        let base = report(vec![base_result]);
        let cand = report(vec![cand_result]);
        let c = compare(&base, &cand, 0.05);
        assert_eq!(c.definition_changed, vec!["s".to_string()]);
    }

    #[test]
    fn no_warning_when_hash_matches_or_is_unknown() {
        // Same hash: no warning. Empty hash on either side (an old
        // committed baseline predating this field): also no warning — a
        // migration must not manufacture false positives for every
        // pre-existing scenario.
        let mut same_a = ScenarioResult::from_trials("s".into(), vec![trial(Final::Pass, 0)]);
        same_a.content_hash = "aaaa".into();
        let mut same_b = ScenarioResult::from_trials("s".into(), vec![trial(Final::Pass, 0)]);
        same_b.content_hash = "aaaa".into();
        let c = compare(&report(vec![same_a]), &report(vec![same_b]), 0.05);
        assert!(c.definition_changed.is_empty());

        let mut old_baseline = ScenarioResult::from_trials("s".into(), vec![trial(Final::Pass, 0)]);
        old_baseline.content_hash = String::new();
        let mut fresh = ScenarioResult::from_trials("s".into(), vec![trial(Final::Pass, 0)]);
        fresh.content_hash = "aaaa".into();
        let c = compare(&report(vec![old_baseline]), &report(vec![fresh]), 0.05);
        assert!(c.definition_changed.is_empty());
    }

    #[test]
    fn warns_when_target_model_differs() {
        // Comparing an Anthropic baseline against an OpenRouter candidate
        // (or a different model on the same provider) must be visible, not
        // silently treated as an apples-to-apples regression check.
        let base = Report::build(
            "t".into(),
            "anthropic/claude-sonnet-4-6".into(),
            "b".into(),
            1,
            vec![ScenarioResult::from_trials(
                "s".into(),
                vec![trial(Final::Pass, 0)],
            )],
        );
        let cand = Report::build(
            "t".into(),
            "openrouter/some-model".into(),
            "b".into(),
            1,
            vec![ScenarioResult::from_trials(
                "s".into(),
                vec![trial(Final::Pass, 0)],
            )],
        );
        let c = compare(&base, &cand, 0.05);
        assert!(c.target_mismatch);
        assert_eq!(c.base_model, "anthropic/claude-sonnet-4-6");
        assert_eq!(c.cand_model, "openrouter/some-model");
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
