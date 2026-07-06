//! Generic environment seeding — the harness knows how to place files, and
//! nothing about what they mean.
//!
//! A scenario declares placements as `src -> dest`, where `dest` is prefixed
//! with the run-root it targets:
//!
//!   [[files]]
//!   src  = "config.toml"            # resolved from the scenario's own dir
//!   dest = "config:config.toml"     # data: | config: | work:
//!
//! The core stays subsystem-agnostic: subsystem-specific layout knowledge
//! (e.g. where a memory file lives) belongs in a `domains::` module that
//! expands its own scenario-TOML sugar into these same placements — see
//! `domains::memory` for the pattern.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};

use crate::backend::RunRoots;
use crate::scenario::Scenario;

/// A resolved placement: absolute source fixture -> absolute destination.
pub struct Placement {
    pub src: PathBuf,
    pub dest: PathBuf,
}

/// Parse a `root:relative/path` destination against the run context.
pub fn resolve_dest(dest: &str, ctx: &RunRoots) -> Result<PathBuf> {
    let (root, rel) = dest
        .split_once(':')
        .ok_or_else(|| anyhow::anyhow!("dest '{dest}' must be 'data:…', 'config:…' or 'work:…'"))?;
    if rel.starts_with('/') || rel.split('/').any(|c| c == "..") {
        bail!("dest '{dest}' must be a relative path without '..'");
    }
    Ok(match root {
        "data" => ctx.data.join(rel),
        "config" => ctx.config.join(rel),
        "work" => ctx.work.join(rel),
        other => bail!("unknown dest root '{other}:' in '{dest}'"),
    })
}

/// Resolve every declared file placement and copy it into the run dir.
pub fn apply(sc: &Scenario, ctx: &RunRoots) -> Result<()> {
    let mut placements: Vec<Placement> = Vec::new();

    for f in &sc.files {
        placements.push(Placement {
            src: sc.resolve_fixture(&f.src)?,
            dest: resolve_dest(&f.dest, ctx)?,
        });
    }

    // Subsystem-specific sugar expands into the same generic placements —
    // one `if let` per `domains::` module.
    if let Some(mem) = &sc.seed.memory {
        placements.extend(crate::domains::memory::expand(mem, sc, ctx)?);
    }

    for p in &placements {
        if let Some(parent) = p.dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(&p.src, &p.dest)
            .with_context(|| format!("seed {} -> {}", p.src.display(), p.dest.display()))?;
    }
    Ok(())
}
