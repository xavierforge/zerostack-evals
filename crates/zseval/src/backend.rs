//! Agent backends — generic driving only, zero domain knowledge.
//!
//! `ZsCli` drives the real `zerostack` binary over its public surface
//! (verified against zerostack source 2026-07-06):
//!   - `-p/--print` headless single-shot, `-c/--continue` to resume,
//!     `--yolo` (skip permission prompts), `--no-color`, positional message.
//!   - `--pure-stdout` ("With -p: also print tool calls/results to stdout")
//!     is kept for the `◈ {name} {summary}` marker lines it tees into
//!     `turn-N.stdout` below — a human-facing debugging artifact, not the
//!     evidence channel. Tool-call evidence is the session JSON's structured
//!     `tool` records instead (zerostack PR #230; see `transcript.rs`'s
//!     module doc), which headless `-p` runs persist unconditionally.
//!   - `--log-file <path>` activates zerostack's trace-level file logging
//!     (`src/logging.rs`); stderr only shows `warn+` by default, which is
//!     why a hanging API retry looks silent — the real story is in the
//!     trace log, so we capture one per turn (`turn-N.zslog`).
//!   - Isolation via `ZS_DATA_DIR` / `ZS_CONFIG_DIR` env overrides
//!     (`src/session/storage.rs`), quorum-style throwaway home per run, plus
//!     `GIT_CEILING_DIRECTORIES` so a git the agent runs can never walk up out
//!     of the trial and find the harness's own checkout (see `git_ceiling`).
//!
//! Environment seeding is delegated entirely to `crate::seed` (generic file
//! placements). This file stays subsystem-agnostic — it only knows how to
//! drive the binary and collect the session files it writes.
//!
//! `Mock` replays a canned session file, so the harness itself is testable
//! in CI without a zerostack build or an API key.

use std::ffi::OsString;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use crate::prompts::PromptPack;
use crate::scenario::{LoopCfg, Scenario, Turn};
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

/// Everything one `zerostack` invocation needs beyond its `Command`: the
/// binary being driven (named in the spawn and timeout errors), the flag that
/// reproduces this invocation shape by hand, the trial-wide `timeout` measured
/// from `started`, and the turn's index plus the three logs its output is
/// tee'd to. Bundled because `run_print` and `run_loop` each hold the whole
/// group already — the only two callers, and the two shapes whose
/// timeout/heartbeat/diagnostic behavior must not drift apart.
struct TurnRun<'a> {
    bin: &'a Path,
    /// Cosmetic only: substituted into the timeout error's "reproduce
    /// manually" hint (`-p` vs `--loop`).
    repro_flag: &'a str,
    /// When the whole trial started — the timeout is measured from here, not
    /// from this invocation's spawn.
    started: Instant,
    timeout: Duration,
    turn_label: usize,
    logs: &'a TurnArtifacts,
}

/// Spawn `cmd`, tee its stdout/stderr to `run.logs` (and the console under
/// `--verbose`), and wait — enforcing `run.timeout`, with the same
/// kill-and-diagnose-on-expiry and non-zero-exit handling either way. Shared
/// by the per-turn `-p`/`--continue` path and the single-invocation `--loop`
/// path.
fn spawn_and_wait(mut cmd: Command, run: &TurnRun) -> Result<()> {
    let TurnRun {
        bin,
        repro_flag,
        started,
        timeout,
        turn_label,
        logs,
    } = *run;
    let (stdout_log, stderr_log, zs_log) = (&logs.stdout, &logs.stderr, &logs.zslog);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .with_context(|| format!("spawn {}", bin.display()))?;
    let t_out = tee(
        child.stdout.take().expect("piped"),
        stdout_log.clone(),
        "out",
        turn_label,
    );
    let t_err = tee(
        child.stderr.take().expect("piped"),
        stderr_log.clone(),
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
                if started.elapsed() > timeout {
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
                if started.elapsed() > next_heartbeat && !verbose() {
                    eprintln!(
                        "     ... turn {turn_label} still running ({}s elapsed; --verbose \
                         streams live output; trace: {})",
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
    /// The identity of what produced this run's evidence, captured once at run
    /// start (never per trial), so `run_suite` can record on every report
    /// which zerostack produced it — or refuse to run.
    ///
    /// `ZsCli` runs `<bin> --version` (recording the first line verbatim, no
    /// format validation) and SHA-256s the binary. Any failure — unrunnable,
    /// non-zero exit, empty output — is an error naming the binary, which
    /// aborts the run before any API spend: this feature exists to make
    /// identity-less reports impossible, and a null fallback would reintroduce
    /// them. A banner that also announces its build's features has them read
    /// off it (`parse_features`); one that doesn't reports `None`, which is
    /// "unknown", not "none enabled".
    ///
    /// `Mock` returns the fixture's identity (`"mock"`, the fixture path, and a
    /// content fingerprint of the fixture). It never aborts on an unreadable
    /// fixture: mock spends nothing, so the abort-before-spend rationale does
    /// not apply, and a missing fixture already surfaces as a fully-ungradable
    /// run via `run`. A `--zs-bin` passed alongside `--backend mock=` is not
    /// consulted — identity records the evidence *source*, and mock's evidence
    /// is the fixture, not an unused binary.
    fn identity(&self) -> Result<crate::verdict::ZsIdentity>;
    /// The prompt pack this backend seeds into every trial, if any. The
    /// backend is the authority on which pack was actually used (it is what
    /// seeds it), so `run_suite` reads the pack's identity for the report from
    /// here rather than threading it separately through `RunOptions`. Default
    /// `None`: only `ZsCli` seeds a pack, and `--prompts` is rejected for
    /// `mock`.
    fn prompt_pack(&self) -> Option<&PromptPack> {
        None
    }
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

/// The marker a `--version` banner uses to announce its build's feature set.
/// Matched case-insensitively and anywhere on a line, so both a dedicated
/// `features: a, b` line and a suffixed `zerostack 1.7.2 (features: a b)` are
/// read the same way.
const FEATURES_MARKER: &str = "features:";

/// The enabled feature set a `--version` banner reports, or `None` when the
/// banner says nothing about features — which today's binary is the case for.
/// The distinction matters: `None` is *no information*, and the preflight gate
/// warns that the build cannot be verified rather than claiming a feature is
/// missing. A marker followed by nothing usable is treated the same way, since
/// an empty list read as fact would condemn every build.
///
/// Names are lowercased and split on commas, semicolons or whitespace, with
/// bracketing punctuation trimmed: the banner is a human string, so this reads
/// it leniently instead of turning its exact shape into a contract.
fn parse_features(stdout: &str) -> Option<Vec<String>> {
    let listed = stdout.lines().find_map(|line| {
        let at = line.to_ascii_lowercase().find(FEATURES_MARKER)?;
        Some(&line[at + FEATURES_MARKER.len()..])
    })?;
    let names: Vec<String> = listed
        .split(|c: char| c == ',' || c == ';' || c.is_whitespace())
        .map(|s| s.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_'))
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase())
        .collect();
    (!names.is_empty()).then_some(names)
}

impl AgentBackend for ZsCli {
    fn name(&self) -> &str {
        "zs-cli"
    }

    fn prompt_pack(&self) -> Option<&PromptPack> {
        self.prompts.as_deref()
    }

    fn identity(&self) -> Result<crate::verdict::ZsIdentity> {
        // `--version` is a plain one-shot: no isolation, no timeout machinery,
        // no session — just the version banner. stdin is closed so a binary
        // that (wrongly) waits on input fails fast rather than hanging.
        let out = Command::new(&self.bin)
            .arg("--version")
            .stdin(Stdio::null())
            .output()
            .with_context(|| {
                format!(
                    "capture zerostack identity: could not run '{} --version' \
                     (is ZS_BIN / --zs-bin a runnable zerostack binary?)",
                    self.bin.display()
                )
            })?;
        if !out.status.success() {
            bail!(
                "capture zerostack identity: '{} --version' exited with {} (expected 0). \
                 Refusing to run: a report must name the zerostack that produced it.",
                self.bin.display(),
                out.status
            );
        }
        // First line, verbatim — no parsing. The version string is human
        // evidence; `zs_bin_sha256` is the machine-comparable identity, so
        // upstream's banner shape never becomes a compatibility contract.
        let stdout = String::from_utf8_lossy(&out.stdout);
        let version = stdout.lines().next().unwrap_or("").to_string();
        if version.trim().is_empty() {
            bail!(
                "capture zerostack identity: '{} --version' produced no output. \
                 Refusing to run: a report must name the zerostack that produced it.",
                self.bin.display()
            );
        }
        let bytes = std::fs::read(&self.bin).with_context(|| {
            format!(
                "capture zerostack identity: could not read '{}' to hash it: ZS_BIN / \
                 --zs-bin must be a path to the binary file (absolute, or relative with a \
                 directory such as ./zerostack), not a bare command name resolved via $PATH \
                 — the run hashes the file to record zs_bin_sha256",
                self.bin.display()
            )
        })?;
        Ok(crate::verdict::ZsIdentity {
            zs_version: version,
            zs_bin_path: crate::verdict::record_path(&self.bin),
            zs_bin_sha256: crate::util::sha256_hex(&bytes),
            git_sha: None,
            features: parse_features(&stdout),
        })
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
        // that seeds a same-named `work:.zerostack/prompts/*.md` overrides it
        // — a scenario's own prompt seed wins over the pack, rather than the
        // pack silently winning last.
        if let Some(pack) = &self.prompts {
            let prompts_dir = work.join(".zerostack").join("prompts");
            std::fs::create_dir_all(&prompts_dir)?;
            for file in pack.files() {
                std::fs::write(prompts_dir.join(&file.file_name), &file.bytes)
                    .with_context(|| format!("seed prompt pack file {}", file.file_name))?;
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

impl RunDirs<'_> {
    /// Point one invocation at this trial: cwd, the four isolation env vars,
    /// and the git ceiling. Shared by both spawn shapes because every line of
    /// it is about *where* the child can reach, and that must not differ
    /// between `-p` and `--loop` — a hole in either shape is a hole in the
    /// harness.
    fn confine(&self, cmd: &mut Command) {
        cmd.current_dir(self.work)
            .env("ZS_DATA_DIR", self.data)
            .env("ZS_CONFIG_DIR", self.config)
            .env("TMPDIR", self.tmp)
            .env("HOME", self.home)
            .env("GIT_CEILING_DIRECTORIES", git_ceiling(self.run_dir));
    }
}

/// The `GIT_CEILING_DIRECTORIES` value confining every git the agent runs:
/// the trial's own run dir.
///
/// git finds a repository by walking up from the working directory until it
/// hits a `.git`, and it does not chdir up *into* a ceiling entry. Naming the
/// run dir therefore stops the walk at `work/`'s parent: a repo the scenario
/// seeded anywhere inside `work/` is still found, while the harness's own
/// checkout above it — which an agent running `git commit` under `--yolo`
/// otherwise discovers and commits into, as one has — is unreachable.
///
/// Canonicalized because git resolves the entries and compares them against
/// the resolved working directory: on macOS a run dir under `/tmp` is really
/// `/private/tmp`, and an unresolved entry would simply never match, leaving
/// the barrier silently absent. A path that cannot be canonicalized (it should
/// exist — `ZsCli::run` created it) falls back to the raw one rather than
/// dropping the variable: a ceiling that fails to match is exactly as
/// permissive as no ceiling at all, so there is nothing to lose by setting it.
fn git_ceiling(run_dir: &Path) -> PathBuf {
    std::fs::canonicalize(run_dir).unwrap_or_else(|_| run_dir.to_path_buf())
}

// ---------------------------------------------------------------------------
// Launch argument assembly
// ---------------------------------------------------------------------------
//
// The two invocation shapes assemble their argument vectors here rather than
// inline at their spawn sites, so what a scenario launches with is a value a
// test can read: nothing below spawns, opens a file, or touches the
// filesystem. `Vec<OsString>` rather than `Vec<String>` because `--log-file`
// carries a filesystem path, which the child is handed verbatim.

/// Every argument of one `-p` turn's invocation, in spawn order: the
/// harness-owned flags, the scenario's prompt, then the turn message as the
/// single positional. Whether this turn resumes the running session is a
/// function of where it sits in the scenario, so `turn_index` comes in and
/// the decision stays here with the rest of the vector.
pub fn print_turn_args(
    sc: &Scenario,
    turn: &Turn,
    turn_index: usize,
    zslog: &Path,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![
        "-p".into(),
        "--yolo".into(),
        "--no-color".into(),
        "--pure-stdout".into(),
        "--log-file".into(),
        zslog.into(),
    ];
    if let Some(name) = &sc.prompt {
        args.push("--load-prompt".into());
        args.push(name.into());
    }
    // Continue the running session unless this is the first turn or the
    // scenario explicitly cuts to a new session.
    if turn_index > 0 && !turn.new_session() {
        args.push("--continue".into());
    }
    args.push(turn.msg().into());
    args
}

/// Every argument of a loop scenario's single `--loop` invocation, in spawn
/// order. No `--pure-stdout`: a loop run has no per-turn stdout worth teeing
/// tool markers into, its evidence is the iteration records.
pub fn loop_args(sc: &Scenario, loop_cfg: &LoopCfg, turn: &Turn, zslog: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![
        "--loop".into(),
        "--yolo".into(),
        "--no-color".into(),
        "--log-file".into(),
        zslog.into(),
        "--loop-max".into(),
        loop_cfg.max_iterations.to_string().into(),
    ];
    if let Some(run_cmd) = &loop_cfg.run {
        args.push("--loop-run".into());
        args.push(run_cmd.into());
    }
    if let Some(name) = &sc.prompt {
        args.push("--load-prompt".into());
        args.push(name.into());
    }
    args.push(turn.msg().into());
    args
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

            let logs = TurnArtifacts {
                stdout: d.run_dir.join(format!("turn-{i}.stdout")),
                stderr: d.run_dir.join(format!("turn-{i}.stderr")),
                // zerostack's own trace-level log: this is where API retries,
                // tool dispatch, and provider errors actually show up.
                zslog: d.run_dir.join(format!("turn-{i}.zslog")),
            };

            let mut cmd = Command::new(&self.bin);
            cmd.args(print_turn_args(sc, turn, i, &logs.zslog));
            d.confine(&mut cmd);

            spawn_and_wait(
                cmd,
                &TurnRun {
                    bin: &self.bin,
                    repro_flag: "-p",
                    started,
                    timeout,
                    turn_label: i,
                    logs: &logs,
                },
            )?;
            if verbose() {
                eprintln!(
                    "  <- turn {i} ok ({:.1}s)",
                    turn_started.elapsed().as_secs_f64()
                );
            }
            turn_logs.push(logs);
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
    fn run_loop(&self, sc: &Scenario, loop_cfg: &LoopCfg, d: &RunDirs) -> Result<RunArtifacts> {
        let started = Instant::now();
        let timeout = Duration::from_secs(sc.timeout_secs);
        let turn = &sc.task.turns()[0];

        let logs = TurnArtifacts {
            stdout: d.run_dir.join("turn-0.stdout"),
            stderr: d.run_dir.join("turn-0.stderr"),
            zslog: d.run_dir.join("turn-0.zslog"),
        };

        let mut cmd = Command::new(&self.bin);
        cmd.args(loop_args(sc, loop_cfg, turn, &logs.zslog));
        d.confine(&mut cmd);

        spawn_and_wait(
            cmd,
            &TurnRun {
                bin: &self.bin,
                repro_flag: "--loop",
                started,
                timeout,
                turn_label: 0,
                logs: &logs,
            },
        )?;

        Ok(RunArtifacts {
            session_files: Vec::new(),
            turns: vec![logs],
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

/// A content fingerprint of a mock fixture — the machine-comparable half of
/// its identity (`ZsIdentity::zs_bin_sha256`). A single-file fixture is the
/// SHA-256 of its bytes. A directory fixture (a captured trial dir) folds every
/// file, sorted by forward-slashed relative path, each entry length-prefixed
/// (`len(rel) || rel || len(bytes) || bytes`, 8-byte little-endian lengths)
/// before hashing.
///
/// Only relative paths and contents feed the hash, never the fixture's own
/// location, so the same contents at two paths fingerprint identically (a mock
/// run is reproducible wherever the fixture sits). Length-prefixing — rather
/// than `PromptPack::fingerprint`'s `name\0bytes\0` separators — is deliberate:
/// NUL separators collide once a name or content can contain the separator, a
/// registered flaw the new code must not copy.
fn fixture_fingerprint(fixture: &Path) -> Result<String> {
    if fixture.is_dir() {
        let mut files: Vec<(String, Vec<u8>)> = Vec::new();
        collect_files(fixture, fixture, &mut files)?;
        files.sort_by(|a, b| a.0.cmp(&b.0));
        let mut buf: Vec<u8> = Vec::new();
        for (rel, bytes) in files {
            buf.extend_from_slice(&(rel.len() as u64).to_le_bytes());
            buf.extend_from_slice(rel.as_bytes());
            buf.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
            buf.extend_from_slice(&bytes);
        }
        Ok(crate::util::sha256_hex(&buf))
    } else {
        let bytes = std::fs::read(fixture)
            .with_context(|| format!("hash mock fixture {}", fixture.display()))?;
        Ok(crate::util::sha256_hex(&bytes))
    }
}

/// Recursively collect every file under `dir` as `(relative-to-root path,
/// bytes)`, the relative path forward-slashed so it does not vary by platform.
fn collect_files(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) -> Result<()> {
    for e in std::fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let e = e?;
        let path = e.path();
        if path.is_dir() {
            collect_files(root, &path, out)?;
        } else {
            let rel = path.strip_prefix(root).unwrap_or(&path);
            let rel = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            out.push((rel, std::fs::read(&path)?));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Mock
// ---------------------------------------------------------------------------

/// Replays canned artifacts; used by the harness's own tests and by
/// `--backend mock=<path>` for local plumbing checks. `fixture` is either a
/// single session JSON file (legacy shape: no turn logs, so the mock never
/// replays a `turn-N.stdout` at all) or a directory — a previously captured
/// trial dir (`data/sessions/*.json` plus `turn-N.{stdout,stderr,zslog}`) —
/// replayed exactly as the real backend produced it, turn logs included:
/// they're decorative, since tool-call evidence comes from the session
/// JSON's `tool` records (see `transcript.rs`'s module doc). The directory
/// form is what backs `zseval regrade` and lets a test exercise the real
/// evidence path without a live zerostack build.
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

    fn identity(&self) -> Result<crate::verdict::ZsIdentity> {
        // Never aborts: mock spends nothing, so the "fail before any API spend"
        // rationale does not apply, and an unreadable fixture already surfaces
        // as a fully-ungradable run in `run`. An unreadable fixture leaves the
        // content fingerprint empty rather than failing the run.
        Ok(crate::verdict::ZsIdentity {
            zs_version: self.name().to_string(),
            zs_bin_path: crate::verdict::record_path(&self.fixture),
            zs_bin_sha256: fixture_fingerprint(&self.fixture).unwrap_or_default(),
            git_sha: None,
            features: None,
        })
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
    fn mock_replays_a_captured_trial_dir_including_its_turn_logs() {
        // Pointing Mock at a directory (a previously captured trial dir)
        // replays the session files *and* republishes the turn logs, so a
        // replayed trial's artifacts look like the run that produced them.
        // The extra `◈ write` marker in the stdout log is the check that
        // those logs stay decorative: tool-call evidence comes from the
        // session's `tool` records only (see transcript.rs's module doc).
        let fixture_dir =
            std::env::temp_dir().join(format!("zseval-mockdir-fixture-{}", std::process::id()));
        std::fs::create_dir_all(fixture_dir.join("data/sessions")).unwrap();
        std::fs::write(
            fixture_dir.join("data/sessions/s.json"),
            r#"{"id":"s","messages":[
                {"role":"user","content":"hi"},
                {"role":"tool_call","content":"bash ls","tool":{"id":0,"name":"bash","args":{"command":"ls"}}},
                {"role":"tool_result","content":"bash:\nfile.txt","tool":{"call_id":0,"name":"bash","truncated":false,"full_output_path":null}},
                {"role":"assistant","content":"done"}
            ],"total_input_tokens":1,"total_output_tokens":1,"total_cost":0.001}"#,
        )
        .unwrap();
        std::fs::write(
            fixture_dir.join("turn-0.stdout"),
            "◈ bash ls\n◈ bash result:\nfile.txt\n◈ write not-evidence\n",
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
            "id = \"x\"\nkind = \"regression\"\ntask = \"hi\"\nexpect = [\"final_contains x\"]\n",
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

/// The git barrier: an agent under `--yolo` runs whatever git it likes, and
/// git's upward repository discovery is what turned a `git commit` inside a
/// trial into a commit in the harness's own checkout.
#[cfg(test)]
mod git_ceiling_tests {
    use super::*;

    fn dirs(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("zseval-ceiling-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&d).ok();
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn git(cwd: &Path, args: &[&str]) -> std::process::Output {
        Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .expect("git must be installed to exercise the discovery barrier")
    }

    /// Both spawn shapes go through `confine`, so the barrier is checked once
    /// on the shared helper: cwd is the work dir, and the ceiling is the
    /// trial's run dir with every symlink resolved (the form git compares
    /// against).
    #[test]
    fn confine_sets_the_ceiling_to_the_canonical_run_dir() {
        let run_dir = dirs("confine");
        let work = run_dir.join("work");
        std::fs::create_dir_all(&work).unwrap();
        let d = RunDirs {
            run_dir: &run_dir,
            data: &run_dir.join("data"),
            config: &run_dir.join("config"),
            work: &work,
            tmp: &run_dir.join("tmp"),
            home: &run_dir.join("home"),
        };

        let mut cmd = Command::new("/bin/true");
        d.confine(&mut cmd);

        let ceiling = cmd
            .get_envs()
            .find(|(k, _)| *k == "GIT_CEILING_DIRECTORIES")
            .and_then(|(_, v)| v)
            .expect("every invocation carries the ceiling");
        assert_eq!(Path::new(ceiling), std::fs::canonicalize(&run_dir).unwrap());
        assert_eq!(cmd.get_current_dir(), Some(work.as_path()));
        std::fs::remove_dir_all(&run_dir).ok();
    }

    /// End to end through `ZsCli::run`, with the trial placed *inside* a git
    /// repository — the shape the harness itself runs in. A repo seeded inside
    /// the trial stays discoverable; the one above the trial does not.
    #[test]
    fn git_inside_a_trial_cannot_discover_the_repo_above_it() {
        use std::os::unix::fs::PermissionsExt;

        let base = dirs("end-to-end");
        let outer = base.join("harness-repo");
        std::fs::create_dir_all(&outer).unwrap();
        assert!(git(&outer, &["init", "-q"]).status.success());

        // The trial dir sits under that repo, exactly as `results/<tag>/...`
        // sits under the harness checkout.
        let run_dir = outer.join("results/tag/sc/trial-0");
        std::fs::create_dir_all(run_dir.join("work/inner-repo")).unwrap();
        assert!(git(&run_dir.join("work/inner-repo"), &["init", "-q"])
            .status
            .success());

        let bin = base.join("fake-zs");
        std::fs::write(
            &bin,
            "#!/usr/bin/env bash\n\
             set -uo pipefail\n\
             if [ \"${1:-}\" = \"--version\" ]; then echo 'zerostack 0.0.0-stub'; exit 0; fi\n\
             printf '%s' \"${GIT_CEILING_DIRECTORIES:-<unset>}\" > \"$ZS_DATA_DIR/ceiling\"\n\
             git rev-parse --show-toplevel > \"$ZS_DATA_DIR/above\" 2>&1\n\
             (cd inner-repo && git rev-parse --show-toplevel) > \"$ZS_DATA_DIR/inside\" 2>&1\n\
             mkdir -p \"$ZS_DATA_DIR/sessions\"\n\
             printf '{\"id\":\"s\",\"messages\":[{\"role\":\"assistant\",\"content\":\"done\"}]}' \
             > \"$ZS_DATA_DIR/sessions/s.json\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();

        let sc_dir = base.join("scenario");
        std::fs::create_dir_all(&sc_dir).unwrap();
        std::fs::write(
            sc_dir.join("scenario.toml"),
            "id = \"ceiling\"\nkind = \"regression\"\ntask = \"go\"\n\
             expect = [\"final_contains done\"]\n",
        )
        .unwrap();
        let sc = crate::scenario::Scenario::load(&sc_dir).unwrap();

        ZsCli {
            bin,
            target: None,
            prompts: None,
        }
        .run(&sc, &run_dir)
        .unwrap();

        let canonical_run_dir = std::fs::canonicalize(&run_dir).unwrap();
        let read = |name: &str| std::fs::read_to_string(run_dir.join("data").join(name)).unwrap();
        assert_eq!(Path::new(&read("ceiling")), canonical_run_dir);

        let above = read("above");
        assert!(
            !above.contains(&std::fs::canonicalize(&outer).unwrap().display().to_string()),
            "git walked out of the trial and found the repo above it: {above}"
        );
        assert_eq!(
            read("inside").trim(),
            std::fs::canonicalize(run_dir.join("work/inner-repo"))
                .unwrap()
                .display()
                .to_string(),
            "a repo seeded inside the trial must still be discoverable"
        );

        std::fs::remove_dir_all(&base).ok();
    }
}

#[cfg(test)]
mod identity_tests {
    use super::*;

    /// Write an executable `--version` stub with the given shell body, in a
    /// fresh per-test dir, and return its path.
    fn stub(name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let dir =
            std::env::temp_dir().join(format!("zseval-zsident-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("zerostack-stub");
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn zs(bin: PathBuf) -> ZsCli {
        ZsCli {
            bin,
            target: None,
            prompts: None,
        }
    }

    /// The first line of `--version` stdout is recorded verbatim — no format
    /// validation (a non-standard string is kept as-is), extra lines tolerated
    /// and ignored. The binary is also hashed (64 hex chars); `git_sha` is
    /// `None` (nothing embeds one) and so are `features` for a banner that
    /// announces none, which is today's binary.
    #[test]
    fn version_first_line_is_captured_verbatim() {
        let bin = stub(
            "verbatim",
            "#!/bin/sh\necho 'zerostack 9.9.9-rc1 (custom build!)'\necho 'ignored second line'\n",
        );
        let id = zs(bin.clone()).identity().unwrap();
        assert_eq!(id.zs_version, "zerostack 9.9.9-rc1 (custom build!)");
        assert_eq!(id.zs_bin_sha256.len(), 64, "{}", id.zs_bin_sha256);
        assert!(id.git_sha.is_none());
        assert!(id.features.is_none());
        std::fs::remove_dir_all(bin.parent().unwrap()).ok();
    }

    /// A banner that announces its features has them captured, off any line
    /// and in any of the shapes `parse_features` tolerates — the first line
    /// stays the verbatim version either way.
    #[test]
    fn a_banner_that_announces_features_has_them_captured() {
        let bin = stub(
            "features-line",
            "#!/bin/sh\necho 'zerostack 1.7.2'\necho 'features: memory, mcp, subagents, loop'\n",
        );
        let id = zs(bin.clone()).identity().unwrap();
        assert_eq!(id.zs_version, "zerostack 1.7.2");
        assert_eq!(
            id.features.as_deref(),
            Some(
                ["memory", "mcp", "subagents", "loop"]
                    .map(String::from)
                    .as_slice()
            )
        );
        std::fs::remove_dir_all(bin.parent().unwrap()).ok();

        let bin = stub(
            "features-suffix",
            "#!/bin/sh\necho 'zerostack 1.7.2 (Features: MCP loop)'\n",
        );
        let id = zs(bin.clone()).identity().unwrap();
        assert_eq!(id.zs_version, "zerostack 1.7.2 (Features: MCP loop)");
        assert_eq!(
            id.features.as_deref(),
            Some(["mcp", "loop"].map(String::from).as_slice())
        );
        std::fs::remove_dir_all(bin.parent().unwrap()).ok();
    }

    /// A marker with nothing usable after it is no information, not an empty
    /// feature set: reported as `None` so the gate warns rather than
    /// condemning the build.
    #[test]
    fn an_empty_feature_list_is_reported_as_no_information() {
        let bin = stub(
            "features-empty",
            "#!/bin/sh\necho 'zerostack 1.7.2'\necho 'features:'\n",
        );
        let id = zs(bin.clone()).identity().unwrap();
        assert!(id.features.is_none(), "{:?}", id.features);
        std::fs::remove_dir_all(bin.parent().unwrap()).ok();
    }

    /// A non-zero exit aborts, even when the binary printed a version line —
    /// exit 0 is required. The error names the binary.
    #[test]
    fn a_nonzero_exit_aborts_naming_the_binary() {
        let bin = stub("nonzero", "#!/bin/sh\necho 'zerostack 1.0.0'\nexit 3\n");
        let err = zs(bin.clone()).identity().unwrap_err();
        assert!(
            format!("{err:#}").contains(&bin.display().to_string()),
            "{err:#}"
        );
        std::fs::remove_dir_all(bin.parent().unwrap()).ok();
    }

    /// Empty stdout (exit 0 but nothing printed) aborts, naming the binary —
    /// there is no version to record.
    #[test]
    fn empty_output_aborts_naming_the_binary() {
        let bin = stub("empty", "#!/bin/sh\nexit 0\n");
        let err = zs(bin.clone()).identity().unwrap_err();
        assert!(
            format!("{err:#}").contains(&bin.display().to_string()),
            "{err:#}"
        );
        std::fs::remove_dir_all(bin.parent().unwrap()).ok();
    }

    /// An unrunnable binary (path does not exist) aborts, naming it.
    #[test]
    fn a_missing_binary_aborts_naming_it() {
        let bin = std::env::temp_dir().join(format!(
            "zseval-zsident-missing-{}/does-not-exist",
            std::process::id()
        ));
        let err = zs(bin.clone()).identity().unwrap_err();
        assert!(
            format!("{err:#}").contains(&bin.display().to_string()),
            "{err:#}"
        );
    }

    fn write(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    /// A single-file mock fixture records `"mock"` and a content fingerprint
    /// of the file's bytes — identical bytes at two different paths hash the
    /// same (the path is not the identity), different bytes differ.
    #[test]
    fn mock_single_file_fixture_hashes_bytes_path_independently() {
        let root = std::env::temp_dir().join(format!("zseval-mockid-file-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let a = root.join("a/mock.json");
        let b = root.join("b/mock.json");
        let c = root.join("c/mock.json");
        write(&a, b"{\"id\":\"s\"}");
        write(&b, b"{\"id\":\"s\"}");
        write(&c, b"{\"id\":\"different\"}");

        let id_a = Mock { fixture: a }.identity().unwrap();
        let id_b = Mock { fixture: b }.identity().unwrap();
        let id_c = Mock { fixture: c }.identity().unwrap();

        assert_eq!(id_a.zs_version, "mock");
        assert!(id_a.git_sha.is_none() && id_a.features.is_none());
        assert_eq!(
            id_a.zs_bin_sha256, id_b.zs_bin_sha256,
            "same bytes at a different path hash the same"
        );
        assert_ne!(id_a.zs_bin_sha256, id_c.zs_bin_sha256);
        assert_eq!(id_a.zs_bin_sha256.len(), 64);
        std::fs::remove_dir_all(&root).ok();
    }

    /// A directory mock fixture folds its files (sorted by relative path,
    /// length-prefixed) — identical contents at two different roots hash the
    /// same, and changing one byte changes the hash. Length-prefixing is what
    /// keeps this off `PromptPack::fingerprint`'s NUL-separated flaw.
    #[test]
    fn mock_directory_fixture_hashes_contents_path_independently() {
        let base = std::env::temp_dir().join(format!("zseval-mockid-dir-{}", std::process::id()));
        std::fs::remove_dir_all(&base).ok();
        let a = base.join("root-a");
        let b = base.join("root-b");
        for root in [&a, &b] {
            write(&root.join("data/sessions/s.json"), b"{\"id\":\"s\"}");
            write(&root.join("turn-0.stdout"), b"hello world\n");
        }
        let id_a = Mock { fixture: a.clone() }.identity().unwrap();
        let id_b = Mock { fixture: b.clone() }.identity().unwrap();
        assert_eq!(id_a.zs_version, "mock");
        assert_eq!(
            id_a.zs_bin_sha256, id_b.zs_bin_sha256,
            "identical relative contents at two roots hash the same"
        );

        // Flip one byte in root-b: the hash must move.
        write(&b.join("turn-0.stdout"), b"HELLO world\n");
        let id_b2 = Mock { fixture: b }.identity().unwrap();
        assert_ne!(id_a.zs_bin_sha256, id_b2.zs_bin_sha256);
        std::fs::remove_dir_all(&base).ok();
    }
}
