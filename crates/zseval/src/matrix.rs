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

use serde::Serialize;

use crate::scenario::Kind;
use crate::verdict::{Report, ScenarioResult};

/// One scenario's outcome in one column. Never a bare `f64` — see the module
/// doc on why "ran, all failed" and "didn't run/couldn't grade" must stay
/// distinguishable all the way to the rendered table.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub enum Cell {
    Rate(f64),
    Hole,
}

/// `Report::judge_model`'s three states, carried through to the column
/// legend verbatim (see that field's doc for what each means).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum JudgeState {
    Unknown,
    NothingGraded,
    Rulers(Vec<String>),
}

/// Footer figures for one column, recomputed over the scenarios gradable in
/// *every* column (not the column's own full scenario set) — see
/// `Matrix::footer_excluded`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ColumnFooter {
    pub pass_at_k: f64,
    pub pass_hat_k: f64,
    pub total_cost_usd: f64,
}

/// One target's column: the header carries only [`Column::label`] (the stem,
/// disambiguated when another column shares it); everything else here is
/// legend-only, per the design's "48 + 12N" width budget.
#[derive(Debug, Clone, PartialEq, Serialize)]
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
    /// `Report::prompts_pack`, legend-only. Empty when the run used no pack —
    /// rendered as its own plain marker (`compare::pack_identity`), never
    /// omitted, so a packless column reads as a fact rather than a blank.
    pub prompts_pack: String,
    /// `Report::prompts_hash`, legend-only. Displayed as a short form
    /// alongside `prompts_pack` so two columns of the same pack path with
    /// different contents are distinguishable by eye (design.md, "Display
    /// the fingerprint, so 'invisible difference' needs no special rule").
    pub prompts_hash: String,
    /// `Report::zs_version`, legend-only. For `--backend mock`, the fixed
    /// `"mock"` label (controlled-variables spec, "Mock columns are
    /// identified as mock").
    pub zs_version: String,
    /// `Report::zs_bin_sha256`, legend-only. Displayed as a short form
    /// alongside `zs_version` (`compare::zs_identity`), same shape as
    /// `prompts_pack`/`prompts_hash`.
    pub zs_bin_sha256: String,
    /// Overall footer figures over the scenarios gradable in *every* column.
    /// `None` when that intersection is empty: there is no shared gradable
    /// basis, so the footer is honestly a hole (rendered `-`) rather than a
    /// real-looking `0.000`. Unchanged in definition by the kind grouping
    /// below (matrix-render spec, "overall over the whole common set,
    /// unchanged in definition from before").
    pub footer: Option<ColumnFooter>,
    /// The same footer figures, filtered to the common gradable set's
    /// regression scenarios only. `None` when the common set has no
    /// regression scenario — rendered `n/a` (not `-`), the same textual
    /// convention as a report's own per-kind summary (matrix-render spec, "A
    /// kind absent from the common set is n/a").
    pub regression_footer: Option<ColumnFooter>,
    /// The same footer figures, filtered to the common gradable set's
    /// capability scenarios only. See `regression_footer`.
    pub capability_footer: Option<ColumnFooter>,
    /// This column's run was cut short by the shared budget
    /// (`Report::budget_truncated`) — the visible trace of a budget-truncated
    /// run (design.md, "Budget is one shared total; truncation is marked").
    /// Keyed off the recorded truncation fact, not a scenario count, so a
    /// column that simply ran a smaller suite in full (a shorter baseline)
    /// is *not* marked — its missing scenarios already show as `-` holes.
    pub incomplete: bool,
    /// This column's `judge_hash` differs from another column's, or this
    /// column's judge is unknown while some other column carries a known hash
    /// — the measuring stick may have moved. When no column has a known hash
    /// there is no ruler to have moved, so nothing drifts. SPREAD/DRIFT are
    /// display heuristics, not statistical or authoritative claims (design.md,
    /// "DRIFT marks, never adjudicates").
    pub judge_drift: bool,
    /// This column's target AND pack identity both differ from some other
    /// column's in this table — no cell difference between the two can be
    /// attributed to either variable alone (`controlled-variables` spec;
    /// design.md, "One independent variable, derived rather than asserted").
    /// Distinct from `judge_drift`/row DRIFT, which flag a moved ruler
    /// (`judge_hash`/`content_hash`); this flags two *subject* variables
    /// (target, pack) moving together. A display heuristic like SPREAD/DRIFT
    /// — it says "look here", never which column is correct. Columns sharing
    /// a target and differing only by pack are the flagship clean experiment
    /// and are never marked by this.
    pub multi_variable: bool,
}

/// One `content_hash` grouping within a row's DRIFT mark: the columns (and
/// their timestamps) that agreed on this hash. No group is "correct" — DRIFT
/// marks, it never adjudicates.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DriftGroup {
    pub hash: String,
    /// Column labels sharing `hash`, aligned index-for-index with `timestamps`.
    pub columns: Vec<String>,
    pub timestamps: Vec<String>,
}

/// One scenario's row: a cell per column, aligned by index with
/// `Matrix::columns`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Row {
    pub id: String,
    /// This scenario's `kind`, read from the report rows that graded it
    /// (never re-read from scenario.toml — matrix-render spec, "Rows are
    /// grouped by kind"). Drives the two-section row grouping the fixed-width
    /// and markdown renderers apply; the field itself keeps `Matrix::rows`
    /// flat, since sectioning is a render-time concern, not a data shape one.
    pub kind: Kind,
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
#[derive(Debug, Clone, PartialEq, Serialize)]
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

    // The common set, filtered to each kind — the footer's per-kind groups
    // are computed over these, never over a kind's full (per-column) set, so
    // a group's numbers stay on the same shared denominator the overall
    // footer already uses (matrix-render spec).
    let regression_ids: Vec<String> = intersection
        .iter()
        .filter(|id| row_kind(reports, id) == Kind::Regression)
        .cloned()
        .collect();
    let capability_ids: Vec<String> = intersection
        .iter()
        .filter(|id| row_kind(reports, id) == Kind::Capability)
        .cloned()
        .collect();

    let labels = disambiguate_labels(reports);
    let judge_drift_flags = judge_drift_flags(reports);
    let multi_variable_flags = multi_variable_flags(reports);
    let columns = reports
        .iter()
        .zip(&labels)
        .zip(judge_drift_flags)
        .zip(multi_variable_flags)
        .map(|(((r, label), judge_drift), multi_variable)| Column {
            stem: crate::target::stem(Path::new(&r.target)),
            label: label.clone(),
            model: r.model.clone(),
            target: r.target.clone(),
            tag: r.tag.clone(),
            timestamp: r.timestamp.clone(),
            judge: judge_state(r),
            prompts_pack: r.prompts_pack.clone(),
            prompts_hash: r.prompts_hash.clone(),
            zs_version: r.zs_version.clone(),
            zs_bin_sha256: r.zs_bin_sha256.clone(),
            footer: column_footer(r, &intersection),
            regression_footer: column_footer(r, &regression_ids),
            capability_footer: column_footer(r, &capability_ids),
            incomplete: r.budget_truncated,
            judge_drift,
            multi_variable,
        })
        .collect();

    let rows = all_ids
        .iter()
        .map(|id| Row {
            id: id.clone(),
            kind: row_kind(reports, id),
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

/// A row's `kind`, read off whichever report actually declares this scenario
/// id (its kind is intrinsic to the scenario, so any report that ran it
/// agrees) — never re-read from scenario.toml (matrix-render spec, "Rows are
/// grouped by kind"). Every id passed in comes from the union of these same
/// reports' own scenario ids, so `find` always succeeds; the fallback is
/// unreachable in practice, not a real default.
fn row_kind(reports: &[&Report], id: &str) -> Kind {
    reports
        .iter()
        .find_map(|r| r.scenarios.iter().find(|s| s.id == id).map(|s| s.kind))
        .unwrap_or(Kind::Regression)
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
fn column_footer(report: &Report, intersection: &[String]) -> Option<ColumnFooter> {
    let gradable: Vec<&ScenarioResult> = report
        .scenarios
        .iter()
        .filter(|s| intersection.contains(&s.id) && s.is_gradable())
        .collect();
    if gradable.is_empty() {
        // No scenario is gradable in every column: there is no shared basis to
        // average, so the footer is a hole rather than a fabricated 0.000.
        return None;
    }
    let g = gradable.len() as f64;
    let pass_at_k = gradable.iter().map(|s| s.pass_at_k).sum::<f64>() / g;
    let pass_hat_k = gradable.iter().map(|s| s.pass_hat_k).sum::<f64>() / g;
    let total_cost_usd = report
        .scenarios
        .iter()
        .filter(|s| intersection.contains(&s.id))
        .flat_map(|s| s.trials.iter())
        .map(|t| t.cost_usd)
        .sum();
    Some(ColumnFooter {
        pass_at_k: round4(pass_at_k),
        pass_hat_k: round4(pass_hat_k),
        total_cost_usd: round4(total_cost_usd),
    })
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

/// Per-column DRIFT: this column's `judge_hash` differs from another column's
/// known hash, or this column's judge is unknown while some *other* column
/// carries a known hash. When no column in the set has a known hash (e.g.
/// every column ran `--no-judge`), there is no ruler that could have moved, so
/// nothing drifts: no known ruler anywhere ⇒ no drift. Symmetric on a real
/// mismatch (design.md: DRIFT never picks a "correct" column), so two
/// disagreeing columns are both marked.
fn judge_drift_flags(reports: &[&Report]) -> Vec<bool> {
    let any_known = reports.iter().any(|r| r.judge_hash.is_some());
    reports
        .iter()
        .enumerate()
        .map(|(i, r)| {
            any_known
                && (r.judge_hash.is_none()
                    || reports.iter().enumerate().any(|(j, other)| {
                        j != i && other.judge_hash.is_some() && other.judge_hash != r.judge_hash
                    }))
        })
        .collect()
}

/// Per-column `multi_variable` mark: this column's target AND pack identity
/// both differ from some *other* column's — see `Column::multi_variable`. Pack
/// identity is the fingerprint hash alone, never the pack path (which
/// `PromptPack::fingerprint` deliberately excludes), so a byte-identical pack
/// supplied from two different paths does not count as a second moved variable.
/// Unlike `judge_drift_flags`, there is no unknown-vs-known special case: every
/// column's target is always known, and an empty pack (`""` hash, i.e. no pack)
/// is itself a valid, comparable identity rather than an absence to skip.
fn multi_variable_flags(reports: &[&Report]) -> Vec<bool> {
    reports
        .iter()
        .enumerate()
        .map(|(i, r)| {
            reports.iter().enumerate().any(|(j, other)| {
                j != i && r.target != other.target && r.prompts_hash != other.prompts_hash
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

/// One footer figure in the fixed-width table: right-aligned to `NUM_COL`,
/// a bare `-` when there is no shared gradable basis (`None`).
fn footer_cell_fixed_width(v: Option<f64>) -> String {
    match v {
        Some(v) => format!("{v:>NUM_COL$.3}"),
        None => format!("{:>NUM_COL$}", "-"),
    }
}

/// One footer figure in the markdown table: a `-` when there is no shared
/// gradable basis (`None`).
fn footer_cell_markdown(v: Option<f64>) -> String {
    match v {
        Some(v) => format!(" {v:.3} |"),
        None => " - |".to_string(),
    }
}

/// One footer figure for a per-kind group row (regression/capability), fixed
/// width: `n/a`, not the bare `-` the overall group uses, when the common
/// gradable set contains no scenario of this kind — the same textual
/// convention `print_run_report_summaries` uses for an empty kind's own line
/// (matrix-render spec, "A kind absent from the common set is n/a").
fn kind_footer_cell_fixed_width(v: Option<f64>) -> String {
    match v {
        Some(v) => format!("{v:>NUM_COL$.3}"),
        None => format!("{:>NUM_COL$}", "n/a"),
    }
}

/// Markdown counterpart of [`kind_footer_cell_fixed_width`].
fn kind_footer_cell_markdown(v: Option<f64>) -> String {
    match v {
        Some(v) => format!(" {v:.3} |"),
        None => " n/a |".to_string(),
    }
}

/// This kind's footer figures for one column (regression/capability) — the
/// single accessor the footer loops below read through, so neither renderer
/// special-cases the field name per kind.
fn kind_footer(col: &Column, kind: Kind) -> Option<ColumnFooter> {
    match kind {
        Kind::Regression => col.regression_footer,
        Kind::Capability => col.capability_footer,
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

/// This kind's rows, in the same order they already appear in `m.rows`
/// (scenario-id order) — grouping into sections is a render-time filter
/// only, never a resort (matrix-render spec, "Within a section, row order
/// follows the existing ordering").
fn rows_of_kind(m: &Matrix, kind: Kind) -> Vec<&Row> {
    m.rows.iter().filter(|r| r.kind == kind).collect()
}

/// Fixed-width terminal table: the default `matrix` output and `run`'s
/// end-of-run stderr table (target-matrix sections 7-8). Rows render in two
/// sections — regression first, then capability, each under its own marker
/// (matrix-render spec, "Rows are grouped by kind"); a kind with no rows
/// prints no marker and no rows for it. The footer renders three metric
/// groups in the same order, regression and capability filtered to the
/// common gradable set and rendered `n/a` when that kind is absent from it,
/// overall last and unchanged from before.
pub fn render_fixed_width(m: &Matrix) -> String {
    let mut out = String::new();
    out.push_str(&format!("{:<ID_COL$}", "scenario"));
    for col in &m.columns {
        out.push_str(&format!("{:>NUM_COL$}", column_header(col)));
    }
    out.push('\n');
    for (label, kind) in [
        ("regression", Kind::Regression),
        ("capability", Kind::Capability),
    ] {
        let rows = rows_of_kind(m, kind);
        if rows.is_empty() {
            continue;
        }
        out.push_str(&format!("-- {label} --\n"));
        for row in rows {
            out.push_str(&format!("{:<ID_COL$}", row.id));
            for cell in &row.cells {
                out.push_str(&format!("{:>NUM_COL$}", format_cell(*cell)));
            }
            out.push_str(&row_marks(row));
            out.push('\n');
        }
    }

    for (label, kind) in [
        ("regression", Kind::Regression),
        ("capability", Kind::Capability),
    ] {
        out.push_str(&format!("{:<ID_COL$}", format!("{label} pass@k")));
        for col in &m.columns {
            out.push_str(&kind_footer_cell_fixed_width(
                kind_footer(col, kind).map(|f| f.pass_at_k),
            ));
        }
        out.push('\n');
        out.push_str(&format!("{:<ID_COL$}", format!("{label} pass^k")));
        for col in &m.columns {
            out.push_str(&kind_footer_cell_fixed_width(
                kind_footer(col, kind).map(|f| f.pass_hat_k),
            ));
        }
        out.push('\n');
    }
    out.push_str(&format!("{:<ID_COL$}", "pass@k"));
    for col in &m.columns {
        out.push_str(&footer_cell_fixed_width(col.footer.map(|f| f.pass_at_k)));
    }
    out.push('\n');
    out.push_str(&format!("{:<ID_COL$}", "pass^k"));
    for col in &m.columns {
        out.push_str(&footer_cell_fixed_width(col.footer.map(|f| f.pass_hat_k)));
    }
    out.push('\n');
    out.push_str(&format!("{:<ID_COL$}", "cost usd"));
    for col in &m.columns {
        out.push_str(&footer_cell_fixed_width(
            col.footer.map(|f| f.total_cost_usd),
        ));
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
            "  {:<16} model={:<32} target={:<28} judge={:<20} prompts={:<20}zs={:<24}{}{}{}\n",
            col.label,
            col.model,
            col.target,
            format_judge(&col.judge),
            crate::compare::pack_identity(&col.prompts_pack, &col.prompts_hash),
            crate::compare::zs_identity(&col.zs_version, &col.zs_bin_sha256),
            if col.incomplete { " incomplete" } else { "" },
            if col.judge_drift { " judge-drift" } else { "" },
            if col.multi_variable { " MULTI-VAR" } else { "" },
        ));
    }
    out.push_str(LEGEND_CAVEAT);
    out
}

/// SPREAD, DRIFT, and MULTI-VAR are display heuristics that flag "look here",
/// not a statistical test or an authoritative verdict about which column is
/// right (design.md, "SPREAD is data-derived and hole-safe"; "DRIFT marks,
/// never adjudicates"; "One independent variable, derived rather than
/// asserted").
const LEGEND_CAVEAT: &str =
    "\nSPREAD, DRIFT, and MULTI-VAR are display heuristics, not statistical or authoritative claims.\n";

/// Markdown table: for `matrix --markdown` and `experiments/` snapshots (no
/// width limit, unlike the fixed-width renderer). Same row sectioning and
/// three-group footer as [`render_fixed_width`] — see its doc.
pub fn render_markdown(m: &Matrix) -> String {
    // SPREAD/DRIFT belong in a real cell: text after a row's final `|` is
    // dropped by GFM table parsers, so the marks would vanish from the very
    // records this renderer exists to keep. Only add the column when some row
    // actually carries a mark, so unmarked tables stay clean.
    let any_marks = m.rows.iter().any(|r| !row_marks(r).trim().is_empty());

    let mut out = String::new();
    out.push_str("| scenario |");
    for col in &m.columns {
        out.push_str(&format!(" {} |", column_header(col)));
    }
    if any_marks {
        out.push_str(" notes |");
    }
    out.push('\n');
    out.push_str("|---|");
    for _ in &m.columns {
        out.push_str("---|");
    }
    if any_marks {
        out.push_str("---|");
    }
    out.push('\n');
    for (label, kind) in [
        ("regression", Kind::Regression),
        ("capability", Kind::Capability),
    ] {
        let rows = rows_of_kind(m, kind);
        if rows.is_empty() {
            continue;
        }
        // A section marker row: plain text after a row's final `|` is
        // dropped by GFM parsers, so the marker sits in its own bolded cell
        // rather than trailing off the end of the row (same reasoning as
        // `any_marks`'s notes column above).
        out.push_str(&format!("| **{label}** |"));
        for _ in &m.columns {
            out.push_str(" |");
        }
        if any_marks {
            out.push_str(" |");
        }
        out.push('\n');
        for row in rows {
            out.push_str(&format!("| {} |", row.id));
            for cell in &row.cells {
                out.push_str(&format!(" {} |", format_cell(*cell)));
            }
            if any_marks {
                out.push_str(&format!(" {} |", row_marks(row).trim()));
            }
            out.push('\n');
        }
    }
    for (label, kind) in [
        ("regression", Kind::Regression),
        ("capability", Kind::Capability),
    ] {
        out.push_str(&format!("| {label} pass@k |"));
        for col in &m.columns {
            out.push_str(&kind_footer_cell_markdown(
                kind_footer(col, kind).map(|f| f.pass_at_k),
            ));
        }
        if any_marks {
            out.push_str(" |");
        }
        out.push('\n');
        out.push_str(&format!("| {label} pass^k |"));
        for col in &m.columns {
            out.push_str(&kind_footer_cell_markdown(
                kind_footer(col, kind).map(|f| f.pass_hat_k),
            ));
        }
        if any_marks {
            out.push_str(" |");
        }
        out.push('\n');
    }
    out.push_str("| pass@k |");
    for col in &m.columns {
        out.push_str(&footer_cell_markdown(col.footer.map(|f| f.pass_at_k)));
    }
    if any_marks {
        out.push_str(" |");
    }
    out.push('\n');
    out.push_str("| pass^k |");
    for col in &m.columns {
        out.push_str(&footer_cell_markdown(col.footer.map(|f| f.pass_hat_k)));
    }
    if any_marks {
        out.push_str(" |");
    }
    out.push('\n');
    out.push_str("| cost usd |");
    for col in &m.columns {
        out.push_str(&footer_cell_markdown(col.footer.map(|f| f.total_cost_usd)));
    }
    if any_marks {
        out.push_str(" |");
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
            "- `{}`: model={}, target={}, judge={}, prompts={}, zs={}{}{}{}\n",
            col.label,
            col.model,
            col.target,
            format_judge(&col.judge),
            crate::compare::pack_identity(&col.prompts_pack, &col.prompts_hash),
            crate::compare::zs_identity(&col.zs_version, &col.zs_bin_sha256),
            if col.incomplete { ", incomplete" } else { "" },
            if col.judge_drift { ", judge-drift" } else { "" },
            if col.multi_variable {
                ", MULTI-VAR"
            } else {
                ""
            },
        ));
    }
    out.push_str(
        "\n_SPREAD, DRIFT, and MULTI-VAR are display heuristics, not statistical or authoritative claims._\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verdict::{Final, ReportMeta, TrialResult, ZsIdentity};

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

    // Mirrors `compare.rs`'s `report_with_pack` test helper (section 8) —
    // same field wiring, so the pack-identity convention stays one shape
    // across subcommands.
    fn report_with_pack(target: &str, tag: &str, pack: &str, hash: &str) -> Report {
        Report::build(
            ReportMeta {
                tag: tag.into(),
                model: format!("anthropic/{tag}"),
                backend: "zs".into(),
                trials: 1,
                target: target.into(),
                prompts_pack: pack.into(),
                prompts_hash: hash.into(),
                ..Default::default()
            },
            vec![],
        )
    }

    // 9.1 — mirrors `report_with_pack`: a fixture with a settable zerostack
    // identity (version + hash), so legend tests can assert `zs_identity`'s
    // wiring without a live zerostack binary.
    fn report_with_zs(target: &str, tag: &str, version: &str, hash: &str) -> Report {
        Report::build(
            ReportMeta {
                tag: tag.into(),
                model: format!("anthropic/{tag}"),
                backend: "zs".into(),
                trials: 1,
                target: target.into(),
                zs: ZsIdentity {
                    zs_version: version.into(),
                    zs_bin_sha256: hash.into(),
                    ..Default::default()
                },
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
                ScenarioResult::from_trials(
                    "zebra".into(),
                    Kind::Regression,
                    vec![trial(Final::Pass)],
                ),
                ScenarioResult::from_trials(
                    "apple".into(),
                    Kind::Regression,
                    vec![trial(Final::Pass)],
                ),
            ],
        );
        let b = report(
            "targets/sonnet.toml",
            "run-b",
            vec![ScenarioResult::from_trials(
                "mango".into(),
                Kind::Regression,
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
                Kind::Regression,
                vec![trial(Final::Fail), trial(Final::Fail)],
            )],
        );
        let all_indeterminate = report(
            "targets/sonnet.toml",
            "run-b",
            vec![ScenarioResult::from_trials(
                "s".into(),
                Kind::Regression,
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
                ScenarioResult::from_trials(
                    "shared".into(),
                    Kind::Regression,
                    vec![trial(Final::Pass)],
                ),
                ScenarioResult::from_trials(
                    "only-a".into(),
                    Kind::Regression,
                    vec![trial(Final::Pass)],
                ),
            ],
        );
        let b = report(
            "targets/sonnet.toml",
            "run-b",
            vec![
                ScenarioResult::from_trials(
                    "shared".into(),
                    Kind::Regression,
                    vec![trial(Final::Fail)],
                ),
                ScenarioResult::from_trials(
                    "only-b".into(),
                    Kind::Regression,
                    vec![trial(Final::Pass)],
                ),
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
        assert_eq!(m.columns[0].footer.unwrap().pass_hat_k, 1.0);
        assert_eq!(m.columns[1].footer.unwrap().pass_hat_k, 0.0);
    }

    // 5.3 — identical suites: the footer equals each report's own summary,
    // nothing excluded.
    #[test]
    fn footer_matches_each_reports_own_summary_when_suites_are_identical() {
        let scenarios = || {
            vec![
                ScenarioResult::from_trials(
                    "one".into(),
                    Kind::Regression,
                    vec![trial(Final::Pass)],
                ),
                ScenarioResult::from_trials(
                    "two".into(),
                    Kind::Regression,
                    vec![trial(Final::Fail)],
                ),
            ]
        };
        let a = report("targets/opus.toml", "run-a", scenarios());
        let b = report("targets/sonnet.toml", "run-b", scenarios());
        let m = build(&[&a, &b]);
        assert!(m.footer_excluded.is_empty());
        let fa = m.columns[0].footer.as_ref().unwrap();
        let fb = m.columns[1].footer.as_ref().unwrap();
        assert_eq!(fa.pass_at_k, a.summary.pass_at_k);
        assert_eq!(fa.pass_hat_k, a.summary.pass_hat_k);
        assert_eq!(fa.total_cost_usd, a.summary.total_cost_usd);
        assert_eq!(fb.pass_at_k, b.summary.pass_at_k);
        assert_eq!(fb.pass_hat_k, b.summary.pass_hat_k);
        assert_eq!(fb.total_cost_usd, b.summary.total_cost_usd);
    }

    // 5.3 — when the shared scenario is gradable in one column but not the
    // other, the gradable intersection is empty, so every column's footer is a
    // hole (`None`) and both renderers print `-`, never a fabricated `0.000`.
    #[test]
    fn empty_gradable_intersection_makes_the_footer_a_hole_not_a_fake_zero() {
        // Both report a scenario "s", so they share an id and are comparable,
        // but column b's "s" is all-indeterminate (not gradable) — the
        // gradable intersection across the two columns is empty.
        let a = report(
            "targets/opus.toml",
            "run-a",
            vec![ScenarioResult::from_trials(
                "s".into(),
                Kind::Regression,
                vec![trial(Final::Pass)],
            )],
        );
        let b = report(
            "targets/sonnet.toml",
            "run-b",
            vec![ScenarioResult::from_trials(
                "s".into(),
                Kind::Regression,
                vec![trial(Final::Indeterminate)],
            )],
        );
        let m = build(&[&a, &b]);
        assert!(m.columns[0].footer.is_none(), "no shared gradable basis");
        assert!(m.columns[1].footer.is_none(), "no shared gradable basis");

        let fixed = render_fixed_width(&m);
        // The three footer rows must show `-`, never `0.000`.
        for label in ["pass@k", "pass^k", "cost usd"] {
            let line = fixed
                .lines()
                .find(|l| l.starts_with(label))
                .unwrap_or_else(|| panic!("missing footer row {label}: {fixed}"));
            assert!(
                line.contains('-') && !line.contains("0.000"),
                "footer row {label} must be a hole, not a fake zero: {line}"
            );
        }

        let md = render_markdown(&m);
        for label in ["| pass@k |", "| pass^k |", "| cost usd |"] {
            let line = md
                .lines()
                .find(|l| l.starts_with(label))
                .unwrap_or_else(|| panic!("missing footer row {label}: {md}"));
            assert!(
                line.contains(" - |") && !line.contains("0.000"),
                "markdown footer row {label} must be a hole, not a fake zero: {line}"
            );
        }
    }

    // 5.4 — header carries only the stem; legend carries model/target/judge.
    #[test]
    fn column_identity_puts_the_stem_in_the_header_and_the_rest_in_the_legend() {
        let r = report(
            "targets/opus.toml",
            "run-a",
            vec![ScenarioResult::from_trials(
                "s".into(),
                Kind::Regression,
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
                Kind::Regression,
                vec![trial(Final::Pass)],
            )],
        );
        let later = report(
            "targets/opus.toml",
            "run-later",
            vec![ScenarioResult::from_trials(
                "s".into(),
                Kind::Regression,
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
                Kind::Regression,
                vec![trial(Final::Pass), trial(Final::Pass)],
            )],
        );
        let b = report(
            "targets/sonnet.toml",
            "run-b",
            vec![ScenarioResult::from_trials(
                "s".into(),
                Kind::Regression,
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
                Kind::Regression,
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
                Kind::Regression,
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
                Kind::Regression,
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
                Kind::Regression,
                "hash-old".into(),
                vec![trial(Final::Pass)],
            )],
        );
        let b = report(
            "targets/sonnet.toml",
            "run-b",
            vec![ScenarioResult::from_trials_with_hash(
                "s".into(),
                Kind::Regression,
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

    // 6.4 — when NO column carries a known judge hash (e.g. every column ran
    // `--no-judge`), there is no ruler that could have moved, so nothing
    // drifts: no known ruler anywhere ⇒ no drift.
    #[test]
    fn all_unknown_judges_do_not_drift() {
        let a = report_with_judge_hash("targets/opus.toml", "run-a", None);
        let b = report_with_judge_hash("targets/sonnet.toml", "run-b", None);
        let m = build(&[&a, &b]);
        assert!(!m.columns[0].judge_drift, "no known ruler => no drift");
        assert!(!m.columns[1].judge_drift, "no known ruler => no drift");
    }

    // 6.4 — a column whose judge is unknown IS marked when ANOTHER column
    // carries a known hash: that is the genuine unknown-vs-ruler drift.
    #[test]
    fn unknown_judge_drifts_against_a_known_ruler() {
        let known = report_with_judge_hash("targets/opus.toml", "run-a", Some("hash-a".into()));
        let unknown = report_with_judge_hash("targets/sonnet.toml", "run-b", None);
        let m = build(&[&known, &unknown]);
        // The known-hash column has no differing ruler to drift against.
        assert!(!m.columns[0].judge_drift);
        // The unknown column drifts against the known ruler.
        assert!(m.columns[1].judge_drift);
    }

    // 6.5 — a column whose run was cut short by the budget is marked
    // incomplete; this is keyed off the recorded `budget_truncated` fact, not
    // a scenario count.
    #[test]
    fn budget_truncated_column_is_marked_incomplete() {
        let full = report(
            "targets/opus.toml",
            "run-a",
            vec![
                ScenarioResult::from_trials(
                    "one".into(),
                    Kind::Regression,
                    vec![trial(Final::Pass)],
                ),
                ScenarioResult::from_trials(
                    "two".into(),
                    Kind::Regression,
                    vec![trial(Final::Pass)],
                ),
            ],
        );
        let mut truncated = report(
            "targets/sonnet.toml",
            "run-b",
            vec![ScenarioResult::from_trials(
                "one".into(),
                Kind::Regression,
                vec![trial(Final::Pass)],
            )],
        );
        truncated.budget_truncated = true;

        let m = build(&[&full, &truncated]);
        assert!(!m.columns[0].incomplete);
        assert!(m.columns[1].incomplete);

        let header = render_fixed_width(&m);
        assert!(
            header.contains("sonnet*") || header.contains("incomplete"),
            "the rendered column header carries the incomplete mark: {header}"
        );
    }

    // 6.5 — a column that ran a *smaller suite in full* (a shorter baseline,
    // not budget-truncated) must NOT be marked incomplete: the scenarios it
    // lacks show as `-` holes, but the `*`/incomplete mark is reserved for a
    // real budget cut. This is the case the old count-based rule got wrong.
    #[test]
    fn a_smaller_but_complete_suite_is_not_marked_incomplete() {
        let wide = report(
            "targets/opus.toml",
            "run-a",
            vec![
                ScenarioResult::from_trials(
                    "one".into(),
                    Kind::Regression,
                    vec![trial(Final::Pass)],
                ),
                ScenarioResult::from_trials(
                    "two".into(),
                    Kind::Regression,
                    vec![trial(Final::Pass)],
                ),
            ],
        );
        // A committed baseline that only ever defined "one" — reached in full,
        // never budget-truncated (budget_truncated defaults to false).
        let baseline = report(
            "targets/baseline.toml",
            "main",
            vec![ScenarioResult::from_trials(
                "one".into(),
                Kind::Regression,
                vec![trial(Final::Pass)],
            )],
        );
        assert!(!baseline.budget_truncated);

        let m = build(&[&wide, &baseline]);
        assert!(
            !m.columns[1].incomplete,
            "a complete smaller suite is not incomplete"
        );
        // "two" is simply a hole in the baseline column.
        let two = m.rows.iter().find(|r| r.id == "two").unwrap();
        assert_eq!(two.cells[1], Cell::Hole);
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

    // 6.2 — a row's SPREAD/DRIFT mark must survive into the markdown table.
    // Text after a row's final `|` is dropped by GFM parsers, so the mark has
    // to land in a real `notes` cell (added only when some row carries one).
    #[test]
    fn markdown_keeps_row_marks_in_a_notes_cell() {
        // Two trials each => resolution 0.5; gap here is 1.0 => SPREAD.
        let a = report(
            "targets/opus.toml",
            "run-a",
            vec![ScenarioResult::from_trials(
                "s".into(),
                Kind::Regression,
                vec![trial(Final::Pass), trial(Final::Pass)],
            )],
        );
        let b = report(
            "targets/sonnet.toml",
            "run-b",
            vec![ScenarioResult::from_trials(
                "s".into(),
                Kind::Regression,
                vec![trial(Final::Fail), trial(Final::Fail)],
            )],
        );
        let m = build(&[&a, &b]);
        assert!(m.rows[0].spread);

        let md = render_markdown(&m);
        assert!(md.contains("| notes |"), "notes column header: {md}");
        // The SPREAD text lands before a closing pipe, not trailing off the
        // end of the row where GFM would discard it.
        assert!(
            md.lines()
                .any(|l| l.contains("| s |") && l.contains("SPREAD |")),
            "SPREAD in a real cell: {md}"
        );
    }

    // A table with no marked rows keeps the tidy, notes-less shape.
    #[test]
    fn markdown_omits_the_notes_column_when_no_row_is_marked() {
        let a = report(
            "targets/opus.toml",
            "run-a",
            vec![ScenarioResult::from_trials(
                "s".into(),
                Kind::Regression,
                vec![trial(Final::Pass)],
            )],
        );
        let m = build(&[&a]);
        assert!(m.rows.iter().all(|r| !r.spread && r.drift.is_empty()));
        assert!(!render_markdown(&m).contains("notes"));
    }

    // 5.6 — both renderers over one fixture.
    #[test]
    fn both_renderers_render_the_same_model() {
        let a = report(
            "targets/opus.toml",
            "run-a",
            vec![ScenarioResult::from_trials(
                "greet".into(),
                Kind::Regression,
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

    // 9.1 — each column's legend line carries its pack identity as path plus
    // short hash (mirroring `compare::pack_identity`), and a plain marker
    // when the column used no pack.
    #[test]
    fn legend_carries_each_columns_pack_identity() {
        let with_pack =
            report_with_pack("targets/opus.toml", "run-a", "packs/a", "aaaaaaaaaaaaaaaa");
        let without_pack = report_with_pack("targets/sonnet.toml", "run-b", "", "");
        let m = build(&[&with_pack, &without_pack]);
        let legend = render_legend_fixed_width(&m);
        assert!(legend.contains("prompts=packs/a#aaaa"), "legend: {legend}");
        assert!(legend.contains("prompts=none"), "legend: {legend}");
    }

    // 9.1 — each column's legend line also carries its zerostack build
    // identity as version plus short hash (`compare::zs_identity`), the same
    // shape as `pack_identity`.
    #[test]
    fn legend_carries_each_columns_zs_identity() {
        let a = report_with_zs(
            "targets/opus.toml",
            "run-a",
            "zerostack 1.7.2",
            "b41c000000000000",
        );
        let m = build(&[&a]);
        let legend = render_legend_fixed_width(&m);
        assert!(
            legend.contains("zs=zerostack 1.7.2#b41c"),
            "legend: {legend}"
        );
    }

    // 9.1 — a mock-backend column's `zs_version` is the fixed `"mock"`
    // label; its legend line shows `mock#<short-hash>`, the fixture
    // fingerprint (controlled-variables spec, "Mock columns are identified
    // as mock").
    #[test]
    fn legend_shows_mock_identity_for_mock_backend_reports() {
        let a = report_with_zs("targets/opus.toml", "run-a", "mock", "abcd000000000000");
        let m = build(&[&a]);
        let legend = render_legend_fixed_width(&m);
        assert!(legend.contains("zs=mock#abcd"), "legend: {legend}");
    }

    // 9.2 — two columns sharing a target and differing only by pack are a
    // clean single-variable comparison: not marked, but their legend lines
    // are still distinguishable by pack identity.
    #[test]
    fn same_target_different_pack_is_not_marked_but_legend_lines_differ() {
        let a = report_with_pack("targets/opus.toml", "run-a", "packs/a", "aaaaaaaaaaaaaaaa");
        let b = report_with_pack("targets/opus.toml", "run-b", "packs/b", "bbbbbbbbbbbbbbbb");
        let m = build(&[&a, &b]);
        assert!(!m.columns[0].multi_variable);
        assert!(!m.columns[1].multi_variable);
        let legend = render_legend_fixed_width(&m);
        assert!(legend.contains("prompts=packs/a#aaaa"), "legend: {legend}");
        assert!(legend.contains("prompts=packs/b#bbbb"), "legend: {legend}");
    }

    // 9.3 — columns differing in both target and pack identity are marked,
    // and that mark is a separate mechanism from judge-drift/DRIFT: neither
    // side here carries a judge hash at all.
    #[test]
    fn different_target_and_pack_is_marked_and_distinct_from_drift() {
        let a = report_with_pack("targets/opus.toml", "run-a", "packs/a", "aaaaaaaaaaaaaaaa");
        let b = report_with_pack(
            "targets/sonnet.toml",
            "run-b",
            "packs/b",
            "bbbbbbbbbbbbbbbb",
        );
        let m = build(&[&a, &b]);
        assert!(m.columns[0].multi_variable);
        assert!(m.columns[1].multi_variable);
        assert!(!m.columns[0].judge_drift, "no judge hash on either side");
        assert!(!m.columns[1].judge_drift, "no judge hash on either side");
        let legend = render_legend_fixed_width(&m);
        assert!(legend.contains("MULTI-VAR"), "legend: {legend}");
        assert!(!legend.contains("judge-drift"), "legend: {legend}");
    }

    // 9.4 — columns differing by target but recording the same pack identity
    // are not marked: only one subject variable moved.
    #[test]
    fn different_target_shared_pack_is_not_marked() {
        let a = report_with_pack(
            "targets/opus.toml",
            "run-a",
            "packs/shared",
            "aaaaaaaaaaaaaaaa",
        );
        let b = report_with_pack(
            "targets/sonnet.toml",
            "run-b",
            "packs/shared",
            "aaaaaaaaaaaaaaaa",
        );
        let m = build(&[&a, &b]);
        assert!(!m.columns[0].multi_variable);
        assert!(!m.columns[1].multi_variable);
    }

    // 9.4b — identity is the fingerprint hash, never the path: a byte-identical
    // pack supplied from two different paths is one pack, so a target-only
    // difference stays a clean single-variable comparison, not MULTI-VAR.
    #[test]
    fn different_target_same_hash_moved_path_is_not_marked() {
        let a = report_with_pack("targets/opus.toml", "run-a", "my-pack", "aaaaaaaaaaaaaaaa");
        let b = report_with_pack(
            "targets/sonnet.toml",
            "run-b",
            "packs/my-pack",
            "aaaaaaaaaaaaaaaa",
        );
        let m = build(&[&a, &b]);
        assert!(!m.columns[0].multi_variable);
        assert!(!m.columns[1].multi_variable);
    }

    // 9.5 — the new mark is documented in the legend caveat alongside SPREAD
    // and DRIFT as a display heuristic, never a verdict.
    #[test]
    fn legend_caveat_labels_multi_variable_as_a_display_heuristic() {
        let a = report_with_pack("targets/opus.toml", "run-a", "packs/a", "aaaaaaaaaaaaaaaa");
        let m = build(&[&a]);
        let legend = render_legend_fixed_width(&m);
        assert!(legend.contains("MULTI-VAR"), "legend: {legend}");
        assert!(
            legend.to_lowercase().contains("heuristic"),
            "legend: {legend}"
        );
    }

    // 9.5 — the markdown renderer carries the same pack identity and mark as
    // the fixed-width one; both go through the same `Column` model.
    #[test]
    fn markdown_legend_carries_pack_identity_and_multi_variable_mark() {
        let a = report_with_pack("targets/opus.toml", "run-a", "packs/a", "aaaaaaaaaaaaaaaa");
        let b = report_with_pack(
            "targets/sonnet.toml",
            "run-b",
            "packs/b",
            "bbbbbbbbbbbbbbbb",
        );
        let m = build(&[&a, &b]);
        let md = render_markdown(&m);
        assert!(md.contains("prompts=packs/a#aaaa"), "markdown: {md}");
        assert!(md.contains("MULTI-VAR"), "markdown: {md}");
    }

    // 9.1 — the markdown renderer carries the same zs identity as the
    // fixed-width one; both go through the same `Column` model.
    #[test]
    fn markdown_legend_carries_zs_identity() {
        let a = report_with_zs(
            "targets/opus.toml",
            "run-a",
            "zerostack 1.7.2",
            "b41c000000000000",
        );
        let m = build(&[&a]);
        let md = render_markdown(&m);
        assert!(md.contains("zs=zerostack 1.7.2#b41c"), "markdown: {md}");
    }

    // trustworthy-numbers 6.1: rows render in two sections, regression first
    // under its own marker, capability rows after under theirs.
    #[test]
    fn rows_render_in_two_sections_regression_first_fixed_width() {
        let cap =
            ScenarioResult::from_trials("cap-a".into(), Kind::Capability, vec![trial(Final::Pass)]);
        let a = report(
            "targets/opus.toml",
            "run-a",
            vec![
                ScenarioResult::from_trials(
                    "reg-a".into(),
                    Kind::Regression,
                    vec![trial(Final::Pass)],
                ),
                cap,
            ],
        );
        let m = build(&[&a]);
        let fixed = render_fixed_width(&m);

        let reg_marker = fixed
            .find("-- regression --")
            .unwrap_or_else(|| panic!("no regression section marker: {fixed}"));
        let cap_marker = fixed
            .find("-- capability --")
            .unwrap_or_else(|| panic!("no capability section marker: {fixed}"));
        let reg_row = fixed
            .find("reg-a")
            .unwrap_or_else(|| panic!("no reg-a row: {fixed}"));
        let cap_row = fixed
            .find("cap-a")
            .unwrap_or_else(|| panic!("no cap-a row: {fixed}"));
        assert!(reg_marker < reg_row, "{fixed}");
        assert!(reg_row < cap_marker, "{fixed}");
        assert!(cap_marker < cap_row, "{fixed}");
    }

    // trustworthy-numbers 6.2: the same sectioning applies to the markdown
    // renderer, via its own marker row (plain text after the final `|` is
    // dropped by GFM parsers, so the marker has to sit in a real cell).
    #[test]
    fn rows_render_in_two_sections_regression_first_markdown() {
        let cap =
            ScenarioResult::from_trials("cap-a".into(), Kind::Capability, vec![trial(Final::Pass)]);
        let a = report(
            "targets/opus.toml",
            "run-a",
            vec![
                ScenarioResult::from_trials(
                    "reg-a".into(),
                    Kind::Regression,
                    vec![trial(Final::Pass)],
                ),
                cap,
            ],
        );
        let m = build(&[&a]);
        let md = render_markdown(&m);

        let reg_marker = md
            .find("**regression**")
            .unwrap_or_else(|| panic!("no regression section marker: {md}"));
        let cap_marker = md
            .find("**capability**")
            .unwrap_or_else(|| panic!("no capability section marker: {md}"));
        let reg_row = md
            .find("| reg-a |")
            .unwrap_or_else(|| panic!("no reg-a row: {md}"));
        let cap_row = md
            .find("| cap-a |")
            .unwrap_or_else(|| panic!("no cap-a row: {md}"));
        assert!(reg_marker < reg_row, "{md}");
        assert!(reg_row < cap_marker, "{md}");
        assert!(cap_marker < cap_row, "{md}");
    }

    // trustworthy-numbers 6.1/6.3: the footer renders three metric groups —
    // regression, capability, overall (unchanged) — in that order, each
    // computed over the common gradable set filtered to that kind.
    #[test]
    fn footer_renders_three_groups_over_the_common_set_filtered_by_kind() {
        let a_cap =
            ScenarioResult::from_trials("cap".into(), Kind::Capability, vec![trial(Final::Fail)]);
        let b_cap =
            ScenarioResult::from_trials("cap".into(), Kind::Capability, vec![trial(Final::Pass)]);
        let a = report(
            "targets/opus.toml",
            "run-a",
            vec![
                ScenarioResult::from_trials(
                    "reg".into(),
                    Kind::Regression,
                    vec![trial(Final::Pass)],
                ),
                a_cap,
            ],
        );
        let b = report(
            "targets/sonnet.toml",
            "run-b",
            vec![
                ScenarioResult::from_trials(
                    "reg".into(),
                    Kind::Regression,
                    vec![trial(Final::Fail)],
                ),
                b_cap,
            ],
        );
        let m = build(&[&a, &b]);
        let fixed = render_fixed_width(&m);

        // Order: regression group, then capability, then the unprefixed
        // overall group last (the historical single "pass@k" line).
        let reg_pos = fixed
            .find("regression pass@k")
            .unwrap_or_else(|| panic!("no regression pass@k line: {fixed}"));
        let cap_pos = fixed
            .find("capability pass@k")
            .unwrap_or_else(|| panic!("no capability pass@k line: {fixed}"));
        let overall_pos = fixed
            .find("\npass@k")
            .unwrap_or_else(|| panic!("no bare overall pass@k line: {fixed}"));
        assert!(reg_pos < cap_pos, "{fixed}");
        assert!(cap_pos < overall_pos, "{fixed}");

        // Values: column a is reg-pass/cap-fail, column b is reg-fail/cap-pass
        // — the two kind groups disagree with each other and with the
        // blended overall, proving each is computed independently.
        let reg_line = fixed
            .lines()
            .find(|l| l.starts_with("regression pass@k"))
            .unwrap();
        assert!(reg_line.contains("1.000"), "{reg_line}");
        assert!(reg_line.contains("0.000"), "{reg_line}");

        let cap_line = fixed
            .lines()
            .find(|l| l.starts_with("capability pass@k"))
            .unwrap();
        assert!(cap_line.contains("0.000"), "{cap_line}");
        assert!(cap_line.contains("1.000"), "{cap_line}");

        let overall_line = fixed.lines().find(|l| l.starts_with("pass@k")).unwrap();
        assert!(overall_line.contains("0.500"), "{overall_line}");
    }

    // trustworthy-numbers 6.1/6.3: a kind with no gradable scenario in the
    // common set renders `n/a` for every column — never a fabricated `-`
    // hole or a `0.000` — the same textual convention a report's own summary
    // uses for an empty kind.
    #[test]
    fn kind_absent_from_common_set_renders_n_a() {
        let a = report(
            "targets/opus.toml",
            "run-a",
            vec![ScenarioResult::from_trials(
                "reg".into(),
                Kind::Regression,
                vec![trial(Final::Pass)],
            )],
        );
        let b = report(
            "targets/sonnet.toml",
            "run-b",
            vec![ScenarioResult::from_trials(
                "reg".into(),
                Kind::Regression,
                vec![trial(Final::Fail)],
            )],
        );
        let m = build(&[&a, &b]);
        let fixed = render_fixed_width(&m);

        let cap_line = fixed
            .lines()
            .find(|l| l.starts_with("capability pass@k"))
            .unwrap_or_else(|| panic!("no capability pass@k line: {fixed}"));
        assert!(cap_line.contains("n/a"), "{cap_line}");
        assert!(!cap_line.contains("0.000"), "{cap_line}");

        // The common set here is entirely regression, so overall == regression:
        // column a (reg pass) is 1.000, column b (reg fail) is 0.000.
        let overall_line = fixed.lines().find(|l| l.starts_with("pass@k")).unwrap();
        assert!(overall_line.contains("1.000"), "{overall_line}");
        assert!(overall_line.contains("0.000"), "{overall_line}");
    }

    // trustworthy-numbers 6.1/6.2: `--json`'s row array stays one flat array
    // and each row carries its own `kind` — sectioning is a render-time
    // concern only, never a JSON nesting.
    #[test]
    fn json_rows_stay_flat_with_kind_per_row() {
        let cap =
            ScenarioResult::from_trials("cap-a".into(), Kind::Capability, vec![trial(Final::Pass)]);
        let a = report(
            "targets/opus.toml",
            "run-a",
            vec![
                ScenarioResult::from_trials(
                    "reg-a".into(),
                    Kind::Regression,
                    vec![trial(Final::Pass)],
                ),
                cap,
            ],
        );
        let m = build(&[&a]);
        let json = serde_json::to_value(&m).unwrap();
        let rows = json["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 2, "rows stay one flat array: {json}");
        let reg_row = rows.iter().find(|r| r["id"] == "reg-a").unwrap();
        assert_eq!(reg_row["kind"], "regression", "{reg_row}");
        let cap_row = rows.iter().find(|r| r["id"] == "cap-a").unwrap();
        assert_eq!(cap_row["kind"], "capability", "{cap_row}");
    }
}
