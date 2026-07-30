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

use crate::coverage::{Claim, Ledger, Status};
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
table { border-collapse: collapse; font-variant-numeric: tabular-nums; }
th, td { padding: 0.25rem 0.6rem; text-align: right; border-bottom: 1px solid rgba(128, 128, 128, 0.3); }
th:first-child, td:first-child { text-align: left; }
.reason { opacity: 0.75; }
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
    render_coverage(&mut page, ledger);
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

fn render_coverage(page: &mut Page, ledger: &Ledger) {
    page.raw(
        r#"<section id="coverage">
<h2>Coverage</h2>
"#,
    );
    // File order is presentation order: nothing here sorts.
    for area in ledger.areas() {
        page.raw("<h3>");
        page.text(&area.title);
        page.raw("</h3>\n<ul>\n");
        for claim in &area.claims {
            page.raw("<li>");
            page.text(&claim.claim);
            if let Some(reason) = reason_of(claim) {
                page.raw(r#" <span class="reason">"#);
                page.text(reason);
                page.raw("</span>");
            }
            page.raw("</li>\n");
        }
        page.raw("</ul>\n");
    }
    page.raw("</section>\n");
}

/// The reason the two judgment statuses owe. `covered` and `uncovered` carry
/// their own evidence instead and are rendered from it.
fn reason_of(claim: &Claim) -> Option<&str> {
    match &claim.status {
        Status::ProductBlocked { reason, .. } | Status::Excluded { reason } => Some(reason),
        Status::Covered { .. } | Status::Uncovered { .. } => None,
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
    use crate::verdict::{ReportMeta, ZsIdentity};

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

    /// A one-area, one-claim ledger built in code rather than parsed, so a
    /// `reason` full of markup does not have to survive TOML's own escaping
    /// on the way in.
    fn ledger(reason: &str) -> Ledger {
        Ledger::new(
            "1.7.2".into(),
            vec!["scenarios".into()],
            vec![Area {
                name: "permission".into(),
                title: "Permission layer".into(),
                claims: vec![Claim {
                    claim: "a symlink out of the workspace is refused".into(),
                    status: Status::Excluded {
                        reason: reason.into(),
                    },
                    note: None,
                }],
            }],
        )
        .unwrap()
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
