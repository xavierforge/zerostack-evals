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
    /// inherently wrong (that's a migration gate: deciding whether to switch
    /// from baseline to candidate), but it must never be silent when the
    /// caller expected an apples-to-apples regression check. Comparing N
    /// targets side by side belongs to `zseval matrix`, not here.
    pub target_mismatch: bool,
    pub base_model: String,
    pub cand_model: String,
    /// See `Report::prompts_pack` / `Report::prompts_hash`. Empty when that
    /// side used no pack.
    pub base_prompts_pack: String,
    pub base_prompts_hash: String,
    pub cand_prompts_pack: String,
    pub cand_prompts_hash: String,
    /// Whether baseline and candidate recorded different pack fingerprints —
    /// the hash alone, never the pack path (`PromptPack::fingerprint`
    /// deliberately excludes the directory, so a moved-but-identical pack is
    /// one identity). Includes one side having a pack and the other none, an
    /// empty hash differing from any real one. Reports now record the
    /// zerostack build identity too (`zs_bin_sha256`, see `zs_mismatch`), so a
    /// same-build pack difference is in principle a clean single-variable
    /// experiment — but this field stays conservative regardless, marking
    /// every pack difference until Day-2 baseline data justifies relaxing it
    /// (see `docs/adr/0001-compare-always-warns-matrix-owns-multivar.md`).
    /// Never affects `exit_code`, same as `target_mismatch`.
    pub pack_mismatch: bool,
    pub summary_base_pass_hat_k: f64,
    pub summary_cand_pass_hat_k: f64,
    pub summary_base_cost: f64,
    pub summary_cand_cost: f64,
    /// Whether baseline and candidate recorded different `zs_bin_sha256`
    /// values. Same version string, different hash still counts — that is
    /// exactly the 07-26 stale-binary incident (`zerostack 1.7.1` printed by
    /// two different binaries) this warning exists to catch mechanically.
    /// Retires `controlled-variables`' former "build is always moved, for
    /// now" assumption: the build is now an observed variable like target
    /// and pack (see
    /// `docs/adr/0001-compare-always-warns-matrix-owns-multivar.md`). Never
    /// affects `exit_code`, same as every other warning here.
    pub zs_mismatch: bool,
    pub base_zs_version: String,
    pub base_zs_bin_sha256: String,
    pub cand_zs_version: String,
    pub cand_zs_bin_sha256: String,
    /// Whether the baseline (resp. candidate) run stopped early on
    /// `--max-total-usd` before every declared scenario ran — see
    /// `Report::budget_truncated`. The truncated side's missing scenarios
    /// already surface in `added`/`removed`; this pair of fields is what lets
    /// the warning supply the *why*. Never affects `exit_code`.
    pub base_budget_truncated: bool,
    pub cand_budget_truncated: bool,
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
                if b.content_hash != c.content_hash {
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
        base_prompts_pack: base.prompts_pack.clone(),
        base_prompts_hash: base.prompts_hash.clone(),
        cand_prompts_pack: cand.prompts_pack.clone(),
        cand_prompts_hash: cand.prompts_hash.clone(),
        // Identity is the fingerprint hash alone, never the pack path:
        // `PromptPack::fingerprint` deliberately excludes the directory, so a
        // byte-identical pack that merely moved between runs stays one identity
        // and is not flagged as a different pack.
        pack_mismatch: base.prompts_hash != cand.prompts_hash,
        summary_base_pass_hat_k: base.summary.pass_hat_k,
        summary_cand_pass_hat_k: cand.summary.pass_hat_k,
        summary_base_cost: base.summary.total_cost_usd,
        summary_cand_cost: cand.summary.total_cost_usd,
        // The hash alone is the identity check — same version, different
        // hash still counts (the 07-26 stale-binary incident).
        zs_mismatch: base.zs_bin_sha256 != cand.zs_bin_sha256,
        base_zs_version: base.zs_version.clone(),
        base_zs_bin_sha256: base.zs_bin_sha256.clone(),
        cand_zs_version: cand.zs_version.clone(),
        cand_zs_bin_sha256: cand.zs_bin_sha256.clone(),
        base_budget_truncated: base.budget_truncated,
        cand_budget_truncated: cand.budget_truncated,
    }
}

/// Which side(s) recorded `budget_truncated`, in prose, for the truncation
/// warning — `None` when neither side was truncated.
fn truncated_sides(base: bool, cand: bool) -> Option<&'static str> {
    match (base, cand) {
        (true, true) => Some("baseline and candidate"),
        (true, false) => Some("baseline"),
        (false, true) => Some("candidate"),
        (false, false) => None,
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

/// `path#hash` display for one side's pack identity (`prompts-pack-identity`
/// spec: "Pack identity is displayed wherever it can differ", example
/// `prompts=my-pack#a3f1`), or the plain `none` marker when that side used no
/// pack — shown rather than omitted, so a packless side reads as a fact, not
/// a blank.
pub(crate) fn pack_identity(pack: &str, hash: &str) -> String {
    if pack.is_empty() {
        "none".to_string()
    } else {
        format!("{pack}#{}", short_hash(hash))
    }
}

/// First 4 hex chars of a `util::fnv1a_hex` fingerprint (16 chars) — enough
/// to distinguish by eye without printing the full hash on every comparison
/// line, matching the length used in the spec's own example.
fn short_hash(hash: &str) -> &str {
    &hash[..hash.len().min(4)]
}

pub fn print_human(c: &Comparison) {
    println!(
        "baseline : {} ({})    candidate: {} ({})",
        c.baseline_tag, c.base_model, c.candidate_tag, c.cand_model
    );
    println!(
        "prompts  : baseline={}    candidate={}",
        pack_identity(&c.base_prompts_pack, &c.base_prompts_hash),
        pack_identity(&c.cand_prompts_pack, &c.cand_prompts_hash)
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

    print_warnings(c);

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

/// Every comparability warning `compare` can raise, in one fixed order. This
/// is the "single render block" structural anchor from the "compare always
/// warns" ADR (`docs/adr/0001-compare-always-warns-matrix-owns-multivar.md`):
/// adding a warning means adding a list entry here, never a new branch in
/// `exit_code()`, which reads none of these fields.
fn print_warnings(c: &Comparison) {
    if c.pack_mismatch {
        println!(
            "\n⚠ comparing different prompt packs — baseline used '{}', \
             candidate used '{}'. A pass-rate diff here is a prompt A/B, not \
             a regression check.",
            pack_identity(&c.base_prompts_pack, &c.base_prompts_hash),
            pack_identity(&c.cand_prompts_pack, &c.cand_prompts_hash)
        );
    }
    if c.target_mismatch {
        println!(
            "\n⚠ comparing different targets — baseline was evaluated against \
             '{}', candidate against '{}'. A pass-rate diff here is a \
             migration gate (deciding whether to switch targets), not a \
             regression check. For a scenario x target table, use `zseval \
             matrix`.",
            c.base_model, c.cand_model
        );
    }
    if c.zs_mismatch {
        println!(
            "\n⚠ comparing different zerostack builds — baseline used '{}#{}', \
             candidate used '{}#{}'. A pass-rate diff here may reflect the \
             build that moved, not the agent (this is the 07-26 stale-binary \
             incident, now caught mechanically).",
            c.base_zs_version,
            short_hash(&c.base_zs_bin_sha256),
            c.cand_zs_version,
            short_hash(&c.cand_zs_bin_sha256)
        );
    }
    if let Some(sides) = truncated_sides(c.base_budget_truncated, c.cand_budget_truncated) {
        println!(
            "\n⚠ {sides} budget-truncated — the cost cap stopped that side \
             before every declared scenario ran, so its denominator is \
             smaller than it looks (see added/removed above for which \
             scenarios are missing)."
        );
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
}

#[cfg(test)]
mod pack_identity_tests {
    use super::*;
    use crate::verdict::{Final, Report, ReportMeta, ScenarioResult, TrialResult};

    fn trial() -> TrialResult {
        TrialResult {
            trial: 0,
            outcome: Final::Pass,
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

    fn report_with_pack(pack: &str, hash: &str) -> Report {
        Report::build(
            ReportMeta {
                tag: "t".into(),
                model: "m".into(),
                backend: "b".into(),
                trials: 1,
                prompts_pack: pack.into(),
                prompts_hash: hash.into(),
                ..Default::default()
            },
            vec![ScenarioResult::from_trials("s".into(), vec![trial()])],
        )
    }

    // 8.1 — compare's header line carries each side's pack identity as path
    // plus short hash, mirroring how `target_mismatch` carries `base_model`
    // / `cand_model` as raw fields for `print_human` to format.
    #[test]
    fn compare_carries_each_sides_pack_identity_fields() {
        let base = report_with_pack("packs/a", "aaaaaaaaaaaaaaaa");
        let cand = report_with_pack("packs/b", "bbbbbbbbbbbbbbbb");
        let c = compare(&base, &cand, 0.05);
        assert_eq!(c.base_prompts_pack, "packs/a");
        assert_eq!(c.base_prompts_hash, "aaaaaaaaaaaaaaaa");
        assert_eq!(c.cand_prompts_pack, "packs/b");
        assert_eq!(c.cand_prompts_hash, "bbbbbbbbbbbbbbbb");
    }

    #[test]
    fn pack_identity_shows_path_and_short_hash() {
        assert_eq!(pack_identity("my-pack", "a3f1000000000000"), "my-pack#a3f1");
    }

    #[test]
    fn pack_identity_shows_plain_marker_when_no_pack() {
        assert_eq!(pack_identity("", ""), "none");
    }

    // 8.2 — the note fires exactly when the two sides' pack identities
    // differ, following the `target_mismatch` precedent of a boolean field
    // `print_human` reads rather than asserting on printed text.
    #[test]
    fn pack_mismatch_true_when_pack_identities_differ() {
        let base = report_with_pack("packs/a", "aaaaaaaaaaaaaaaa");
        let cand = report_with_pack("packs/b", "bbbbbbbbbbbbbbbb");
        assert!(compare(&base, &cand, 0.05).pack_mismatch);
    }

    #[test]
    fn pack_mismatch_false_when_identical_or_both_packless() {
        let same_a = report_with_pack("packs/a", "aaaaaaaaaaaaaaaa");
        let same_b = report_with_pack("packs/a", "aaaaaaaaaaaaaaaa");
        assert!(!compare(&same_a, &same_b, 0.05).pack_mismatch);

        let none_a = report_with_pack("", "");
        let none_b = report_with_pack("", "");
        assert!(!compare(&none_a, &none_b, 0.05).pack_mismatch);
    }

    // Identity is the fingerprint hash, never the path: a byte-identical pack
    // that merely moved between runs (same hash, different path) is one pack,
    // not a different one, so a legitimate regression comparison is not flagged.
    #[test]
    fn pack_mismatch_false_when_same_hash_moved_path() {
        let base = report_with_pack("my-pack", "aaaaaaaaaaaaaaaa");
        let cand = report_with_pack("packs/my-pack", "aaaaaaaaaaaaaaaa");
        assert!(!compare(&base, &cand, 0.05).pack_mismatch);
    }

    // 8.3 — the note must never move the exit code: a pack difference with
    // no regression still exits 0.
    #[test]
    fn pack_mismatch_does_not_change_exit_code() {
        let base = report_with_pack("packs/a", "aaaaaaaaaaaaaaaa");
        let cand = report_with_pack("packs/b", "bbbbbbbbbbbbbbbb");
        let c = compare(&base, &cand, 0.05);
        assert!(c.pack_mismatch);
        assert_eq!(c.exit_code(), 0);
    }
}

#[cfg(test)]
mod exit_code_tests {
    use super::*;
    use crate::verdict::{Final, Report, ReportMeta, ScenarioResult, TrialResult};

    fn trial(outcome: Final, tool_call_count: usize) -> TrialResult {
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
            tool_call_count,
            run_dir: String::new(),
        }
    }

    fn report(scenarios: Vec<ScenarioResult>) -> Report {
        report_for_model("m", scenarios)
    }

    fn report_for_model(model: &str, scenarios: Vec<ScenarioResult>) -> Report {
        Report::build(
            ReportMeta {
                tag: "t".into(),
                model: model.into(),
                backend: "b".into(),
                trials: 1,
                ..Default::default()
            },
            scenarios,
        )
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
            vec![
                trial(Final::Pass, 0),
                trial(Final::Pass, 0),
                trial(Final::Pass, 0),
            ],
        )]);
        let cand = report(vec![ScenarioResult::from_trials(
            "s".into(),
            vec![
                trial(Final::Pass, 0),
                trial(Final::Pass, 0),
                trial(Final::Pass, 0),
            ],
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
            vec![
                trial(Final::Pass, 0),
                trial(Final::Pass, 0),
                trial(Final::Pass, 0),
            ],
        )]);
        let cand = report(vec![ScenarioResult::from_trials(
            "s".into(),
            vec![
                trial(Final::Pass, 0),
                trial(Final::Pass, 0),
                trial(Final::Pass, 0),
            ],
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
    fn no_warning_when_hash_matches() {
        // Same hash: no warning — whether both sides recorded a real hash or
        // (a hand-built `ScenarioResult` in a test) both left it empty. S7
        // removed the old empty-hash skip branch (current runs always hash,
        // so there is no longer a pre-field baseline to tolerate); plain
        // inequality already gives the right answer here since equal hashes
        // are equal whether or not they happen to be `""`.
        let mut same_a = ScenarioResult::from_trials("s".into(), vec![trial(Final::Pass, 0)]);
        same_a.content_hash = "aaaa".into();
        let mut same_b = ScenarioResult::from_trials("s".into(), vec![trial(Final::Pass, 0)]);
        same_b.content_hash = "aaaa".into();
        let c = compare(&report(vec![same_a]), &report(vec![same_b]), 0.05);
        assert!(c.definition_changed.is_empty());
    }

    #[test]
    fn warns_when_target_model_differs() {
        // Comparing an Anthropic baseline against an OpenRouter candidate
        // (or a different model on the same provider) must be visible, not
        // silently treated as an apples-to-apples regression check.
        let base = report_for_model(
            "anthropic/claude-sonnet-4-6",
            vec![ScenarioResult::from_trials(
                "s".into(),
                vec![trial(Final::Pass, 0)],
            )],
        );
        let cand = report_for_model(
            "openrouter/some-model",
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

#[cfg(test)]
mod warning_tests {
    use super::*;
    use crate::verdict::{Final, Report, ReportMeta, ScenarioResult, TrialResult, ZsIdentity};

    fn trial(outcome: Final, tool_call_count: usize) -> TrialResult {
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
            tool_call_count,
            run_dir: String::new(),
        }
    }

    fn passing_scenario(id: &str) -> ScenarioResult {
        ScenarioResult::from_trials(id.into(), vec![trial(Final::Pass, 0)])
    }

    /// A `Report` with every field this module's tests need to vary: model
    /// (target), pack identity, zerostack identity, and truncation. Mirrors
    /// `report_with_pack`/`report_for_model` in the sibling test modules —
    /// this module needs more knobs at once, so it gets its own builder
    /// rather than stacking theirs.
    #[allow(clippy::too_many_arguments)]
    fn full_report(
        model: &str,
        prompts_pack: &str,
        prompts_hash: &str,
        budget_truncated: bool,
        zs_version: &str,
        zs_bin_sha256: &str,
        scenarios: Vec<ScenarioResult>,
    ) -> Report {
        Report::build(
            ReportMeta {
                tag: "t".into(),
                model: model.into(),
                backend: "b".into(),
                trials: 1,
                budget_truncated,
                prompts_pack: prompts_pack.into(),
                prompts_hash: prompts_hash.into(),
                zs: ZsIdentity {
                    zs_version: zs_version.into(),
                    zs_bin_sha256: zs_bin_sha256.into(),
                    ..Default::default()
                },
                ..Default::default()
            },
            scenarios,
        )
    }

    // --- truncation: fires when either side (or both) recorded
    // budget_truncated, naming the side(s); never moves the exit code. ---

    #[test]
    fn truncated_sides_names_baseline_only() {
        assert_eq!(truncated_sides(true, false), Some("baseline"));
    }

    #[test]
    fn truncated_sides_names_candidate_only() {
        assert_eq!(truncated_sides(false, true), Some("candidate"));
    }

    #[test]
    fn truncated_sides_names_both() {
        assert_eq!(truncated_sides(true, true), Some("baseline and candidate"));
    }

    #[test]
    fn truncated_sides_is_none_when_neither_side_truncated() {
        assert_eq!(truncated_sides(false, false), None);
    }

    #[test]
    fn compare_carries_each_sides_budget_truncated_flag() {
        let base = full_report(
            "m",
            "",
            "",
            true,
            "zerostack 1.7.1",
            "hash",
            vec![passing_scenario("s")],
        );
        let cand = full_report(
            "m",
            "",
            "",
            false,
            "zerostack 1.7.1",
            "hash",
            vec![passing_scenario("s")],
        );
        let c = compare(&base, &cand, 0.05);
        assert!(c.base_budget_truncated);
        assert!(!c.cand_budget_truncated);
        assert_eq!(c.exit_code(), 0);
    }

    #[test]
    fn truncation_coexists_with_a_regression_exit_code() {
        let base = full_report(
            "m",
            "",
            "",
            true,
            "zerostack 1.7.1",
            "hash",
            vec![passing_scenario("s")],
        );
        let cand = full_report(
            "m",
            "",
            "",
            true,
            "zerostack 1.7.1",
            "hash",
            vec![ScenarioResult::from_trials(
                "s".into(),
                vec![trial(Final::Fail, 0)],
            )],
        );
        let c = compare(&base, &cand, 0.05);
        assert!(c.base_budget_truncated && c.cand_budget_truncated);
        assert_eq!(c.exit_code(), 1);
    }

    // --- zs_mismatch: fires on differing zs_bin_sha256, including
    // same-version-different-hash; quiet on identical. ---

    #[test]
    fn zs_mismatch_true_on_same_version_different_hash() {
        // The 07-26 incident, caught mechanically: both sides print
        // `zerostack 1.7.1` but ran different binaries.
        let base = full_report(
            "m",
            "",
            "",
            false,
            "zerostack 1.7.1",
            "hash-a",
            vec![passing_scenario("s")],
        );
        let cand = full_report(
            "m",
            "",
            "",
            false,
            "zerostack 1.7.1",
            "hash-b",
            vec![passing_scenario("s")],
        );
        let c = compare(&base, &cand, 0.05);
        assert!(c.zs_mismatch);
        assert_eq!(c.exit_code(), 0);
    }

    #[test]
    fn zs_mismatch_true_on_differing_version_and_hash() {
        let base = full_report(
            "m",
            "",
            "",
            false,
            "zerostack 1.7.1",
            "hash-a",
            vec![passing_scenario("s")],
        );
        let cand = full_report(
            "m",
            "",
            "",
            false,
            "zerostack 1.7.2",
            "hash-b",
            vec![passing_scenario("s")],
        );
        let c = compare(&base, &cand, 0.05);
        assert!(c.zs_mismatch);
    }

    #[test]
    fn zs_mismatch_false_when_hash_identical() {
        let base = full_report(
            "m",
            "",
            "",
            false,
            "zerostack 1.7.1",
            "hash-a",
            vec![passing_scenario("s")],
        );
        let cand = full_report(
            "m",
            "",
            "",
            false,
            "zerostack 1.7.1",
            "hash-a",
            vec![passing_scenario("s")],
        );
        let c = compare(&base, &cand, 0.05);
        assert!(!c.zs_mismatch);
    }

    // --- the all-warnings-lit invariant: exit_code() must not move,
    // regardless of how many comparability warnings are lit. ---

    #[test]
    fn all_warnings_lit_leaves_exit_code_unchanged() {
        // One scenario whose hash, evidence, and trial count differ enough to
        // trip definition_changed, evidence_warnings, and low_resolution all
        // at once; report-level fields differ enough to trip target_mismatch,
        // pack_mismatch, zs_mismatch, and the truncation warning.
        let mut base_s = ScenarioResult::from_trials("s".into(), vec![trial(Final::Pass, 1)]);
        base_s.content_hash = "aaaa".into();
        let mut cand_s = ScenarioResult::from_trials("s".into(), vec![trial(Final::Pass, 0)]);
        cand_s.content_hash = "bbbb".into();

        let lit_base = full_report(
            "model-a",
            "packs/a",
            "aaaaaaaaaaaaaaaa",
            true,
            "zerostack 1.7.1",
            "buildhash-a",
            vec![base_s],
        );
        let lit_cand = full_report(
            "model-b",
            "packs/b",
            "bbbbbbbbbbbbbbbb",
            false,
            "zerostack 1.7.1",
            "buildhash-b",
            vec![cand_s],
        );
        let lit = compare(&lit_base, &lit_cand, 0.05);

        assert!(lit.target_mismatch);
        assert!(lit.pack_mismatch);
        assert!(lit.zs_mismatch);
        assert!(lit.base_budget_truncated && !lit.cand_budget_truncated);
        assert_eq!(lit.definition_changed, vec!["s".to_string()]);
        assert_eq!(lit.evidence_warnings, vec!["s".to_string()]);
        assert_eq!(lit.low_resolution, vec!["s".to_string()]);
        assert!(lit.regressions.is_empty());
        assert_eq!(lit.exit_code(), 0);

        // Same shared scenario, no regression, but every warning quiet:
        // matching hash, matching evidence, enough trials to clear the
        // resolution floor, matching model/pack/zs/truncation on both sides.
        let quiet_trials = || (0..25).map(|_| trial(Final::Pass, 1)).collect::<Vec<_>>();
        let mut quiet_base_s = ScenarioResult::from_trials("s".into(), quiet_trials());
        quiet_base_s.content_hash = "same".into();
        let mut quiet_cand_s = ScenarioResult::from_trials("s".into(), quiet_trials());
        quiet_cand_s.content_hash = "same".into();

        let quiet_base = full_report(
            "m",
            "packs/a",
            "aaaaaaaaaaaaaaaa",
            false,
            "zerostack 1.7.1",
            "buildhash-a",
            vec![quiet_base_s],
        );
        let quiet_cand = full_report(
            "m",
            "packs/a",
            "aaaaaaaaaaaaaaaa",
            false,
            "zerostack 1.7.1",
            "buildhash-a",
            vec![quiet_cand_s],
        );
        let quiet = compare(&quiet_base, &quiet_cand, 0.05);

        assert!(!quiet.target_mismatch);
        assert!(!quiet.pack_mismatch);
        assert!(!quiet.zs_mismatch);
        assert!(!quiet.base_budget_truncated && !quiet.cand_budget_truncated);
        assert!(quiet.definition_changed.is_empty());
        assert!(quiet.evidence_warnings.is_empty());
        assert!(quiet.low_resolution.is_empty());
        assert!(quiet.regressions.is_empty());

        assert_eq!(lit.exit_code(), quiet.exit_code());
    }
}
