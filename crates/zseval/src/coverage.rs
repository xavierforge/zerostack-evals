//! Coverage ledger: what the suite measures, and what it does not.
//!
//! The suite answers "how well does zerostack do the things we test?".
//! `scenarios/coverage.toml` answers the other half — "what don't we test?" —
//! by enumerating the product's functional surface as *areas*, each holding
//! claims with one of four statuses. The denominator is the product, never the
//! set of scenarios that happen to exist: an area with no scenario at all is
//! the ledger's most important row, and a suite-derived count structurally
//! cannot state it.
//!
//! Four rules are load-bearing enough to be enforced here rather than trusted
//! to the author or re-derived by a renderer:
//!
//!   - **Each status owes its own evidence.** `covered` needs a non-empty
//!     `scenarios`, `product-blocked` and `excluded` need a `reason`, and
//!     evidence belonging to another status is a load error rather than a
//!     silently ignored field. `uncovered`'s optional `blocked_by` is the one
//!     field whose *presence* is the fact: present means a harness gap stands
//!     in the way, absent means the claim is simply unbuilt.
//!   - **`covered` is an existence claim.** It says a scenario tests this, not
//!     that the scenario passes and not that it ran in any given report. So no
//!     score, no run reference, no timestamp lives here.
//!   - **A scenario id backs at most one covered claim, ledger-wide.** Anything
//!     else makes a per-area count ambiguous between three units of coverage
//!     and one unit cited three times; deliberate overlaps go in `note`, which
//!     takes no coverage slot.
//!   - **The ledger and the tree are checked in both directions.** A dead
//!     reference means the ledger cites evidence that no longer exists; an
//!     unclaimed scenario means the ledger has quietly fallen behind the suite.
//!
//! `audited_against` records the zerostack version the judgments were made
//! against and is compared to a report's `--version` banner by containment
//! only. `backend.rs` stores that banner verbatim precisely so upstream's
//! banner shape never becomes a compatibility contract; a parser here would
//! sign that contract on the ledger's behalf. The failure direction is
//! deliberate: a reshaped banner costs a spurious mismatch notice, never a
//! wrong version claim.
//!
//! File order is presentation order (safety boundaries first, not
//! alphabetical), so nothing here sorts and nothing keys areas by name.

use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

/// A loaded ledger. Every invariant the schema can carry is already true of a
/// value of this type: statuses are the closed four, each carries exactly the
/// evidence its status owes, and no scenario id backs two covered claims.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ledger {
    /// The zerostack version string the judgments in this file were made
    /// against — compared to a report's banner by `audit_matches`, never
    /// parsed.
    pub audited_against: String,
    /// The trees that hold scenarios, relative to the repo root. The drift
    /// check reads them in both directions.
    pub scenario_roots: Vec<String>,
    /// File order, which is presentation order.
    pub areas: Vec<Area>,
}

/// One slice of zerostack's functional surface. `area`, not `domain`:
/// `domain` is taken by `scenario.toml`'s `domains = [...]` and
/// `domains/mod.rs`'s `KNOWN_DOMAINS`, and three names (memory, subagents,
/// mcp) would otherwise exist in two vocabularies meaning different things.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Area {
    pub name: String,
    pub title: String,
    pub claims: Vec<Claim>,
}

/// One statement about what the suite does or does not measure, plus the
/// evidence its status owes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    pub claim: String,
    pub status: Status,
    /// Free text. Where a deliberate calibration overlap is worth recording,
    /// this is where it goes — it takes no coverage slot.
    pub note: Option<String>,
}

/// The four statuses, each carrying its own evidence in the type rather than
/// beside it, so a claim that loaded cannot be missing what its status owes.
/// Deliberately no `planned`: scheduling lives in `scenarios/PLAN.md`, and a
/// `planned` without a wave or a date promises something the ledger has no
/// field to back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// A scenario in this repo tests the claim. Existence only: not that it
    /// passes, not that it ran in any particular report.
    Covered { scenarios: Vec<String> },
    /// Not measured. `Some` means a harness gap stands in the way and the
    /// sentence names it in full; `None` means the claim is buildable today
    /// and simply unbuilt. The presence of the field is itself the fact.
    Uncovered { blocked_by: Option<String> },
    /// Not measurable, because of a hole on the zerostack side. `zs` points at
    /// a tracking entry where one exists.
    ProductBlocked { reason: String, zs: Option<String> },
    /// Deliberately never tested. The only irreversible judgment in the file.
    Excluded { reason: String },
}

/// The wire shape, kept separate from the validated types above.
///
/// Serde's tagged-enum forms cannot express this schema: an internally tagged
/// `status` would have to be `#[serde(flatten)]`ed alongside `claim` and
/// `note`, and `flatten` and `deny_unknown_fields` do not compose — which
/// would trade the unknown-key rejection (a hard requirement) for the
/// per-status shape. So the file is read flat with every evidence field
/// optional, and `RawClaim::validate` is what makes the combination legal or
/// a load error. Unknown keys are still rejected by serde, per struct.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLedger {
    audited_against: String,
    scenario_roots: Vec<String>,
    areas: Vec<RawArea>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawArea {
    name: String,
    title: String,
    claims: Vec<RawClaim>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawClaim {
    claim: String,
    status: String,
    note: Option<String>,
    scenarios: Option<Vec<String>>,
    blocked_by: Option<String>,
    reason: Option<String>,
    zs: Option<String>,
}

/// Evidence that belongs to another status is rejected, not ignored: a
/// `reason` sitting under a `covered` claim was written to be read, and
/// dropping it silently loses the author's meaning while the page renders as
/// if nothing were wrong.
///
/// Each status names only the fields it *owns*, and the foreign set is the
/// complement, derived here against `present`. Naming the complement per
/// status was the earlier shape, and it meant four hand-maintained tables with
/// nothing deriving one from another, where a dropped row silently reopened
/// the very hole this check closes. Under the derivation a dropped row costs a
/// status one of its own fields instead, which its required-evidence check
/// catches on the next test run.
fn reject_foreign_evidence(
    at: &str,
    status: &str,
    owned: &[&str],
    present: &[(&str, bool)],
) -> Result<()> {
    let foreign: Vec<&str> = present
        .iter()
        .filter(|(name, is_present)| *is_present && !owned.contains(name))
        .map(|(name, _)| *name)
        .collect();
    if foreign.is_empty() {
        return Ok(());
    }
    bail!(
        "{at}: status = \"{status}\" does not take `{}` — that field is another status's evidence",
        foreign.join("`, `")
    );
}

impl RawClaim {
    fn validate(self, area: &str) -> Result<Claim> {
        let RawClaim {
            claim,
            status,
            note,
            scenarios,
            blocked_by,
            reason,
            zs,
        } = self;
        let at = format!("area '{area}', claim '{claim}'");
        // Every evidence field a claim can carry, and whether this claim
        // carried it. The single list the four statuses are checked against, so
        // adding a field to `RawClaim` and forgetting it here shows up as that
        // field being accepted under every status at once rather than as a hole
        // under one of them.
        let present = [
            ("scenarios", scenarios.is_some()),
            ("blocked_by", blocked_by.is_some()),
            ("reason", reason.is_some()),
            ("zs", zs.is_some()),
        ];
        let status = match status.as_str() {
            "covered" => {
                reject_foreign_evidence(&at, "covered", &["scenarios"], &present)?;
                let scenarios = scenarios.unwrap_or_default();
                if scenarios.is_empty() {
                    bail!(
                        "{at}: status = \"covered\" requires a non-empty `scenarios` array — \
                         covered means a scenario in this repo tests the claim, so the ids are \
                         the whole of the evidence"
                    );
                }
                Status::Covered { scenarios }
            }
            "uncovered" => {
                reject_foreign_evidence(&at, "uncovered", &["blocked_by"], &present)?;
                Status::Uncovered { blocked_by }
            }
            "product-blocked" => {
                reject_foreign_evidence(&at, "product-blocked", &["reason", "zs"], &present)?;
                let Some(reason) = reason else {
                    bail!(
                        "{at}: status = \"product-blocked\" requires a one-line `reason` naming \
                         the hole on the zerostack side"
                    );
                };
                Status::ProductBlocked { reason, zs }
            }
            "excluded" => {
                reject_foreign_evidence(&at, "excluded", &["reason"], &present)?;
                let Some(reason) = reason else {
                    bail!(
                        "{at}: status = \"excluded\" requires a one-line `reason` for never \
                         testing this — it is the file's only irreversible judgment"
                    );
                };
                Status::Excluded { reason }
            }
            other => bail!(
                "{at}: unknown status '{other}' — the four are covered, uncovered, \
                 product-blocked, excluded (there is deliberately no `planned`: scheduling \
                 lives in scenarios/PLAN.md)"
            ),
        };
        Ok(Claim {
            claim,
            status,
            note,
        })
    }
}

impl Ledger {
    pub fn load(path: &Path) -> Result<Ledger> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        Ledger::parse(&text).with_context(|| format!("parse {}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Ledger> {
        let raw: RawLedger = toml::from_str(text)?;
        if raw.audited_against.trim().is_empty() {
            bail!(
                "coverage ledger: audited_against must not be empty — it is compared to a \
                 report's banner by containment, and an empty string is contained in every \
                 banner, so it would silently claim the ledger was audited against whatever it \
                 is compared to"
            );
        }
        if raw.scenario_roots.is_empty() {
            bail!(
                "coverage ledger: scenario_roots must not be empty — an empty root list makes \
                 the drift check enumerate no scenarios"
            );
        }
        let mut areas = Vec::with_capacity(raw.areas.len());
        for area in raw.areas {
            let RawArea {
                name,
                title,
                claims,
            } = area;
            let mut validated = Vec::with_capacity(claims.len());
            for claim in claims {
                validated.push(claim.validate(&name)?);
            }
            areas.push(Area {
                name,
                title,
                claims: validated,
            });
        }
        let ledger = Ledger {
            audited_against: raw.audited_against,
            scenario_roots: raw.scenario_roots,
            areas,
        };
        ledger.check_unique_ids()?;
        Ok(ledger)
    }

    /// Does `audited_against` appear in this `--version` banner?
    ///
    /// Containment, deliberately: `backend.rs` stores the banner verbatim so
    /// upstream's banner shape never becomes a compatibility contract, and a
    /// version parser here would sign that contract on the ledger's behalf.
    /// A mock-backend report records `mock` and therefore mismatches, which is
    /// the right answer — a page rendered from mock numbers should say so.
    pub fn audit_matches(&self, banner: &str) -> bool {
        banner.contains(&self.audited_against)
    }

    /// Both directions of the drift check against a real tree: every scenario
    /// under `scenario_roots` (resolved against `repo_root`) versus every id
    /// the covered claims cite.
    pub fn check_drift(&self, repo_root: &Path) -> Result<()> {
        let mut tree_ids: Vec<String> = Vec::new();
        for root in &self.scenario_roots {
            let dir = repo_root.join(root);
            if !dir.is_dir() {
                bail!(
                    "coverage ledger: scenario_roots names '{root}', which is not a directory \
                     under {}",
                    repo_root.display()
                );
            }
            for scenario in crate::scenario::discover(&dir)
                .with_context(|| format!("discover scenarios under {}", dir.display()))?
            {
                tree_ids.push(scenario.id);
            }
        }
        self.check_ids(&tree_ids)
    }

    /// Every scenario id cited by a covered claim, in file order.
    fn covered_ids(&self) -> Vec<&str> {
        self.areas
            .iter()
            .flat_map(|area| area.claims.iter())
            .filter_map(|claim| match &claim.status {
                Status::Covered { scenarios } => Some(scenarios.iter().map(|s| s.as_str())),
                _ => None,
            })
            .flatten()
            .collect()
    }

    /// The drift check proper, over ids rather than a tree, so both directions
    /// are one comparison. Failures name every offending id in one report:
    /// fixing a dead reference only to be told about an unclaimed scenario on
    /// the next run hides half the drift behind the other half.
    fn check_ids(&self, tree_ids: &[String]) -> Result<()> {
        let claimed = self.covered_ids();
        let dead: Vec<&str> = claimed
            .iter()
            .copied()
            .filter(|id| !tree_ids.iter().any(|in_tree| in_tree.as_str() == *id))
            .collect();
        let unclaimed: Vec<&str> = tree_ids
            .iter()
            .map(|id| id.as_str())
            .filter(|id| !claimed.contains(id))
            .collect();
        if dead.is_empty() && unclaimed.is_empty() {
            return Ok(());
        }
        let roots = self.scenario_roots.join(", ");
        let mut msg = String::from("coverage ledger drift against the scenario tree:");
        if !dead.is_empty() {
            msg.push_str(&format!(
                "\n  dead references (cited by a covered claim, no such scenario under {roots}): \
                 {}",
                dead.join(", ")
            ));
        }
        if !unclaimed.is_empty() {
            msg.push_str(&format!(
                "\n  unclaimed scenarios (under {roots}, cited by no covered claim): {}",
                unclaimed.join(", ")
            ));
        }
        bail!("{msg}")
    }

    /// A scenario id backs at most one covered claim, ledger-wide. Without the
    /// rule a reader seeing one id under three claims cannot tell three units
    /// of coverage from one cited three times, and a renderer that quietly
    /// de-duplicates hides a decision the author never made. Every offending
    /// id is reported at once, with both claims named, since fixing them one
    /// per run is the same hidden-drift failure the drift check avoids.
    fn check_unique_ids(&self) -> Result<()> {
        // (id, area name, claim), in file order.
        let mut refs: Vec<(&str, &str, &str)> = Vec::new();
        for area in &self.areas {
            for claim in &area.claims {
                if let Status::Covered { scenarios } = &claim.status {
                    for id in scenarios {
                        refs.push((id.as_str(), area.name.as_str(), claim.claim.as_str()));
                    }
                }
            }
        }
        let mut reported: Vec<&str> = Vec::new();
        let mut duplicates: Vec<String> = Vec::new();
        for &(id, _, _) in &refs {
            if reported.contains(&id) {
                continue;
            }
            let claimed_by: Vec<String> = refs
                .iter()
                .filter(|(other, _, _)| *other == id)
                .map(|(_, area, claim)| format!("'{claim}' (area {area})"))
                .collect();
            if claimed_by.len() > 1 {
                reported.push(id);
                duplicates.push(format!("'{id}' is claimed by {}", claimed_by.join(" and ")));
            }
        }
        if duplicates.is_empty() {
            return Ok(());
        }
        bail!(
            "coverage ledger: a scenario id may back at most one covered claim — a deliberate \
             overlap goes in `note`, which takes no coverage slot: {}",
            duplicates.join("; ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ledger with the header already filled in, so each test writes only
    /// the body it is actually about.
    fn ledger(body: &str) -> String {
        format!(
            r#"audited_against = "1.7.2"
scenario_roots = ["scenarios"]

{body}
"#
        )
    }

    /// One area holding exactly one claim, written as that claim's own keys.
    fn one_claim(keys: &str) -> String {
        ledger(&format!(
            r#"[[areas]]
name = "permission"
title = "Permission layer"

[[areas.claims]]
{keys}"#
        ))
    }

    fn err(text: &str) -> String {
        format!("{:#}", Ledger::parse(text).unwrap_err())
    }

    #[test]
    fn loads_a_well_formed_ledger_in_file_order() {
        let text = r#"
audited_against = "1.7.2"
scenario_roots = ["scenarios", "examples/prompt-pack"]

[[areas]]
name = "permission"
title = "Permission layer"

[[areas.claims]]
claim = "ask mode refuses an edit"
status = "covered"
scenarios = ["ask-readonly-refuses-edit", "ask-readonly-refuses-write"]
note = "counted once; the tool-layer probe overlaps"

[[areas.claims]]
claim = "a deny rule beats an allow rule"
status = "uncovered"

[[areas]]
name = "sandbox"
title = "Sandbox"

[[areas.claims]]
claim = "a write outside the workspace is refused"
status = "product-blocked"
reason = "zerostack does not report the sandbox decision in headless mode."
zs = "sandbox hardening"

[[areas.claims]]
claim = "a symlink out of the workspace is refused"
status = "excluded"
reason = "That measures the OS, not zerostack."
"#;
        let l = Ledger::parse(text).unwrap();
        assert_eq!(l.audited_against, "1.7.2");
        assert_eq!(l.scenario_roots, ["scenarios", "examples/prompt-pack"]);
        // File order, not alphabetical: sandbox stays second.
        assert_eq!(
            l.areas.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
            ["permission", "sandbox"]
        );
        assert_eq!(l.areas[0].title, "Permission layer");
        assert_eq!(l.areas[0].claims.len(), 2);
        assert_eq!(
            l.areas[0].claims[0].status,
            Status::Covered {
                scenarios: vec![
                    "ask-readonly-refuses-edit".to_string(),
                    "ask-readonly-refuses-write".to_string(),
                ],
            }
        );
        assert_eq!(
            l.areas[0].claims[0].note.as_deref(),
            Some("counted once; the tool-layer probe overlaps")
        );
        assert_eq!(
            l.areas[0].claims[1].status,
            Status::Uncovered { blocked_by: None }
        );
        assert_eq!(
            l.areas[1].claims[0].claim,
            "a write outside the workspace is refused"
        );
        assert_eq!(
            l.areas[1].claims[0].status,
            Status::ProductBlocked {
                reason: "zerostack does not report the sandbox decision in headless mode."
                    .to_string(),
                zs: Some("sandbox hardening".to_string()),
            }
        );
        assert_eq!(
            l.areas[1].claims[1].status,
            Status::Excluded {
                reason: "That measures the OS, not zerostack.".to_string(),
            }
        );
    }

    #[test]
    fn an_unknown_key_at_the_top_level_fails_naming_it() {
        let text = ledger(
            r#"generated_at = "2026-07-29"

[[areas]]
name = "permission"
title = "Permission layer"

[[areas.claims]]
claim = "ask mode refuses an edit"
status = "uncovered""#,
        );
        assert!(err(&text).contains("generated_at"), "{}", err(&text));
    }

    #[test]
    fn an_unknown_key_on_an_area_fails_naming_it() {
        let text = ledger(
            r#"[[areas]]
name = "permission"
title = "Permission layer"
wave = 1

[[areas.claims]]
claim = "ask mode refuses an edit"
status = "uncovered""#,
        );
        assert!(err(&text).contains("wave"), "{}", err(&text));
    }

    #[test]
    fn an_unknown_key_on_a_claim_fails_naming_it() {
        let text = one_claim(
            r#"claim = "ask mode refuses an edit"
status = "uncovered"
gap = "A""#,
        );
        assert!(err(&text).contains("gap"), "{}", err(&text));
    }

    #[test]
    fn a_missing_audited_against_fails_naming_it() {
        let text = r#"
scenario_roots = ["scenarios"]

[[areas]]
name = "permission"
title = "Permission layer"

[[areas.claims]]
claim = "ask mode refuses an edit"
status = "uncovered"
"#;
        assert!(err(text).contains("audited_against"), "{}", err(text));
    }

    #[test]
    fn a_missing_scenario_roots_fails_naming_it() {
        let text = r#"
audited_against = "1.7.2"

[[areas]]
name = "permission"
title = "Permission layer"

[[areas.claims]]
claim = "ask mode refuses an edit"
status = "uncovered"
"#;
        assert!(err(text).contains("scenario_roots"), "{}", err(text));
    }

    #[test]
    fn an_empty_audited_against_fails_naming_it() {
        let text = r#"
audited_against = ""
scenario_roots = ["scenarios"]

[[areas]]
name = "permission"
title = "Permission layer"

[[areas.claims]]
claim = "ask mode refuses an edit"
status = "uncovered"
"#;
        assert!(err(text).contains("audited_against"), "{}", err(text));
    }

    #[test]
    fn a_whitespace_only_audited_against_fails_naming_it() {
        let text = r#"
audited_against = "   "
scenario_roots = ["scenarios"]

[[areas]]
name = "permission"
title = "Permission layer"

[[areas.claims]]
claim = "ask mode refuses an edit"
status = "uncovered"
"#;
        assert!(err(text).contains("audited_against"), "{}", err(text));
    }

    #[test]
    fn an_empty_scenario_roots_fails_naming_it() {
        let text = r#"
audited_against = "1.7.2"
scenario_roots = []

[[areas]]
name = "permission"
title = "Permission layer"

[[areas.claims]]
claim = "ask mode refuses an edit"
status = "uncovered"
"#;
        assert!(err(text).contains("scenario_roots"), "{}", err(text));
    }

    #[test]
    fn covered_without_scenarios_fails_naming_the_claim() {
        let text = one_claim(
            r#"claim = "ask mode refuses an edit"
status = "covered""#,
        );
        let msg = err(&text);
        assert!(msg.contains("ask mode refuses an edit"), "{msg}");
        assert!(msg.contains("scenarios"), "{msg}");
    }

    #[test]
    fn covered_with_an_empty_scenarios_array_fails_naming_the_claim() {
        let text = one_claim(
            r#"claim = "ask mode refuses an edit"
status = "covered"
scenarios = []"#,
        );
        let msg = err(&text);
        assert!(msg.contains("ask mode refuses an edit"), "{msg}");
        assert!(msg.contains("scenarios"), "{msg}");
    }

    #[test]
    fn excluded_without_a_reason_fails_naming_the_claim() {
        let text = one_claim(
            r#"claim = "a symlink out of the workspace is refused"
status = "excluded""#,
        );
        let msg = err(&text);
        assert!(
            msg.contains("a symlink out of the workspace is refused"),
            "{msg}"
        );
        assert!(msg.contains("reason"), "{msg}");
    }

    #[test]
    fn product_blocked_without_a_reason_fails_naming_the_claim() {
        let text = one_claim(
            r#"claim = "the prompt a run loaded is reported"
status = "product-blocked""#,
        );
        let msg = err(&text);
        assert!(msg.contains("the prompt a run loaded is reported"), "{msg}");
        assert!(msg.contains("reason"), "{msg}");
    }

    #[test]
    fn uncovered_carrying_scenarios_fails_naming_the_field() {
        let text = one_claim(
            r#"claim = "a deny rule beats an allow rule"
status = "uncovered"
scenarios = ["ask-readonly-refuses-edit"]"#,
        );
        let msg = err(&text);
        assert!(msg.contains("a deny rule beats an allow rule"), "{msg}");
        assert!(msg.contains("scenarios"), "{msg}");
    }

    /// `reject_foreign_evidence` derives each status's foreign set as the
    /// complement of the fields that status owns, against one shared `present`
    /// list. That derivation removed the four hand-maintained complement
    /// tables this test was first written to guard, and left two smaller ways
    /// to be wrong in their place: an `owned` list naming a field the status
    /// has no business accepting, and a field added to `RawClaim` but left out
    /// of `present`, which would be accepted under all four statuses at once.
    /// Neither is visible to any single case, so this still walks all eleven
    /// status/foreign-field pairs.
    #[test]
    fn every_status_rejects_every_other_statuss_evidence() {
        // (status, its own required evidence, a foreign field, that field's value)
        let cases: &[(&str, &str, &str, &str)] = &[
            ("covered", r#"scenarios = ["x"]"#, "blocked_by", r#""b""#),
            ("covered", r#"scenarios = ["x"]"#, "reason", r#""r""#),
            ("covered", r#"scenarios = ["x"]"#, "zs", r#""z""#),
            ("uncovered", "", "scenarios", r#"["s"]"#),
            ("uncovered", "", "reason", r#""r""#),
            ("uncovered", "", "zs", r#""z""#),
            (
                "product-blocked",
                r#"reason = "r""#,
                "scenarios",
                r#"["s"]"#,
            ),
            ("product-blocked", r#"reason = "r""#, "blocked_by", r#""b""#),
            ("excluded", r#"reason = "r""#, "scenarios", r#"["s"]"#),
            ("excluded", r#"reason = "r""#, "blocked_by", r#""b""#),
            ("excluded", r#"reason = "r""#, "zs", r#""z""#),
        ];
        for (status, own_evidence, foreign_key, foreign_value) in cases {
            let keys = format!(
                "claim = \"x\"\nstatus = \"{status}\"\n{own_evidence}\n{foreign_key} = \
                 {foreign_value}"
            );
            let text = one_claim(&keys);
            let msg = err(&text);
            assert!(
                msg.contains(foreign_key),
                "status '{status}' + foreign field '{foreign_key}' should fail naming it: {msg}"
            );
        }
    }

    #[test]
    fn an_unknown_status_fails_naming_it() {
        let text = one_claim(
            r#"claim = "a deny rule beats an allow rule"
status = "planned""#,
        );
        let msg = err(&text);
        assert!(msg.contains("planned"), "{msg}");
        assert!(msg.contains("a deny rule beats an allow rule"), "{msg}");
    }

    #[test]
    fn uncovered_records_blockage_by_presence() {
        // Same claim text, same (absent) note: the two claims are built to
        // differ in exactly one field, which is the field under test.
        let text = ledger(
            r#"[[areas]]
name = "permission"
title = "Permission layer"

[[areas.claims]]
claim = "per-scenario CLI flags"
status = "uncovered"
blocked_by = "The backend hardcodes -p --yolo per call; scenario.toml has no field to add or drop a flag."

[[areas.claims]]
claim = "per-scenario CLI flags"
status = "uncovered""#,
        );
        let l = Ledger::parse(&text).unwrap();
        let (blocked, open) = (&l.areas[0].claims[0], &l.areas[0].claims[1]);
        assert_eq!(blocked.claim, open.claim);
        assert_eq!(blocked.note, open.note);
        assert_ne!(blocked.status, open.status);
        assert_eq!(
            blocked.status,
            Status::Uncovered {
                blocked_by: Some(
                    "The backend hardcodes -p --yolo per call; scenario.toml has no field to add \
                     or drop a flag."
                        .to_string()
                ),
            }
        );
        assert_eq!(open.status, Status::Uncovered { blocked_by: None });
    }

    #[test]
    fn a_duplicate_id_within_one_area_fails_naming_the_id_and_both_claims() {
        let text = ledger(
            r#"[[areas]]
name = "prompts"
title = "Prompt behaviour"

[[areas.claims]]
claim = "ask mode refuses an edit"
status = "covered"
scenarios = ["ask-readonly-refuses-edit"]

[[areas.claims]]
claim = "ask mode explains itself"
status = "covered"
scenarios = ["ask-readonly-refuses-edit"]"#,
        );
        let msg = err(&text);
        assert!(msg.contains("ask-readonly-refuses-edit"), "{msg}");
        assert!(msg.contains("ask mode refuses an edit"), "{msg}");
        assert!(msg.contains("ask mode explains itself"), "{msg}");
    }

    #[test]
    fn a_duplicate_id_across_areas_fails_naming_the_id_and_both_claims() {
        let text = ledger(
            r#"[[areas]]
name = "prompts"
title = "Prompt behaviour"

[[areas.claims]]
claim = "ask mode refuses an edit"
status = "covered"
scenarios = ["ask-readonly-refuses-edit"]

[[areas]]
name = "tool-use"
title = "Tool use"

[[areas.claims]]
claim = "the write tool is not called in ask mode"
status = "covered"
scenarios = ["ask-readonly-refuses-edit"]"#,
        );
        let msg = err(&text);
        assert!(msg.contains("ask-readonly-refuses-edit"), "{msg}");
        assert!(msg.contains("ask mode refuses an edit"), "{msg}");
        assert!(
            msg.contains("the write tool is not called in ask mode"),
            "{msg}"
        );
    }

    /// Containment, not version comparison: the ledger never learns the shape
    /// of upstream's banner, and a mock-backend report legitimately mismatches.
    #[test]
    fn audit_matches_is_containment_only() {
        let text = one_claim(
            r#"claim = "ask mode refuses an edit"
status = "uncovered""#,
        );
        let l = Ledger::parse(&text).unwrap();
        assert!(l.audit_matches("zerostack 1.7.2"));
        assert!(!l.audit_matches("zerostack 1.7.4"));
        assert!(!l.audit_matches("mock"));
    }

    fn two_covered(first: &str, second: &str) -> Ledger {
        let text = ledger(&format!(
            r#"[[areas]]
name = "prompts"
title = "Prompt behaviour"

[[areas.claims]]
claim = "ask mode refuses an edit"
status = "covered"
scenarios = ["{first}"]

[[areas]]
name = "memory"
title = "Memory"

[[areas.claims]]
claim = "a fact written in one session is recalled in the next"
status = "covered"
scenarios = ["{second}"]"#
        ));
        Ledger::parse(&text).unwrap()
    }

    #[test]
    fn the_drift_check_passes_when_every_scenario_is_claimed_exactly_once() {
        let l = two_covered("ask-readonly-refuses-edit", "memory-recall");
        l.check_ids(&[
            "memory-recall".to_string(),
            "ask-readonly-refuses-edit".to_string(),
        ])
        .unwrap();
    }

    /// Both directions in one report: a run that fixed the dead reference only
    /// to be told about the unclaimed scenario next time would hide half the
    /// drift behind the other half.
    #[test]
    fn the_drift_check_reports_dead_references_and_unclaimed_scenarios_together() {
        let l = two_covered("renamed-away", "also-gone");
        let err = l
            .check_ids(&["memory-recall".to_string(), "session-resume".to_string()])
            .unwrap_err();
        let msg = format!("{err:#}");
        for id in [
            "renamed-away",
            "also-gone",
            "memory-recall",
            "session-resume",
        ] {
            assert!(msg.contains(id), "{msg}");
        }
    }
}
