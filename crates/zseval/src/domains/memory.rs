//! zerostack memory-subsystem knowledge (and nothing else lives outside
//! this module).
//!
//! Verified against zerostack `src/extras/memory/mod.rs` (2026-07-06):
//!   `Mem::open()` roots at `<config_dir>/agent/memory/`:
//!     MEMORY.md                          (global, shared across projects)
//!     projects/<slug>/notes/<name>.md
//!   where <slug> = project_slug(current_dir()), FNV-1a based.
//!
//! `memory` is NOT a zerostack default feature — a binary under eval must be
//! built `cargo build --features memory`, or the tools never register and
//! `verify` below will say so.
//!
//! Scenario sugar:
//!   [seed.memory]
//!   long_term = "_fixtures/MEMORY.md"
//!   notes = [ { name = "deploy-strategy", file = "_fixtures/deploy.md" } ]
//!
//! Deliberately excluded (YAGNI until a scenario needs them): `scratchpad`
//! and `daily` seeding — `daily` in particular depends on zerostack's local
//! (not UTC) "today", which this harness's date helpers don't model.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::backend::RunRoots;
use crate::scenario::Scenario;
use crate::seed::Placement;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MemorySeed {
    pub long_term: Option<PathBuf>,
    #[serde(default)]
    pub notes: Vec<NoteSeed>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NoteSeed {
    pub name: String,
    pub file: PathBuf,
}

/// Where `Mem::open()` roots memory for a given run's config dir. The one
/// place this path is computed — `expand` (seeding) and `verify` (grading)
/// both call it, so there is exactly one fact to update if zerostack's
/// layout ever changes, not two that can quietly drift apart.
fn layout_root(config: &Path) -> PathBuf {
    config.join("agent").join("memory")
}

/// Load-time check that every declared fixture actually resolves — a typo'd
/// path fails `zseval list`/load, not mid-run after burning an API call.
pub fn validate(mem: &MemorySeed, sc: &Scenario) -> Result<()> {
    if let Some(p) = &mem.long_term {
        sc.resolve_fixture(p)
            .with_context(|| format!("{}: bad [seed.memory] long_term", sc.id))?;
    }
    for n in &mem.notes {
        sc.resolve_fixture(&n.file)
            .with_context(|| format!("{}: bad [seed.memory] note '{}'", sc.id, n.name))?;
    }
    Ok(())
}

/// Expand memory sugar into generic placements, rooted at
/// `<config>/agent/memory/` (matches `Mem::open()`).
pub fn expand(mem: &MemorySeed, sc: &Scenario, ctx: &RunRoots) -> Result<Vec<Placement>> {
    let root = layout_root(ctx.config);
    let proj = root.join("projects").join(project_slug(ctx.work));
    let mut out = Vec::new();
    if let Some(p) = &mem.long_term {
        out.push(Placement {
            src: sc.resolve_fixture(p)?,
            dest: root.join("MEMORY.md"),
        });
    }
    for n in &mem.notes {
        out.push(Placement {
            src: sc.resolve_fixture(&n.file)?,
            dest: proj.join("notes").join(format!("{}.md", n.name)),
        });
    }
    Ok(out)
}

/// Replicates zerostack's `project_slug` (`src/extras/memory/mod.rs`):
/// FNV-1a 64 over the path bytes, low 32 bits as 8 hex, prefixed with the
/// sanitized basename (max 40 chars). Verified 2026-07-06 — if zerostack
/// changes this, `verify_layout` catches the drift at run time instead of
/// this silently mis-seeding notes nobody reads.
pub fn project_slug(path: &Path) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &byte in path.as_os_str().as_encoded_bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let short = hash as u32;
    let base = path.file_name().and_then(|s| s.to_str()).unwrap_or("root");
    let mut slug: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    slug.truncate(40);
    if slug.is_empty() {
        slug.push_str("root");
    }
    format!("{slug}-{short:08x}")
}

/// Cross-check our snapshot of zerostack's memory layout against reality.
/// Computes the expected root/project once (via `layout_root` and
/// `project_slug`, the same helpers `expand` seeds with) and scans the given
/// zslogs for the `memory open: root=…, project=…` line zerostack's
/// `Mem::open()` logs at debug level — the harness always captures a
/// trace-level log per turn via `--log-file` (`zerostack=trace`, see
/// zerostack's `src/logging.rs`), so that line is there whenever the
/// `memory` feature is compiled in and the tool actually ran.
///   - no zslogs given -> nothing to verify (e.g. the mock backend).
///   - zslogs given but the line never appears -> the memory subsystem never
///     opened (feature not compiled in, or the model never touched memory).
///   - the line appears with a different root/project than expected -> our
///     snapshot of zerostack's internals is stale (or the zslog is evidence
///     from a *different* run's roots than the ones passed in here — see
///     `backend::Mock`'s doc on why `--backend mock=<dir> run` can't replay
///     a memory scenario, only `regrade`/a live build can).
///
/// Every failure mode returns `Err` with a message naming the fix, so the
/// runner can grade Indeterminate instead of blaming the agent.
pub fn verify(roots: &RunRoots, zslogs: &[PathBuf]) -> Result<(), String> {
    if zslogs.is_empty() {
        return Ok(());
    }
    let expected_root = layout_root(roots.config);
    let expected_project = project_slug(roots.work);

    let expected_root_str = expected_root.display().to_string();
    let mut found = false;
    for log in zslogs {
        let text = std::fs::read_to_string(log).unwrap_or_default();
        for line in text.lines() {
            let Some(rest) = line.split_once("memory open: root=").map(|(_, r)| r) else {
                continue;
            };
            found = true;
            let mut parts = rest.splitn(2, ", project=");
            let root = parts.next().unwrap_or("").trim();
            let project = parts.next().unwrap_or("").trim();
            if root != expected_root_str {
                return Err(format!(
                    "memory root drift: zerostack opened '{root}', harness seeded \
                     '{expected_root_str}' — update the root in domains/memory.rs::expand"
                ));
            }
            if project != expected_project {
                return Err(format!(
                    "memory project-slug drift: zerostack computed '{project}', harness \
                     expected '{expected_project}' — update project_slug() in domains/memory.rs"
                ));
            }
        }
    }
    if !found {
        return Err(
            "scenario declares [seed.memory] but no 'memory open: root=…' trace line was \
             found in any turn-*.zslog — is the zerostack build missing --features memory?"
                .to_string(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_slug_is_stable_and_collision_resistant() {
        let a = project_slug(Path::new("/Users/x/work/zerostack"));
        let b = project_slug(Path::new("/Users/x/work/zerostack"));
        assert_eq!(a, b);
        let c = project_slug(Path::new("/Users/x/other/zerostack"));
        assert_ne!(
            a, c,
            "different absolute paths sharing a basename must not collide"
        );
        assert!(a.ends_with(&format!("{:08x}", {
            let mut hash: u64 = 0xcbf29ce484222325;
            for &byte in Path::new("/Users/x/work/zerostack")
                .as_os_str()
                .as_encoded_bytes()
            {
                hash ^= byte as u64;
                hash = hash.wrapping_mul(0x100000001b3);
            }
            hash as u32
        })));
    }

    #[test]
    fn verify_ok_when_no_zslogs_present() {
        let roots = RunRoots {
            data: Path::new("/d"),
            config: Path::new("/c"),
            work: Path::new("/w"),
        };
        assert!(verify(&roots, &[]).is_ok());
    }

    #[test]
    fn verify_errs_when_feature_never_opened_memory() {
        let dir = std::env::temp_dir().join(format!("zsmem-test-missing-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("turn-0.zslog");
        std::fs::write(&log, "some trace line with no memory open\n").unwrap();
        let roots = RunRoots {
            data: &dir,
            config: &dir,
            work: &dir,
        };
        let err = verify(&roots, &[log]).unwrap_err();
        assert!(err.contains("--features memory"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn verify_errs_on_root_drift() {
        let dir = std::env::temp_dir().join(format!("zsmem-test-drift-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("config");
        let work = dir.join("work");
        let expected_project = project_slug(&work);
        let log = dir.join("turn-0.zslog");
        std::fs::write(
            &log,
            format!(
                "2026-01-01T00:00:00Z DEBUG zerostack: memory open: root=/actual/root, project={expected_project}\n"
            ),
        )
        .unwrap();
        let roots = RunRoots {
            data: &dir,
            config: &config,
            work: &work,
        };
        let err = verify(&roots, &[log]).unwrap_err();
        assert!(err.contains("root drift"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn verify_errs_on_project_drift() {
        let dir = std::env::temp_dir().join(format!("zsmem-test-projdrift-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("config");
        let work = dir.join("work");
        let expected_root = layout_root(&config);
        let log = dir.join("turn-0.zslog");
        std::fs::write(
            &log,
            format!(
                "2026-01-01T00:00:00Z DEBUG zerostack: memory open: root={}, project=some-other-slug\n",
                expected_root.display()
            ),
        )
        .unwrap();
        let roots = RunRoots {
            data: &dir,
            config: &config,
            work: &work,
        };
        let err = verify(&roots, &[log]).unwrap_err();
        assert!(err.contains("project-slug drift"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn verify_passes_on_exact_match() {
        let dir = std::env::temp_dir().join(format!("zsmem-test-match-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("config");
        let work = dir.join("work");
        let expected_root = layout_root(&config);
        let expected_project = project_slug(&work);
        let log = dir.join("turn-0.zslog");
        std::fs::write(
            &log,
            format!(
                "2026-01-01T00:00:00Z DEBUG zerostack: memory open: root={}, project={expected_project}\n",
                expected_root.display()
            ),
        )
        .unwrap();
        let roots = RunRoots {
            data: &dir,
            config: &config,
            work: &work,
        };
        assert!(verify(&roots, &[log]).is_ok());
        std::fs::remove_dir_all(&dir).ok();
    }
}
