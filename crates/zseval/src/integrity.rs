//! Input integrity: proof that a run's own inputs did not change while it ran.
//!
//! Every trial drives a real zerostack under `--yolo`, inside a work dir that
//! is only as isolating as the sandbox underneath it. A sandbox that fails
//! open leaves nothing between the agent and the files that define the run —
//! the scenario tree it is graded against, the shared `_fixtures/` its seeds
//! are copied from, the target config naming what is evaluated, the judge file
//! ruling on it. A trial that rewrote one of those was graded against inputs
//! nobody declared, and every number downstream of it describes an experiment
//! that was edited mid-flight.
//!
//! Hashing those inputs before and after turns that from an invisible event
//! into a recorded one. The roots are handed in whole by the caller rather
//! than derived per scenario on purpose: a seed fixture resolves by walking
//! *up* out of the scenario's own directory (`Scenario::resolve_fixture`), so
//! the file a scenario depends on routinely lives above it in a suite-level
//! `_fixtures/`, outside anything a per-scenario snapshot would cover.
//! `Scenario::content_hash` is the neighbouring, deliberately narrower fact:
//! one `scenario.toml`'s own bytes, for comparing two runs, not for catching a
//! write during one.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Content hashes of every file under a set of roots.
///
/// Keyed by the path each file was reached through — the root exactly as the
/// caller passed it, joined with the file's path relative to that root —
/// rather than by the bare relative path. Two roots would otherwise collide
/// (`targets/a.toml` and `targets/b.toml` are single-file roots whose relative
/// path is the same empty one), and a drift line has to name a path the person
/// who typed the command recognises.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    files: BTreeMap<PathBuf, String>,
}

impl Snapshot {
    /// Hash every file under `roots`. A directory root is walked recursively —
    /// dotfiles included, nothing excluded, since the point is to notice *any*
    /// change and a skip list is a list of places an escape goes unnoticed. A
    /// single-file root is hashed directly.
    ///
    /// A root that does not exist contributes nothing rather than failing, so
    /// a root deleted between two snapshots reads as its files going missing:
    /// drift to report, not an error that would abort before reporting it.
    /// Anything that does exist but cannot be read *is* an error — a snapshot
    /// with a hole in it would silently stop watching whatever fell through.
    pub fn of(roots: &[PathBuf]) -> Result<Snapshot> {
        let mut files = BTreeMap::new();
        for root in roots {
            // Follows a symlinked root: a caller passing `scenarios ->
            // ../shared/scenarios` means the tree, not the link.
            let meta = match std::fs::metadata(root) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if meta.is_dir() {
                walk(root, &mut files)?;
            } else {
                let bytes = std::fs::read(root)
                    .with_context(|| format!("snapshot input {}", root.display()))?;
                files.insert(root.clone(), crate::util::sha256_hex(&bytes));
            }
        }
        Ok(Snapshot { files })
    }

    /// What changed between this snapshot (taken first) and `now`, sorted by
    /// path so the same drift always reads the same way.
    pub fn diff(&self, now: &Snapshot) -> Vec<Drift> {
        let mut out: Vec<Drift> = Vec::new();
        for (path, hash) in &self.files {
            match now.files.get(path) {
                None => out.push(Drift::new(path, DriftKind::Removed)),
                Some(now_hash) if now_hash != hash => {
                    out.push(Drift::new(path, DriftKind::Modified))
                }
                Some(_) => {}
            }
        }
        for path in now.files.keys() {
            if !self.files.contains_key(path) {
                out.push(Drift::new(path, DriftKind::Added));
            }
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out
    }
}

/// One input path that did not survive the run unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drift {
    pub path: PathBuf,
    pub kind: DriftKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriftKind {
    Added,
    Removed,
    Modified,
}

impl DriftKind {
    fn label(self) -> &'static str {
        match self {
            DriftKind::Added => "added",
            DriftKind::Removed => "removed",
            DriftKind::Modified => "modified",
        }
    }
}

impl Drift {
    fn new(path: &Path, kind: DriftKind) -> Drift {
        Drift {
            path: path.to_path_buf(),
            kind,
        }
    }
}

impl std::fmt::Display for Drift {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.kind.label(), self.path.display())
    }
}

/// A one-line summary of `drift`, capped at `max` entries.
///
/// For the places that carry the drift into stored evidence — the reason
/// recorded on every trial of the affected scenario, which lands in that
/// trial's `trial.json` and again in `report.json`. An agent that ran a build
/// inside a watched root can drift thousands of files, and the same
/// thousand-path string copied once per trial is evidence nobody can read.
/// The console listing is uncapped instead: it prints once, and it is what a
/// reader diagnosing the escape needs in full.
pub fn summarize(drift: &[Drift], max: usize) -> String {
    let shown: Vec<String> = drift.iter().take(max).map(|d| d.to_string()).collect();
    let rest = drift.len().saturating_sub(shown.len());
    if rest == 0 {
        shown.join(", ")
    } else {
        format!(
            "{}, and {rest} more (the run's console output lists every path)",
            shown.join(", ")
        )
    }
}

/// Recurse into a real directory. Entry types come from `read_dir` (which does
/// not follow links), so only true directories are descended into and the walk
/// is finite even if a link points at one of its own ancestors; a symlink is
/// recorded as the text of what it points at, which still notices it being
/// repointed at something else.
fn walk(dir: &Path, out: &mut BTreeMap<PathBuf, String>) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("snapshot {}", dir.display()))? {
        let entry = entry.with_context(|| format!("snapshot {}", dir.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("snapshot {}", path.display()))?;
        if file_type.is_dir() {
            walk(&path, out)?;
        } else if file_type.is_symlink() {
            let dest = std::fs::read_link(&path)
                .with_context(|| format!("snapshot link {}", path.display()))?;
            out.insert(
                path,
                crate::util::sha256_hex(dest.to_string_lossy().as_bytes()),
            );
        } else {
            let bytes =
                std::fs::read(&path).with_context(|| format!("snapshot {}", path.display()))?;
            out.insert(path, crate::util::sha256_hex(&bytes));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh, empty directory for one test's tree.
    fn dir(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("zseval-integrity-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&d).ok();
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write(path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    /// A small tree with a nested file and a dotfile, the two shapes a naive
    /// walk gets wrong.
    fn tree(name: &str) -> PathBuf {
        let root = dir(name);
        write(&root.join("scenario.toml"), "id = \"x\"\n");
        write(&root.join("_fixtures/shared.txt"), "original\n");
        write(&root.join("nested/deep/file.txt"), "deep\n");
        write(&root.join(".hidden"), "dotfiles count\n");
        root
    }

    #[test]
    fn an_unchanged_tree_snapshots_identically() {
        let root = tree("stable");
        let roots = vec![root.clone()];
        let before = Snapshot::of(&roots).unwrap();
        let after = Snapshot::of(&roots).unwrap();
        assert_eq!(before, after);
        assert!(before.diff(&after).is_empty());
        assert_eq!(
            before.files.len(),
            4,
            "every file including the dotfile is covered: {before:?}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_edited_file_surfaces_as_modified() {
        let root = tree("modified");
        let roots = vec![root.clone()];
        let before = Snapshot::of(&roots).unwrap();
        // The shape of the escape this exists to catch: a suite-level shared
        // fixture rewritten from inside a trial, through an absolute path.
        write(&root.join("_fixtures/shared.txt"), "edited by the agent\n");
        let drift = before.diff(&Snapshot::of(&roots).unwrap());
        assert_eq!(
            drift,
            vec![Drift {
                path: root.join("_fixtures/shared.txt"),
                kind: DriftKind::Modified,
            }]
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_new_file_surfaces_as_added() {
        let root = tree("added");
        let roots = vec![root.clone()];
        let before = Snapshot::of(&roots).unwrap();
        write(
            &root.join("stray.txt"),
            "written where nothing declared it\n",
        );
        let drift = before.diff(&Snapshot::of(&roots).unwrap());
        assert_eq!(
            drift,
            vec![Drift {
                path: root.join("stray.txt"),
                kind: DriftKind::Added,
            }]
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_deleted_file_surfaces_as_removed() {
        let root = tree("removed");
        let roots = vec![root.clone()];
        let before = Snapshot::of(&roots).unwrap();
        std::fs::remove_file(root.join("nested/deep/file.txt")).unwrap();
        let drift = before.diff(&Snapshot::of(&roots).unwrap());
        assert_eq!(
            drift,
            vec![Drift {
                path: root.join("nested/deep/file.txt"),
                kind: DriftKind::Removed,
            }]
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The three classifications are reported together, sorted by path, so one
    /// scattered escape reads as one list rather than three findings.
    #[test]
    fn added_removed_and_modified_are_reported_together() {
        let root = tree("mixed");
        let roots = vec![root.clone()];
        let before = Snapshot::of(&roots).unwrap();
        write(&root.join("_fixtures/shared.txt"), "edited\n");
        write(&root.join("_fixtures/extra.txt"), "new\n");
        std::fs::remove_file(root.join(".hidden")).unwrap();
        let drift = before.diff(&Snapshot::of(&roots).unwrap());
        assert_eq!(
            drift,
            vec![
                Drift {
                    path: root.join(".hidden"),
                    kind: DriftKind::Removed,
                },
                Drift {
                    path: root.join("_fixtures/extra.txt"),
                    kind: DriftKind::Added,
                },
                Drift {
                    path: root.join("_fixtures/shared.txt"),
                    kind: DriftKind::Modified,
                },
            ],
            "{drift:?}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// A `--target` or `--judge` root: one file, hashed directly, and two of
    /// them do not collide on a shared basename.
    #[test]
    fn single_file_roots_are_hashed_directly_and_do_not_collide() {
        let base = dir("single-file");
        write(&base.join("a/config.toml"), "model = \"one\"\n");
        write(&base.join("b/config.toml"), "model = \"two\"\n");
        let roots = vec![base.join("a/config.toml"), base.join("b/config.toml")];
        let before = Snapshot::of(&roots).unwrap();
        assert_eq!(before.files.len(), 2, "two roots, two entries: {before:?}");

        write(&base.join("b/config.toml"), "model = \"edited\"\n");
        let drift = before.diff(&Snapshot::of(&roots).unwrap());
        assert_eq!(
            drift,
            vec![Drift {
                path: base.join("b/config.toml"),
                kind: DriftKind::Modified,
            }]
        );
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn no_roots_watches_nothing() {
        let snap = Snapshot::of(&[]).unwrap();
        assert!(snap.files.is_empty());
        assert!(snap.diff(&Snapshot::of(&[]).unwrap()).is_empty());
    }

    /// A root that never existed is not an error — it contributes nothing, and
    /// a root that vanishes mid-run reads as its files being removed.
    #[test]
    fn a_missing_root_contributes_nothing_and_a_vanishing_one_reads_as_removed() {
        let root = tree("vanishing");
        let roots = vec![root.clone(), root.join("no-such-thing")];
        let before = Snapshot::of(&roots).unwrap();
        assert_eq!(before.files.len(), 4);

        std::fs::remove_dir_all(&root).unwrap();
        let drift = before.diff(&Snapshot::of(&roots).unwrap());
        assert_eq!(drift.len(), 4, "{drift:?}");
        assert!(drift.iter().all(|d| d.kind == DriftKind::Removed));
    }

    /// The stored-evidence summary caps its list; the tail says how many it
    /// left out rather than trailing off.
    #[test]
    fn summarize_caps_the_list_and_says_how_many_it_left_out() {
        let drift: Vec<Drift> = (0..5)
            .map(|i| Drift {
                path: PathBuf::from(format!("f{i}.txt")),
                kind: DriftKind::Modified,
            })
            .collect();
        assert_eq!(
            summarize(&drift, 5),
            "modified f0.txt, modified f1.txt, modified f2.txt, modified f3.txt, \
             modified f4.txt"
        );
        let capped = summarize(&drift, 2);
        assert!(
            capped.starts_with("modified f0.txt, modified f1.txt,"),
            "{capped}"
        );
        assert!(capped.contains("3 more"), "{capped}");
    }
}
