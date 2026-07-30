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
//! Section order is header, coverage, results (design D11): identity first,
//! because every number under it is only meaningful once you know what
//! produced it, and coverage before results, because a reader who meets the
//! scores first reads coverage as a caveat when the intended reading is the
//! reverse.

use std::collections::BTreeSet;

use crate::coverage::{Area, Ledger, Status};
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
.headline { margin: 0 0 1.5rem; font-weight: 600; }
.claims > li { margin: 0.6rem 0; }
.status {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 0.8rem;
  padding: 0 0.3rem;
  border: 1px solid rgba(128, 128, 128, 0.4);
  border-radius: 0.2rem;
}
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

/// The whole page for one report and the ledger it is read against.
pub fn render(report: &Report, ledger: &Ledger) -> String {
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
    render_header(&mut page, report);
    render_coverage(&mut page, report, ledger);
    render_results(&mut page);
    page.raw(
        r#"</body>
</html>
"#,
    );
    page.finish()
}

/// The run's identity, read back field by field with no inference, no
/// defaulting, and no computed substitute (spec: "The header reads report
/// fields back without deriving them"; design D5). Every value here is
/// `report`'s own, verbatim; the only choice this function makes is how to
/// spell an absent or empty one so the two are never confused.
fn render_header(page: &mut Page, report: &Report) {
    page.raw(
        r#"<section id="header">
<h1>zseval</h1>
<dl>
<dt>zerostack</dt><dd>"#,
    );
    page.text(&report.zs_version);
    page.raw(
        r#"</dd>
<dt>binary sha256</dt><dd>"#,
    );
    page.text(&report.zs_bin_sha256);
    page.raw(
        r#"</dd>
<dt>binary path</dt><dd>"#,
    );
    page.text(&report.zs_bin_path);
    page.raw(
        r#"</dd>
<dt>git sha</dt><dd>"#,
    );
    render_opt_str(page, report.git_sha.as_deref());
    page.raw(
        r#"</dd>
<dt>features</dt><dd>"#,
    );
    render_opt_list(page, report.features.as_deref());
    page.raw(
        r#"</dd>
<dt>model</dt><dd>"#,
    );
    page.text(&report.model);
    page.raw(
        r#"</dd>
<dt>backend</dt><dd>"#,
    );
    page.text(&report.backend);
    page.raw(
        r#"</dd>
<dt>target</dt><dd>"#,
    );
    page.text(&report.target);
    page.raw(
        r#"</dd>
<dt>timestamp</dt><dd>"#,
    );
    page.text(&report.timestamp);
    page.raw(
        r#"</dd>
<dt>trials</dt><dd>"#,
    );
    page.text(&report.trials.to_string());
    page.raw(
        r#"</dd>
<dt>total cost</dt><dd>"#,
    );
    page.text(&format!("${:.4}", report.summary.total_cost_usd));
    page.raw(
        r#"</dd>
<dt>budget truncated</dt><dd>"#,
    );
    page.raw(if report.budget_truncated { "yes" } else { "no" });
    page.raw(
        r#"</dd>
<dt>configured</dt><dd>"#,
    );
    // The judge is two facts, never collapsed into one (design D5): what the
    // run was told to grade with (`judge_file`, `judge_hash`), labelled apart
    // from what actually graded (`judge_model`) below.
    page.text(&report.judge_file);
    page.raw(" (hash: ");
    render_opt_str(page, report.judge_hash.as_deref());
    page.raw(
        r#")</dd>
<dt>graded by</dt><dd>"#,
    );
    render_judge_model(page, report.judge_model.as_deref());
    page.raw(
        r#"</dd>
</dl>
</section>
"#,
    );
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
/// The section derives exactly one thing, and it derives it from `report`
/// rather than from the ledger: which of a `covered` claim's cited ids this
/// run did not exercise (design D6). Everything else is a read-back, which is
/// why an area no scenario touches still gets its row — that row is the fact a
/// suite-derived count structurally cannot state, and dropping it is the
/// failure this whole capability exists to prevent.
fn render_coverage(page: &mut Page, report: &Report, ledger: &Ledger) {
    let exercised: BTreeSet<&str> = report.scenarios.iter().map(|s| s.id.as_str()).collect();
    page.raw(
        r#"<section id="coverage">
<h2>Coverage</h2>
"#,
    );
    render_headline(page, ledger);
    // File order is presentation order: nothing here sorts.
    for area in ledger.areas() {
        page.raw("<h3>");
        page.text(&area.title);
        // The headline counts these areas; naming them where they sit is what
        // lets a reader find the ones it counted.
        if !has_scenario(area) {
            page.raw(r#" <span class="mark">no scenario at all</span>"#);
        }
        page.raw("</h3>\n<ul class=\"claims\">\n");
        for claim in &area.claims {
            page.raw(r#"<li><span class="status">"#);
            page.raw(status_label(&claim.status));
            page.raw("</span> ");
            page.text(&claim.claim);
            render_evidence(page, &claim.status, &exercised);
            // Free text on any status, so it is rendered outside the match
            // rather than repeated in four arms.
            if let Some(note) = &claim.note {
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
fn render_headline(page: &mut Page, ledger: &Ledger) {
    let empty = ledger.areas().iter().filter(|a| !has_scenario(a)).count();
    page.raw(r#"<p class="headline">Areas with no scenario at all: "#);
    page.text(&empty.to_string());
    page.raw(" of ");
    page.text(&ledger.areas().len().to_string());
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
/// the label enters the buffer as markup does.
fn status_label(status: &Status) -> &'static str {
    match status {
        Status::Covered { .. } => "covered",
        Status::Uncovered { .. } => "uncovered",
        Status::ProductBlocked { .. } => "product-blocked",
        Status::Excluded { .. } => "excluded",
    }
}

/// The evidence a status owes, and only that. The match is exhaustive over
/// `Status` on purpose: each variant carries its own fields and no others, so
/// a fifth status could not be added without deciding here what it shows.
///
/// `uncovered`'s `blocked_by` is the one field whose *presence* is the fact,
/// so a claim without one renders nothing rather than an empty label: the
/// absence says the claim is buildable today and simply unbuilt.
fn render_evidence(page: &mut Page, status: &Status, exercised: &BTreeSet<&str>) {
    match status {
        Status::Covered { scenarios } => {
            page.raw("\n<ul class=\"ids\">\n");
            for id in scenarios {
                page.raw("<li><code>");
                page.text(id);
                page.raw("</code>");
                // Derived from this report, never recorded in the ledger
                // (design D6). The drift gate has already established that
                // every cited id exists in the tree, so "exists, and this run
                // did not exercise it" is the only reading left.
                if !exercised.contains(id.as_str()) {
                    page.raw(r#" <span class="mark">not exercised by this run</span>"#);
                }
                page.raw("</li>\n");
            }
            page.raw("</ul>");
        }
        Status::Uncovered { blocked_by } => {
            if let Some(blocked_by) = blocked_by {
                page.raw("\n<p class=\"evidence\">blocked by: ");
                page.text(blocked_by);
                page.raw("</p>");
            }
        }
        Status::ProductBlocked { reason, zs } => {
            page.raw("\n<p class=\"evidence\">");
            page.text(reason);
            page.raw("</p>");
            if let Some(zs) = zs {
                page.raw("\n<p class=\"evidence\">tracked as: ");
                page.text(zs);
                page.raw("</p>");
            }
        }
        Status::Excluded { reason } => {
            page.raw("\n<p class=\"evidence\">");
            page.text(reason);
            page.raw("</p>");
        }
    }
}

fn render_results(page: &mut Page) {
    page.raw(
        r#"<section id="results">
<h2>Results</h2>
</section>
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
        let page = render(&report(banner), &ledger("that measures the OS."));
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
        let page = render(&report("zerostack 1.7.2"), &ledger(reason));
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
    fn the_page_runs_header_then_coverage_then_results() {
        let page = render(&report("zerostack 1.7.2"), &ledger("that measures the OS."));
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
            header < coverage && coverage < results,
            "sections are out of order (design D11):\n{page}"
        );
    }

    #[test]
    fn an_unavailable_build_fact_is_not_shown_as_empty() {
        // `report("...")`'s `ZsIdentity` defaults `git_sha` and `features` to
        // `None`, the same as today's real binary (design D5).
        let page = render(&report("zerostack 1.7.2"), &ledger("that measures the OS."));
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
        let page = render(&report, &ledger("that measures the OS."));
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
        let page = render(&report, &ledger("that measures the OS."));
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
        let page = render(&report, &ledger("that measures the OS."));
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
        let page = render(&report_running(&["prompts/ask"]), &ledger);
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

        let page = render(&report_running(&[]), &ledger_of(areas.clone()));
        assert!(
            at(&page, "Session") < at(&page, "Hooks") && at(&page, "Hooks") < at(&page, "Memory"),
            "the section did not follow the ledger's order:\n{page}"
        );

        let reversed: Vec<Area> = areas.into_iter().rev().collect();
        let page = render(&report_running(&[]), &ledger_of(reversed));
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
        let page = render(&report_running(&["prompts/ask"]), &ledger);
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
        let page = render(&report_running(&["prompts/ask", "prompts/edit"]), &ledger);

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
            !page.contains("uncovered"),
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
        let page = render(&report_running(&["sandbox/confine"]), &ledger);

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
        let page = render(&report("zerostack 1.7.2"), &ledger("that measures the OS."));
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
