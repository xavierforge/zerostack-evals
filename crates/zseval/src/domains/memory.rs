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
//! `verify_layout` below will say so.
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

use anyhow::Result;
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

/// Expand memory sugar into generic placements, rooted at
/// `<config>/agent/memory/` (matches `Mem::open()`).
pub fn expand(mem: &MemorySeed, sc: &Scenario, ctx: &RunRoots) -> Result<Vec<Placement>> {
    let root = ctx.config.join("agent").join("memory");
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
///
/// zerostack's `Mem::open()` logs `memory open: root=…, project=…` at debug
/// level; the harness always captures a trace-level log per turn via
/// `--log-file` (`zerostack=trace`, see zerostack's `src/logging.rs`), so
/// that line is there whenever the `memory` feature is compiled in and the
/// tool actually ran. Scan every `turn-*.zslog` under `run_dir`:
///   - no zslog files at all -> nothing to verify (e.g. the mock backend).
///   - zslogs exist but the line never appears -> the memory subsystem never
///     opened (feature not compiled in, or the model never touched memory).
///   - the line appears with a different root/project than we seeded -> our
///     snapshot of zerostack's internals is stale.
/// Every failure mode returns `Err` with a message naming the fix, so the
/// runner can grade Indeterminate instead of blaming the agent.
pub fn verify_layout(
    run_dir: &Path,
    expected_root: &Path,
    expected_project: &str,
) -> Result<(), String> {
    let mut logs: Vec<PathBuf> = std::fs::read_dir(run_dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
            name.starts_with("turn-") && name.ends_with(".zslog")
        })
        .collect();
    logs.sort();
    if logs.is_empty() {
        return Ok(());
    }

    let expected_root_str = expected_root.display().to_string();
    let mut found = false;
    for log in &logs {
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
    fn verify_layout_ok_when_no_zslogs_present() {
        let dir = std::env::temp_dir().join(format!("zsmem-test-nolog-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(verify_layout(&dir, Path::new("/whatever"), "slug").is_ok());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn verify_layout_errs_when_feature_never_opened_memory() {
        let dir = std::env::temp_dir().join(format!("zsmem-test-missing-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("turn-0.zslog"),
            "some trace line with no memory open\n",
        )
        .unwrap();
        let err = verify_layout(&dir, Path::new("/whatever"), "slug").unwrap_err();
        assert!(err.contains("--features memory"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn verify_layout_errs_on_root_drift() {
        let dir = std::env::temp_dir().join(format!("zsmem-test-drift-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("turn-0.zslog"),
            "2026-01-01T00:00:00Z DEBUG zerostack: memory open: root=/actual/root, project=slug\n",
        )
        .unwrap();
        let err = verify_layout(&dir, Path::new("/expected/root"), "slug").unwrap_err();
        assert!(err.contains("root drift"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn verify_layout_passes_on_exact_match() {
        let dir = std::env::temp_dir().join(format!("zsmem-test-match-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("turn-0.zslog"),
            "2026-01-01T00:00:00Z DEBUG zerostack: memory open: root=/expected/root, project=slug-1234\n",
        )
        .unwrap();
        assert!(verify_layout(&dir, Path::new("/expected/root"), "slug-1234").is_ok());
        std::fs::remove_dir_all(&dir).ok();
    }
}
