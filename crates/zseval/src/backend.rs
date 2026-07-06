//! Agent backends — generic driving only, zero domain knowledge.
//!
//! `ZsCli` drives the real `zerostack` binary over its public surface
//! (verified against zerostack source 2026-07-06):
//!   - `-p/--print` headless single-shot, `-c/--continue` to resume,
//!     `--model`, `--yolo` (skip permission prompts), `--no-color`,
//!     positional message.
//!   - `--pure-stdout` ("With -p: also print tool calls/results to stdout")
//!     is the only channel that reveals tool calls at all in headless mode:
//!     zerostack's session persistence only records `tool_call`/`tool_result`
//!     messages from the interactive TUI's event handler, never from the `-p`
//!     path (`agent/runner.rs::run_print`) — so a real session JSON never
//!     contains a tool call. `transcript.rs` reconstructs `ToolCall`s from
//!     the `◈ name summary` / `◈ name result:` markers this flag prints,
//!     captured in `turn-N.stdout` below.
//!   - `--log-file <path>` activates zerostack's trace-level file logging
//!     (`src/logging.rs`); stderr only shows `warn+` by default, which is
//!     why a hanging API retry looks silent — the real story is in the
//!     trace log, so we capture one per turn (`turn-N.zslog`).
//!   - Isolation via `ZS_DATA_DIR` / `ZS_CONFIG_DIR` env overrides
//!     (`src/session/storage.rs`), quorum-style throwaway home per run.
//!
//! Environment seeding is delegated entirely to `crate::seed` (generic file
//! placements). This file stays subsystem-agnostic — it only knows how to
//! drive the binary and collect the session files it writes.
//!
//! `Mock` replays a canned session file, so the harness itself is testable
//! in CI without a zerostack build or an API key.

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use crate::scenario::Scenario;
use crate::seed::{self, SeedCtx};
use crate::util::tail_of;

/// Live-stream the agent's stdout/stderr to the console (set by --verbose).
/// Output is always tee'd to `turn-N.stdout` / `turn-N.stderr` regardless.
pub static VERBOSE: AtomicBool = AtomicBool::new(false);

pub fn set_verbose(v: bool) {
    VERBOSE.store(v, Ordering::Relaxed);
}

fn verbose() -> bool {
    VERBOSE.load(Ordering::Relaxed)
}

/// Reader thread: tee child pipe -> log file (+ console when verbose).
fn tee(
    stream: impl std::io::Read + Send + 'static,
    file: PathBuf,
    prefix: &'static str,
    turn: usize,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut out =
            std::io::BufWriter::new(std::fs::File::create(&file).expect("create turn log"));
        let reader = std::io::BufReader::new(stream);
        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => break,
            };
            use std::io::Write;
            let _ = writeln!(out, "{line}");
            let _ = out.flush();
            if verbose() {
                eprintln!("    [zs:{turn}:{prefix}] {line}");
            }
        }
    })
}

pub struct RunArtifacts {
    /// Session JSON files produced, in chronological order.
    pub session_files: Vec<PathBuf>,
    /// The throwaway ZS_DATA_DIR (for `file_*` outcome asserts, default root).
    pub data_dir: PathBuf,
    /// The throwaway ZS_CONFIG_DIR (`file_*` `config:` root; e.g. memory
    /// layout lives under `<config_dir>/agent/memory/`).
    pub config_dir: PathBuf,
    /// The throwaway working dir (`file_*` `work:` root).
    pub work_dir: PathBuf,
    pub wall_secs: f64,
}

pub trait AgentBackend {
    fn name(&self) -> &str;
    /// `model` is `None` unless the user explicitly passed `--model`; when None
    /// the backend must not force a model, so zerostack uses its own configured
    /// provider + default model.
    fn run(
        &self,
        sc: &Scenario,
        model: Option<&str>,
        run_dir: &Path,
    ) -> Result<RunArtifacts>;
}

// ---------------------------------------------------------------------------
// ZsCli
// ---------------------------------------------------------------------------

pub struct ZsCli {
    pub bin: PathBuf,
    /// A zerostack `config.toml` seeded into every run's isolated
    /// `ZS_CONFIG_DIR`. This is how a run declares which provider + model it
    /// evaluates against — explicit, committable, reproducible. Secrets stay
    /// out of it: reference an env var (`api_key_env`, or the provider's
    /// standard `*_API_KEY`), which the harness passes through to zerostack.
    pub target: Option<PathBuf>,
}

impl AgentBackend for ZsCli {
    fn name(&self) -> &str {
        "zs-cli"
    }

    fn run(
        &self,
        sc: &Scenario,
        model: Option<&str>,
        run_dir: &Path,
    ) -> Result<RunArtifacts> {
        // Absolute paths: the child runs with cwd set to `work/`, so relative
        // ZS_DATA_DIR / --log-file paths would resolve against the wrong dir
        // (session files written where we don't look for them, logs unwritable).
        let run_dir = std::fs::canonicalize(run_dir).unwrap_or_else(|_| run_dir.to_path_buf());
        let data = run_dir.join("data");
        let config = run_dir.join("config");
        let work = run_dir.join("work");
        let tmp = run_dir.join("tmp");
        let home = run_dir.join("home");
        for d in [&data, &config, &work, &tmp, &home] {
            std::fs::create_dir_all(d)?;
        }

        // Seed the run-level target config (provider + model) so zerostack
        // reads it via its normal config path. Scenario `config:` seeds run
        // after and can override for a specific case if ever needed.
        if let Some(target) = &self.target {
            std::fs::copy(target, config.join("config.toml"))
                .with_context(|| format!("seed target {}", target.display()))?;
        }

        seed::apply(
            sc,
            &SeedCtx {
                data: &data,
                config: &config,
                work: &work,
            },
        )?;

        let started = Instant::now();
        let timeout = Duration::from_secs(sc.timeout_secs);
        let turns = sc.task.turns();
        for (i, turn) in turns.iter().enumerate() {
            if verbose() {
                let msg_preview: String = turn.msg().chars().take(60).collect();
                eprintln!(
                    "  -> turn {i}{}: {msg_preview}{}",
                    if turn.new_session() { " (new session)" } else { "" },
                    if turn.msg().chars().count() > 60 { "..." } else { "" }
                );
            }
            let turn_started = Instant::now();

            let stdout_log = run_dir.join(format!("turn-{i}.stdout"));
            let stderr_log = run_dir.join(format!("turn-{i}.stderr"));
            // zerostack's own trace-level log: this is where API retries,
            // tool dispatch, and provider errors actually show up.
            let zs_log = run_dir.join(format!("turn-{i}.zslog"));

            let mut cmd = Command::new(&self.bin);
            cmd.arg("-p")
                .arg("--yolo")
                .arg("--no-color")
                .arg("--pure-stdout")
                .arg("--log-file")
                .arg(&zs_log);
            // Only force a model when the user asked; otherwise zerostack picks
            // its own configured provider + default model.
            if let Some(m) = model {
                cmd.arg("--model").arg(m);
            }
            if let Some(name) = &sc.prompt {
                cmd.arg("--load-prompt").arg(name);
            }
            // Continue the running session unless this is the first turn or
            // the scenario explicitly cuts to a new session.
            if i > 0 && !turn.new_session() {
                cmd.arg("--continue");
            }
            cmd.arg(turn.msg());
            cmd.current_dir(&work)
                .env("ZS_DATA_DIR", &data)
                .env("ZS_CONFIG_DIR", &config)
                .env("TMPDIR", &tmp)
                .env("HOME", &home)
                // Never let the child block on our terminal: any unexpected
                // interactive prompt now fails fast (EOF) instead of hanging
                // silently until the timeout.
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());

            let mut child = cmd
                .spawn()
                .with_context(|| format!("spawn {}", self.bin.display()))?;
            let t_out = tee(child.stdout.take().expect("piped"), stdout_log.clone(), "out", i);
            let t_err = tee(child.stderr.take().expect("piped"), stderr_log.clone(), "err", i);

            // Poll-based timeout (std has no wait_timeout) + heartbeat so a
            // long-running turn is visibly alive, not silently stuck.
            let mut next_heartbeat = Duration::from_secs(15);
            let status = loop {
                match child.try_wait()? {
                    Some(status) => break status,
                    None => {
                        if started.elapsed() > timeout {
                            let _ = child.kill();
                            let _ = child.wait();
                            let _ = (t_out.join(), t_err.join());
                            bail!(
                                "timeout after {}s at turn {i}\n\
                                 --- tail of {} (zerostack trace) ---\n{}\n\
                                 --- tail of {} ---\n{}\n\
                                 --- tail of {} ---\n{}\n\
                                 hint: rerun with --verbose, or reproduce manually:\n  \
                                 ZS_DATA_DIR=$(mktemp -d) {} -p --yolo --no-color{} --log-level debug 'ping'",
                                sc.timeout_secs,
                                zs_log.display(),
                                tail_of(&zs_log, 25),
                                stderr_log.display(),
                                tail_of(&stderr_log, 10),
                                stdout_log.display(),
                                tail_of(&stdout_log, 10),
                                self.bin.display(),
                                model.map(|m| format!(" --model {m}")).unwrap_or_default(),
                            );
                        }
                        if started.elapsed() > next_heartbeat && !verbose() {
                            eprintln!(
                                "     ... turn {i} still running ({}s elapsed; --verbose streams live output; trace: {})",
                                started.elapsed().as_secs(),
                                zs_log.display(),
                            );
                            next_heartbeat += Duration::from_secs(15);
                        }
                        std::thread::sleep(Duration::from_millis(200));
                    }
                }
            };
            let _ = (t_out.join(), t_err.join());
            if !status.success() {
                bail!(
                    "turn {i}: zerostack exited with {status}\n\
                     --- tail of {} ---\n{}\n\
                     --- tail of {} (zerostack trace) ---\n{}",
                    stderr_log.display(),
                    tail_of(&stderr_log, 10),
                    zs_log.display(),
                    tail_of(&zs_log, 25),
                );
            }
            if verbose() {
                eprintln!("  <- turn {i} ok ({:.1}s)", turn_started.elapsed().as_secs_f64());
            }
        }

        let mut session_files = list_sessions(&data)?;
        if session_files.is_empty() {
            bail!("no session file produced under {}", data.display());
        }
        // Chronological by mtime so cross-session transcripts concatenate in
        // the order they happened.
        session_files.sort_by_key(|p| {
            std::fs::metadata(p)
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        });

        Ok(RunArtifacts {
            session_files,
            data_dir: data,
            config_dir: config,
            work_dir: work,
            wall_secs: started.elapsed().as_secs_f64(),
        })
    }
}

fn list_sessions(data: &Path) -> Result<Vec<PathBuf>> {
    let dir = data.join("sessions");
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().map(|x| x == "json").unwrap_or(false) {
                out.push(p);
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Mock
// ---------------------------------------------------------------------------

/// Replays a canned session file; used by the harness's own tests and by
/// `--backend mock=<file>` for local plumbing checks.
pub struct Mock {
    pub fixture: PathBuf,
}

impl AgentBackend for Mock {
    fn name(&self) -> &str {
        "mock"
    }

    fn run(
        &self,
        _sc: &Scenario,
        _model: Option<&str>,
        run_dir: &Path,
    ) -> Result<RunArtifacts> {
        let data = run_dir.join("data");
        let config = run_dir.join("config");
        let work = run_dir.join("work");
        std::fs::create_dir_all(data.join("sessions"))?;
        std::fs::create_dir_all(&config)?;
        std::fs::create_dir_all(&work)?;
        let dst = data.join("sessions").join("mock.json");
        std::fs::copy(&self.fixture, &dst)
            .with_context(|| format!("copy mock fixture {}", self.fixture.display()))?;
        Ok(RunArtifacts {
            session_files: vec![dst],
            data_dir: data,
            config_dir: config,
            work_dir: work,
            wall_secs: 0.0,
        })
    }
}
