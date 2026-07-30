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
//! These rules are load-bearing enough to be enforced here rather than trusted
//! to the author or re-derived by a renderer. Every one of them belongs to the
//! type, not to the load path: `Ledger::new` is the only constructor and runs
//! them all, so a fixture built in code is held to what a file is held to.
//!
//!   - **Each status owes its own evidence.** `covered` needs a non-empty
//!     `scenarios`, `product-blocked` and `excluded` need a `reason`, and a
//!     field that is present but blank is absent: every one of them exists to
//!     say something. `uncovered`'s optional `blocked_by` is the one field
//!     whose *presence* is the fact: present means a harness gap stands in the
//!     way and the sentence names it, absent means the claim is simply
//!     unbuilt. Evidence belonging to *another* status is a load error rather
//!     than a silently ignored field — the one rule of the five the wire shape
//!     needs and the type does not, since each `Status` variant carries its
//!     own fields and no others.
//!   - **`covered` is an existence claim.** It says a scenario tests this, not
//!     that the scenario passes and not that it ran in any given report. So no
//!     score, no run reference, no timestamp lives here.
//!   - **A scenario id backs at most one covered claim, ledger-wide.** Anything
//!     else makes a per-area count ambiguous between three units of coverage
//!     and one unit cited three times; deliberate overlaps go in `note`, which
//!     takes no coverage slot.
//!   - **Every area is exactly one row, is nameable, and accounts for
//!     something.** A name declared twice splits one row of the denominator in
//!     two, a row with no name is one nothing can refer to, and an area with no
//!     claims is counted while stating nothing. None of the three is visible to
//!     a test that pins the area set, since sorted names still hold every name.
//!   - **Every root names a tree inside this repo, and no two name one tree.**
//!     `scenario_roots` is the sole input to the drift check's walk and is
//!     joined onto the repo root, so an absolute entry walks off the machine's
//!     root instead of failing, and two overlapping roots enumerate a scenario
//!     twice, which arrives as the duplicate-id rule below firing on a healthy
//!     tree.
//!   - **The ledger and the tree agree, in every direction.** A dead reference
//!     means the ledger cites evidence that no longer exists; an unclaimed
//!     scenario means the ledger has quietly fallen behind the suite; and two
//!     scenarios sharing one id mean a single claim silently stands in for
//!     both, which is the third thing "claimed exactly once" forbids and the
//!     one a membership test cannot see.
//!
//! `audited_against` records the zerostack version the judgments were made
//! against and is compared to a report's `--version` banner by containment
//! only. `backend.rs` stores that banner verbatim precisely so upstream's
//! banner shape never becomes a compatibility contract; a parser here would
//! sign that contract on the ledger's behalf. The failure direction is
//! deliberate: a reshaped banner costs a spurious mismatch notice, never a
//! wrong version claim. Holding that second half true takes one rule beyond
//! plain containment — a hit glued to more version characters is not a hit,
//! or `1.7.2` would match `zerostack 1.7.20` — and `audit_matches` documents
//! why that is still not a parse.
//!
//! File order is presentation order (safety boundaries first, not
//! alphabetical), so nothing here sorts and nothing keys areas by name.

use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

/// A loaded ledger. Every invariant the schema can carry is already true of a
/// value of this type: statuses are the closed four, each carries exactly the
/// evidence its status owes, every area is one nameable row that accounts for
/// something, every root names a tree inside this repo and no two name one
/// tree, and no scenario id backs two covered claims.
///
/// The fields are private and `Ledger::new` is the only constructor, so those
/// invariants hold however the value was made. They were `pub` with the checks
/// living inside `parse`, which made every rule here a property of the load
/// path rather than of the type: a hand-built `Ledger`, the natural shape of a
/// renderer's test fixture, could carry a duplicate id or an empty
/// `audited_against` and be believed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ledger {
    audited_against: String,
    scenario_roots: Vec<String>,
    areas: Vec<Area>,
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
    /// Wire shape to type, and nothing more: which variant a status name means,
    /// and the refusal of evidence belonging to another status, which is a
    /// question only the flat wire shape can ask (each `Status` variant carries
    /// its own fields and no others, so there is nothing foreign left to hold).
    ///
    /// The evidence each status *owes* is deliberately not checked here.
    /// `Ledger::new` checks it, so the rule holds for the value however it was
    /// built, and absent evidence arrives there as the empty string it means
    /// rather than as a second way of saying missing.
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
                Status::Covered {
                    scenarios: scenarios.unwrap_or_default(),
                }
            }
            "uncovered" => {
                reject_foreign_evidence(&at, "uncovered", &["blocked_by"], &present)?;
                Status::Uncovered { blocked_by }
            }
            "product-blocked" => {
                reject_foreign_evidence(&at, "product-blocked", &["reason", "zs"], &present)?;
                Status::ProductBlocked {
                    reason: reason.unwrap_or_default(),
                    zs,
                }
            }
            "excluded" => {
                reject_foreign_evidence(&at, "excluded", &["reason"], &present)?;
                Status::Excluded {
                    reason: reason.unwrap_or_default(),
                }
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
    /// The only way to build a `Ledger`, so every rule below holds whatever the
    /// caller is: the file on disk, or a fixture a renderer wrote by hand.
    pub fn new(
        audited_against: String,
        scenario_roots: Vec<String>,
        areas: Vec<Area>,
    ) -> Result<Ledger> {
        let ledger = Ledger {
            audited_against,
            scenario_roots,
            areas,
        };
        ledger.check_header()?;
        ledger.check_areas()?;
        ledger.check_claims()?;
        ledger.check_unique_ids()?;
        Ok(ledger)
    }

    /// The zerostack version the judgments were made against — compared to a
    /// report's banner by `audit_matches`, never parsed.
    pub fn audited_against(&self) -> &str {
        &self.audited_against
    }

    /// The trees that hold scenarios, relative to the repo root. The drift
    /// check reads them in every direction.
    pub fn scenario_roots(&self) -> &[String] {
        &self.scenario_roots
    }

    /// File order, which is presentation order.
    pub fn areas(&self) -> &[Area] {
        &self.areas
    }

    pub fn load(path: &Path) -> Result<Ledger> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        Ledger::parse(&text).with_context(|| format!("parse {}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Ledger> {
        let raw: RawLedger = toml::from_str(text)?;
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
        Ledger::new(raw.audited_against, raw.scenario_roots, areas)
    }

    fn check_header(&self) -> Result<()> {
        if self.audited_against.trim().is_empty() {
            bail!(
                "coverage ledger: audited_against must not be empty — it is compared to a \
                 report's banner by containment, and an empty string is contained in every \
                 banner, so it would silently claim the ledger was audited against whatever it \
                 is compared to"
            );
        }
        if self.scenario_roots.is_empty() {
            bail!(
                "coverage ledger: scenario_roots must not be empty — an empty root list makes \
                 the drift check enumerate no scenarios"
            );
        }
        self.check_roots()
    }

    /// Each root names a tree inside this repo, spelled relative to its root,
    /// and no two of them name the same tree.
    ///
    /// The entries are the sole input to a filesystem walk: `check_drift` joins
    /// each onto the repo root and enumerates whatever comes out. `Path::join`
    /// discards what it is joined onto as soon as its operand is absolute, so
    /// an unchecked entry does not fail — it walks somewhere else on the
    /// machine. `..` climbs out of the checkout the same way, and an entry
    /// naming no directory below the root resolves to the checkout itself,
    /// where the walk would go looking for scenarios in `target/` and `.git`.
    /// The ledger file is the thing this module exists to distrust, so the
    /// entries are held to what the field means rather than to a list of bad
    /// spellings.
    ///
    /// Overlap is refused for a different reason. Two roots where one holds the
    /// other enumerate every scenario below the inner one twice, and a scenario
    /// enumerated twice is reported by `check_ids` as a duplicate id — a true
    /// statement about the walk and a false one about the tree, which sends the
    /// author to a page of healthy scenarios over a defect in one line of this
    /// header. The comparison is over what the entries name, `.` segments
    /// dropped, so `scenarios` and `./scenarios` are one tree rather than two.
    /// A root symlinked to another is the case this spelling-level test
    /// structurally cannot see, and `check_drift` catches that one where the
    /// paths are resolved.
    fn check_roots(&self) -> Result<()> {
        let mut named: Vec<(&str, Vec<Component>)> = Vec::new();
        for root in &self.scenario_roots {
            let path = Path::new(root.as_str());
            if path
                .components()
                .any(|c| matches!(c, Component::RootDir | Component::Prefix(_)))
            {
                bail!(
                    "coverage ledger: scenario_roots names '{root}', which is an absolute path — \
                     every root is joined onto the repo root, and joining onto a path discards it \
                     when the operand is absolute, so this would not fail: it would walk \
                     somewhere else on the machine entirely"
                );
            }
            if path.components().any(|c| c == Component::ParentDir) {
                bail!(
                    "coverage ledger: scenario_roots names '{root}', which climbs out of the \
                     checkout with '..' — a root names a tree inside this repo, and the drift \
                     check walks in both directions whatever the entry resolves to"
                );
            }
            let parts: Vec<Component> = path
                .components()
                .filter(|c| matches!(c, Component::Normal(_)))
                .collect();
            if parts.is_empty() {
                bail!(
                    "coverage ledger: scenario_roots names '{root}', which names no directory \
                     below the repo root — an empty entry or a bare '.' resolves to the checkout \
                     itself, and the drift check would go looking for scenarios in target/ and \
                     .git"
                );
            }
            if let Some((other, _)) = named
                .iter()
                .find(|(_, seen)| seen.starts_with(&parts) || parts.starts_with(seen))
            {
                bail!(
                    "coverage ledger: scenario_roots names '{other}' and '{root}', one of which \
                     holds the other — every scenario under the inner root would be enumerated \
                     once per root above it, and the drift check reports a scenario enumerated \
                     twice as a duplicate id, which names healthy scenarios for a defect in this \
                     header"
                );
            }
            named.push((root, parts));
        }
        Ok(())
    }

    /// An area is one row of the denominator, so it must be one row, must be
    /// nameable, and must account for something. Two areas under one name split
    /// a row in two while the pinned area-set test stays green, since the sorted
    /// names still hold every expected name; an area with no name is a row
    /// nothing can refer to, neither the duplicate rule that compares names nor
    /// a renderer that prints them; an area with no claims is counted in the
    /// denominator while stating nothing. That denominator is the one number
    /// this file exists to keep honest.
    fn check_areas(&self) -> Result<()> {
        let mut seen: Vec<&str> = Vec::new();
        let mut duplicated: Vec<&str> = Vec::new();
        let mut empty: Vec<&str> = Vec::new();
        // A nameless area is reported by where it sits, since the name is the
        // only other thing that could point at it, and its remaining faults
        // wait for the next run: they would be reported under a name that is
        // not there.
        let mut unnamed: Vec<String> = Vec::new();
        for (at, area) in self.areas.iter().enumerate() {
            let name = area.name.as_str();
            if name.trim().is_empty() {
                unnamed.push((at + 1).to_string());
                continue;
            }
            if seen.contains(&name) && !duplicated.contains(&name) {
                duplicated.push(name);
            }
            seen.push(name);
            if area.claims.is_empty() {
                empty.push(name);
            }
        }
        if duplicated.is_empty() && empty.is_empty() && unnamed.is_empty() {
            return Ok(());
        }
        let mut msg =
            String::from("coverage ledger: every area is one row of the denominator, and:");
        if !duplicated.is_empty() {
            msg.push_str(&format!(
                "\n  declared more than once, splitting one row in two: {}",
                duplicated.join(", ")
            ));
        }
        if !unnamed.is_empty() {
            msg.push_str(&format!(
                "\n  carries no name, so nothing can refer to its row (position in file order): {}",
                unnamed.join(", ")
            ));
        }
        if !empty.is_empty() {
            msg.push_str(&format!(
                "\n  carries no claims, so it is counted while stating nothing: {}",
                empty.join(", ")
            ));
        }
        bail!("{msg}")
    }

    /// Each status owes its own evidence, and a claim states something.
    ///
    /// The rule lives here rather than on the load path because it is a rule
    /// about the value: `Area` and `Claim` carry public fields and `Status`'s
    /// variants do too, so `Covered { scenarios: vec![] }` and
    /// `Excluded { reason: String::new() }` are as constructible as a file is
    /// writable, and a renderer's first hand-built fixture is exactly where
    /// they would be constructed. Checking here also leaves one implementation
    /// of each sentence, so a fixture built in code fails in the same words a
    /// file does.
    ///
    /// Blank counts as absent throughout. A `reason` of `""` satisfies "the
    /// field is present" while saying nothing, and every one of these fields
    /// exists to say something: the ids are the whole of a covered claim's
    /// evidence, and a `blocked_by` that names no gap records the presence of a
    /// gap it does not name.
    fn check_claims(&self) -> Result<()> {
        for area in &self.areas {
            for claim in &area.claims {
                if claim.claim.trim().is_empty() {
                    bail!(
                        "coverage ledger: area '{}' holds a claim with no text — a claim is one \
                         statement about what the suite measures, and a statement that says \
                         nothing takes a row of the page to do it",
                        area.name
                    );
                }
                // The same `at` a parsed claim's error carries, since it is the
                // same error about the same claim.
                let at = format!("area '{}', claim '{}'", area.name, claim.claim);
                match &claim.status {
                    Status::Covered { scenarios } => {
                        // `all` rather than `is_empty`, since blank is absent:
                        // an array of blank ids cites nothing, which is the
                        // same claim as an empty one. A blank alongside a real
                        // id is an id the drift check reports as dead.
                        if scenarios.iter().all(|id| id.trim().is_empty()) {
                            bail!(
                                "{at}: status = \"covered\" requires a non-empty `scenarios` \
                                 array — covered means a scenario in this repo tests the claim, \
                                 so the ids are the whole of the evidence"
                            );
                        }
                    }
                    Status::Uncovered { blocked_by } => {
                        if blocked_by.as_deref().is_some_and(|b| b.trim().is_empty()) {
                            bail!(
                                "{at}: status = \"uncovered\" carries an empty `blocked_by` — the \
                                 presence of that field is itself the claim that a harness gap \
                                 stands in the way, so it has to name the gap; a claim that is \
                                 simply unbuilt leaves the field out"
                            );
                        }
                    }
                    Status::ProductBlocked { reason, .. } => {
                        if reason.trim().is_empty() {
                            bail!(
                                "{at}: status = \"product-blocked\" requires a one-line `reason` \
                                 naming the hole on the zerostack side"
                            );
                        }
                    }
                    Status::Excluded { reason } => {
                        if reason.trim().is_empty() {
                            bail!(
                                "{at}: status = \"excluded\" requires a one-line `reason` for \
                                 never testing this — it is the file's only irreversible judgment"
                            );
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Does `audited_against` appear in this `--version` banner, as the whole
    /// of a version rather than the front of a longer one?
    ///
    /// Containment, deliberately: `backend.rs` stores the banner verbatim so
    /// upstream's banner shape never becomes a compatibility contract, and a
    /// version parser here would sign that contract on the ledger's behalf.
    /// A mock-backend report records `mock` and therefore mismatches, which is
    /// the right answer — a page rendered from mock numbers should say so.
    ///
    /// Plain `str::contains` was not enough, and it failed in the one
    /// direction D6 promises is impossible. `1.7.2` is a substring of
    /// `zerostack 1.7.20` and of `zerostack 1.7.2-rc1`, so a *newer* binary
    /// read as a match and the page would have claimed the ledger was audited
    /// against the version that actually ran — a wrong version claim, not the
    /// spurious mismatch the design is willing to pay. So a hit is refused
    /// when it is glued to a character that could extend a version: a digit
    /// or `.` on either side, and additionally `-` or `+` on the right, where
    /// a pre-release or build suffix sits.
    ///
    /// That is a boundary rule, not a parse. Nothing here splits the banner on
    /// dots, compares numbers, orders two versions, or assumes where in the
    /// line the version sits — the only thing added is a refusal to accept
    /// part of a longer token as the whole of it. Anything it gets wrong still
    /// fails in the safe direction, as a mismatch.
    pub fn audit_matches(&self, banner: &str) -> bool {
        // A version can be continued to the right by a suffix separator but
        // never begun by one, so `-` and `+` are only checked on that side:
        // a banner reading `zerostack-1.7.2` still matches.
        fn extends_left(c: char) -> bool {
            c.is_ascii_digit() || c == '.'
        }
        fn extends_right(c: char) -> bool {
            c.is_ascii_digit() || c == '.' || c == '-' || c == '+'
        }
        banner
            .match_indices(&self.audited_against)
            .any(|(at, hit)| {
                let left_clear = banner[..at]
                    .chars()
                    .next_back()
                    .is_none_or(|c| !extends_left(c));
                let right_clear = banner[at + hit.len()..]
                    .chars()
                    .next()
                    .is_none_or(|c| !extends_right(c));
                left_clear && right_clear
            })
    }

    /// Both directions of the drift check against a real tree: every scenario
    /// under `scenario_roots` (resolved against `repo_root`) versus every id
    /// the covered claims cite.
    pub fn check_drift(&self, repo_root: &Path) -> Result<()> {
        let mut tree_ids: Vec<String> = Vec::new();
        // Where each root actually landed. `check_roots` has already refused
        // two roots that name one tree, but it compares the entries as written
        // and a symlinked root is spelled like any other; the scenarios under
        // it would otherwise be enumerated twice and reported as duplicate ids,
        // which is the tree's fault in the message and the header's in fact.
        let mut resolved: Vec<(&str, PathBuf)> = Vec::new();
        for root in &self.scenario_roots {
            let dir = repo_root.join(root);
            if !dir.is_dir() {
                bail!(
                    "coverage ledger: scenario_roots names '{root}', which resolves to {} — not \
                     a directory, and every root is a tree of this repo the drift check \
                     enumerates in both directions",
                    dir.display()
                );
            }
            let real = dir
                .canonicalize()
                .with_context(|| format!("resolve {}", dir.display()))?;
            if let Some((other, seen)) = resolved
                .iter()
                .find(|(_, seen)| real.starts_with(seen) || seen.starts_with(&real))
            {
                bail!(
                    "coverage ledger: scenario_roots names '{other}' and '{root}', which resolve \
                     to {} and {} — one holds the other, so every scenario below the inner one \
                     would be enumerated twice and reported as a duplicate id, naming healthy \
                     scenarios for a defect in this header",
                    seen.display(),
                    real.display()
                );
            }
            resolved.push((root, real));
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

    /// The drift check proper, over ids rather than a tree, so every direction
    /// is one comparison. Failures name every offending id in one report:
    /// fixing a dead reference only to be told about an unclaimed scenario on
    /// the next run hides part of the drift behind the rest.
    ///
    /// Three things can be wrong, not two. "Every scenario is claimed exactly
    /// once" is a statement about multiplicity, and membership alone cannot
    /// check it: `discover` sorts the tree but neither dedupes nor rejects
    /// repeated ids, and one covered claim satisfies every copy of an id, so
    /// two scenarios sharing one id would report clean while a single claim
    /// stood in for both. That is the one-id-many-meanings ambiguity
    /// `check_unique_ids` rules out on the ledger side, arriving from the tree
    /// side instead, so it is refused here on the same grounds.
    fn check_ids(&self, tree_ids: &[String]) -> Result<()> {
        let claimed = self.covered_ids();
        let dead: Vec<&str> = claimed
            .iter()
            .copied()
            .filter(|id| !tree_ids.iter().any(|in_tree| in_tree.as_str() == *id))
            .collect();
        // One pass, and each id reported once however many times the tree holds
        // it, so a duplicate does not also arrive as a repeated line above it.
        let mut unclaimed: Vec<&str> = Vec::new();
        let mut duplicated: Vec<&str> = Vec::new();
        for id in tree_ids.iter().map(|id| id.as_str()) {
            if !claimed.contains(&id) && !unclaimed.contains(&id) {
                unclaimed.push(id);
            }
            if !duplicated.contains(&id)
                && tree_ids.iter().filter(|other| other.as_str() == id).count() > 1
            {
                duplicated.push(id);
            }
        }
        if dead.is_empty() && unclaimed.is_empty() && duplicated.is_empty() {
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
        if !duplicated.is_empty() {
            msg.push_str(&format!(
                "\n  duplicate scenario ids (declared by more than one scenario under {roots}, so \
                 one covered claim would silently stand in for several): {}",
                duplicated.join(", ")
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

    /// A ledger whose header names these roots, so each roots test writes only
    /// the entry it is about.
    fn with_roots(roots: &str) -> String {
        format!(
            r#"audited_against = "1.7.2"
scenario_roots = [{roots}]

[[areas]]
name = "permission"
title = "Permission layer"

[[areas.claims]]
claim = "ask mode refuses an edit"
status = "uncovered"
"#
        )
    }

    /// Every root is joined onto the repo root, and joining onto a path
    /// discards it when the operand is absolute — so an absolute entry does not
    /// fail on its own, it walks the machine's root instead of the checkout.
    #[test]
    fn an_absolute_scenario_root_fails_naming_it() {
        for root in [r#""/""#, r#""/etc""#] {
            let text = with_roots(root);
            let msg = err(&text);
            assert!(msg.contains("scenario_roots"), "{msg}");
            assert!(msg.contains("absolute"), "{msg}");
        }
    }

    #[test]
    fn a_scenario_root_that_climbs_out_of_the_checkout_fails_naming_it() {
        for root in [r#""..""#, r#""../..""#, r#""scenarios/../../elsewhere""#] {
            let text = with_roots(root);
            let msg = err(&text);
            assert!(msg.contains("scenario_roots"), "{msg}");
            assert!(msg.contains(".."), "{msg}");
        }
    }

    /// An entry naming nothing below the root resolves to the checkout itself,
    /// where the walk would go looking for scenarios in `target/` and `.git`.
    #[test]
    fn a_scenario_root_naming_no_directory_fails_naming_it() {
        for root in [r#""""#, r#"".""#, r#""./""#] {
            let text = with_roots(root);
            let msg = err(&text);
            assert!(msg.contains("scenario_roots"), "{msg}");
            assert!(msg.contains("no directory"), "{msg}");
        }
    }

    /// Overlapping roots enumerate every scenario below the inner one twice,
    /// and `check_ids` reports a scenario enumerated twice as a duplicate id —
    /// nineteen healthy scenarios named for one line of the header. Refused
    /// where it is written rather than diagnosed after the walk.
    #[test]
    fn a_scenario_root_holding_another_fails_naming_both() {
        let text = with_roots(r#""scenarios", "scenarios/prompts""#);
        let msg = err(&text);
        assert!(msg.contains("scenarios/prompts"), "{msg}");
        assert!(msg.contains("holds the other"), "{msg}");
    }

    /// The comparison is over what an entry names, not over how it is spelled,
    /// so `.` segments do not buy a second copy of one tree.
    #[test]
    fn one_tree_spelled_two_ways_fails_naming_both() {
        for roots in [
            r#""scenarios", "scenarios""#,
            r#""scenarios", "./scenarios""#,
        ] {
            let text = with_roots(roots);
            let msg = err(&text);
            assert!(msg.contains("scenarios"), "{msg}");
            assert!(msg.contains("holds the other"), "{msg}");
        }
    }

    #[test]
    fn a_duplicate_area_name_fails_naming_it() {
        // Two rows under one name, which the pinned area-set test cannot catch:
        // sorting the names still yields every name the spec expects.
        let text = ledger(
            r#"[[areas]]
name = "permission"
title = "Permission layer"

[[areas.claims]]
claim = "ask mode refuses an edit"
status = "uncovered"

[[areas]]
name = "permission"
title = "Permission layer, again"

[[areas.claims]]
claim = "a deny rule beats an allow rule"
status = "uncovered""#,
        );
        let msg = err(&text);
        assert!(msg.contains("permission"), "{msg}");
        assert!(msg.contains("more than once"), "{msg}");
    }

    #[test]
    fn an_area_with_no_claims_fails_naming_it() {
        let text = ledger(
            r#"[[areas]]
name = "permission"
title = "Permission layer"
claims = []"#,
        );
        let msg = err(&text);
        assert!(msg.contains("permission"), "{msg}");
        assert!(msg.contains("no claims"), "{msg}");
    }

    /// The name is the row's identity — what the duplicate rule compares and
    /// what a renderer prints — so a row without one is a row nothing can refer
    /// to, and the position in file order is all the error has left to name it
    /// by.
    #[test]
    fn an_area_with_no_name_fails_naming_its_position() {
        let text = ledger(
            r#"[[areas]]
name = "permission"
title = "Permission layer"

[[areas.claims]]
claim = "ask mode refuses an edit"
status = "uncovered"

[[areas]]
name = ""
title = "Nameless"

[[areas.claims]]
claim = "a deny rule beats an allow rule"
status = "uncovered""#,
        );
        let msg = err(&text);
        assert!(msg.contains("no name"), "{msg}");
        assert!(msg.contains("file order): 2"), "{msg}");
    }

    /// The invariants belong to the type, not to the load path. `Ledger`'s
    /// fields were `pub` with the checks running inside `parse`, so a
    /// hand-constructed value — the natural shape of a renderer's fixture —
    /// could carry a duplicate id past every rule in this module. `new` is now
    /// the only constructor, and it runs the same checks the file gets.
    #[test]
    fn a_hand_built_ledger_is_checked_like_a_parsed_one() {
        let covered = |claim: &str, id: &str| Claim {
            claim: claim.to_string(),
            status: Status::Covered {
                scenarios: vec![id.to_string()],
            },
            note: None,
        };
        let area = |name: &str, claims: Vec<Claim>| Area {
            name: name.to_string(),
            title: name.to_string(),
            claims,
        };
        let roots = || vec!["scenarios".to_string()];

        // The duplicate-id rule.
        let err = Ledger::new(
            "1.7.2".to_string(),
            roots(),
            vec![area(
                "prompts",
                vec![
                    covered("ask mode refuses an edit", "ask-readonly-refuses-edit"),
                    covered("ask mode explains itself", "ask-readonly-refuses-edit"),
                ],
            )],
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("ask-readonly-refuses-edit"),
            "{err:#}"
        );

        // The header rules, which used to live in `parse` alone.
        let err = Ledger::new(
            "  ".to_string(),
            roots(),
            vec![area("prompts", vec![covered("x", "y")])],
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("audited_against"), "{err:#}");

        let err = Ledger::new(
            "1.7.2".to_string(),
            vec![],
            vec![area("prompts", vec![covered("x", "y")])],
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("scenario_roots"), "{err:#}");

        // The per-root rules, which decide where a filesystem walk goes and so
        // must not wait for a file to be the thing that carries them.
        let err = Ledger::new(
            "1.7.2".to_string(),
            vec!["/".to_string()],
            vec![area("prompts", vec![covered("x", "y")])],
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("absolute"), "{err:#}");

        // The area rules.
        let err =
            Ledger::new("1.7.2".to_string(), roots(), vec![area("prompts", vec![])]).unwrap_err();
        assert!(format!("{err:#}").contains("no claims"), "{err:#}");

        let err = Ledger::new(
            "1.7.2".to_string(),
            roots(),
            vec![area("", vec![covered("x", "y")])],
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("no name"), "{err:#}");

        // And a well-formed one still builds, reachable through the accessors.
        let ok = Ledger::new(
            "1.7.2".to_string(),
            roots(),
            vec![area("prompts", vec![covered("x", "y")])],
        )
        .unwrap();
        assert_eq!(ok.audited_against(), "1.7.2");
        assert_eq!(ok.scenario_roots(), ["scenarios"]);
        assert_eq!(ok.areas().len(), 1);
    }

    /// The same argument one level down, for the rule the module lists first.
    /// `Status`'s variants carry public fields, so `Covered { scenarios:
    /// vec![] }` and `Excluded { reason: String::new() }` are as constructible
    /// as a file is writable, and the required-evidence half used to run on the
    /// parse path alone — where a renderer's fixture never goes. Every shape
    /// here is one a file is refused for.
    #[test]
    fn a_hand_built_ledger_owes_the_same_evidence_a_parsed_one_does() {
        let build = |status: Status| {
            Ledger::new(
                "1.7.2".to_string(),
                vec!["scenarios".to_string()],
                vec![Area {
                    name: "prompts".to_string(),
                    title: "Prompt behaviour".to_string(),
                    claims: vec![Claim {
                        claim: "ask mode refuses an edit".to_string(),
                        status,
                        note: None,
                    }],
                }],
            )
        };

        // (the shape, the word its error has to carry)
        let cases = [
            (Status::Covered { scenarios: vec![] }, "scenarios"),
            (
                Status::Covered {
                    scenarios: vec!["  ".to_string()],
                },
                "scenarios",
            ),
            (
                Status::Excluded {
                    reason: String::new(),
                },
                "reason",
            ),
            (
                Status::ProductBlocked {
                    reason: "   ".to_string(),
                    zs: None,
                },
                "reason",
            ),
            (
                Status::Uncovered {
                    blocked_by: Some(String::new()),
                },
                "blocked_by",
            ),
        ];
        for (status, word) in cases {
            let err = build(status).unwrap_err();
            let msg = format!("{err:#}");
            assert!(msg.contains(word), "{msg}");
            assert!(msg.contains("ask mode refuses an edit"), "{msg}");
        }

        // A claim with no text is the same fault a row above: one statement
        // about what the suite measures, stating nothing.
        let err = Ledger::new(
            "1.7.2".to_string(),
            vec!["scenarios".to_string()],
            vec![Area {
                name: "prompts".to_string(),
                title: "Prompt behaviour".to_string(),
                claims: vec![Claim {
                    claim: String::new(),
                    status: Status::Uncovered { blocked_by: None },
                    note: None,
                }],
            }],
        )
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("prompts"), "{msg}");
        assert!(msg.contains("no text"), "{msg}");

        // And the evidence each status does owe still builds.
        build(Status::Covered {
            scenarios: vec!["ask-readonly-refuses-edit".to_string()],
        })
        .unwrap();
        build(Status::Uncovered { blocked_by: None }).unwrap();
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

    /// Blank is absent: a `reason` of `""` satisfies "the field is present"
    /// while saying nothing, and the field exists to say something. One rule
    /// for both statuses that owe a reason.
    #[test]
    fn a_blank_reason_fails_like_a_missing_one() {
        for status in ["excluded", "product-blocked"] {
            let text = one_claim(&format!(
                "claim = \"a symlink out of the workspace is refused\"\nstatus = \
                 \"{status}\"\nreason = \"   \""
            ));
            let msg = err(&text);
            assert!(
                msg.contains("a symlink out of the workspace is refused"),
                "{msg}"
            );
            assert!(msg.contains("reason"), "{msg}");
        }
    }

    /// `blocked_by`'s presence is itself the claim that a harness gap stands in
    /// the way, so an empty one records a gap it does not name. A claim that is
    /// simply unbuilt leaves the field out, which is the whole distinction.
    #[test]
    fn a_blank_blocked_by_fails_naming_the_claim() {
        let text = one_claim(
            r#"claim = "per-scenario CLI flags"
status = "uncovered"
blocked_by = """#,
        );
        let msg = err(&text);
        assert!(msg.contains("per-scenario CLI flags"), "{msg}");
        assert!(msg.contains("blocked_by"), "{msg}");
    }

    #[test]
    fn a_claim_with_no_text_fails_naming_its_area() {
        let text = one_claim(
            r#"claim = ""
status = "uncovered""#,
        );
        let msg = err(&text);
        assert!(msg.contains("permission"), "{msg}");
        assert!(msg.contains("no text"), "{msg}");
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

    /// The boundary rule, which is the whole of what `audit_matches` adds to
    /// `str::contains`. Plain containment read a *newer* binary as a match,
    /// because `1.7.2` sits inside `1.7.20`, and that is a wrong version claim
    /// rather than the spurious mismatch D6 is willing to pay. Every case here
    /// that must not match is a banner plain containment accepted.
    #[test]
    fn audit_matches_refuses_a_version_it_is_only_the_front_of() {
        let text = one_claim(
            r#"claim = "ask mode refuses an edit"
status = "uncovered""#,
        );
        let l = Ledger::parse(&text).unwrap();

        // Newer releases whose version merely starts with the audited one.
        assert!(!l.audit_matches("zerostack 1.7.20"));
        assert!(!l.audit_matches("zerostack 1.7.21 (a1b2c3)"));
        assert!(!l.audit_matches("zerostack 1.7.2.1"));
        // A pre-release or a build of 1.7.2 is not 1.7.2 either.
        assert!(!l.audit_matches("zerostack 1.7.2-rc1"));
        assert!(!l.audit_matches("zerostack 1.7.2+build9"));
        // The same rule on the left, where a longer version ends in the
        // audited one instead of beginning with it.
        assert!(!l.audit_matches("zerostack 21.7.2"));
        assert!(!l.audit_matches("zerostack 0.1.7.2"));

        // Still a match: the exact version, wherever in the line it sits and
        // whatever introduces it. A separator can extend a version to the
        // right but never begin one, so `zerostack-1.7.2` matches.
        assert!(l.audit_matches("zerostack 1.7.2"));
        assert!(l.audit_matches("zerostack-1.7.2"));
        assert!(l.audit_matches("zerostack 1.7.2 (a1b2c3)"));
        assert!(l.audit_matches("zerostack v1.7.2, built 2026-07-01"));
        // Both shapes at once: the exact occurrence carries the match, so the
        // rule rejects a hit rather than the whole banner.
        assert!(l.audit_matches("zerostack 1.7.20 (branched from 1.7.2)"));
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

    /// Two scenarios in the tree declaring one id. `discover` sorts but neither
    /// dedupes nor rejects, and one covered claim satisfies every copy, so a
    /// membership-only check reported clean while that claim silently stood in
    /// for both scenarios — the ledger-side ambiguity `check_unique_ids`
    /// forbids, reached from the tree side.
    #[test]
    fn the_drift_check_rejects_two_tree_scenarios_sharing_one_id() {
        let l = two_covered("ask-readonly-refuses-edit", "memory-recall");
        let err = l
            .check_ids(&[
                "ask-readonly-refuses-edit".to_string(),
                "ask-readonly-refuses-edit".to_string(),
                "memory-recall".to_string(),
            ])
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("duplicate scenario ids"), "{msg}");
        assert!(msg.contains("ask-readonly-refuses-edit"), "{msg}");
        // Multiplicity is the only thing wrong, so it is the only thing said:
        // both ids are claimed and both claims are live.
        assert!(!msg.contains("unclaimed"), "{msg}");
        assert!(!msg.contains("dead references"), "{msg}");
    }

    /// A duplicated id that is also unclaimed is named once per direction, not
    /// once per copy: a tree holding three of something is one decision to
    /// make, and three identical lines read as three.
    #[test]
    fn a_repeated_tree_id_is_reported_once_per_direction() {
        let l = two_covered("ask-readonly-refuses-edit", "memory-recall");
        let err = l
            .check_ids(&[
                "ask-readonly-refuses-edit".to_string(),
                "memory-recall".to_string(),
                "stowaway".to_string(),
                "stowaway".to_string(),
                "stowaway".to_string(),
            ])
            .unwrap_err();
        let msg = format!("{err:#}");
        assert_eq!(msg.matches("stowaway").count(), 2, "{msg}");
        assert!(msg.contains("unclaimed scenarios"), "{msg}");
        assert!(msg.contains("duplicate scenario ids"), "{msg}");
    }
}
