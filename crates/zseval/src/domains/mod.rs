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
//! subsystem (subagents, chains, …) = one new module here, one `SeedSugar`
//! field, one arm in each function below. Nothing outside this file ever
//! names a specific domain.
//!
//! Because this knowledge is a snapshot of zerostack internals, every module
//! here pairs its layout knowledge with a runtime drift check dispatched via
//! `verify` after driving the agent — a stale snapshot grades Indeterminate,
//! never a silent Fail (see `memory::verify`).

use std::path::PathBuf;

use anyhow::Result;

use crate::backend::RunRoots;
use crate::scenario::Scenario;
use crate::seed::Placement;

pub mod memory;

/// Load-time validation of every domain's scenario sugar (fixture paths
/// resolve, names are sane) — fail at `zseval list`/load, not mid-run.
pub fn validate(sc: &Scenario) -> Result<()> {
    if let Some(mem) = &sc.seed.memory {
        memory::validate(mem, sc)?;
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
    Ok(out)
}

/// Post-run drift check for every domain the scenario seeded. `Err` means
/// our snapshot of zerostack's internals no longer matches reality (or the
/// required feature isn't compiled in) — the runner grades Indeterminate
/// with the returned message, never blaming the agent.
pub fn verify(sc: &Scenario, roots: &RunRoots, zslogs: &[PathBuf]) -> Result<(), String> {
    if sc.seed.memory.is_some() {
        memory::verify(roots, zslogs)?;
    }
    Ok(())
}
