//! Peek a zerostack target `config.toml` for provider+model — shared by the
//! CLI's auto-tag naming and the run report's identity, so a run's folder
//! name and its recorded `model` never disagree about what was evaluated.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::Context;
use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
struct Peek {
    provider: Option<String>,
    model: Option<String>,
    /// zerostack's `default_prompt` config key: the prompt a session loads
    /// when a scenario names none. Read here so the harness can derive which
    /// prompt those scenarios actually loaded, rather than leaving them blank
    /// — the group most exposed to a `code.md` override is exactly the one
    /// that declares no prompt.
    default_prompt: Option<String>,
    /// zerostack's `[custom_providers.<name>]` tables: an OpenAI-compatible
    /// gateway a target may name in `provider` instead of a built-in. Read so
    /// the key check can follow the same indirection zerostack does (the
    /// gateway's `api_key_env` names the variable its key is read from)
    /// rather than mistaking a declared gateway for a typo'd provider.
    #[serde(default)]
    custom_providers: HashMap<String, CustomProvider>,
    /// zerostack's `[api_keys]` table: a key written straight into the config.
    /// zerostack accepts it (after the environment, before giving up), but a
    /// target file lives committed beside the suite, so the preflight gate
    /// reads this only to refuse it — see `embedded_api_keys`.
    #[serde(default)]
    api_keys: HashMap<String, String>,
}

/// The two fields of a `[custom_providers.<name>]` table that decide which key
/// a gateway needs. The rest of the table (base URL, headers, timeouts, certs)
/// is zerostack's business and deliberately not modelled here — no
/// `deny_unknown_fields` anywhere in this file, because a target config is a
/// full zerostack config and the harness only peeks at a few of its fields.
#[derive(Debug, Clone, Default, Deserialize)]
struct CustomProvider {
    provider_type: Option<String>,
    api_key_env: Option<String>,
}

fn parse(path: &Path) -> Option<Peek> {
    toml::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// `parse`, but loud: an unreadable or unparseable target is an error naming
/// the path and the reason. The silent `(None, None)` degradation the other
/// readers here rely on is right for describing a run in progress, and wrong
/// for the preflight gate — a target zerostack could never read is a broken
/// input, and naming it before a run starts is exactly what the gate is for.
fn read(path: &Path) -> anyhow::Result<Peek> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("read target {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parse target {}", path.display()))
}

/// What a target needs in the environment before a run can drive it. Derived
/// from the target's `provider` alone (plus the `[custom_providers]` table it
/// may point into), never from a key or a variable name sitting loose in the
/// file.
#[derive(Debug, Clone, PartialEq)]
pub enum TargetKey {
    /// The named provider's key is read from `var`, which must hold a
    /// non-empty value before the first trial.
    Required { provider: String, var: String },
    /// Nothing to check: zerostack's `ollama` provider talks to a local server
    /// and needs no key at all.
    Keyless { provider: String },
    /// No key requirement could be derived — the file names no provider, or
    /// routes through a gateway whose `provider_type` is not one this harness
    /// knows. `reason` says which. A check that cannot be made is not a failed
    /// check: callers warn, they do not block.
    Undetermined { reason: String },
}

/// Derive `path`'s key requirement the way zerostack itself resolves one: a
/// built-in provider reads its fixed variable, a `[custom_providers.<name>]`
/// gateway reads the `api_key_env` it names (falling back to its underlying
/// `provider_type`'s fixed variable, as zerostack does), and `ollama` reads
/// nothing. The fixed variables come from the judge's own routing table
/// (`JudgeProvider::key_env`) so there is one provider-to-variable mapping in
/// this crate, not two that can disagree.
///
/// `Err` is reserved for the two cases a run could not survive: a target that
/// cannot be read or parsed, and one naming a provider zerostack itself would
/// reject.
pub fn key_requirement(path: &Path) -> anyhow::Result<TargetKey> {
    let cfg = read(path)?;
    let Some(provider) = cfg
        .provider
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    else {
        return Ok(TargetKey::Undetermined {
            reason: format!(
                "{} names no `provider`, so zseval cannot tell which key it needs",
                path.display()
            ),
        });
    };

    if let Some(custom) = cfg.custom_providers.get(provider) {
        if let Some(var) = custom
            .api_key_env
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            return Ok(TargetKey::Required {
                provider: provider.to_string(),
                var: var.to_string(),
            });
        }
        // A gateway that names no variable of its own falls back to whatever
        // its `provider_type` client would read, which is what zerostack does.
        return Ok(match custom.provider_type.as_deref().map(builtin_key_var) {
            Some(BuiltinKey::Var(var)) => TargetKey::Required {
                provider: provider.to_string(),
                var: var.to_string(),
            },
            Some(BuiltinKey::None) => TargetKey::Keyless {
                provider: provider.to_string(),
            },
            Some(BuiltinKey::Unrecognized) | None => TargetKey::Undetermined {
                reason: format!(
                    "{}: custom provider '{provider}' names neither `api_key_env` nor a \
                     `provider_type` zseval knows, so zseval cannot tell which key it needs",
                    path.display()
                ),
            },
        });
    }

    match builtin_key_var(provider) {
        BuiltinKey::Var(var) => Ok(TargetKey::Required {
            provider: provider.to_string(),
            var: var.to_string(),
        }),
        BuiltinKey::None => Ok(TargetKey::Keyless {
            provider: provider.to_string(),
        }),
        BuiltinKey::Unrecognized => anyhow::bail!(
            "{}: unknown provider '{provider}'. zerostack ships anthropic, openai, openrouter, \
             gemini, and ollama; a self-hosted or gateway provider must declare itself in a \
             [custom_providers.{provider}] table naming its `base_url` and `api_key_env`",
            path.display()
        ),
    }
}

/// What variable a zerostack built-in provider name reads its key from.
enum BuiltinKey {
    Var(&'static str),
    /// The provider takes no key (`ollama` talks to a local server).
    None,
    /// Not a name zerostack maps to a built-in client.
    Unrecognized,
}

/// zerostack's provider-name table, as `auth.rs::ProviderKind::from_name`
/// spells it: `google` is another name for gemini and `custom` for the OpenAI
/// client, and `ollama` is the one built-in that needs no key. The four keyed
/// names then hand off to the judge's routing table for the variable itself.
fn builtin_key_var(name: &str) -> BuiltinKey {
    let lowered = name.to_ascii_lowercase();
    let canonical = match lowered.as_str() {
        "google" => "gemini",
        "custom" => "openai",
        "ollama" => return BuiltinKey::None,
        other => other,
    };
    match crate::judge::JudgeProvider::from_name(canonical) {
        Some(p) => BuiltinKey::Var(p.key_env()),
        None => BuiltinKey::Unrecognized,
    }
}

/// The `[api_keys]` entries in `path` holding a non-empty value, sorted by
/// name. zerostack itself would honour these (its key resolution falls back to
/// the config file after the environment), which is exactly why the gate asks:
/// a target file lives committed beside the suite, so a key that works from
/// inside one is a secret in git. Separate from `key_requirement` because it
/// answers a different question — not "which variable must be set" but "is a
/// secret sitting where secrets must not live". Silent on an unreadable file:
/// `key_requirement` already reports that loudly.
pub fn embedded_api_keys(path: &Path) -> Vec<String> {
    let Some(cfg) = parse(path) else {
        return Vec::new();
    };
    let mut names: Vec<String> = cfg
        .api_keys
        .iter()
        .filter(|(_, v)| !v.trim().is_empty())
        .map(|(name, _)| name.clone())
        .collect();
    names.sort();
    names
}

/// Read `provider`/`model` out of a target config.toml. A missing file or
/// unparseable TOML yields `(None, None)` — an unreadable target is graded
/// downstream (the run itself will fail or go indeterminate); this helper
/// just describes what it could read.
pub fn peek(path: &Path) -> (Option<String>, Option<String>) {
    let p = parse(path).unwrap_or_default();
    (p.provider, p.model)
}

/// Read `default_prompt` out of a target config.toml. `None` when the file is
/// missing or unparseable (same degradation as `peek`) or simply sets no such
/// key. The harness derives the prompt a no-`prompt` scenario loads from this,
/// falling back to zerostack's own `code` default when it is `None` (see
/// `runner::derive_prompt`).
pub fn default_prompt(path: &Path) -> Option<String> {
    parse(path)?.default_prompt
}

/// Human-identifiable label for what a run evaluates against:
/// `"<provider>/<model>"`, or just one of the two when only it is known.
/// `target` is mandatory for the `zs` backend, so this is only ever called
/// with `Some` in production; the `None` arm (peek's own
/// unreadable/unparseable-file fallback) still degrades to whichever half is
/// known rather than panicking.
pub fn describe(target: Option<&Path>) -> String {
    let (provider, model) = target.map(peek).unwrap_or((None, None));
    match (provider, model) {
        (Some(p), Some(m)) => format!("{p}/{m}"),
        (Some(p), None) => p,
        (None, Some(m)) => m,
        (None, None) => String::new(),
    }
}

/// The stem a multi-target run nests this target's results under: the target
/// file's name without its extension. Chosen over provider/model because a
/// config's ~30 fields can differ (e.g. only temperature) while provider+model
/// collide; a filename is unique by construction. A path with no filename
/// (e.g. `/`) falls back to `"target"` rather than panicking — an unlikely
/// input the caller's own file-not-found error will already have surfaced.
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
        // `a/opus.toml` and `b/opus.toml` differ in directory but collide on
        // stem, which is what the results layout keys on — must be a hard
        // error before any trial runs.
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

    /// Write `contents` to a fresh per-test file and read its key requirement.
    fn requirement(name: &str, contents: &str) -> anyhow::Result<TargetKey> {
        let dir =
            std::env::temp_dir().join(format!("zseval-target-key-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.toml");
        std::fs::write(&p, contents).unwrap();
        let got = key_requirement(&p);
        std::fs::remove_dir_all(&dir).ok();
        got
    }

    #[test]
    fn every_built_in_provider_maps_to_its_fixed_variable() {
        for (provider, var) in [
            ("anthropic", "ANTHROPIC_API_KEY"),
            ("openai", "OPENAI_API_KEY"),
            ("openrouter", "OPENROUTER_API_KEY"),
            ("gemini", "GEMINI_API_KEY"),
            // zerostack's own aliases: `google` is gemini, `custom` is the
            // OpenAI client.
            ("google", "GEMINI_API_KEY"),
            ("custom", "OPENAI_API_KEY"),
        ] {
            let got = requirement("builtin", &format!("provider = \"{provider}\"\n")).unwrap();
            assert_eq!(
                got,
                TargetKey::Required {
                    provider: provider.into(),
                    var: var.into()
                },
                "{provider}"
            );
        }
    }

    /// The target-side mapping must be the judge's own routing table, not a
    /// second copy of it — checked by comparing against `key_env` directly.
    #[test]
    fn the_variable_comes_from_the_judges_routing_table() {
        let got = requirement("shared", "provider = \"anthropic\"\n").unwrap();
        assert_eq!(
            got,
            TargetKey::Required {
                provider: "anthropic".into(),
                var: crate::judge::JudgeProvider::Anthropic.key_env().into(),
            }
        );
    }

    #[test]
    fn ollama_needs_no_key() {
        assert_eq!(
            requirement("ollama", "provider = \"ollama\"\nmodel = \"llama3.1\"\n").unwrap(),
            TargetKey::Keyless {
                provider: "ollama".into()
            }
        );
    }

    /// A gateway declared in `[custom_providers]` is a legitimate target, not a
    /// typo: the variable it names is what gets checked.
    #[test]
    fn a_custom_gateway_requires_the_variable_it_names() {
        let got = requirement(
            "gateway",
            "provider = \"mylocal\"\nmodel = \"m\"\n\
             [custom_providers.mylocal]\nprovider_type = \"openai\"\n\
             base_url = \"http://localhost:11434/v1\"\napi_key_env = \"MYLOCAL_API_KEY\"\n",
        )
        .unwrap();
        assert_eq!(
            got,
            TargetKey::Required {
                provider: "mylocal".into(),
                var: "MYLOCAL_API_KEY".into()
            }
        );
    }

    /// No `api_key_env` of its own: the gateway falls back to its
    /// `provider_type`'s fixed variable, the same fallback zerostack makes.
    #[test]
    fn a_custom_gateway_without_a_variable_falls_back_to_its_provider_type() {
        let got = requirement(
            "gateway-fallback",
            "provider = \"mylocal\"\n\
             [custom_providers.mylocal]\nprovider_type = \"anthropic\"\n\
             base_url = \"http://localhost:1/v1\"\n",
        )
        .unwrap();
        assert_eq!(
            got,
            TargetKey::Required {
                provider: "mylocal".into(),
                var: "ANTHROPIC_API_KEY".into()
            }
        );
    }

    /// Neither a variable nor a recognizable `provider_type`: undetermined, so
    /// the caller warns instead of blocking a target it simply cannot read a
    /// requirement out of.
    #[test]
    fn an_opaque_custom_gateway_is_undetermined_not_an_error() {
        let got = requirement(
            "gateway-opaque",
            "provider = \"mylocal\"\n\
             [custom_providers.mylocal]\nprovider_type = \"something-else\"\n\
             base_url = \"http://localhost:1/v1\"\n",
        )
        .unwrap();
        assert!(
            matches!(got, TargetKey::Undetermined { ref reason } if reason.contains("mylocal")),
            "{got:?}"
        );
    }

    #[test]
    fn a_target_naming_no_provider_is_undetermined() {
        let got = requirement("no-provider", "model = \"m\"\n").unwrap();
        assert!(
            matches!(got, TargetKey::Undetermined { ref reason } if reason.contains("provider")),
            "{got:?}"
        );
    }

    /// A provider zerostack itself would reject is an error, and the error
    /// names both the offending value and the way to declare a gateway.
    #[test]
    fn an_unknown_provider_is_an_error_naming_it() {
        let err = requirement("unknown-provider", "provider = \"mistral\"\n").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("mistral"), "{msg}");
        assert!(msg.contains("custom_providers"), "{msg}");
    }

    #[test]
    fn an_unreadable_or_unparseable_target_is_an_error_naming_the_path() {
        let err = key_requirement(Path::new("/no/such/target.toml")).unwrap_err();
        assert!(
            format!("{err:#}").contains("/no/such/target.toml"),
            "{err:#}"
        );

        let err = requirement("broken", "provider = \n").unwrap_err();
        assert!(format!("{err:#}").contains("config.toml"), "{err:#}");
    }

    /// Only entries carrying an actual value count as embedded — an empty
    /// string is a placeholder someone blanked, not a leaked secret — and an
    /// absent or unreadable file embeds nothing.
    #[test]
    fn embedded_api_keys_reports_non_empty_entries_sorted() {
        let dir = std::env::temp_dir().join(format!("zseval-target-embed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.toml");
        std::fs::write(
            &p,
            "provider = \"anthropic\"\n[api_keys]\nopenai = \"sk-x\"\nanthropic = \"sk-y\"\n\
             gemini = \"  \"\n",
        )
        .unwrap();
        assert_eq!(embedded_api_keys(&p), vec!["anthropic", "openai"]);
        std::fs::remove_dir_all(&dir).ok();

        assert!(embedded_api_keys(Path::new("/no/such/target.toml")).is_empty());
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
