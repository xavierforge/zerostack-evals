//! Peek a zerostack target `config.toml` for provider+model — shared by the
//! CLI's auto-tag naming and the run report's identity, so a run's folder
//! name and its recorded `model` never disagree about what was evaluated.

use std::collections::HashSet;
use std::path::Path;

use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
struct Peek {
    provider: Option<String>,
    model: Option<String>,
    /// zerostack's `default_prompt` config key: the prompt a session loads
    /// when a scenario names none. Read here so the harness can derive which
    /// prompt those scenarios actually loaded (prompts-pack section 6), rather
    /// than leaving them blank — the group most exposed to a `code.md`
    /// override is exactly the one that declares no prompt.
    default_prompt: Option<String>,
}

/// Read `provider`/`model` out of a target config.toml. A missing file or
/// unparseable TOML yields `(None, None)` — an unreadable target is graded
/// downstream (the run itself will fail or go indeterminate); this helper
/// just describes what it could read.
pub fn peek(path: &Path) -> (Option<String>, Option<String>) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return (None, None);
    };
    let Ok(p) = toml::from_str::<Peek>(&text) else {
        return (None, None);
    };
    (p.provider, p.model)
}

/// Read `default_prompt` out of a target config.toml. `None` when the file is
/// missing or unparseable (same degradation as `peek`) or simply sets no such
/// key. The harness derives the prompt a no-`prompt` scenario loads from this,
/// falling back to zerostack's own `code` default when it is `None` (see
/// `runner::resolve_prompt`).
pub fn default_prompt(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    toml::from_str::<Peek>(&text).ok()?.default_prompt
}

/// Human-identifiable label for what a run evaluates against:
/// `"<provider>/<model>"`, or just one of the two when only it is known.
/// `target` is mandatory for the `zs` backend (target-matrix section 2), so
/// this is only ever called with `Some` in production; the `None` arm
/// (peek's own unreadable/unparseable-file fallback) still degrades to
/// whichever half is known rather than panicking.
pub fn describe(target: Option<&Path>) -> String {
    let (provider, model) = target.map(peek).unwrap_or((None, None));
    match (provider, model) {
        (Some(p), Some(m)) => format!("{p}/{m}"),
        (Some(p), None) => p,
        (None, Some(m)) => m,
        (None, None) => String::new(),
    }
}

/// The stem a multi-target run nests this target's results under
/// (target-matrix section 3): the target file's name without its extension.
/// Chosen over provider/model because a config's ~30 fields can differ
/// (e.g. only temperature) while provider+model collide; a filename is
/// unique by construction. A path with no filename (e.g. `/`) falls back to
/// `"target"` rather than panicking — an unlikely input the caller's own
/// file-not-found error will already have surfaced.
pub fn stem(target: &Path) -> String {
    target
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("target")
        .to_string()
}

/// Reject two `--target` files that would collide on `stem` within one run:
/// `a/opus.toml` and `b/opus.toml` would otherwise overwrite each other's
/// report and trial dirs under `results/<tag>/opus/`. Checked before any
/// trial runs, so the collision is a hard error rather than silently lost
/// results.
pub fn check_stem_collision(targets: &[&Path]) -> anyhow::Result<()> {
    let mut seen = HashSet::new();
    for t in targets {
        let s = stem(t);
        if !seen.insert(s.clone()) {
            anyhow::bail!(
                "--target files collide on stem '{s}' ({}): rename one of them so \
                 results/<tag>/{s}/ isn't shared by two targets",
                t.display()
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_target(dir: &Path, contents: &str) -> std::path::PathBuf {
        let p = dir.join("config.toml");
        std::fs::write(&p, contents).unwrap();
        p
    }

    #[test]
    fn peek_returns_none_on_missing_file() {
        assert_eq!(peek(Path::new("/no/such/target.toml")), (None, None));
    }

    #[test]
    fn peek_reads_provider_and_model() {
        let dir = std::env::temp_dir().join(format!("zseval-target-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = write_target(
            &dir,
            "provider = \"anthropic\"\nmodel = \"claude-sonnet-4-6\"\n",
        );
        assert_eq!(
            peek(&p),
            (Some("anthropic".into()), Some("claude-sonnet-4-6".into()))
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn default_prompt_reads_the_config_key_and_degrades_to_none() {
        let dir =
            std::env::temp_dir().join(format!("zseval-default-prompt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let with = write_target(
            &dir,
            "provider = \"anthropic\"\ndefault_prompt = \"review\"\n",
        );
        assert_eq!(default_prompt(&with).as_deref(), Some("review"));
        let without = dir.join("bare.toml");
        std::fs::write(&without, "provider = \"anthropic\"\n").unwrap();
        assert_eq!(default_prompt(&without), None);
        assert_eq!(default_prompt(Path::new("/no/such/config.toml")), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn stem_strips_the_extension() {
        assert_eq!(stem(Path::new("targets/opus.toml")), "opus");
        assert_eq!(stem(Path::new("/abs/path/a/sonnet.toml")), "sonnet");
    }

    #[test]
    fn check_stem_collision_errors_on_two_targets_sharing_a_stem() {
        // target-matrix 3.5: `a/opus.toml` and `b/opus.toml` differ in
        // directory but collide on stem, which is what the results layout
        // keys on — must be a hard error before any trial runs.
        let a = Path::new("a/opus.toml");
        let b = Path::new("b/opus.toml");
        let err = check_stem_collision(&[a, b]).unwrap_err();
        assert!(format!("{err:#}").contains("opus"), "{err:#}");
    }

    #[test]
    fn check_stem_collision_allows_distinct_stems() {
        let a = Path::new("a/opus.toml");
        let b = Path::new("b/sonnet.toml");
        assert!(check_stem_collision(&[a, b]).is_ok());
    }

    #[test]
    fn describe_combines_provider_and_model() {
        let dir = std::env::temp_dir().join(format!("zseval-target-test2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = write_target(
            &dir,
            "provider = \"anthropic\"\nmodel = \"claude-sonnet-4-6\"\n",
        );
        assert_eq!(describe(Some(&p)), "anthropic/claude-sonnet-4-6");
        std::fs::remove_dir_all(&dir).ok();
    }
}
