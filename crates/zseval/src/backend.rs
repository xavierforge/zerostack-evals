//! Agent backends — generic driving only, zero domain knowledge.
//!
//! `ZsCli` drives the real `zerostack` binary over its public surface
//! (verified against zerostack source 2026-07-06):
//!   - `-p/--print` headless single-shot, `-c/--continue` to resume,
//!     `--yolo` (skip permission prompts), `--no-color`, positional message.
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
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use crate::prompts::PromptPack;
use crate::scenario::Scenario;
use crate::seed;
use crate::util::tail_of;

/// The three isolated root directories of one run: everything a seed
/// placement or a `file_*` assert resolves against. Owned here because
/// `backend` is what carves a run dir into `data`/`config`/`work` in the
/// first place — `seed`, `asserts`, and `domains::` modules are all just
/// consumers of this same shape, before (`seed::apply`) and after
/// (grading) the agent actually runs.
pub struct RunRoots<'a> {
    pub data: &'a Path,
    pub config: &'a Path,
    pub work: &'a Path,
}

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

/// Spawn `cmd`, tee its stdout/stderr to `stdout_log`/`stderr_log` (and the
/// console under `--verbose`), and wait — enforcing `timeout` measured from
/// `overall_started`, with the same kill-and-diagnose-on-expiry and
/// non-zero-exit handling either way. Shared by the per-turn `-p`/
/// `--continue` path and the single-invocation `--loop` path so their
/// timeout/heartbeat/diagnostic behavior can't drift apart. `repro_flag` is
/// only cosmetic — it's substituted into the timeout error's "reproduce
/// manually" hint (`-p` vs `--loop`).
#[allow(clippy::too_many_arguments)]
fn spawn_and_wait(
    mut cmd: Command,
    bin: &Path,
    repro_flag: &str,
    overall_started: Instant,
    timeout: Duration,
    turn_label: usize,
    stdout_log: &Path,
    stderr_log: &Path,
    zs_log: &Path,
) -> Result<()> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .with_context(|| format!("spawn {}", bin.display()))?;
    let t_out = tee(
        child.stdout.take().expect("piped"),
        stdout_log.to_path_buf(),
        "out",
        turn_label,
    );
    let t_err = tee(
        child.stderr.take().expect("piped"),
        stderr_log.to_path_buf(),
        "err",
        turn_label,
    );

    // Poll-based timeout (std has no wait_timeout) + heartbeat so a
    // long-running turn is visibly alive, not silently stuck.
    let mut next_heartbeat = Duration::from_secs(15);
    let status = loop {
        match child.try_wait()? {
            Some(status) => break status,
            None => {
                if overall_started.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = (t_out.join(), t_err.join());
                    bail!(
                        "timeout after {}s at turn {turn_label}\n\
                         --- tail of {} (zerostack trace) ---\n{}\n\
                         --- tail of {} ---\n{}\n\
                         --- tail of {} ---\n{}\n\
                         hint: rerun with --verbose, or reproduce manually:\n  \
                         ZS_DATA_DIR=$(mktemp -d) {} {repro_flag} --yolo --no-color \
                         --log-level debug 'ping'",
                        timeout.as_secs(),
                        zs_log.display(),
                        tail_of(zs_log, 25),
                        stderr_log.display(),
                        tail_of(stderr_log, 10),
                        stdout_log.display(),
                        tail_of(stdout_log, 10),
                        bin.display(),
                    );
                }
                if overall_started.elapsed() > next_heartbeat && !verbose() {
                    eprintln!(
                        "     ... turn {turn_label} still running ({}s elapsed; --verbose \
                         streams live output; trace: {})",
                        overall_started.elapsed().as_secs(),
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
            "turn {turn_label}: zerostack exited with {status}\n\
             --- tail of {} ---\n{}\n\
             --- tail of {} (zerostack trace) ---\n{}",
            stderr_log.display(),
            tail_of(stderr_log, 10),
            zs_log.display(),
            tail_of(zs_log, 25),
        );
    }
    Ok(())
}

/// The three logs captured for one turn (one `zerostack` invocation).
/// Constructing this is backend's job — it's the module that decides the
/// `turn-N.{stdout,stderr,zslog}` naming convention in the first place, so
/// nothing downstream should rediscover it by re-globbing a run dir.
#[derive(Debug, Clone)]
pub struct TurnArtifacts {
    pub stdout: PathBuf,
    pub stderr: PathBuf,
    pub zslog: PathBuf,
}

pub struct RunArtifacts {
    /// Session JSON files produced, in chronological order.
    pub session_files: Vec<PathBuf>,
    /// Per-turn logs, in the order the turns ran. Empty for backends (e.g.
    /// `Mock`) that don't drive a real `zerostack` process.
    pub turns: Vec<TurnArtifacts>,
    /// The throwaway ZS_DATA_DIR (for `file_*` outcome asserts, default root).
    pub data_dir: PathBuf,
    /// The throwaway ZS_CONFIG_DIR (`file_*` `config:` root; e.g. memory
    /// layout lives under `<config_dir>/agent/memory/`).
    pub config_dir: PathBuf,
    /// The throwaway working dir (`file_*` `work:` root).
    pub work_dir: PathBuf,
    pub wall_secs: f64,
}

impl RunArtifacts {
    /// Borrow this run's three roots, for grading (`Assert::eval`,
    /// `domains::memory::verify`) after the agent has already run.
    pub fn roots(&self) -> RunRoots<'_> {
        RunRoots {
            data: &self.data_dir,
            config: &self.config_dir,
            work: &self.work_dir,
        }
    }
}

/// Reconstruct `TurnArtifacts` for a run dir when no live `RunArtifacts` is
/// available — the one legitimate case is `zseval explain <trial-dir>`,
/// which reads a *previous* run from a fresh process with no in-memory
/// struct to consult. Matches turn indices across the three extensions and
/// returns them in turn order; a missing extension for a given turn still
/// yields a (non-existent) path, since every consumer already handles a
/// missing/unreadable log gracefully.
pub fn discover_turn_artifacts(run_dir: &Path) -> Vec<TurnArtifacts> {
    let mut indices = std::collections::BTreeSet::new();
    if let Ok(entries) = std::fs::read_dir(run_dir) {
        for e in entries.flatten() {
            let name = e.file_name();
            let name = name.to_string_lossy();
            if let Some(rest) = name.strip_prefix("turn-") {
                if let Some((n, _ext)) = rest.split_once('.') {
                    if let Ok(n) = n.parse::<usize>() {
                        indices.insert(n);
                    }
                }
            }
        }
    }
    indices
        .into_iter()
        .map(|i| TurnArtifacts {
            stdout: run_dir.join(format!("turn-{i}.stdout")),
            stderr: run_dir.join(format!("turn-{i}.stderr")),
            zslog: run_dir.join(format!("turn-{i}.zslog")),
        })
        .collect()
}

/// `Sync` so a shared `&dyn AgentBackend` can be handed to several trial
/// worker threads at once (`runner::run_trials_for_scenario`'s `--jobs`
/// path) — every real implementor (`ZsCli`, `Mock`) is plain owned data with
/// no interior mutability, so this costs nothing, it just makes the bound
/// explicit at the trait object boundary.
pub trait AgentBackend: Sync {
    fn name(&self) -> &str;
    /// The backend must not force a model: what a run evaluates against comes
    /// from the backend's own configuration, not from the caller.
    fn run(&self, sc: &Scenario, run_dir: &Path) -> Result<RunArtifacts>;
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
    /// A prompt pack (`--prompts`) seeded into every trial's
    /// `work:.zerostack/prompts/`, ahead of the scenario's own placements so
    /// a scenario seeding the same name still wins. `Arc` because one loaded
    /// pack is shared across every target in a multi-target run
    /// (`run_over_targets`' `make_backend` closure), not reloaded per target.
    pub prompts: Option<Arc<PromptPack>>,
}

impl AgentBackend for ZsCli {
    fn name(&self) -> &str {
        "zs-cli"
    }

    fn run(&self, sc: &Scenario, run_dir: &Path) -> Result<RunArtifacts> {
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

        // Seed the pack before the scenario's own placements, so a scenario
        // that seeds a same-named `work:.zerostack/prompts/*.md` overrides
        // it (`prompts-pack-run` spec, "A scenario's own prompt seed wins
        // over the pack") rather than the pack silently winning last.
        if let Some(pack) = &self.prompts {
            let prompts_dir = work.join(".zerostack").join("prompts");
            std::fs::create_dir_all(&prompts_dir)?;
            for file in pack.files() {
                std::fs::write(prompts_dir.join(&file.file_name), &file.bytes).with_context(
                    || format!("seed prompt pack file {}", file.file_name),
                )?;
            }
        }

        seed::apply(
            sc,
            &RunRoots {
                data: &data,
                config: &config,
                work: &work,
            },
        )?;

        let roots = RunDirs {
            run_dir: &run_dir,
            data: &data,
            config: &config,
            work: &work,
            tmp: &tmp,
            home: &home,
        };

        match &sc.loop_cfg {
            Some(loop_cfg) => self.run_loop(sc, loop_cfg, &roots),
            None => self.run_print(sc, &roots),
        }
    }
}

/// The five isolated directories one trial's invocation(s) run inside —
/// bundled so both `run_print` and `run_loop` take one argument instead of
/// five, now that there are two invocation shapes to share it between.
struct RunDirs<'a> {
    run_dir: &'a Path,
    data: &'a Path,
    config: &'a Path,
    work: &'a Path,
    tmp: &'a Path,
    home: &'a Path,
}

impl ZsCli {
    /// `mode = "print"` (default): the per-turn `-p`/`--continue` loop —
    /// unchanged behavior from before `mode`/`loop` existed, just relocated
    /// out of `AgentBackend::run` now that it has a loop-mode sibling.
    fn run_print(&self, sc: &Scenario, d: &RunDirs) -> Result<RunArtifacts> {
        let started = Instant::now();
        let timeout = Duration::from_secs(sc.timeout_secs);
        let turns = sc.task.turns();
        let mut turn_logs: Vec<TurnArtifacts> = Vec::with_capacity(turns.len());
        for (i, turn) in turns.iter().enumerate() {
            if verbose() {
                let msg_preview: String = turn.msg().chars().take(60).collect();
                eprintln!(
                    "  -> turn {i}{}: {msg_preview}{}",
                    if turn.new_session() {
                        " (new session)"
                    } else {
                        ""
                    },
                    if turn.msg().chars().count() > 60 {
                        "..."
                    } else {
                        ""
                    }
                );
            }
            let turn_started = Instant::now();

            let stdout_log = d.run_dir.join(format!("turn-{i}.stdout"));
            let stderr_log = d.run_dir.join(format!("turn-{i}.stderr"));
            // zerostack's own trace-level log: this is where API retries,
            // tool dispatch, and provider errors actually show up.
            let zs_log = d.run_dir.join(format!("turn-{i}.zslog"));

            let mut cmd = Command::new(&self.bin);
            cmd.arg("-p")
                .arg("--yolo")
                .arg("--no-color")
                .arg("--pure-stdout")
                .arg("--log-file")
                .arg(&zs_log);
            if let Some(name) = &sc.prompt {
                cmd.arg("--load-prompt").arg(name);
            }
            // Continue the running session unless this is the first turn or
            // the scenario explicitly cuts to a new session.
            if i > 0 && !turn.new_session() {
                cmd.arg("--continue");
            }
            cmd.arg(turn.msg());
            cmd.current_dir(d.work)
                .env("ZS_DATA_DIR", d.data)
                .env("ZS_CONFIG_DIR", d.config)
                .env("TMPDIR", d.tmp)
                .env("HOME", d.home);

            spawn_and_wait(
                cmd,
                &self.bin,
                "-p",
                started,
                timeout,
                i,
                &stdout_log,
                &stderr_log,
                &zs_log,
            )?;
            if verbose() {
                eprintln!(
                    "  <- turn {i} ok ({:.1}s)",
                    turn_started.elapsed().as_secs_f64()
                );
            }
            turn_logs.push(TurnArtifacts {
                stdout: stdout_log,
                stderr: stderr_log,
                zslog: zs_log,
            });
        }

        let session_files = discover_session_files(d.data);
        if session_files.is_empty() {
            bail!("no session file produced under {}", d.data.display());
        }

        Ok(RunArtifacts {
            session_files,
            turns: turn_logs,
            data_dir: d.data.to_path_buf(),
            config_dir: d.config.to_path_buf(),
            work_dir: d.work.to_path_buf(),
            wall_secs: started.elapsed().as_secs_f64(),
        })
    }

    /// `mode = "loop"`: one single `zerostack --loop --loop-max N [--loop-run
    /// CMD] <task>` invocation instead of the per-turn `-p` loop.
    /// `Scenario::load` already guarantees `sc.task` is exactly one turn and
    /// `loop_cfg.max_iterations >= 1` for any scenario that reaches here.
    ///
    /// No session file is produced (`run_headless_loop` never calls
    /// `save_session`) — grading evidence is the iteration records at
    /// `$ZS_DATA_DIR/loops/<uuid>/iter-NNNN.json`, which `transcript.rs`
    /// folds in directly from `data_dir`, so `session_files` is legitimately
    /// empty here (unlike `run_print`, this is not an error condition).
    fn run_loop(
        &self,
        sc: &Scenario,
        loop_cfg: &crate::scenario::LoopCfg,
        d: &RunDirs,
    ) -> Result<RunArtifacts> {
        let started = Instant::now();
        let timeout = Duration::from_secs(sc.timeout_secs);
        let turn = &sc.task.turns()[0];

        let stdout_log = d.run_dir.join("turn-0.stdout");
        let stderr_log = d.run_dir.join("turn-0.stderr");
        let zs_log = d.run_dir.join("turn-0.zslog");

        let mut cmd = Command::new(&self.bin);
        cmd.arg("--loop")
            .arg("--yolo")
            .arg("--no-color")
            .arg("--log-file")
            .arg(&zs_log)
            .arg("--loop-max")
            .arg(loop_cfg.max_iterations.to_string());
        if let Some(run_cmd) = &loop_cfg.run {
            cmd.arg("--loop-run").arg(run_cmd);
        }
        if let Some(name) = &sc.prompt {
            cmd.arg("--load-prompt").arg(name);
        }
        cmd.arg(turn.msg());
        cmd.current_dir(d.work)
            .env("ZS_DATA_DIR", d.data)
            .env("ZS_CONFIG_DIR", d.config)
            .env("TMPDIR", d.tmp)
            .env("HOME", d.home);

        spawn_and_wait(
            cmd,
            &self.bin,
            "--loop",
            started,
            timeout,
            0,
            &stdout_log,
            &stderr_log,
            &zs_log,
        )?;

        Ok(RunArtifacts {
            session_files: Vec::new(),
            turns: vec![TurnArtifacts {
                stdout: stdout_log,
                stderr: stderr_log,
                zslog: zs_log,
            }],
            data_dir: d.data.to_path_buf(),
            config_dir: d.config.to_path_buf(),
            work_dir: d.work.to_path_buf(),
            wall_secs: started.elapsed().as_secs_f64(),
        })
    }
}

/// Every `*.json` file directly under `data_dir/sessions/`, chronological by
/// mtime so cross-session transcripts concatenate in the order they
/// happened. The shape both `ZsCli` (a fresh run) and `regrade` (an existing
/// run_dir) scan to find gradable session files — exposed so
/// `runner::regrade` can reconstruct a `RunArtifacts` from an
/// already-completed run_dir without a live backend.
pub fn discover_session_files(data_dir: &Path) -> Vec<PathBuf> {
    let dir = data_dir.join("sessions");
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().map(|x| x == "json").unwrap_or(false) {
                out.push(p);
            }
        }
    }
    out.sort_by_key(|p| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
    });
    out
}

/// Recursively copy a directory tree — used by `Mock` to replay a captured
/// `mode = "loop"` fixture's `data/loops/**` into a fresh run_dir (unlike
/// turn logs, these can't just be read in place: `transcript.rs` expects
/// them under `artifacts.data_dir`, which is the new run_dir here, not the
/// fixture's original one).
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for e in std::fs::read_dir(src).with_context(|| format!("read {}", src.display()))? {
        let e = e?;
        let from = e.path();
        let to = dst.join(e.file_name());
        if from.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)
                .with_context(|| format!("copy {} -> {}", from.display(), to.display()))?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Mock
// ---------------------------------------------------------------------------

/// Replays canned artifacts; used by the harness's own tests and by
/// `--backend mock=<path>` for local plumbing checks. `fixture` is either a
/// single session JSON file (legacy shape: no turn logs, so stdout-based
/// tool-call reconstruction is never exercised) or a directory — a
/// previously captured trial dir (`data/sessions/*.json` plus
/// `turn-N.{stdout,stderr,zslog}`) — replayed exactly as the real backend
/// produced it, including the stdout channel that's the only place tool
/// calls actually appear in headless mode (see `transcript.rs`'s module
/// doc). The directory form is what backs `zseval regrade` and lets a test
/// exercise the real evidence path without a live zerostack build.
///
/// Caveat: a `[seed.memory]` scenario replayed this way (`--backend
/// mock=<dir> run`, as opposed to `regrade`, which reads the fixture dir
/// in place) will not clear `domains::memory::verify` — the zslog's
/// `memory open: root=…` line records paths from wherever the fixture was
/// *originally* captured, while `run` seeds a brand-new run_dir here, so
/// the roots this call graded against can never match. It grades
/// Indeterminate, never a false Pass/Fail, but a memory scenario is only
/// meaningfully replayable via `regrade` (in place) or a live `zerostack`
/// build, not `--backend mock=<dir> run`.
pub struct Mock {
    pub fixture: PathBuf,
}

impl AgentBackend for Mock {
    fn name(&self) -> &str {
        "mock"
    }

    fn run(&self, _sc: &Scenario, run_dir: &Path) -> Result<RunArtifacts> {
        let data = run_dir.join("data");
        let config = run_dir.join("config");
        let work = run_dir.join("work");
        std::fs::create_dir_all(data.join("sessions"))?;
        std::fs::create_dir_all(&config)?;
        std::fs::create_dir_all(&work)?;

        if self.fixture.is_dir() {
            // A `mode = "loop"` fixture has no `data/sessions/` at all (loop
            // mode never calls `save_session`) — tolerate a missing dir
            // instead of erroring, and only bail if *neither* sessions nor
            // loop iteration records exist (nothing to grade at all).
            let src_sessions = self.fixture.join("data").join("sessions");
            let mut session_files = Vec::new();
            if let Ok(entries) = std::fs::read_dir(&src_sessions) {
                for e in entries.flatten() {
                    let p = e.path();
                    if p.extension().map(|x| x == "json").unwrap_or(false) {
                        let dst = data.join("sessions").join(p.file_name().unwrap());
                        std::fs::copy(&p, &dst).with_context(|| format!("copy {}", p.display()))?;
                        session_files.push(dst);
                    }
                }
            }
            session_files.sort_by_key(|p| {
                std::fs::metadata(p)
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
            });

            // Unlike turn logs (read in place from the fixture, below), loop
            // iteration records must be copied: `transcript.rs`'s
            // `loop_transcript` reads them from `artifacts.data_dir`, which
            // here is the fresh run_dir's `data/`, not the fixture's.
            let src_loops = self.fixture.join("data").join("loops");
            if src_loops.is_dir() {
                copy_dir_recursive(&src_loops, &data.join("loops"))?;
            }

            if session_files.is_empty() && !src_loops.is_dir() {
                bail!(
                    "no session file or loop iteration records found under {}",
                    self.fixture.display()
                );
            }
            // Turn logs are replayed in place from the fixture, not copied —
            // they're read-only evidence, and the fixture dir outlives this
            // run_dir.
            let turns = discover_turn_artifacts(&self.fixture);
            return Ok(RunArtifacts {
                session_files,
                turns,
                data_dir: data,
                config_dir: config,
                work_dir: work,
                wall_secs: 0.0,
            });
        }

        let dst = data.join("sessions").join("mock.json");
        std::fs::copy(&self.fixture, &dst)
            .with_context(|| format!("copy mock fixture {}", self.fixture.display()))?;
        Ok(RunArtifacts {
            session_files: vec![dst],
            turns: Vec::new(),
            data_dir: data,
            config_dir: config,
            work_dir: work,
            wall_secs: 0.0,
        })
    }
}

#[cfg(test)]
mod mock_trial_dir_tests {
    use super::*;

    #[test]
    fn mock_replays_a_captured_trial_dir_including_stdout_tool_calls() {
        // Production evidence for tool calls in headless mode is always
        // `--pure-stdout` markers, never the session JSON (see
        // transcript.rs's module doc) — a mock fixture that only writes
        // tool_call messages into session JSON never exercises that real
        // path. Pointing Mock at a directory (a previously captured trial
        // dir) replays both channels exactly as a real run produced them.
        let fixture_dir =
            std::env::temp_dir().join(format!("zseval-mockdir-fixture-{}", std::process::id()));
        std::fs::create_dir_all(fixture_dir.join("data/sessions")).unwrap();
        std::fs::write(
            fixture_dir.join("data/sessions/s.json"),
            r#"{"id":"s","messages":[{"role":"user","content":"hi"},{"role":"assistant","content":"done"}],"total_input_tokens":1,"total_output_tokens":1,"total_cost":0.001}"#,
        )
        .unwrap();
        std::fs::write(
            fixture_dir.join("turn-0.stdout"),
            "◈ bash ls\n◈ bash result:\nfile.txt\n",
        )
        .unwrap();

        let run_dir =
            std::env::temp_dir().join(format!("zseval-mockdir-rundir-{}", std::process::id()));
        std::fs::create_dir_all(&run_dir).unwrap();

        let backend = Mock {
            fixture: fixture_dir.clone(),
        };
        let sc_dir = std::env::temp_dir().join(format!("zseval-mockdir-sc-{}", std::process::id()));
        std::fs::create_dir_all(&sc_dir).unwrap();
        std::fs::write(
            sc_dir.join("scenario.toml"),
            "id = \"x\"\ntask = \"hi\"\nexpect = [\"final_contains x\"]\n",
        )
        .unwrap();
        let sc = crate::scenario::Scenario::load(&sc_dir).unwrap();

        let artifacts = backend.run(&sc, &run_dir).unwrap();
        assert_eq!(artifacts.turns.len(), 1);
        assert_eq!(artifacts.turns[0].stdout, fixture_dir.join("turn-0.stdout"));

        let t = crate::transcript::Transcript::from_run(&artifacts).unwrap();
        assert_eq!(t.tool_calls.len(), 1, "{:?}", t.tool_calls);
        assert_eq!(t.tool_calls[0].name, "bash");
        assert_eq!(t.final_assistant, "done");

        std::fs::remove_dir_all(&fixture_dir).ok();
        std::fs::remove_dir_all(&run_dir).ok();
        std::fs::remove_dir_all(&sc_dir).ok();
    }
}

#[cfg(test)]
mod turn_artifacts_tests {
    use super::*;

    #[test]
    fn discover_turn_artifacts_sorts_numerically_not_lexicographically() {
        let dir = std::env::temp_dir().join(format!("zseval-turns-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A lexicographic sort would put "turn-10" before "turn-2". Every
        // extension is constructed for every discovered index regardless of
        // which files actually exist on disk (turn-0.zslog here is enough to
        // register index 0; turn-0.stdout/.stderr still get paths built).
        for name in ["turn-2.stdout", "turn-10.stdout", "turn-0.zslog"] {
            std::fs::write(dir.join(name), "x").unwrap();
        }
        let turns = discover_turn_artifacts(&dir);
        let indices: Vec<usize> = turns
            .iter()
            .map(|t| {
                t.stdout
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .and_then(|s| s.strip_prefix("turn-"))
                    .and_then(|s| s.parse().ok())
                    .unwrap()
            })
            .collect();
        assert_eq!(indices, vec![0, 2, 10], "{turns:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn discover_turn_artifacts_on_missing_dir_is_empty() {
        assert!(discover_turn_artifacts(Path::new("/no/such/run-dir")).is_empty());
    }
}
