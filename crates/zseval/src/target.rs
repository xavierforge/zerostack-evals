//! Peek a zerostack target `config.toml` for provider+model — shared by the
//! CLI's auto-tag naming and the run report's identity, so a run's folder
//! name and its recorded `model` never disagree about what was evaluated.

use std::path::Path;

use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
struct Peek {
    provider: Option<String>,
    model: Option<String>,
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

/// Human-identifiable label for what a run evaluates against:
/// `"<provider>/<model>"`, just one of the two, or `"provider-default"` when
/// neither is known. `model_override` (an explicit `--model`) always wins
/// over the target file's own model.
pub fn describe(target: Option<&Path>, model_override: Option<&str>) -> String {
    let (provider, target_model) = target.map(peek).unwrap_or((None, None));
    let model = model_override.map(String::from).or(target_model);
    match (provider, model) {
        (Some(p), Some(m)) => format!("{p}/{m}"),
        (Some(p), None) => p,
        (None, Some(m)) => m,
        (None, None) => "provider-default".to_string(),
    }
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
    fn describe_combines_provider_and_model() {
        let dir = std::env::temp_dir().join(format!("zseval-target-test2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = write_target(
            &dir,
            "provider = \"anthropic\"\nmodel = \"claude-sonnet-4-6\"\n",
        );
        assert_eq!(describe(Some(&p), None), "anthropic/claude-sonnet-4-6");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn describe_prefers_model_override_over_target_model() {
        let dir = std::env::temp_dir().join(format!("zseval-target-test3-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = write_target(
            &dir,
            "provider = \"anthropic\"\nmodel = \"claude-sonnet-4-6\"\n",
        );
        assert_eq!(
            describe(Some(&p), Some("claude-opus-4-8")),
            "anthropic/claude-opus-4-8"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn describe_falls_back_to_provider_default_when_nothing_known() {
        assert_eq!(describe(None, None), "provider-default");
    }

    #[test]
    fn describe_uses_model_override_alone_when_no_target() {
        assert_eq!(describe(None, Some("claude-opus-4-8")), "claude-opus-4-8");
    }
}
