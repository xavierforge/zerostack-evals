//! Scenario x target table: a pure model built from `Report`s, plus two
//! renderers over the same model (fixed-width terminal, markdown for
//! records). `matrix` (the subcommand, target-matrix section 7) and `run`'s
//! end-of-run table (section 8) both go through [`build`] and the render
//! functions here — neither owns its own table logic.
//!
//! A cell is a scenario's `trial_pass_rate()` for one column, *if* that
//! column actually graded it: `trial_pass_rate()` alone cannot tell "ran, all
//! failed" (a real `0.0`) from "didn't run it" or "every trial was
//! indeterminate" (both also read as `0.0`). [`ScenarioResult::is_gradable`]
//! is what disambiguates, so every cell is either [`Cell::Rate`] or
//! [`Cell::Hole`], never a bare float.

use std::collections::HashMap;
use std::path::Path;

use crate::verdict::{Report, ScenarioResult};

/// One scenario's outcome in one column. Never a bare `f64` — see the module
/// doc on why "ran, all failed" and "didn't run/couldn't grade" must stay
/// distinguishable all the way to the rendered table.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Cell {
    Rate(f64),
    Hole,
}

/// `Report::judge_model`'s three states, carried through to the column
/// legend verbatim (see that field's doc for what each means).
#[derive(Debug, Clone, PartialEq)]
pub enum JudgeState {
    Unknown,
    NothingGraded,
    Rulers(Vec<String>),
}

/// Footer figures for one column, recomputed over the scenarios gradable in
/// *every* column (not the column's own full scenario set) — see
/// `Matrix::footer_excluded`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColumnFooter {
    pub pass_at_k: f64,
    pub pass_hat_k: f64,
    pub total_cost_usd: f64,
}

/// One target's column: the header carries only [`Column::label`] (the stem,
/// disambiguated when another column shares it); everything else here is
/// legend-only, per the design's "48 + 12N" width budget.
#[derive(Debug, Clone, PartialEq)]
pub struct Column {
    /// Target filename without extension, e.g. `"opus"`. Never disambiguated
    /// — use `label` for display.
    pub stem: String,
    /// What the header actually prints: `stem`, or `stem` plus a tag/
    /// timestamp suffix when another column in this table shares the stem.
    pub label: String,
    /// `Report.model` (full provider/model), legend-only.
    pub model: String,
    /// `Report.target` (the target path), legend-only.
    pub target: String,
    pub tag: String,
    pub timestamp: String,
    pub judge: JudgeState,
    pub footer: ColumnFooter,
}

/// One scenario's row: a cell per column, aligned by index with
/// `Matrix::columns`.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub id: String,
    pub cells: Vec<Cell>,
}

/// The scenario x target table. Built once by [`build`] from a set of
/// reports; both renderers below read it without touching a `Report` again.
#[derive(Debug, Clone, PartialEq)]
pub struct Matrix {
    pub columns: Vec<Column>,
    /// Rows in scenario-id order (the union of every column's scenarios,
    /// sorted), independent of which report happened to list a scenario
    /// first.
    pub rows: Vec<Row>,
    /// Scenario ids present in the row union but not gradable in *every*
    /// column — excluded from each column's `footer`, listed here so the
    /// exclusion is visible rather than silently narrowing the denominator.
    pub footer_excluded: Vec<String>,
}

/// Build the table from a set of reports, one per column, in the given
/// order. Pure and infallible: identity/overlap validation (a target-less
/// report, zero shared scenarios) is the caller's job — `matrix` (section 7)
/// and the N>1 `run` path (section 8) both have their own report in hand to
/// name in an error, which this function does not.
pub fn build(reports: &[&Report]) -> Matrix {
    let all_ids = union_sorted_scenario_ids(reports);
    let intersection: Vec<String> = all_ids
        .iter()
        .filter(|id| reports.iter().all(|r| gradable_scenario(r, id).is_some()))
        .cloned()
        .collect();
    let footer_excluded: Vec<String> = all_ids
        .iter()
        .filter(|id| !intersection.contains(id))
        .cloned()
        .collect();

    let labels = disambiguate_labels(reports);
    let columns = reports
        .iter()
        .zip(labels)
        .map(|(r, label)| Column {
            stem: crate::target::stem(Path::new(&r.target)),
            label,
            model: r.model.clone(),
            target: r.target.clone(),
            tag: r.tag.clone(),
            timestamp: r.timestamp.clone(),
            judge: judge_state(r),
            footer: column_footer(r, &intersection),
        })
        .collect();

    let rows = all_ids
        .iter()
        .map(|id| Row {
            id: id.clone(),
            cells: reports.iter().map(|r| cell(r, id)).collect(),
        })
        .collect();

    Matrix {
        columns,
        rows,
        footer_excluded,
    }
}

fn union_sorted_scenario_ids(reports: &[&Report]) -> Vec<String> {
    let mut ids: Vec<String> = reports
        .iter()
        .flat_map(|r| r.scenarios.iter().map(|s| s.id.clone()))
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

fn gradable_scenario<'a>(report: &'a Report, id: &str) -> Option<&'a ScenarioResult> {
    report
        .scenarios
        .iter()
        .find(|s| s.id == id && s.is_gradable())
}

fn cell(report: &Report, id: &str) -> Cell {
    match gradable_scenario(report, id) {
        Some(s) => Cell::Rate(s.trial_pass_rate()),
        None => Cell::Hole,
    }
}

fn judge_state(report: &Report) -> JudgeState {
    match &report.judge_model {
        None => JudgeState::Unknown,
        Some(v) if v.is_empty() => JudgeState::NothingGraded,
        Some(v) => JudgeState::Rulers(v.clone()),
    }
}

/// Footer figures for one column, over only the scenarios gradable in every
/// column (`intersection`) — never the column's own full scenario set, so
/// two columns are never compared on two different denominators (see the
/// module's footer requirement).
fn column_footer(report: &Report, intersection: &[String]) -> ColumnFooter {
    let gradable: Vec<&ScenarioResult> = report
        .scenarios
        .iter()
        .filter(|s| intersection.contains(&s.id) && s.is_gradable())
        .collect();
    let g = gradable.len().max(1) as f64;
    let pass_at_k = gradable.iter().map(|s| s.pass_at_k).sum::<f64>() / g;
    let pass_hat_k = gradable.iter().map(|s| s.pass_hat_k).sum::<f64>() / g;
    let total_cost_usd = report
        .scenarios
        .iter()
        .filter(|s| intersection.contains(&s.id))
        .flat_map(|s| s.trials.iter())
        .map(|t| t.cost_usd)
        .sum();
    ColumnFooter {
        pass_at_k: round4(pass_at_k),
        pass_hat_k: round4(pass_hat_k),
        total_cost_usd: round4(total_cost_usd),
    }
}

/// Two columns for the same target evaluated at different times are a
/// legitimate view ("did this target regress"), not a collision — so a
/// repeated stem is disambiguated by tag, or by timestamp when the tag also
/// repeats, rather than rejected.
fn disambiguate_labels(reports: &[&Report]) -> Vec<String> {
    let stems: Vec<String> = reports
        .iter()
        .map(|r| crate::target::stem(Path::new(&r.target)))
        .collect();

    let mut stem_counts: HashMap<&str, usize> = HashMap::new();
    for s in &stems {
        *stem_counts.entry(s.as_str()).or_insert(0) += 1;
    }

    let mut tag_counts: HashMap<(&str, &str), usize> = HashMap::new();
    for (r, s) in reports.iter().zip(&stems) {
        *tag_counts.entry((s.as_str(), r.tag.as_str())).or_insert(0) += 1;
    }

    reports
        .iter()
        .zip(&stems)
        .map(|(r, stem)| {
            if stem_counts[stem.as_str()] <= 1 {
                stem.clone()
            } else if tag_counts[&(stem.as_str(), r.tag.as_str())] <= 1 {
                format!("{stem}@{}", r.tag)
            } else {
                format!("{stem}@{}", r.timestamp)
            }
        })
        .collect()
}

fn round4(x: f64) -> f64 {
    (x * 10_000.0).round() / 10_000.0
}

fn format_cell(c: Cell) -> String {
    match c {
        Cell::Rate(r) => format!("{r:.3}"),
        Cell::Hole => "-".to_string(),
    }
}

fn format_judge(j: &JudgeState) -> String {
    match j {
        JudgeState::Unknown => "unknown".to_string(),
        JudgeState::NothingGraded => "nothing graded".to_string(),
        JudgeState::Rulers(v) => v.join(", "),
    }
}

const ID_COL: usize = 30;
const NUM_COL: usize = 12;

/// Fixed-width terminal table: the default `matrix` output and `run`'s
/// end-of-run stderr table (target-matrix sections 7-8).
pub fn render_fixed_width(m: &Matrix) -> String {
    let mut out = String::new();
    out.push_str(&format!("{:<ID_COL$}", "scenario"));
    for col in &m.columns {
        out.push_str(&format!("{:>NUM_COL$}", col.label));
    }
    out.push('\n');
    for row in &m.rows {
        out.push_str(&format!("{:<ID_COL$}", row.id));
        for cell in &row.cells {
            out.push_str(&format!("{:>NUM_COL$}", format_cell(*cell)));
        }
        out.push('\n');
    }
    out.push_str(&format!("{:<ID_COL$}", "pass@k"));
    for col in &m.columns {
        out.push_str(&format!("{:>NUM_COL$.3}", col.footer.pass_at_k));
    }
    out.push('\n');
    out.push_str(&format!("{:<ID_COL$}", "pass^k"));
    for col in &m.columns {
        out.push_str(&format!("{:>NUM_COL$.3}", col.footer.pass_hat_k));
    }
    out.push('\n');
    out.push_str(&format!("{:<ID_COL$}", "cost usd"));
    for col in &m.columns {
        out.push_str(&format!("{:>NUM_COL$.3}", col.footer.total_cost_usd));
    }
    out.push('\n');
    if !m.footer_excluded.is_empty() {
        out.push_str(&format!(
            "\nexcluded from footer (not gradable in every column): {}\n",
            m.footer_excluded.join(", ")
        ));
    }
    out.push_str(&render_legend_fixed_width(m));
    out
}

fn render_legend_fixed_width(m: &Matrix) -> String {
    let mut out = String::from("\nlegend:\n");
    for col in &m.columns {
        out.push_str(&format!(
            "  {:<16} model={:<32} target={:<28} judge={}\n",
            col.label,
            col.model,
            col.target,
            format_judge(&col.judge)
        ));
    }
    out
}

/// Markdown table: for `matrix --markdown` and `experiments/` snapshots (no
/// width limit, unlike the fixed-width renderer).
pub fn render_markdown(m: &Matrix) -> String {
    let mut out = String::new();
    out.push_str("| scenario |");
    for col in &m.columns {
        out.push_str(&format!(" {} |", col.label));
    }
    out.push('\n');
    out.push_str("|---|");
    for _ in &m.columns {
        out.push_str("---|");
    }
    out.push('\n');
    for row in &m.rows {
        out.push_str(&format!("| {} |", row.id));
        for cell in &row.cells {
            out.push_str(&format!(" {} |", format_cell(*cell)));
        }
        out.push('\n');
    }
    out.push_str("| pass@k |");
    for col in &m.columns {
        out.push_str(&format!(" {:.3} |", col.footer.pass_at_k));
    }
    out.push('\n');
    out.push_str("| pass^k |");
    for col in &m.columns {
        out.push_str(&format!(" {:.3} |", col.footer.pass_hat_k));
    }
    out.push('\n');
    out.push_str("| cost usd |");
    for col in &m.columns {
        out.push_str(&format!(" {:.3} |", col.footer.total_cost_usd));
    }
    out.push('\n');
    if !m.footer_excluded.is_empty() {
        out.push_str(&format!(
            "\nExcluded from footer (not gradable in every column): {}\n",
            m.footer_excluded.join(", ")
        ));
    }
    out.push_str("\n**Legend**\n\n");
    for col in &m.columns {
        out.push_str(&format!(
            "- `{}`: model={}, target={}, judge={}\n",
            col.label,
            col.model,
            col.target,
            format_judge(&col.judge)
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verdict::{Final, ReportMeta, TrialResult};

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
            cost_usd: 1.0,
            wall_secs: 0.0,
            tool_call_count: 0,
            run_dir: String::new(),
        }
    }

    fn report(target: &str, tag: &str, scenarios: Vec<ScenarioResult>) -> Report {
        Report::build(
            ReportMeta {
                tag: tag.into(),
                model: format!("anthropic/{tag}"),
                backend: "zs".into(),
                trials: 1,
                target: target.into(),
                ..Default::default()
            },
            scenarios,
        )
    }

    fn report_with_judge(
        target: &str,
        tag: &str,
        judge_model: Option<Vec<String>>,
        scenarios: Vec<ScenarioResult>,
    ) -> Report {
        Report::build(
            ReportMeta {
                tag: tag.into(),
                model: format!("anthropic/{tag}"),
                backend: "zs".into(),
                trials: 1,
                target: target.into(),
                judge_model,
                ..Default::default()
            },
            scenarios,
        )
    }

    // 5.1 — rows keyed by scenario id in id order, independent of the order
    // any one report listed them in.
    #[test]
    fn rows_are_keyed_by_scenario_id_in_id_order() {
        let a = report(
            "targets/opus.toml",
            "run-a",
            vec![
                ScenarioResult::from_trials("zebra".into(), vec![trial(Final::Pass)]),
                ScenarioResult::from_trials("apple".into(), vec![trial(Final::Pass)]),
            ],
        );
        let b = report(
            "targets/sonnet.toml",
            "run-b",
            vec![ScenarioResult::from_trials(
                "mango".into(),
                vec![trial(Final::Pass)],
            )],
        );
        let m = build(&[&a, &b]);
        let ids: Vec<&str> = m.rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["apple", "mango", "zebra"]);
    }

    // 5.1 — a graded all-fail cell is a real 0.000, distinct from a `-` hole
    // for a scenario a column never ran or never graded.
    #[test]
    fn graded_all_fail_is_a_real_zero_absent_or_indeterminate_is_a_hole() {
        let all_fail = report(
            "targets/opus.toml",
            "run-a",
            vec![ScenarioResult::from_trials(
                "s".into(),
                vec![trial(Final::Fail), trial(Final::Fail)],
            )],
        );
        let all_indeterminate = report(
            "targets/sonnet.toml",
            "run-b",
            vec![ScenarioResult::from_trials(
                "s".into(),
                vec![trial(Final::Indeterminate)],
            )],
        );
        let never_ran = report("targets/haiku.toml", "run-c", vec![]);

        let m = build(&[&all_fail, &all_indeterminate, &never_ran]);
        assert_eq!(m.rows.len(), 1);
        let cells = &m.rows[0].cells;
        assert_eq!(cells[0], Cell::Rate(0.0), "ran, every trial failed");
        assert_eq!(cells[1], Cell::Hole, "ran, but nothing gradable");
        assert_eq!(cells[2], Cell::Hole, "never ran the scenario at all");
    }

    // 5.3 — differing scenario sets: footer over the intersection, the rest
    // listed as excluded.
    #[test]
    fn footer_is_computed_over_the_intersection_and_lists_the_rest_as_excluded() {
        let a = report(
            "targets/opus.toml",
            "run-a",
            vec![
                ScenarioResult::from_trials("shared".into(), vec![trial(Final::Pass)]),
                ScenarioResult::from_trials("only-a".into(), vec![trial(Final::Pass)]),
            ],
        );
        let b = report(
            "targets/sonnet.toml",
            "run-b",
            vec![
                ScenarioResult::from_trials("shared".into(), vec![trial(Final::Fail)]),
                ScenarioResult::from_trials("only-b".into(), vec![trial(Final::Pass)]),
            ],
        );
        let m = build(&[&a, &b]);
        assert_eq!(
            m.footer_excluded,
            vec!["only-a".to_string(), "only-b".to_string()]
        );
        // Only "shared" feeds the footer: column a is all-pass on it, column
        // b is all-fail on it — neither column's own (larger) summary would
        // show these numbers.
        assert_eq!(m.columns[0].footer.pass_hat_k, 1.0);
        assert_eq!(m.columns[1].footer.pass_hat_k, 0.0);
    }

    // 5.3 — identical suites: the footer equals each report's own summary,
    // nothing excluded.
    #[test]
    fn footer_matches_each_reports_own_summary_when_suites_are_identical() {
        let scenarios = || {
            vec![
                ScenarioResult::from_trials("one".into(), vec![trial(Final::Pass)]),
                ScenarioResult::from_trials("two".into(), vec![trial(Final::Fail)]),
            ]
        };
        let a = report("targets/opus.toml", "run-a", scenarios());
        let b = report("targets/sonnet.toml", "run-b", scenarios());
        let m = build(&[&a, &b]);
        assert!(m.footer_excluded.is_empty());
        assert_eq!(m.columns[0].footer.pass_at_k, a.summary.pass_at_k);
        assert_eq!(m.columns[0].footer.pass_hat_k, a.summary.pass_hat_k);
        assert_eq!(m.columns[0].footer.total_cost_usd, a.summary.total_cost_usd);
        assert_eq!(m.columns[1].footer.pass_at_k, b.summary.pass_at_k);
        assert_eq!(m.columns[1].footer.pass_hat_k, b.summary.pass_hat_k);
        assert_eq!(m.columns[1].footer.total_cost_usd, b.summary.total_cost_usd);
    }

    // 5.4 — header carries only the stem; legend carries model/target/judge.
    #[test]
    fn column_identity_puts_the_stem_in_the_header_and_the_rest_in_the_legend() {
        let r = report(
            "targets/opus.toml",
            "run-a",
            vec![ScenarioResult::from_trials(
                "s".into(),
                vec![trial(Final::Pass)],
            )],
        );
        let m = build(&[&r]);
        let col = &m.columns[0];
        assert_eq!(col.stem, "opus");
        assert_eq!(col.label, "opus");
        assert_eq!(col.model, "anthropic/run-a");
        assert_eq!(col.target, "targets/opus.toml");
    }

    // 5.4 — the three judge_model states must render distinguishably.
    #[test]
    fn the_three_judge_states_render_distinguishably_in_the_legend() {
        let unknown = report_with_judge("targets/opus.toml", "unknown-run", None, vec![]);
        let nothing = report_with_judge("targets/sonnet.toml", "nothing-run", Some(vec![]), vec![]);
        let ruled = report_with_judge(
            "targets/haiku.toml",
            "ruled-run",
            Some(vec!["claude-opus-4-8".into()]),
            vec![],
        );
        let m = build(&[&unknown, &nothing, &ruled]);
        assert_eq!(m.columns[0].judge, JudgeState::Unknown);
        assert_eq!(m.columns[1].judge, JudgeState::NothingGraded);
        assert_eq!(
            m.columns[2].judge,
            JudgeState::Rulers(vec!["claude-opus-4-8".into()])
        );

        let legend = render_legend_fixed_width(&m);
        assert!(legend.contains("judge=unknown"));
        assert!(legend.contains("judge=nothing graded"));
        assert!(legend.contains("judge=claude-opus-4-8"));
    }

    // 5.5 — two reports for the same target from different runs must both
    // render, disambiguated rather than colliding.
    #[test]
    fn same_stem_columns_from_different_runs_are_both_present_and_distinctly_labelled() {
        let earlier = report(
            "targets/opus.toml",
            "run-earlier",
            vec![ScenarioResult::from_trials(
                "s".into(),
                vec![trial(Final::Pass)],
            )],
        );
        let later = report(
            "targets/opus.toml",
            "run-later",
            vec![ScenarioResult::from_trials(
                "s".into(),
                vec![trial(Final::Fail)],
            )],
        );
        let m = build(&[&earlier, &later]);
        assert_eq!(m.columns.len(), 2);
        assert_ne!(m.columns[0].label, m.columns[1].label);
        assert!(m.columns[0].label.starts_with("opus"));
        assert!(m.columns[1].label.starts_with("opus"));
        assert_eq!(m.columns[0].stem, "opus");
        assert_eq!(m.columns[1].stem, "opus");
    }

    // 5.6 — both renderers over one fixture.
    #[test]
    fn both_renderers_render_the_same_model() {
        let a = report(
            "targets/opus.toml",
            "run-a",
            vec![ScenarioResult::from_trials(
                "greet".into(),
                vec![trial(Final::Pass), trial(Final::Fail)],
            )],
        );
        let b = report("targets/sonnet.toml", "run-b", vec![]);
        let m = build(&[&a, &b]);

        let fixed = render_fixed_width(&m);
        assert!(fixed.contains("opus"));
        assert!(fixed.contains("sonnet"));
        assert!(fixed.contains("greet"));
        assert!(fixed.contains("0.500"));
        assert!(fixed.contains("-"), "the never-run column shows a hole");
        assert!(fixed.contains("legend:"));

        let md = render_markdown(&m);
        assert!(md.contains("| scenario |"));
        assert!(md.contains("| greet |"));
        assert!(md.contains("0.500"));
        assert!(md.contains("**Legend**"));
        assert_ne!(fixed, md);
    }
}
