//! Domain modules: zerostack-subsystem-specific knowledge, quarantined.
//!
//! The harness core (scenario/backend/seed/asserts/verdict) is generic.
//! Anything that encodes how a *particular* zerostack subsystem lays out
//! files or derives identifiers lives here, one module per subsystem, and
//! only ever surfaces as scenario-TOML sugar that expands into generic
//! `seed::Placement`s.
//!
//! The three functions below are the core's *only* entry points into domain
//! knowledge — `Scenario::load` calls `validate`, `seed::apply` calls
//! `expand`, the runner calls `verify`. Adding eval support for another
//! subsystem (subagents, chains, …) = one new module here, one arm in each
//! function below, plus one new `SeedSugar` field *if and only if* the
//! domain actually seeds something — a domain with nothing to place (see
//! `subagents`, whose scenarios opt in purely via `domains = [...]`) skips
//! that field entirely. Nothing outside this file ever names a specific
//! domain.
//!
//! Because this knowledge is a snapshot of zerostack internals, every module
//! here pairs its layout knowledge with a runtime drift check dispatched via
//! `verify` after driving the agent — a stale snapshot grades Indeterminate,
//! never a silent Fail (see `memory::verify`). A domain with no reliable
//! drift evidence at all (see `subagents::verify`) says so explicitly in its
//! own module doc rather than faking a check.

use std::path::PathBuf;

use anyhow::Result;

use crate::backend::RunRoots;
use crate::scenario::Scenario;
use crate::seed::Placement;

pub mod mcp;
pub mod memory;
pub mod subagents;

/// Every domain name a scenario's `domains = [...]` field may declare — the
/// only other place (besides `[seed.*]` presence) that decides "which
/// domains apply to this scenario," kept as one list so both routes stay in
/// sync and a typo'd name is a load-time error, not a silently-skipped
/// drift check.
const KNOWN_DOMAINS: &[&str] = &["memory", "subagents", "mcp"];

fn wants(sc: &Scenario, name: &str) -> bool {
    sc.domains.iter().any(|d| d == name)
}

/// Load-time validation of every domain's scenario sugar (fixture paths
/// resolve, names are sane) — fail at `zseval list`/load, not mid-run.
pub fn validate(sc: &Scenario) -> Result<()> {
    for d in &sc.domains {
        if !KNOWN_DOMAINS.contains(&d.as_str()) {
            anyhow::bail!(
                "{}: unknown domain '{d}' in `domains = [...]` (known: {})",
                sc.id,
                KNOWN_DOMAINS.join(", ")
            );
        }
    }
    if let Some(mem) = &sc.seed.memory {
        memory::validate(mem, sc)?;
    }
    if let Some(mcp) = &sc.seed.mcp {
        mcp::validate(mcp, sc)?;
    }
    Ok(())
}

/// Expand every domain's sugar into the same generic placements `[[files]]`
/// produces.
pub fn expand(sc: &Scenario, ctx: &RunRoots) -> Result<Vec<Placement>> {
    let mut out = Vec::new();
    if let Some(mem) = &sc.seed.memory {
        out.extend(memory::expand(mem, sc, ctx)?);
    }
    if let Some(mcp) = &sc.seed.mcp {
        out.extend(mcp::expand(mcp, sc, ctx)?);
    }
    Ok(out)
}

/// Post-run drift check for every domain the scenario seeded, or explicitly
/// opted into via `domains = [...]` (for a scenario that seeds nothing but
/// still needs the layout snapshot guarded — e.g. one that starts from an
/// empty memory store and only asserts the agent wrote to it). `Err` means
/// our snapshot of zerostack's internals no longer matches reality (or the
/// required feature isn't compiled in) — the runner grades Indeterminate
/// with the returned message, never blaming the agent.
pub fn verify(sc: &Scenario, roots: &RunRoots, zslogs: &[PathBuf]) -> Result<(), String> {
    if sc.seed.memory.is_some() || wants(sc, "memory") {
        memory::verify(roots, zslogs)?;
    }
    // subagents has no [seed.*] sugar (nothing to seed), so `wants` is the
    // only route in — see subagents.rs's module doc for why `verify` there
    // is a deliberate no-op rather than a real drift check.
    if wants(sc, "subagents") {
        subagents::verify(roots, zslogs)?;
    }
    // mcp has no "empty store" case (a tool can't exist without a
    // configured server), so every mcp scenario declares [seed.mcp] and
    // that presence alone is the dispatch route — see mcp.rs's module doc.
    if let Some(mcp) = &sc.seed.mcp {
        mcp::verify(mcp, zslogs)?;
    }
    Ok(())
}
