//! The single-file HTML page: one run's identity, the coverage ledger, and
//! that run's scenario table, assembled into one artifact a human can open
//! from a bare file path.
//!
//! **Every runtime value is escaped, and the type is what enforces that**
//! (design D9). `zs_version` is captured verbatim from `ZS_BIN --version` and
//! deliberately never format-validated, so upstream's banner shape never
//! becomes a compatibility contract; `zs_bin_path`, `target` and `judge_file`
//! come off a filesystem. Markup can therefore reach this module from
//! outside, and a rule of the form "remember to call `escape`" holds only as
//! long as every future editor remembers it. So the buffer is private to
//! [`Page`] and there are exactly two ways into it: [`Page::text`], which
//! escapes, and [`Page::raw`], which takes `&'static str` and so accepts only
//! what a source literal spells. A runtime `String` has no accidental route
//! past the escape. Author-controlled ledger prose and scenario ids go
//! through `text` as well, because a rule with an exemption list is one
//! nobody can check.
//!
//! `Page` is `pub(crate)` rather than private because the results section's
//! renderer lives in `matrix.rs`, beside the private cell and footer
//! formatting it has to reuse (design D4). One writer shared by both modules
//! is what keeps D9's "no exemptions" true of the whole page rather than of
//! this file only.
//!
//! **The page is self-contained** (design D7): CSS inline, no script, no
//! external stylesheet, font or image. A page that fetches at view time
//! breaks when it is opened from a file path, when the network is absent, and
//! when whatever it fetched moves, which makes it not a deliverable. The
//! markup is pushed onto a `String` the way `matrix::render_markdown` pushes
//! its markdown (design D8); a template crate would buy a separation of
//! markup from logic that one page does not need.
//!
//! Section order is header, results, coverage (owner ruling 2026-08-03,
//! superseding design D11's coverage-first order): identity first, because
//! every number under it is only meaningful once you know what produced it;
//! then the results the reader came for; then coverage as the denominator
//! that says how much of the surface those results actually touched.

use std::collections::BTreeSet;

use serde::Serialize;

use crate::coverage::{Area, Ledger, Status};
use crate::matrix::Matrix;
use crate::verdict::Report;

/// The page's whole stylesheet, inlined into the document. A `&'static str`,
/// so it enters the buffer through [`Page::raw`] like every other piece of
/// markup, and it names no font, image or sheet to fetch (design D7).
const CSS: &str = r#"
:root { color-scheme: light dark; }
body {
  max-width: 70rem;
  margin: 0 auto;
  padding: 2rem 1.5rem 4rem;
  font-family: ui-sans-serif, system-ui, sans-serif;
  line-height: 1.5;
}
h1 { font-size: 1.5rem; margin: 0 0 1.5rem; }
h2 { font-size: 1.2rem; margin: 0 0 0.75rem; }
h3 { font-size: 1rem; margin: 1.25rem 0 0.4rem; }
section { margin: 0 0 3rem; }
dl { display: grid; grid-template-columns: max-content 1fr; gap: 0.3rem 1.5rem; margin: 0; }
dt { font-weight: 600; }
dd { margin: 0; font-family: ui-monospace, SFMono-Regular, Menlo, monospace; }
ul { margin: 0; padding-left: 1.25rem; }
li { margin: 0.2rem 0; }
code { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; }
table { border-collapse: collapse; font-variant-numeric: tabular-nums; }
th, td { padding: 0.25rem 0.6rem; text-align: right; border-bottom: 1px solid rgba(128, 128, 128, 0.3); }
th:first-child, td:first-child { text-align: left; }
details { margin: 1.5rem 0; }
summary { cursor: pointer; }
.headline { margin: 0 0 1.5rem; font-weight: 600; }
.claims > li { margin: 0.6rem 0; }
.status {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 0.8rem;
  padding: 0 0.3rem;
  border: 1px solid rgba(128, 128, 128, 0.4);
  border-radius: 0.2rem;
}
.status.covered { color: #1a7f37; border-color: #1a7f37; }
.status.uncovered { color: #bc4c00; border-color: #bc4c00; }
.status.product-blocked { color: #cf222e; border-color: #cf222e; }
.status.excluded { color: #6e7781; border-color: #6e7781; }
.ids { margin: 0.15rem 0 0; }
.evidence { margin: 0.15rem 0 0; opacity: 0.75; }
.mark { opacity: 0.75; font-style: italic; }
"#;

/// The page under construction. The buffer is private and only [`raw`] and
/// [`text`] reach it, which is what makes an unescaped runtime value
/// unwritable rather than merely discouraged (design D9, and this module's
/// header).
///
/// [`raw`]: Page::raw
/// [`text`]: Page::text
pub(crate) struct Page {
    buf: String,
}

impl Page {
    pub(crate) fn new() -> Page {
        Page { buf: String::new() }
    }

    /// Markup. `&'static str` is the whole point: a value computed at runtime
    /// cannot be passed here, so the only thing that can be written unescaped
    /// is what a literal in this crate's source spells.
    pub(crate) fn raw(&mut self, markup: &'static str) {
        self.buf.push_str(markup);
    }

    /// A value from a report, the ledger, or a filesystem. Escaped on the way
    /// in, with no exemption for values that look like they could not carry
    /// markup.
    pub(crate) fn text(&mut self, value: &str) {
        self.buf.push_str(&escape(value));
    }

    pub(crate) fn finish(self) -> String {
        self.buf
    }
}

/// The one escape, covering the five characters that are significant in
/// markup. `'` is included even though a single-quoted attribute appears
/// nowhere on this page today, because the alternative is a rule whose
/// correctness depends on that staying true.
fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Everything the page states, before any of it is markup: the header's
/// read-back fields, the coverage rows with their marks, and `matrix`'s own
/// model of the results (design D2).
///
/// `site --json` emits exactly this value and [`render`] is a view of the same
/// one, so a machine reading the JSON reads what the page shows rather than a
/// second derivation that could drift from it. Borrowed throughout: every
/// string here belongs to the report or the ledger it was built from, which is
/// what makes "the header derives nothing" a property of the type rather than
/// a promise about the builder.
#[derive(Serialize)]
pub struct PageModel<'a> {
    pub header: Header<'a>,
    pub coverage: Coverage<'a>,
    pub results: Matrix,
}

/// The run's identity, field for field as the report records it, plus the one
/// thing the page says about the ledger's age ([`Audit`]).
#[derive(Serialize)]
pub struct Header<'a> {
    pub zs_version: &'a str,
    pub zs_bin_sha256: &'a str,
    pub zs_bin_path: &'a str,
    /// `None` is "not provided", never an empty string: an absent fact and an
    /// empty fact are different claims (design D5).
    pub git_sha: Option<&'a str>,
    pub features: Option<&'a [String]>,
    pub model: &'a str,
    pub backend: &'a str,
    /// The report's own `""` when no target file was named, kept raw here: a
    /// machine reads what the report holds, and the absence wording is the
    /// page's reading for a human ([`render_absent_or`], design D5).
    pub target: &'a str,
    pub timestamp: &'a str,
    pub trials: usize,
    pub total_cost_usd: f64,
    pub budget_truncated: bool,
    /// What the run was configured to grade with, kept apart from
    /// `judge_model` below, which is what actually graded. The report's own
    /// `""` when no judge was named, kept raw for the same reason `target` is.
    pub judge_file: &'a str,
    pub judge_hash: Option<&'a str>,
    /// Three readings, none collapsible into another: `None` unknown,
    /// `Some([])` nothing graded, `Some([m, ..])` these rulers graded.
    pub judge_model: Option<&'a [String]>,
    pub audit: Audit<'a>,
}

/// The ledger's `audited_against` beside whether this run's banner names it.
/// Disclosed, never fatal (design D3): a `--backend mock` report records
/// `mock` and so mismatches by construction, and `coverage-ledger` requires
/// the worst outcome of the comparison to be a spurious mismatch notice rather
/// than a blocked publish.
#[derive(Serialize)]
pub struct Audit<'a> {
    /// The zerostack version the ledger's judgments were made against.
    pub audited_against: &'a str,
    /// `Ledger::audit_matches` over this report's banner: containment, with a
    /// boundary rule, and nothing stronger. It says the two name one version,
    /// not that the ledger's prose is current.
    pub agrees: bool,
}

/// The ledger as the page states it: every area it declares, in file order,
/// and the count of the ones holding no scenario at all.
#[derive(Serialize)]
pub struct Coverage<'a> {
    /// The headline figure, and the total it is counted out of. A count, never
    /// a ratio (design D12).
    pub areas_with_no_scenario: usize,
    pub areas_total: usize,
    pub areas: Vec<AreaRow<'a>>,
}

#[derive(Serialize)]
pub struct AreaRow<'a> {
    pub name: &'a str,
    pub title: &'a str,
    /// Any claim of this area is `covered`, so some scenario tests some part
    /// of it. `false` is what the headline counts.
    pub has_scenario: bool,
    pub claims: Vec<ClaimRow<'a>>,
}

#[derive(Serialize)]
pub struct ClaimRow<'a> {
    pub claim: &'a str,
    pub note: Option<&'a str>,
    pub evidence: Evidence<'a>,
}

/// One claim's status and the evidence that status owes, mirroring
/// `coverage::Status` so the page's vocabulary and the ledger's are one
/// vocabulary. The one thing added is the per-id mark on `covered`.
#[derive(Serialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum Evidence<'a> {
    Covered {
        scenarios: Vec<CitedId<'a>>,
    },
    Uncovered {
        blocked_by: Option<&'a str>,
    },
    ProductBlocked {
        reason: &'a str,
        zs: Option<&'a str>,
    },
    Excluded {
        reason: &'a str,
    },
}

/// A scenario id a `covered` claim cites, and whether this run exercised it.
#[derive(Serialize)]
pub struct CitedId<'a> {
    pub id: &'a str,
    /// The report's results hold this id. Derived per render from the report
    /// and never written back to the ledger (design D6): `covered` is an
    /// existence claim, and run membership is a fact about one report.
    pub exercised: bool,
}

/// The page model for one report and the ledger it is read against.
///
/// Pure and infallible, like `matrix::build`: the drift gate that makes the
/// coverage section true of this repo's tree is the caller's, because it needs
/// a path to name in an error and this function has none.
pub fn build<'a>(report: &'a Report, ledger: &'a Ledger) -> PageModel<'a> {
    PageModel {
        header: Header {
            zs_version: &report.zs_version,
            zs_bin_sha256: &report.zs_bin_sha256,
            zs_bin_path: &report.zs_bin_path,
            git_sha: report.git_sha.as_deref(),
            features: report.features.as_deref(),
            model: &report.model,
            backend: &report.backend,
            target: &report.target,
            timestamp: &report.timestamp,
            trials: report.trials,
            total_cost_usd: report.summary.total_cost_usd,
            budget_truncated: report.budget_truncated,
            judge_file: &report.judge_file,
            judge_hash: report.judge_hash.as_deref(),
            judge_model: report.judge_model.as_deref(),
            audit: Audit {
                audited_against: ledger.audited_against(),
                agrees: ledger.audit_matches(&report.zs_version),
            },
        },
        coverage: build_coverage(report, ledger),
        results: crate::matrix::build(&[report]),
    }
}

/// The ledger's rows, in the ledger's order, with the one derivation the
/// coverage section makes: which of a `covered` claim's cited ids this run did
/// not exercise (design D6).
fn build_coverage<'a>(report: &'a Report, ledger: &'a Ledger) -> Coverage<'a> {
    let exercised: BTreeSet<&str> = report.scenarios.iter().map(|s| s.id.as_str()).collect();
    // File order is presentation order: nothing here sorts.
    let areas: Vec<AreaRow<'a>> = ledger
        .areas()
        .iter()
        .map(|area| AreaRow {
            name: &area.name,
            title: &area.title,
            has_scenario: has_scenario(area),
            claims: area
                .claims
                .iter()
                .map(|claim| ClaimRow {
                    claim: &claim.claim,
                    note: claim.note.as_deref(),
                    evidence: build_evidence(&claim.status, &exercised),
                })
                .collect(),
        })
        .collect();
    Coverage {
        areas_with_no_scenario: areas.iter().filter(|a| !a.has_scenario).count(),
        areas_total: areas.len(),
        areas,
    }
}

/// A status and its own evidence, plus the per-id mark on `covered`. The
/// drift gate has already established that every cited id exists in the tree,
/// so an id missing from the report reads as "exists, and this run did not
/// exercise it" and as nothing else.
fn build_evidence<'a>(status: &'a Status, exercised: &BTreeSet<&str>) -> Evidence<'a> {
    match status {
        Status::Covered { scenarios } => Evidence::Covered {
            scenarios: scenarios
                .iter()
                .map(|id| CitedId {
                    id,
                    exercised: exercised.contains(id.as_str()),
                })
                .collect(),
        },
        Status::Uncovered { blocked_by } => Evidence::Uncovered {
            blocked_by: blocked_by.as_deref(),
        },
        Status::ProductBlocked { reason, zs } => Evidence::ProductBlocked {
            reason,
            zs: zs.as_deref(),
        },
        Status::Excluded { reason } => Evidence::Excluded { reason },
    }
}

/// The whole page for one page model.
pub fn render(model: &PageModel) -> String {
    let mut page = Page::new();
    page.raw(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>zseval</title>
<style>"#,
    );
    page.raw(CSS);
    page.raw(
        r#"</style>
</head>
<body>
"#,
    );
    render_header(&mut page, &model.header);
    render_results(&mut page, &model.results);
    render_coverage(&mut page, &model.coverage);
    page.raw(
        r#"</body>
</html>
"#,
    );
    page.finish()
}

/// One header field that is nothing more than a literal label and a plain
/// `&str` value: closes the previous `<dd>`, opens `<dt>label</dt><dd>`, and
/// writes `value` through the page's own escaping writer. Every field with
/// anything else — an `Option`, a list, the judge's three-state, a formatted
/// number, a yes/no ternary — keeps its own call site rather than going
/// through here.
fn field(page: &mut Page, label: &'static str, value: &str) {
    page.raw("</dd>\n<dt>");
    page.raw(label);
    page.raw("</dt><dd>");
    page.text(value);
}

/// The run's identity, read back field by field with no inference, no
/// defaulting, and no computed substitute (spec: "The header reads report
/// fields back without deriving them"; design D5). Every value here is the
/// report's own, verbatim; the only choice this function makes is how to
/// spell an absent or empty one so the two are never confused.
///
/// The one exception is a field whose schema documents `""` as "none was
/// named" ([`render_absent_or`]): that sentinel is read as the absence it
/// encodes, because verbatim would state a judge file or a target was named
/// and then withhold its name.
///
/// The audit rows are the one thing here that is not the report's: they are
/// the ledger's `audited_against` beside the version this run measured, and
/// they sit next to `zerostack` so the two strings are read together
/// ([`render_audit`], design D3).
fn render_header(page: &mut Page, header: &Header) {
    page.raw(
        r#"<section id="header">
<h1>zseval</h1>
<dl>
<dt>zerostack</dt><dd>"#,
    );
    page.text(header.zs_version);
    page.raw("</dd>\n");
    render_audit(page, header);
    page.raw("<dt>binary sha256</dt><dd>");
    page.text(header.zs_bin_sha256);
    field(page, "binary path", header.zs_bin_path);
    page.raw(
        r#"</dd>
<dt>git sha</dt><dd>"#,
    );
    render_opt_str(page, header.git_sha);
    page.raw(
        r#"</dd>
<dt>features</dt><dd>"#,
    );
    render_opt_list(page, header.features);
    field(page, "model", header.model);
    field(page, "backend", header.backend);
    page.raw(
        r#"</dd>
<dt>target</dt><dd>"#,
    );
    render_absent_or(page, header.target, "no target file");
    field(page, "timestamp", header.timestamp);
    page.raw(
        r#"</dd>
<dt>trials</dt><dd>"#,
    );
    page.text(&header.trials.to_string());
    page.raw(
        r#"</dd>
<dt>total cost</dt><dd>"#,
    );
    page.text(&format!("${:.4}", header.total_cost_usd));
    page.raw(
        r#"</dd>
<dt>budget truncated</dt><dd>"#,
    );
    page.raw(if header.budget_truncated { "yes" } else { "no" });
    page.raw(
        r#"</dd>
<dt>configured</dt><dd>"#,
    );
    // The judge is two facts, never collapsed into one (design D5): what the
    // run was told to grade with (`judge_file`, `judge_hash`), labelled apart
    // from what actually graded (`judge_model`) below.
    //
    // The hash fingerprints the file that was named, so it goes where the file
    // goes: a run that named none has no hash to report, and `(hash: not
    // provided)` beside the absence would read as one it failed to compute.
    if render_absent_or(page, header.judge_file, "no judge configured") {
        page.raw(" (hash: ");
        render_opt_str(page, header.judge_hash);
        page.raw(")");
    }
    page.raw(
        r#"</dd>
<dt>graded by</dt><dd>"#,
    );
    render_judge_model(page, header.judge_model);
    page.raw(
        r#"</dd>
</dl>
</section>
"#,
    );
}

/// The ledger's age against this run, disclosed on the page and never fatal
/// (spec: "An `audited_against` mismatch is disclosed, never fatal"; design
/// D3). Both strings appear, each labelled as what it is: the version the
/// ledger's judgments were made against, and the version this run measured.
///
/// The agreeing sentence is scoped to the version deliberately. The comparison
/// behind it is containment with a boundary rule, so the strongest thing it
/// supports is that the two name one version — not that the ledger's prose is
/// current, which no comparison on this page can establish.
fn render_audit(page: &mut Page, header: &Header) {
    page.raw("<dt>ledger audited against</dt><dd>");
    page.text(header.audit.audited_against);
    page.raw(
        r#"</dd>
<dt>audit vs this run</dt><dd>"#,
    );
    if header.audit.agrees {
        page.raw(
            "the version this run measured names it, so the audit and the run agree on the version",
        );
    } else {
        page.raw("the version this run measured (");
        page.text(header.zs_version);
        page.raw(
            ") does not name it, so the ledger's judgments were made against a different build",
        );
    }
    page.raw("</dd>\n");
}

/// A field whose schema documents `""` as "none was named" reads as that
/// absence, `absent`, rather than verbatim (design D5's sentinel exemption).
/// `judge_file` and `target` are today's two: the schema spells one absence
/// two ways, so reading the sentinel back verbatim leaves a blank `<dd>` that
/// states a file was named while withholding its name. `--json` keeps emitting
/// the raw field; this wording is the page's reading for a human. The
/// exemption, and this function with it, ends when both fields become `Option`
/// in the report schema.
///
/// Returns whether a value was named, which is the question the judge row asks
/// again for the hash that fingerprints it.
fn render_absent_or(page: &mut Page, value: &str, absent: &'static str) -> bool {
    if value.is_empty() {
        page.raw(absent);
        return false;
    }
    page.text(value);
    true
}

/// A `null` field reads as "not provided", distinct from a value that is
/// present and empty (design D5): an absent fact and an empty fact are
/// different claims.
fn render_opt_str(page: &mut Page, value: Option<&str>) {
    match value {
        Some(v) => page.text(v),
        None => page.raw("not provided"),
    }
}

/// Same distinction as [`render_opt_str`], for a list-valued field: `null`
/// reads as "not provided"; a present-but-empty list reads as "(none)", a
/// visibly different string so the two claims are never confused.
fn render_opt_list(page: &mut Page, value: Option<&[String]>) {
    match value {
        None => page.raw("not provided"),
        Some([]) => page.raw("(none)"),
        Some(items) => page.text(&items.join(", ")),
    }
}

/// `judge_model`'s three states, each with its own reading (design D5):
/// `None` is unknown, `Some([])` is nothing graded, `Some([m, ..])` names the
/// rulers that did. `Some([])` must read as "nothing was graded" rather than
/// falling through to whatever the caller configured — that fallthrough is
/// exactly the computed substitute this function exists to refuse.
fn render_judge_model(page: &mut Page, value: Option<&[String]>) {
    match value {
        None => page.raw("unknown"),
        Some([]) => page.raw("nothing was graded"),
        Some(items) => page.text(&items.join(", ")),
    }
}

/// The ledger, as the ledger has it: every area it declares, in file order,
/// and every claim under the status it carries (spec: "The coverage section
/// shows every area and no percentage").
///
/// Nothing is derived here: the marks and the headline count came off
/// [`build_coverage`], so the page and `--json` state one set of facts. Every
/// area gets its row, including the ones no scenario touches — that row is the
/// fact a suite-derived count structurally cannot state, and dropping it is
/// the failure this whole capability exists to prevent.
fn render_coverage(page: &mut Page, coverage: &Coverage) {
    page.raw(
        r#"<section id="coverage">
<h2>Coverage</h2>
"#,
    );
    render_headline(page, coverage);
    // File order is presentation order: nothing here sorts.
    for area in &coverage.areas {
        page.raw("<h3>");
        page.text(area.title);
        // The headline counts these areas; naming them where they sit is what
        // lets a reader find the ones it counted.
        if !area.has_scenario {
            page.raw(r#" <span class="mark">no scenario at all</span>"#);
        }
        page.raw("</h3>\n<ul class=\"claims\">\n");
        for claim in &area.claims {
            let label = status_label(&claim.evidence);
            page.raw(r#"<li><span class="status "#);
            page.raw(label);
            page.raw(r#"">"#);
            page.raw(label);
            page.raw("</span> ");
            page.text(claim.claim);
            render_evidence(page, &claim.evidence);
            // Free text on any status, so it is rendered outside the match
            // rather than repeated in four arms.
            if let Some(note) = claim.note {
                page.raw("\n<p class=\"evidence\">note: ");
                page.text(note);
                page.raw("</p>");
            }
            page.raw("</li>\n");
        }
        page.raw("</ul>\n");
    }
    page.raw("</section>\n");
}

/// A count of the areas holding no scenario at all, with the number of areas
/// it is counted out of. Never a ratio and never a percentage, here or
/// anywhere else on the page (design D12): fine-grained `covered` claims sit
/// beside coarse `uncovered` ones, so a rate over them moves when the author
/// re-slices a claim, while this count does not.
fn render_headline(page: &mut Page, coverage: &Coverage) {
    page.raw(r#"<p class="headline">Areas with no scenario at all: "#);
    page.text(&coverage.areas_with_no_scenario.to_string());
    page.raw(" of ");
    page.text(&coverage.areas_total.to_string());
    page.raw(".</p>\n");
}

/// Does any scenario test any part of this area? A `covered` claim is the only
/// thing that can cite one, and `Ledger::new` refuses a `covered` claim whose
/// `scenarios` are all blank, so the presence of the status is the presence of
/// a scenario — that invariant belongs to the ledger, and this is a reader of
/// it rather than a second implementation.
fn has_scenario(area: &Area) -> bool {
    area.claims
        .iter()
        .any(|c| matches!(c.status, Status::Covered { .. }))
}

/// The status, in the ledger's own vocabulary, so a reader of the page and a
/// reader of `coverage.toml` use one word for one thing. `&'static str`, so
/// the label enters the buffer as markup does. The label doubles as the
/// status's CSS class, which is why it stays a single hyphenated word.
fn status_label(evidence: &Evidence) -> &'static str {
    match evidence {
        Evidence::Covered { .. } => "covered",
        Evidence::Uncovered { .. } => "uncovered",
        Evidence::ProductBlocked { .. } => "product-blocked",
        Evidence::Excluded { .. } => "excluded",
    }
}

/// The evidence a status owes, and only that. The match is exhaustive over
/// `Evidence` on purpose: each variant carries its own fields and no others,
/// so a fifth status could not be added without deciding here what it shows.
///
/// `uncovered`'s `blocked_by` is the one field whose *presence* is the fact,
/// so a claim without one renders nothing rather than an empty label: the
/// absence says the claim is buildable today and simply unbuilt.
fn render_evidence(page: &mut Page, evidence: &Evidence) {
    match evidence {
        Evidence::Covered { scenarios } => {
            page.raw("\n<ul class=\"ids\">\n");
            for cited in scenarios {
                page.raw("<li><code>");
                page.text(cited.id);
                page.raw("</code>");
                // Derived from this report, never recorded in the ledger
                // (design D6). The drift gate has already established that
                // every cited id exists in the tree, so "exists, and this run
                // did not exercise it" is the only reading left.
                if !cited.exercised {
                    page.raw(r#" <span class="mark">not exercised by this run</span>"#);
                }
                page.raw("</li>\n");
            }
            page.raw("</ul>");
        }
        Evidence::Uncovered { blocked_by } => {
            if let Some(blocked_by) = blocked_by {
                page.raw("\n<p class=\"evidence\">blocked by: ");
                page.text(blocked_by);
                page.raw("</p>");
            }
        }
        Evidence::ProductBlocked { reason, zs } => {
            page.raw("\n<p class=\"evidence\">");
            page.text(reason);
            page.raw("</p>");
            if let Some(zs) = zs {
                page.raw("\n<p class=\"evidence\">tracked as: ");
                page.text(zs);
                page.raw("</p>");
            }
        }
        Evidence::Excluded { reason } => {
            page.raw("\n<p class=\"evidence\">");
            page.text(reason);
            page.raw("</p>");
        }
    }
}

/// This run's results: `matrix`'s own model, built over the one report the
/// page describes, rendered by the HTML renderer that lives beside `matrix`'s
/// other two (design D4, spec: "The results section reuses the matrix model
/// and its meanings"). That renderer leads with the summary figures and folds
/// the per-scenario rows into a collapsed `<details>` (owner ruling
/// 2026-08-03); the fold is native markup, so the page still carries no script
/// (design D7).
///
/// The page contributes the section container and nothing else. Cells, holes,
/// the per-kind grouping, the footer over the common gradable set, the marks
/// and the footer-excluded disclosure all keep the meanings `matrix-render`
/// defines for them, because this computes none of them: a second, differently
/// defined pass rate is exactly what going through `matrix::build` refuses.
fn render_results(page: &mut Page, results: &Matrix) {
    page.raw(
        r#"<section id="results">
<h2>Results</h2>
"#,
    );
    crate::matrix::render_html(page, results);
    page.raw(
        r#"</section>
"#,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coverage::{Area, Claim, Status};
    use crate::scenario::Kind;
    use crate::verdict::{Final, ReportMeta, ScenarioResult, TrialResult, ZsIdentity};

    fn report(zs_version: &str) -> Report {
        Report::build(
            ReportMeta {
                zs: ZsIdentity {
                    zs_version: zs_version.into(),
                    ..Default::default()
                },
                ..Default::default()
            },
            vec![],
        )
    }

    /// The same local `trial` fixture `matrix.rs` and `compare.rs` keep: one
    /// graded trial, every other field at its zero.
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

    /// A report whose results hold exactly these scenario ids — the input the
    /// covered-but-not-exercised derivation reads (design D6).
    fn report_running(ids: &[&str]) -> Report {
        Report::build(
            ReportMeta {
                zs: ZsIdentity {
                    zs_version: "zerostack 1.7.2".into(),
                    ..Default::default()
                },
                ..Default::default()
            },
            ids.iter()
                .map(|id| {
                    ScenarioResult::from_trials((*id).into(), Kind::Regression, vec![trial()])
                })
                .collect(),
        )
    }

    fn area(name: &str, title: &str, claims: Vec<Claim>) -> Area {
        Area {
            name: name.into(),
            title: title.into(),
            claims,
        }
    }

    fn covered(claim: &str, scenarios: &[&str]) -> Claim {
        Claim {
            claim: claim.into(),
            status: Status::Covered {
                scenarios: scenarios.iter().map(|s| (*s).into()).collect(),
            },
            note: None,
        }
    }

    fn uncovered(claim: &str, blocked_by: Option<&str>) -> Claim {
        Claim {
            claim: claim.into(),
            status: Status::Uncovered {
                blocked_by: blocked_by.map(Into::into),
            },
            note: None,
        }
    }

    fn product_blocked(claim: &str, reason: &str, zs: Option<&str>) -> Claim {
        Claim {
            claim: claim.into(),
            status: Status::ProductBlocked {
                reason: reason.into(),
                zs: zs.map(Into::into),
            },
            note: None,
        }
    }

    fn excluded(claim: &str, reason: &str) -> Claim {
        Claim {
            claim: claim.into(),
            status: Status::Excluded {
                reason: reason.into(),
            },
            note: None,
        }
    }

    fn noted(claim: Claim, note: &str) -> Claim {
        Claim {
            note: Some(note.into()),
            ..claim
        }
    }

    /// Built in code rather than parsed, so ledger prose full of markup does
    /// not have to survive TOML's own escaping on the way in. `Ledger::new` is
    /// the only constructor and runs every rule, so a fixture is held to what
    /// a file is held to.
    fn ledger_of(areas: Vec<Area>) -> Ledger {
        Ledger::new("1.7.2".into(), vec!["scenarios".into()], areas).unwrap()
    }

    /// The one-area, one-claim ledger the escaping and header tests read
    /// against.
    fn ledger(reason: &str) -> Ledger {
        ledger_of(vec![area(
            "permission",
            "Permission layer",
            vec![excluded(
                "a symlink out of the workspace is refused",
                reason,
            )],
        )])
    }

    /// The rendered page for one report and one ledger — the two steps a
    /// caller takes (`build` then `render`) in the one order they take them,
    /// so a test asserts on the page a `site` run would have written.
    fn page_of(report: &Report, ledger: &Ledger) -> String {
        render(&build(report, ledger))
    }

    /// The `<dd>` a header row's `<dt>` label carries, so one row can be
    /// asserted on without matching the whole page.
    fn dd<'a>(page: &'a str, label: &str) -> &'a str {
        let open = format!("<dt>{label}</dt><dd>");
        let at = page
            .find(&open)
            .unwrap_or_else(|| panic!("no {label} row:\n{page}"));
        let rest = &page[at + open.len()..];
        let end = rest
            .find("</dd>")
            .unwrap_or_else(|| panic!("the {label} row never closes:\n{page}"));
        &rest[..end]
    }

    /// The list item a covered claim's cited `id` was rendered into, so a
    /// per-id mark can be asserted against the id it attaches to rather than
    /// against the page as a whole.
    fn id_row<'a>(page: &'a str, id: &str) -> &'a str {
        let open = format!("<code>{id}</code>");
        let at = page
            .find(&open)
            .unwrap_or_else(|| panic!("no row for {id}:\n{page}"));
        let rest = &page[at..];
        let end = rest
            .find("</li>")
            .unwrap_or_else(|| panic!("the row for {id} never closes:\n{page}"));
        &rest[..end]
    }

    #[test]
    fn escape_covers_every_markup_character() {
        assert_eq!(escape("<"), "&lt;");
        assert_eq!(escape(">"), "&gt;");
        assert_eq!(escape("&"), "&amp;");
        assert_eq!(escape("\""), "&quot;");
        assert_eq!(escape("'"), "&#39;");
        assert_eq!(
            escape("<a href=\"x\">o'brien & co</a>"),
            "&lt;a href=&quot;x&quot;&gt;o&#39;brien &amp; co&lt;/a&gt;"
        );
        // An escape's own ampersand is escaped once, not twice: the input is
        // text, so `&lt;` is four characters a reader typed, not markup.
        assert_eq!(escape("&lt;"), "&amp;lt;");
        assert_eq!(escape("plain text 1.7.2"), "plain text 1.7.2");
    }

    #[test]
    fn a_hostile_version_banner_is_escaped_in_the_page() {
        let banner = "<script>alert('zs')</script>";
        let page = page_of(&report(banner), &ledger("that measures the OS."));
        assert!(
            page.contains("&lt;script&gt;alert(&#39;zs&#39;)&lt;/script&gt;"),
            "the escaped banner is not in the page:\n{page}"
        );
        assert!(
            !page.contains(banner),
            "the raw banner reached the page:\n{page}"
        );
    }

    #[test]
    fn ledger_prose_is_escaped_in_the_page() {
        let reason = "that measures the OS <not> zerostack, & it's \"theirs\".";
        let page = page_of(&report("zerostack 1.7.2"), &ledger(reason));
        assert!(
            page.contains(
                "that measures the OS &lt;not&gt; zerostack, &amp; it&#39;s &quot;theirs&quot;."
            ),
            "the escaped reason is not in the page:\n{page}"
        );
        assert!(
            !page.contains(reason),
            "the raw reason reached the page:\n{page}"
        );
    }

    #[test]
    fn the_page_runs_header_then_results_then_coverage() {
        let page = page_of(&report("zerostack 1.7.2"), &ledger("that measures the OS."));
        assert!(page.starts_with("<!doctype html>"), "no doctype:\n{page}");
        let at = |id: &str| {
            page.find(id)
                .unwrap_or_else(|| panic!("no {id} section:\n{page}"))
        };
        let (header, coverage, results) = (
            at(r#"id="header""#),
            at(r#"id="coverage""#),
            at(r#"id="results""#),
        );
        assert!(
            header < results && results < coverage,
            "sections are out of order:\n{page}"
        );
    }

    #[test]
    fn an_unavailable_build_fact_is_not_shown_as_empty() {
        // `report("...")`'s `ZsIdentity` defaults `git_sha` and `features` to
        // `None`, the same as today's real binary (design D5).
        let page = page_of(&report("zerostack 1.7.2"), &ledger("that measures the OS."));
        assert!(
            page.contains("<dt>git sha</dt><dd>not provided</dd>"),
            "git_sha did not render as not provided:\n{page}"
        );
        assert!(
            page.contains("<dt>features</dt><dd>not provided</dd>"),
            "features did not render as not provided:\n{page}"
        );
    }

    #[test]
    fn configured_and_actual_judge_are_shown_as_two_facts_labelled_apart() {
        let report = Report::build(
            ReportMeta {
                judge_file: "judges/opus.toml".into(),
                judge_hash: Some("abc123".into()),
                judge_model: Some(vec!["claude-opus-4-8".into()]),
                zs: ZsIdentity {
                    zs_version: "zerostack 1.7.2".into(),
                    ..Default::default()
                },
                ..Default::default()
            },
            vec![],
        );
        let page = page_of(&report, &ledger("that measures the OS."));
        assert!(
            page.contains("<dt>configured</dt><dd>judges/opus.toml (hash: abc123)</dd>"),
            "the configured judge (file and hash) is not shown:\n{page}"
        );
        assert!(
            page.contains("<dt>graded by</dt><dd>claude-opus-4-8</dd>"),
            "the model that actually graded is not shown:\n{page}"
        );
    }

    #[test]
    fn nothing_graded_is_not_the_same_as_unknown_and_does_not_echo_the_configured_model() {
        let report = Report::build(
            ReportMeta {
                model: "anthropic/claude-sonnet-4-6".into(),
                judge_file: "judges/opus.toml".into(),
                judge_hash: Some("abc123".into()),
                judge_model: Some(vec![]),
                zs: ZsIdentity {
                    zs_version: "zerostack 1.7.2".into(),
                    ..Default::default()
                },
                ..Default::default()
            },
            vec![],
        );
        let page = page_of(&report, &ledger("that measures the OS."));
        assert!(
            page.contains("<dt>graded by</dt><dd>nothing was graded</dd>"),
            "nothing-graded did not render distinctly from unknown:\n{page}"
        );
        // Neither the agent's own model nor the configured judge file stands
        // in for "who graded" once the report says nothing did.
        let graded_at = page
            .find("<dt>graded by</dt>")
            .unwrap_or_else(|| panic!("no graded-by row:\n{page}"));
        let graded_row = &page[graded_at..];
        let graded_dd_end = graded_row
            .find("</dd>")
            .unwrap_or_else(|| panic!("graded-by row has no closing </dd>:\n{page}"));
        let graded_row = &graded_row[..graded_dd_end];
        assert!(
            !graded_row.contains("anthropic/claude-sonnet-4-6"),
            "the agent's model was echoed as though it had graded:\n{page}"
        );
        assert!(
            !graded_row.contains("judges/opus.toml"),
            "the configured judge file was echoed as though it had graded:\n{page}"
        );
    }

    // 7.1 — `judge_file` documents `""` as "no judge file was named"
    // (verdict.rs), so reading it back verbatim puts a blank filename and a
    // dangling `(hash: not provided)` on the page of a run that configured no
    // judge: a claim the report does not make.
    #[test]
    fn a_run_that_configured_no_judge_says_so_instead_of_rendering_a_blank() {
        let report = Report::build(
            ReportMeta {
                judge_file: String::new(),
                judge_hash: None,
                judge_model: Some(vec![]),
                zs: ZsIdentity {
                    zs_version: "zerostack 1.7.2".into(),
                    ..Default::default()
                },
                ..Default::default()
            },
            vec![],
        );
        let page = page_of(&report, &ledger("that measures the OS."));
        assert!(
            page.contains("<dt>configured</dt><dd>no judge configured</dd>"),
            "an unconfigured judge did not render as an absence:\n{page}"
        );
        let configured = dd(&page, "configured");
        assert!(
            !configured.contains("(hash:"),
            "the absence still carries a hash of a file that was never named:\n{page}"
        );
        assert!(
            !configured.is_empty(),
            "the configured row rendered as an empty <dd>:\n{page}"
        );
    }

    // 7.1 — the same sentinel in `target`, which the canonical `--backend
    // mock` flow records: a targetless run must not render a blank row that
    // reads as an unnamed target file.
    #[test]
    fn a_run_with_no_target_file_says_so_instead_of_rendering_a_blank() {
        let report = Report::build(
            ReportMeta {
                target: String::new(),
                zs: ZsIdentity {
                    zs_version: "zerostack 1.7.2".into(),
                    ..Default::default()
                },
                ..Default::default()
            },
            vec![],
        );
        let page = page_of(&report, &ledger("that measures the OS."));
        assert_eq!(
            dd(&page, "target"),
            "no target file",
            "a targetless run did not render as an absence:\n{page}"
        );
    }

    // 7.1 — the amendment reads `""` and nothing else: a run that did name a
    // judge file and a target still gets both back verbatim.
    #[test]
    fn a_named_judge_file_and_target_are_still_read_back_verbatim() {
        let report = Report::build(
            ReportMeta {
                judge_file: "judges/opus.toml".into(),
                judge_hash: Some("abc123".into()),
                target: "targets/local.toml".into(),
                zs: ZsIdentity {
                    zs_version: "zerostack 1.7.2".into(),
                    ..Default::default()
                },
                ..Default::default()
            },
            vec![],
        );
        let page = page_of(&report, &ledger("that measures the OS."));
        assert_eq!(
            dd(&page, "configured"),
            "judges/opus.toml (hash: abc123)",
            "a named judge file was swallowed by the absence wording:\n{page}"
        );
        assert_eq!(
            dd(&page, "target"),
            "targets/local.toml",
            "a named target was swallowed by the absence wording:\n{page}"
        );
    }

    // 5.1 — a `--backend mock` report records `mock` as its banner, so it
    // mismatches the ledger's `audited_against` by construction (design D3).
    // The page discloses it instead of refusing to render: both strings
    // appear, each labelled as what it is.
    #[test]
    fn a_version_mismatch_states_both_strings_and_which_is_which() {
        let page = page_of(&report("mock"), &ledger("that measures the OS."));
        assert_eq!(
            dd(&page, "zerostack"),
            "mock",
            "the version this run measured is not shown:\n{page}"
        );
        assert_eq!(
            dd(&page, "ledger audited against"),
            "1.7.2",
            "the version the ledger's judgments were made against is not shown:\n{page}"
        );
        let audit = dd(&page, "audit vs this run");
        assert!(
            audit.contains("mock"),
            "the mismatch does not name what this run measured:\n{page}"
        );
        assert!(
            audit.contains("different build"),
            "the mismatch is not stated as a mismatch:\n{page}"
        );
    }

    // 5.1 — the other side of the same disclosure: the comparison is
    // containment (`Ledger::audit_matches`), so agreement is stated about the
    // version and nothing more. "The ledger is up to date" is the claim
    // containment cannot support — the audit's prose can be stale against a
    // build it was made on.
    #[test]
    fn a_matching_version_is_shown_as_agreeing_and_claims_nothing_more() {
        let page = page_of(&report("zerostack 1.7.2"), &ledger("that measures the OS."));
        assert_eq!(
            dd(&page, "ledger audited against"),
            "1.7.2",
            "the version the ledger's judgments were made against is not shown:\n{page}"
        );
        let audit = dd(&page, "audit vs this run");
        assert!(
            audit.contains("agree on the version"),
            "a matching version is not shown as agreeing:\n{page}"
        );
        assert!(
            !audit.contains("up to date"),
            "the page claims more than containment supports:\n{page}"
        );
    }

    #[test]
    fn budget_truncated_is_visible() {
        let report = Report::build(
            ReportMeta {
                budget_truncated: true,
                zs: ZsIdentity {
                    zs_version: "zerostack 1.7.2".into(),
                    ..Default::default()
                },
                ..Default::default()
            },
            vec![],
        );
        let page = page_of(&report, &ledger("that measures the OS."));
        assert!(
            page.contains("<dt>budget truncated</dt><dd>yes</dd>"),
            "budget_truncated=true is not visible:\n{page}"
        );
    }

    // 3.1 — the area a suite-derived count structurally cannot state: it has
    // no scenario at all, so it is listed like every other area and is what
    // the headline figure counts.
    #[test]
    fn an_area_with_no_covered_claim_is_listed_and_counted_in_the_headline() {
        let ledger = ledger_of(vec![
            area(
                "permission",
                "Permission layer",
                vec![uncovered("a deny rule still denies", None)],
            ),
            area(
                "prompts",
                "Prompt behaviour",
                vec![covered("ask mode refuses an edit", &["prompts/ask"])],
            ),
            area(
                "sandbox",
                "Sandbox",
                vec![product_blocked(
                    "the network is closed off",
                    "zerostack never unshares the network.",
                    None,
                )],
            ),
        ]);
        let page = page_of(&report_running(&["prompts/ask"]), &ledger);
        assert!(
            page.contains("Permission layer"),
            "an area with no covered claim was dropped:\n{page}"
        );
        assert!(
            page.contains("Sandbox"),
            "an area with no covered claim was dropped:\n{page}"
        );
        assert!(
            page.contains("2 of 3"),
            "the headline does not count the two areas with no scenario:\n{page}"
        );
    }

    // 3.1 — file order is presentation order (the ledger's own rule), so the
    // section follows the ledger and sorts nothing. Titles are deliberately
    // not in alphabetical order, in either direction, so a sort would show.
    #[test]
    fn the_coverage_section_follows_file_order_with_no_sorting() {
        let areas = vec![
            area(
                "session",
                "Session",
                vec![uncovered("a session resumes", None)],
            ),
            area(
                "hooks",
                "Hooks",
                vec![uncovered("a hook fires before a tool call", None)],
            ),
            area(
                "memory",
                "Memory",
                vec![uncovered("a fact is recalled", None)],
            ),
        ];
        let at = |page: &str, title: &str| {
            page.find(&format!("<h3>{title}"))
                .unwrap_or_else(|| panic!("no area {title}:\n{page}"))
        };

        let page = page_of(&report_running(&[]), &ledger_of(areas.clone()));
        assert!(
            at(&page, "Session") < at(&page, "Hooks") && at(&page, "Hooks") < at(&page, "Memory"),
            "the section did not follow the ledger's order:\n{page}"
        );

        let reversed: Vec<Area> = areas.into_iter().rev().collect();
        let page = page_of(&report_running(&[]), &ledger_of(reversed));
        assert!(
            at(&page, "Memory") < at(&page, "Hooks") && at(&page, "Hooks") < at(&page, "Session"),
            "reordering the ledger did not reorder the section:\n{page}"
        );
    }

    // 3.1 — no ratio and no percentage, anywhere the page states something
    // (design D12): fine-grained covered claims sit beside coarse uncovered
    // ones, so a ratio over them is manipulable by re-slicing. The stylesheet
    // is excluded because a `%` there is a length unit and states nothing
    // about coverage.
    #[test]
    fn no_ratio_or_percentage_appears_in_the_page() {
        let ledger = ledger_of(vec![
            area(
                "prompts",
                "Prompt behaviour",
                vec![covered("ask mode refuses an edit", &["prompts/ask"])],
            ),
            area(
                "permission",
                "Permission layer",
                vec![uncovered("a deny rule still denies", None)],
            ),
        ]);
        let page = page_of(&report_running(&["prompts/ask"]), &ledger);
        let at = page
            .find("</style>")
            .unwrap_or_else(|| panic!("no stylesheet:\n{page}"));
        let body = &page[at..];
        assert!(!body.contains('%'), "the page states a percentage:\n{body}");
        assert!(
            !body.to_lowercase().contains("percent"),
            "the page states a percentage:\n{body}"
        );
    }

    // 3.1 / 3.3 — the derivation is per cited id, not per claim: the ids this
    // run exercised are unmarked, the one it did not is marked, and the claim
    // itself stays covered (design D6 — covered is an existence claim, and
    // run membership is a fact about the report).
    #[test]
    fn a_partially_exercised_covered_claim_marks_only_the_missing_ids() {
        let ledger = ledger_of(vec![area(
            "prompts",
            "Prompt behaviour",
            vec![covered(
                "the three prompt modes each refuse what they forbid",
                &["prompts/ask", "prompts/plan", "prompts/edit"],
            )],
        )]);
        let page = page_of(&report_running(&["prompts/ask", "prompts/edit"]), &ledger);

        const MARK: &str = "not exercised by this run";
        assert!(
            id_row(&page, "prompts/plan").contains(MARK),
            "the id this run never ran is not marked:\n{page}"
        );
        assert!(
            !id_row(&page, "prompts/ask").contains(MARK),
            "an exercised id was marked as not exercised:\n{page}"
        );
        assert!(
            !id_row(&page, "prompts/edit").contains(MARK),
            "an exercised id was marked as not exercised:\n{page}"
        );
        assert!(
            !page.contains("status uncovered"),
            "a partially exercised claim was reported as uncovered:\n{page}"
        );
    }

    // 3.2 — each status carries the evidence it owes, and no more: the cited
    // ids for `covered`, the `blocked_by` sentence when an `uncovered` claim
    // has one, the `reason` for `product-blocked` and `excluded`, the `zs`
    // pointer where there is one, and any `note`.
    #[test]
    fn each_status_carries_the_evidence_it_owes() {
        let ledger = ledger_of(vec![area(
            "sandbox",
            "Sandbox",
            vec![
                noted(
                    covered("a sandboxed command is confined", &["sandbox/confine"]),
                    "overlaps the permission calibration on purpose.",
                ),
                uncovered(
                    "a missing sandbox binary refuses the command",
                    Some("the harness cannot remove the sandbox binary from the run's PATH."),
                ),
                uncovered("credential directories are hidden", None),
                product_blocked(
                    "the network is closed off",
                    "zerostack never unshares the network.",
                    Some("sandbox hardening"),
                ),
                excluded(
                    "the kernel enforces the confinement",
                    "that measures the OS, not zerostack.",
                ),
            ],
        )]);
        let page = page_of(&report_running(&["sandbox/confine"]), &ledger);

        assert!(
            page.contains("sandbox/confine"),
            "a covered claim's cited id is missing:\n{page}"
        );
        assert!(
            page.contains("the harness cannot remove the sandbox binary from the run&#39;s PATH."),
            "an uncovered claim's blocked_by sentence is missing:\n{page}"
        );
        assert!(
            page.contains("zerostack never unshares the network."),
            "a product-blocked claim's reason is missing:\n{page}"
        );
        assert!(
            page.contains("sandbox hardening"),
            "a product-blocked claim's zs pointer is missing:\n{page}"
        );
        assert!(
            page.contains("that measures the OS, not zerostack."),
            "an excluded claim's reason is missing:\n{page}"
        );
        assert!(
            page.contains("overlaps the permission calibration on purpose."),
            "a claim's note is missing:\n{page}"
        );
        // The presence of `blocked_by` is itself the fact, so the claim
        // without one says nothing rather than saying it emptily.
        assert_eq!(
            page.matches("blocked by").count(),
            1,
            "an uncovered claim with no blocked_by rendered the label anyway:\n{page}"
        );
    }

    #[test]
    fn the_page_references_nothing_external() {
        let page = page_of(&report("zerostack 1.7.2"), &ledger("that measures the OS."));
        // The counterweight to the absences below: a page with no styling at
        // all would satisfy every one of them.
        assert!(page.contains("<style>"), "the CSS is not inline:\n{page}");
        for pattern in [
            "http://", "https://", "<script", "<link", "<img", "<iframe", " src=", "@import",
            "url(",
        ] {
            assert!(
                !page.contains(pattern),
                "the page reaches outside itself ({pattern}):\n{page}"
            );
        }
    }
}
