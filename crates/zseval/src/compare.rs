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
        summary_base_pass_hat_k: base.summary.pass_hat_k,
        summary_cand_pass_hat_k: cand.summary.pass_hat_k,
        summary_base_cost: base.summary.total_cost_usd,
        summary_cand_cost: cand.summary.total_cost_usd,
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
