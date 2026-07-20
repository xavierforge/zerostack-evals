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
    /// This column ran fewer scenarios than the suite defined (the widest
    /// scenario set observed across this table's columns) — the visible
    /// trace of a budget-truncated run (design.md, "Budget is one shared
    /// total; truncation is marked").
    pub incomplete: bool,
    /// This column's `judge_hash` differs from another column's, or this
    /// column's judge is unknown — the measuring stick may have moved.
    /// SPREAD/DRIFT are display heuristics, not statistical or authoritative
    /// claims (design.md, "DRIFT marks, never adjudicates").
    pub judge_drift: bool,
}

/// One `content_hash` grouping within a row's DRIFT mark: the columns (and
/// their timestamps) that agreed on this hash. No group is "correct" — DRIFT
/// marks, it never adjudicates.
#[derive(Debug, Clone, PartialEq)]
pub struct DriftGroup {
    pub hash: String,
    /// Column labels sharing `hash`, aligned index-for-index with `timestamps`.
    pub columns: Vec<String>,
    pub timestamps: Vec<String>,
}

/// One scenario's row: a cell per column, aligned by index with
/// `Matrix::columns`.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub id: String,
    pub cells: Vec<Cell>,
    /// The gradable cells' gap (max - min) exceeds one trial's resolution
    /// (`1 / min(n_graded_trials)`) over those cells — a display heuristic,
    /// not a statistical claim (design.md, "SPREAD is data-derived and
    /// hole-safe").
    pub spread: bool,
    /// `content_hash` disagreement across columns that graded this scenario,
    /// grouped by hash. Empty when every column that graded it agrees (or
    /// too few columns carry a known hash to compare).
    pub drift: Vec<DriftGroup>,
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
    let judge_drift_flags = judge_drift_flags(reports);
    let columns = reports
        .iter()
        .zip(&labels)
        .zip(judge_drift_flags)
        .map(|((r, label), judge_drift)| Column {
            stem: crate::target::stem(Path::new(&r.target)),
            label: label.clone(),
            model: r.model.clone(),
            target: r.target.clone(),
            tag: r.tag.clone(),
            timestamp: r.timestamp.clone(),
            judge: judge_state(r),
            footer: column_footer(r, &intersection),
            incomplete: r.scenarios.len() < all_ids.len(),
            judge_drift,
        })
        .collect();

    let rows = all_ids
        .iter()
        .map(|id| Row {
            id: id.clone(),
            cells: reports.iter().map(|r| cell(r, id)).collect(),
            spread: spread_for_row(reports, id),
            drift: drift_for_row(reports, &labels, id),
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

/// SPREAD threshold: `1 / min(n_graded_trials)` over this row's gradable
/// cells. `-` cells are excluded from max, min, and the threshold — both to
/// avoid a divide-by-zero on a hole-only row and because a hole is "did not
/// run", not "ran and disagreed" (design.md, "SPREAD is data-derived and
/// hole-safe").
fn spread_for_row(reports: &[&Report], id: &str) -> bool {
    let gradable: Vec<(f64, usize)> = reports
        .iter()
        .filter_map(|r| {
            gradable_scenario(r, id).map(|s| (s.trial_pass_rate(), s.n_graded_trials()))
        })
        .collect();
    if gradable.len() < 2 {
        return false;
    }
    let max = gradable
        .iter()
        .map(|(rate, _)| *rate)
        .fold(f64::MIN, f64::max);
    let min = gradable
        .iter()
        .map(|(rate, _)| *rate)
        .fold(f64::MAX, f64::min);
    let min_n = gradable.iter().map(|(_, n)| *n).min().unwrap_or(1).max(1);
    let threshold = 1.0 / min_n as f64;
    (max - min) > threshold
}

/// Per-row DRIFT: `content_hash` mismatch across the columns that graded
/// this scenario, grouped by hash. An empty `content_hash` (a baseline
/// predating the field) is "unknown, skip" — same precedent as
/// `ScenarioResult::content_hash`'s own doc — never treated as a mismatch on
/// its own. No group is named "correct" (design.md, "DRIFT marks, never
/// adjudicates").
fn drift_for_row(reports: &[&Report], labels: &[String], id: &str) -> Vec<DriftGroup> {
    let entries: Vec<(String, String, String)> = reports
        .iter()
        .zip(labels)
        .filter_map(|(r, label)| {
            let s = gradable_scenario(r, id)?;
            if s.content_hash.is_empty() {
                return None;
            }
            Some((s.content_hash.clone(), label.clone(), r.timestamp.clone()))
        })
        .collect();

    let distinct: std::collections::HashSet<&str> =
        entries.iter().map(|(hash, ..)| hash.as_str()).collect();
    if distinct.len() <= 1 {
        return vec![];
    }

    let mut groups: Vec<DriftGroup> = vec![];
    for (hash, label, timestamp) in entries {
        match groups.iter_mut().find(|g| g.hash == hash) {
            Some(g) => {
                g.columns.push(label);
                g.timestamps.push(timestamp);
            }
            None => groups.push(DriftGroup {
                hash,
                columns: vec![label],
                timestamps: vec![timestamp],
            }),
        }
    }
    groups
}

/// Per-column DRIFT: this column's `judge_hash` differs from another
/// column's known hash, or this column's judge is unknown outright. Symmetric
/// on a real mismatch (design.md: DRIFT never picks a "correct" column), so
/// two disagreeing columns are both marked.
fn judge_drift_flags(reports: &[&Report]) -> Vec<bool> {
    reports
        .iter()
        .enumerate()
        .map(|(i, r)| {
            r.judge_hash.is_none()
                || reports.iter().enumerate().any(|(j, other)| {
                    j != i && other.judge_hash.is_some() && other.judge_hash != r.judge_hash
                })
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

/// Header text for one column: its label plus a trailing `*` when the
/// column is incomplete (ran fewer scenarios than the suite defines) — the
/// visible trace of a budget-truncated run.
fn column_header(col: &Column) -> String {
    if col.incomplete {
        format!("{}*", col.label)
    } else {
        col.label.clone()
    }
}

/// Trailing annotation for one row: `SPREAD` when the row's gap exceeds its
/// resolution threshold, `DRIFT[...]` listing the hash groups when this
/// scenario's content differs across columns. Both are display heuristics —
/// see the legend caveat in [`render_legend_fixed_width`].
fn row_marks(row: &Row) -> String {
    let mut marks = String::new();
    if row.spread {
        marks.push_str("  SPREAD");
    }
    if !row.drift.is_empty() {
        let groups: Vec<String> = row
            .drift
            .iter()
            .map(|g| format!("{}:{}", &g.hash[..g.hash.len().min(8)], g.columns.join(",")))
            .collect();
        marks.push_str(&format!("  DRIFT[{}]", groups.join(" ")));
    }
    marks
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
        out.push_str(&format!("{:>NUM_COL$}", column_header(col)));
    }
    out.push('\n');
    for row in &m.rows {
        out.push_str(&format!("{:<ID_COL$}", row.id));
        for cell in &row.cells {
            out.push_str(&format!("{:>NUM_COL$}", format_cell(*cell)));
        }
        out.push_str(&row_marks(row));
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
            "  {:<16} model={:<32} target={:<28} judge={:<20}{}{}\n",
            col.label,
            col.model,
            col.target,
            format_judge(&col.judge),
            if col.incomplete { " incomplete" } else { "" },
            if col.judge_drift { " judge-drift" } else { "" },
        ));
    }
    out.push_str(LEGEND_CAVEAT);
    out
}

/// SPREAD and DRIFT are display heuristics that flag "look here", not a
/// statistical test or an authoritative verdict about which column is right
/// (design.md, "SPREAD is data-derived and hole-safe"; "DRIFT marks, never
/// adjudicates").
const LEGEND_CAVEAT: &str =
    "\nSPREAD and DRIFT are display heuristics, not statistical or authoritative claims.\n";

/// Markdown table: for `matrix --markdown` and `experiments/` snapshots (no
/// width limit, unlike the fixed-width renderer).
pub fn render_markdown(m: &Matrix) -> String {
    let mut out = String::new();
    out.push_str("| scenario |");
    for col in &m.columns {
        out.push_str(&format!(" {} |", column_header(col)));
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
        let marks = row_marks(row);
        if !marks.is_empty() {
            out.push_str(marks.trim());
            out.push(' ');
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
            "- `{}`: model={}, target={}, judge={}{}{}\n",
            col.label,
            col.model,
            col.target,
            format_judge(&col.judge),
            if col.incomplete { ", incomplete" } else { "" },
            if col.judge_drift { ", judge-drift" } else { "" },
        ));
    }
    out.push_str(
        "\n_SPREAD and DRIFT are display heuristics, not statistical or authoritative claims._\n",
    );
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

    fn report_with_judge_hash(target: &str, tag: &str, judge_hash: Option<String>) -> Report {
        Report::build(
            ReportMeta {
                tag: tag.into(),
                model: format!("anthropic/{tag}"),
                backend: "zs".into(),
                trials: 1,
                target: target.into(),
                judge_hash,
                ..Default::default()
            },
            vec![],
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

    // 6.1 — a row whose gap exceeds one trial's resolution is marked SPREAD.
    #[test]
    fn row_with_gap_beyond_one_trial_resolution_is_marked_spread() {
        // Two trials each => resolution 1/2 = 0.5. Gap here is 1.0 - 0.0 = 1.0.
        let a = report(
            "targets/opus.toml",
            "run-a",
            vec![ScenarioResult::from_trials(
                "s".into(),
                vec![trial(Final::Pass), trial(Final::Pass)],
            )],
        );
        let b = report(
            "targets/sonnet.toml",
            "run-b",
            vec![ScenarioResult::from_trials(
                "s".into(),
                vec![trial(Final::Fail), trial(Final::Fail)],
            )],
        );
        let m = build(&[&a, &b]);
        assert!(m.rows[0].spread, "gap of 1.0 exceeds resolution of 0.5");
    }

    // 6.1 — a row whose gap sits within one trial's resolution is not marked.
    #[test]
    fn row_with_gap_within_one_trial_resolution_is_not_marked_spread() {
        // Four trials each => resolution 1/4 = 0.25. Gap here is 0.25 - 0.0 = 0.25,
        // not strictly greater than the threshold.
        let a = report(
            "targets/opus.toml",
            "run-a",
            vec![ScenarioResult::from_trials(
                "s".into(),
                vec![
                    trial(Final::Pass),
                    trial(Final::Fail),
                    trial(Final::Fail),
                    trial(Final::Fail),
                ],
            )],
        );
        let b = report(
            "targets/sonnet.toml",
            "run-b",
            vec![ScenarioResult::from_trials(
                "s".into(),
                vec![
                    trial(Final::Fail),
                    trial(Final::Fail),
                    trial(Final::Fail),
                    trial(Final::Fail),
                ],
            )],
        );
        let m = build(&[&a, &b]);
        assert!(
            !m.rows[0].spread,
            "gap of 0.25 does not exceed resolution of 0.25"
        );
    }

    // 6.1 — a row with a hole neither divides by zero nor lets the hole feed
    // max/min or the threshold.
    #[test]
    fn row_with_a_hole_is_safe_and_excludes_it_from_spread_computation() {
        let a = report(
            "targets/opus.toml",
            "run-a",
            vec![ScenarioResult::from_trials(
                "s".into(),
                vec![trial(Final::Pass)],
            )],
        );
        let b = report("targets/sonnet.toml", "run-b", vec![]); // hole: never ran "s"
        let m = build(&[&a, &b]);
        assert_eq!(m.rows[0].cells[1], Cell::Hole);
        // A single gradable cell (the hole excluded) cannot produce a spread.
        assert!(!m.rows[0].spread);
    }

    // 6.3 — per-row DRIFT on content_hash mismatch: the row is marked and both
    // differing columns are listed, grouped by hash, with timestamps, naming
    // no column as correct.
    #[test]
    fn row_drift_marks_content_hash_mismatch_and_lists_both_columns() {
        let a = report(
            "targets/opus.toml",
            "run-a",
            vec![ScenarioResult::from_trials_with_hash(
                "s".into(),
                "hash-old".into(),
                vec![trial(Final::Pass)],
            )],
        );
        let b = report(
            "targets/sonnet.toml",
            "run-b",
            vec![ScenarioResult::from_trials_with_hash(
                "s".into(),
                "hash-new".into(),
                vec![trial(Final::Pass)],
            )],
        );
        let m = build(&[&a, &b]);
        let row = &m.rows[0];
        assert_eq!(row.drift.len(), 2, "two distinct hashes => two groups");
        let all_columns: Vec<&str> = row
            .drift
            .iter()
            .flat_map(|g| g.columns.iter().map(|c| c.as_str()))
            .collect();
        assert!(all_columns.contains(&"opus"));
        assert!(all_columns.contains(&"sonnet"));
        for g in &row.drift {
            assert!(!g.timestamps.is_empty());
        }
    }

    // 6.4 — per-column DRIFT when judge_hash differs from the others.
    #[test]
    fn column_drift_marks_differing_judge_hash() {
        let a = report_with_judge_hash("targets/opus.toml", "run-a", Some("hash-a".into()));
        let b = report_with_judge_hash("targets/sonnet.toml", "run-b", Some("hash-b".into()));
        let m = build(&[&a, &b]);
        assert!(m.columns[0].judge_drift);
        assert!(m.columns[1].judge_drift);
    }

    // 6.4 — per-column DRIFT when the judge is unknown.
    #[test]
    fn column_drift_marks_unknown_judge() {
        let a = report_with_judge_hash("targets/opus.toml", "run-a", None);
        let b = report_with_judge_hash("targets/sonnet.toml", "run-b", None);
        let m = build(&[&a, &b]);
        assert!(m.columns[0].judge_drift, "unknown judge always marks");
        assert!(m.columns[1].judge_drift, "unknown judge always marks");
    }

    // 6.5 — a column that ran fewer scenarios than the suite defines is
    // marked incomplete.
    #[test]
    fn truncated_column_is_marked_incomplete() {
        let full = report(
            "targets/opus.toml",
            "run-a",
            vec![
                ScenarioResult::from_trials("one".into(), vec![trial(Final::Pass)]),
                ScenarioResult::from_trials("two".into(), vec![trial(Final::Pass)]),
            ],
        );
        let truncated = report(
            "targets/sonnet.toml",
            "run-b",
            vec![ScenarioResult::from_trials(
                "one".into(),
                vec![trial(Final::Pass)],
            )],
        );
        let m = build(&[&full, &truncated]);
        assert!(!m.columns[0].incomplete);
        assert!(m.columns[1].incomplete);

        let header = render_fixed_width(&m);
        assert!(
            header.contains("sonnet*") || header.contains("incomplete"),
            "the rendered column header carries the incomplete mark: {header}"
        );
    }

    // 6.6 — SPREAD and DRIFT are labelled in the legend as display
    // heuristics, not statistical or authoritative claims.
    #[test]
    fn legend_labels_spread_and_drift_as_display_heuristics() {
        let a = report("targets/opus.toml", "run-a", vec![]);
        let m = build(&[&a]);
        let legend = render_legend_fixed_width(&m);
        assert!(legend.to_lowercase().contains("spread"));
        assert!(legend.to_lowercase().contains("drift"));
        assert!(
            legend.to_lowercase().contains("heuristic")
                || legend.to_lowercase().contains("not statistical")
                || legend.to_lowercase().contains("not authoritative"),
            "legend: {legend}"
        );
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
