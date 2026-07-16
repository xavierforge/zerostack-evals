//! zerostack MCP (Model Context Protocol) subsystem knowledge.
//!
//! Unblocked 2026-07-08 by upstream **R2** landing (zerostack `dfb9f75`,
//! `feat(headless): connect configured MCP servers in -p and --loop`,
//! shipped in v1.6.2): `connect_headless_mcp` now runs in both the `-p` and
//! `--loop` branches of `main.rs`, so `cfg.mcp_servers` is no longer dead in
//! every mode this harness can drive (see `EVAL-COVERAGE-PLAN.md`'s Commit
//! 13 write-up for the prior investigation that blocked this). MCP tools
//! register under their **bare tool name** (`McpTool::name()` returns
//! `self.definition.name` directly, `src/extras/mcp/tool.rs`, no
//! server-name prefix), so `tool_called <bare_name>` works with zero domain
//! code once a server is connected — the only harness-owned knowledge left
//! is (a) how to get an `[mcp_servers.<name>]` table into the run's seeded
//! `config.toml`, since that's config data, not a file `[[files]]` can just
//! copy over, and (b) the drift check.
//!
//! Verified live against zerostack v1.6.2 (`zerostack -p --yolo
//! --pure-stdout --log-level trace`, isolated `ZS_DATA_DIR`/`ZS_CONFIG_DIR`,
//! a hand-rolled stdio MCP server — see `scenarios/mcp/_fixtures/
//! mock_mcp_server.py`) on 2026-07-08:
//!   - `McpClientManager::connect_all` logs `tracing::info!("Connected to
//!     MCP server '{}'", name)` (`extras/mcp/mod.rs`) at INFO, which lands
//!     in the turn's `--log-level trace` file regardless — the drift-check
//!     anchor, same pattern as `memory::verify`'s `memory open: root=…`
//!     line.
//!   - **Gotcha, easy to miss**: zerostack injects a live, real
//!     "Exa Web Search" MCP server by default —
//!     `config::load::inject_mcp_defaults` resolves `enable-exa-mcp` (note
//!     the **kebab-case** TOML key — the field is `enable_exa_mcp` in Rust
//!     but `#[serde(rename = "enable-exa-mcp")]` on the wire, `config/
//!     mod.rs`) to `true` unless the seeded config explicitly sets it
//!     `false`. Left alone, every mcp scenario silently gains a second,
//!     network-backed, real web-search tool alongside the mock one under
//!     test — confirmed by a stray `Connected to MCP server 'Exa Web
//!     Search'` trace line appearing even when the seeded `config.toml`
//!     only declared our own `[mcp_servers.tickets]`. `expand` below always
//!     forces `enable-exa-mcp = false` in the seeded config so the mock
//!     server is the only MCP tool present — a scenario testing "does the
//!     agent use the tool it was given" would otherwise be confounded by a
//!     tool it wasn't.
//!
//! Scenario sugar:
//!   [seed.mcp]
//!   servers = [ { name = "tickets", script = "_fixtures/mock_mcp_server.py" } ]
//!
//! Unlike `memory`, there is no "start from an empty store" case — an MCP
//! tool can't exist without a configured server, so every mcp scenario
//! declares `[seed.mcp]`; `domains::verify` dispatches purely off its
//! presence, no `domains = ["mcp"]` bare opt-in is needed (though the name
//! is still registered in `KNOWN_DOMAINS` so a stray `domains = ["mcp"]`
//! doesn't read as a typo).

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::backend::RunRoots;
use crate::scenario::Scenario;
use crate::seed::Placement;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct McpSeed {
    pub servers: Vec<McpServerSeed>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct McpServerSeed {
    pub name: String,
    /// Path to a `python3` stdio MCP server script, resolved the same way
    /// as any other fixture (walking up from the scenario dir).
    pub script: PathBuf,
}

/// Load-time check: at least one server, names are non-empty and unique,
/// every script fixture resolves — a typo'd path fails `zseval list`/load,
/// not mid-run after burning an API call.
pub fn validate(mcp: &McpSeed, sc: &Scenario) -> Result<()> {
    if mcp.servers.is_empty() {
        anyhow::bail!("{}: [seed.mcp] must declare at least one server", sc.id);
    }
    let mut seen = std::collections::HashSet::new();
    for s in &mcp.servers {
        if s.name.trim().is_empty() {
            anyhow::bail!("{}: [seed.mcp] server name must not be empty", sc.id);
        }
        if !seen.insert(s.name.as_str()) {
            anyhow::bail!("{}: [seed.mcp] duplicate server name '{}'", sc.id, s.name);
        }
        sc.resolve_fixture(&s.script)
            .with_context(|| format!("{}: bad [seed.mcp] server '{}' script", sc.id, s.name))?;
    }
    Ok(())
}

/// Append `[mcp_servers.<name>]` tables (a `command`+`args` stdio server per
/// `McpServerConfig::Command`, `src/extras/mcp/config.rs`) into the seeded
/// `config.toml`'s TOML text, and force `enable-exa-mcp = false` — see the
/// module doc's "Gotcha" paragraph. Pure string-in/string-out so it's
/// testable without a full `Scenario`/`RunRoots`; `expand` below is the
/// thin I/O wrapper that reads/writes the actual file.
fn rewrite_config(text: &str, servers: &[(String, PathBuf)]) -> Result<String> {
    let mut doc: toml::Value = text.parse().context("parse seeded config.toml as TOML")?;
    let table = doc
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("seeded config.toml is not a TOML table"))?;

    table.insert("enable-exa-mcp".to_string(), toml::Value::Boolean(false));

    let mcp_servers = table
        .entry("mcp_servers".to_string())
        .or_insert_with(|| toml::Value::Table(Default::default()))
        .as_table_mut()
        .ok_or_else(|| {
            anyhow::anyhow!("existing 'mcp_servers' key in config.toml is not a table")
        })?;

    for (name, script) in servers {
        let mut entry = toml::value::Table::new();
        entry.insert(
            "command".to_string(),
            toml::Value::String("python3".to_string()),
        );
        entry.insert(
            "args".to_string(),
            toml::Value::Array(vec![toml::Value::String(script.display().to_string())]),
        );
        mcp_servers.insert(name.clone(), toml::Value::Table(entry));
    }

    toml::to_string(&doc).context("serialize updated config.toml")
}

/// Expand `[seed.mcp]` sugar by rewriting the run's already-seeded
/// `config.toml` in place (the target config is copied before `seed::apply`
/// runs — see `backend.rs`'s `ZsCli::run` — so it's guaranteed to exist by
/// the time this runs). Returns no `Placement`s: unlike a file-copy domain,
/// there is nothing left to place, the mutation already happened.
pub fn expand(mcp: &McpSeed, sc: &Scenario, ctx: &RunRoots) -> Result<Vec<Placement>> {
    let config_path = ctx.config.join("config.toml");
    let text = std::fs::read_to_string(&config_path).with_context(|| {
        format!(
            "{}: read seeded {} to append [mcp_servers] — mcp scenarios need a \
             --target config.toml seeded before domain expansion runs",
            sc.id,
            config_path.display()
        )
    })?;

    let mut resolved = Vec::with_capacity(mcp.servers.len());
    for s in &mcp.servers {
        let script = sc.resolve_fixture(&s.script)?;
        let script = std::fs::canonicalize(&script).with_context(|| {
            format!(
                "{}: canonicalize mcp server '{}' script {}",
                sc.id,
                s.name,
                script.display()
            )
        })?;
        resolved.push((s.name.clone(), script));
    }

    let rewritten = rewrite_config(&text, &resolved)
        .with_context(|| format!("{}: expand [seed.mcp]", sc.id))?;
    std::fs::write(&config_path, rewritten)
        .with_context(|| format!("{}: write updated {}", sc.id, config_path.display()))?;

    Ok(Vec::new())
}

/// Cross-check that every seeded server actually connected: scan the given
/// zslogs for `Connected to MCP server '{name}'` (`extras/mcp/mod.rs`'s
/// `connect_all`, logged at INFO — always captured by the harness's
/// trace-level `--log-file`, see `memory::verify`'s doc for why).
///   - no zslogs given -> nothing to verify (e.g. the mock backend, which
///     never calls `seed::apply`/`expand` at all).
///   - a server's connection line never appears -> either the `mcp` feature
///     isn't compiled in, headless MCP connection wiring regressed, or the
///     mock server itself failed to start (check `turn-*.stderr` for the
///     `MCP server '{name}' not connected: …` notice `connect_headless_mcp`
///     prints on failure).
///
/// Every failure mode returns `Err` with a message naming the fix, so the
/// runner grades Indeterminate instead of blaming the agent.
pub fn verify(mcp: &McpSeed, zslogs: &[PathBuf]) -> Result<(), String> {
    if zslogs.is_empty() {
        return Ok(());
    }
    let mut text = String::new();
    for log in zslogs {
        text.push_str(&std::fs::read_to_string(log).unwrap_or_default());
        text.push('\n');
    }
    for s in &mcp.servers {
        let needle = format!("Connected to MCP server '{}'", s.name);
        if !text.contains(&needle) {
            return Err(format!(
                "mcp domain drift: no \"{needle}\" trace line found in any turn-*.zslog — is \
                 the zerostack build missing the mcp feature, did headless MCP connection \
                 wiring regress (see EVAL-COVERAGE-PLAN.md's upstream R2), or did the mock \
                 server fail to start (check turn-*.stderr for \"MCP server '{}' not \
                 connected\")?",
                s.name
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrite_config_appends_server_and_disables_exa_default() {
        let text = "provider = \"anthropic\"\nmodel = \"claude-sonnet-4-6\"\n";
        let out = rewrite_config(
            text,
            &[(
                "tickets".to_string(),
                PathBuf::from("/abs/mock_mcp_server.py"),
            )],
        )
        .unwrap();
        assert!(out.contains("enable-exa-mcp = false"), "{out}");
        assert!(out.contains("[mcp_servers.tickets]"), "{out}");
        assert!(out.contains("command = \"python3\""), "{out}");
        assert!(out.contains("/abs/mock_mcp_server.py"), "{out}");
        assert!(out.contains("provider = \"anthropic\""), "{out}");
    }

    #[test]
    fn rewrite_config_preserves_existing_mcp_servers() {
        let text =
            "provider = \"anthropic\"\n\n[mcp_servers.other]\ncommand = \"foo\"\nargs = []\n";
        let out = rewrite_config(
            text,
            &[("tickets".to_string(), PathBuf::from("/abs/mock.py"))],
        )
        .unwrap();
        assert!(out.contains("[mcp_servers.other]"), "{out}");
        assert!(out.contains("[mcp_servers.tickets]"), "{out}");
    }

    #[test]
    fn rewrite_config_overrides_an_existing_enable_exa_mcp_true() {
        let text = "provider = \"anthropic\"\n\"enable-exa-mcp\" = true\n";
        let out = rewrite_config(text, &[]).unwrap();
        assert!(out.contains("enable-exa-mcp = false"), "{out}");
        assert!(!out.contains("true"), "{out}");
    }

    #[test]
    fn rewrite_config_errs_on_invalid_toml() {
        let err = rewrite_config("not valid toml [[[", &[]).unwrap_err();
        assert!(err.to_string().contains("parse"), "{err}");
    }

    #[test]
    fn verify_ok_when_no_zslogs_present() {
        let mcp = McpSeed {
            servers: vec![McpServerSeed {
                name: "tickets".to_string(),
                script: PathBuf::from("mock.py"),
            }],
        };
        assert!(verify(&mcp, &[]).is_ok());
    }

    #[test]
    fn verify_errs_when_connection_line_missing() {
        let dir = std::env::temp_dir().join(format!("zsmcp-test-missing-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("turn-0.zslog");
        std::fs::write(&log, "some trace line with no mcp connection\n").unwrap();
        let mcp = McpSeed {
            servers: vec![McpServerSeed {
                name: "tickets".to_string(),
                script: PathBuf::from("mock.py"),
            }],
        };
        let err = verify(&mcp, &[log]).unwrap_err();
        assert!(err.contains("Connected to MCP server 'tickets'"), "{err}");
        assert!(err.contains("upstream R2"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn verify_passes_when_connection_line_present() {
        let dir = std::env::temp_dir().join(format!("zsmcp-test-match-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("turn-0.zslog");
        std::fs::write(
            &log,
            "2026-01-01T00:00:00Z INFO zerostack::extras::mcp: Connected to MCP server 'tickets'\n",
        )
        .unwrap();
        let mcp = McpSeed {
            servers: vec![McpServerSeed {
                name: "tickets".to_string(),
                script: PathBuf::from("mock.py"),
            }],
        };
        assert!(verify(&mcp, std::slice::from_ref(&log)).is_ok());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn verify_errs_when_only_some_servers_connected() {
        let dir = std::env::temp_dir().join(format!("zsmcp-test-partial-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("turn-0.zslog");
        std::fs::write(
            &log,
            "INFO zerostack::extras::mcp: Connected to MCP server 'tickets'\n",
        )
        .unwrap();
        let mcp = McpSeed {
            servers: vec![
                McpServerSeed {
                    name: "tickets".to_string(),
                    script: PathBuf::from("mock.py"),
                },
                McpServerSeed {
                    name: "other".to_string(),
                    script: PathBuf::from("mock2.py"),
                },
            ],
        };
        let err = verify(&mcp, &[log]).unwrap_err();
        assert!(err.contains("'other'"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
