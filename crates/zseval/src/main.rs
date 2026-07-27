//! zseval — eval harness CLI for zerostack agents.
//!
//! Every subcommand supports `--json`; exit codes are the machine contract:
//!   0 = pass / no regression
//!   1 = fail / regression
//!   2 = usage or harness error

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use zseval::backend::{AgentBackend, Mock, ZsCli};
use zseval::compare::{compare, load_report, print_human};
use zseval::runner::{run_suite, RunOptions};
use zseval::scenario::{discover, Scenario};
use zseval::verdict::Report;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (cmd, rest) = match args.split_first() {
        Some((c, r)) => (c.as_str(), r.to_vec()),
        None => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    let result = match cmd {
        "run" => cmd_run(rest),
        "compare" => cmd_compare(rest),
        "explain" => cmd_explain(rest),
        "list" => cmd_list(rest),
        "regrade" => cmd_regrade(rest),
        "matrix" => cmd_matrix(rest),
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            Ok(ExitCode::SUCCESS)
        }
        other => {
            eprintln!("unknown command '{other}'\n{USAGE}");
            Ok(ExitCode::from(2))
        }
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(2)
        }
    }
}

const USAGE: &str = "\
zseval — eval harness for zerostack agents

USAGE:
  zseval run <scenario-path> [--target config.toml]... [--trials N]
             [--tag T] [--zs-bin PATH] [--backend zs|mock=<session.json>]
             [--judge judges/opus.toml] [--no-judge] [--max-total-usd X]
             [--prompts DIR] [--results DIR] [--jobs N] [--json] [--verbose]
  zseval compare <baseline.json> <candidate.json> [--threshold 0.05] [--json]
  zseval explain <trial-dir>
  zseval list [scenarios-root]
  zseval regrade <scenario-dir> <trial-dir> [--judge F] [--no-judge] [--json]
  zseval matrix <report.json>... [--json] [--markdown]

  --target is a zerostack config.toml (provider + model) seeded into each run's
  isolated config dir — the reproducible way to pick what you evaluate against.
  Put the API key in an env var (not the file); it is passed through to zerostack.
  Required for --backend zs; rejected for --backend mock. Repeatable: give
  --target more than once to evaluate N targets sequentially against the same
  suite in one invocation, under one shared --max-total-usd (an earlier
  target's spend shrinks what is left for the next one, not a fresh cap each);
  at the end, a scenario x target table renders to stderr (the same renderer
  `zseval matrix` uses). --json is a usage error when more than one --target is
  given (N reports have no single JSON form) — use `zseval matrix --json`
  over the resulting reports instead.

  --judge is a judge file naming which LLM grades the subjective layer — see
  judges/README.md. It is an inert ruler card with exactly four required
  fields: provider (anthropic | openai | openrouter | gemini), model,
  price_in_usd_per_mtok, price_out_usd_per_mtok. Nothing else is accepted: a
  judge file can never name a network destination or an env var — routing
  (endpoint and key) is derived from provider alone, in code. Unlike --target
  it may be given at most once: a matrix holds everything but the target
  fixed, so the ruler must not vary with the column.

  There is no built-in default judge. A suite with at least one judge-graded
  scenario requires an explicit choice: --judge <file> or --no-judge; giving
  neither is a usage error (exit 2) before any trial runs. A suite with no
  judge-graded scenarios needs neither flag.

  Before any trial spends money, --judge is preflighted: the provider's key
  env var must be set, and one live dry-run call (in the same prompt/token/
  temperature shape as real grading) must return a parseable verdict.
  Either failure exits 2 relaying the problem, before any trial — the probe
  itself is never recorded in the report or counted toward --max-total-usd.

  Every report records the judge file and a hash of its bytes, plus the models
  that actually graded as the judge's own responses reported them ([] if
  nothing was graded, absent if unknown — never the configured model echoed
  back as if it were a fact).

  --prompts is a directory of zerostack prompt files to evaluate against — a
  prompt pack. A pack holds top-level *.md files only, each file's stem being
  the prompt name it overrides; a subdirectory or a non-.md entry is a usage
  error naming it, since zerostack reads neither, and so is a directory with
  no *.md file at all. The whole pack is validated before any trial spends
  money. Unlike --target it may be given at most once: a run evaluates exactly
  one pack, and two packs are compared by two runs plus `zseval matrix` over
  their reports. Rejected for --backend mock, which replays canned artifacts
  and never constructs a zerostack invocation, so it could not load a pack.

  --backend mock=<path> replays canned artifacts instead of a live zerostack
  build: a single session JSON file, or a directory shaped like a captured
  trial dir (data/sessions/*.json + turn-N.{stdout,stderr,zslog}) to also
  replay stdout-based tool-call evidence.

  --jobs N runs up to N trials of the same scenario concurrently (default 1,
  strictly sequential). Trials are independent — their own isolated run_dir —
  so this only changes wall-clock time, never grading. Trial 0 always runs
  solo first to warm the provider's prompt cache before the rest fan out.
  Scenarios themselves always run one at a time.

  regrade re-scores an already-completed <trial-dir> against <scenario-dir>'s
  *current* asserts/judge, without driving the agent again — for checking
  whether an assert edit would have changed the verdict on frozen evidence.

  matrix renders a scenario x target table from one or more existing
  report.json files. It is a pure renderer: no API calls, nothing written to
  disk. Give it two reports to compare two targets, or reuse a committed
  baseline as one column to compose across time. Defaults to the fixed-width
  terminal table; --json emits the table model, --markdown emits a table for
  records (e.g. experiments/). A report with no target identity, or one that
  shares no scenario id with any other report given, is a usage error (exit
  2) naming the offending file; partial overlap instead renders `-` holes.

ENV:
  ZS_BIN             default path to the zerostack binary
  <PROVIDER>_API_KEY the target provider's key (e.g. ANTHROPIC_API_KEY, OPENROUTER_API_KEY)
  the judge's key: fixed per its card's provider, never named by the file —
                     ANTHROPIC_API_KEY, OPENAI_API_KEY, OPENROUTER_API_KEY, or
                     GEMINI_API_KEY. LLM-judge scenarios only; skip with
                     --no-judge.

EXIT CODES: 0 pass / no regression, 1 fail or regression, 2 harness error.";

struct Flags {
    positional: Vec<String>,
    kv: Vec<(String, String)>,
    switches: Vec<String>,
}

/// Parse `--k v` (valued) and `--switch` flags. Any `--flag` that is in
/// neither allowlist is an error — a typo'd `--triasl` must fail loudly, not
/// be silently swallowed and change nothing.
fn parse_flags(rest: Vec<String>, valued: &[&str], switches: &[&str]) -> anyhow::Result<Flags> {
    let mut f = Flags {
        positional: Vec::new(),
        kv: Vec::new(),
        switches: Vec::new(),
    };
    let mut it = rest.into_iter().peekable();
    while let Some(a) = it.next() {
        if let Some(name) = a.strip_prefix("--") {
            if valued.contains(&name) {
                let v = it
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--{name} needs a value"))?;
                f.kv.push((name.to_string(), v));
            } else if switches.contains(&name) {
                f.switches.push(name.to_string());
            } else {
                anyhow::bail!("unknown flag '--{name}'");
            }
        } else {
            f.positional.push(a);
        }
    }
    Ok(f)
}

impl Flags {
    fn get(&self, k: &str) -> Option<&str> {
        self.kv
            .iter()
            .rev()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v.as_str())
    }
    fn has(&self, k: &str) -> bool {
        self.switches.iter().any(|s| s == k)
    }
    fn count(&self, k: &str) -> usize {
        self.kv.iter().filter(|(n, _)| n == k).count()
    }
    /// Every occurrence of `k`, in the order given on the command line —
    /// unlike `get`, which only surfaces the last one. `--target` is
    /// repeatable (design.md: "Repeatable `--target`"), so `cmd_run` reaches
    /// for this to build the N-target loop (`run_over_targets`) instead of
    /// `get`'s last-wins single value.
    fn get_all(&self, k: &str) -> Vec<&str> {
        self.kv
            .iter()
            .filter(|(n, _)| n == k)
            .map(|(_, v)| v.as_str())
            .collect()
    }
}

/// The three ways `--judge` / `--no-judge` can resolve (design.md decision
/// 4, spec `judge-selection`): an explicit file (with the card already
/// loaded and validated), an explicit opt-out, or neither flag given at all.
///
/// `Unspecified` is deliberately not folded into `NoJudge`: whether it's
/// treated as "no judge" or as a loud mandatory-choice error depends on
/// whether the suite actually has rubric scenarios to grade — a decision
/// only the caller (`cmd_run`/`cmd_regrade`, after `discover`/`Scenario::load`)
/// can make, since `resolve_judge` itself never looks at scenarios.
#[derive(Debug)]
enum JudgeChoice {
    File(PathBuf, zseval::judge::JudgeConfig),
    NoJudge,
    Unspecified,
}

/// Resolve `--judge` / `--no-judge` into a `JudgeChoice`.
///
/// `--judge` is single-arity on purpose — the opposite of `--target`. A
/// matrix's premise is that everything except the target is held fixed, so
/// the ruler must not vary with the column: two `--judge` flags is a usage
/// error, not a last-one-wins pick.
///
/// `JudgeConfig` has no built-in default (see `judge-provider-card` section
/// 2: no committed card, no ruler) — when neither flag is given this returns
/// `Unspecified` rather than a pinned stand-in; it is the caller's job to
/// turn that into a mandatory-choice error for a rubric suite (section 4.2).
fn resolve_judge(f: &Flags) -> anyhow::Result<JudgeChoice> {
    if f.count("judge") > 1 {
        anyhow::bail!(
            "--judge may be given at most once (unlike --target): a run is graded \
             by exactly one judge, or the scores it produces aren't comparable"
        );
    }
    match f.get("judge") {
        Some(p) => {
            if f.has("no-judge") {
                anyhow::bail!("--judge and --no-judge contradict each other: pick one");
            }
            let path = PathBuf::from(p);
            // Loud on anything unreadable or invalid — never a silent
            // fallback, which would grade with a ruler the caller didn't ask
            // for.
            let cfg = zseval::judge::JudgeConfig::load(&path)?;
            Ok(JudgeChoice::File(path, cfg))
        }
        None => {
            if f.has("no-judge") {
                Ok(JudgeChoice::NoJudge)
            } else {
                Ok(JudgeChoice::Unspecified)
            }
        }
    }
}

/// Stand-in used whenever no real judge will grade: `--no-judge`, or
/// `Unspecified` on a suite with no rubric scenarios (see
/// `require_judge_decision`). `JudgeConfig` has no built-in default (section
/// 2 of `judge-provider-card` removed it deliberately: no committed card, no
/// ruler), so there is nothing to build a real `LlmJudge` from in either
/// case. This reports itself unavailable and errors loudly if ever actually
/// asked to grade — which should never happen, since both cases above are
/// only reached when no scenario has a rubric, or `--no-judge` was explicit.
struct NoJudgeConfigured;

impl zseval::judge::Judge for NoJudgeConfigured {
    fn available(&self) -> bool {
        false
    }
    fn unavailable_hint(&self) -> String {
        "no judge configured".to_string()
    }
    fn judge(
        &self,
        _rubric: &str,
        _evidence: &str,
        _run_dir: &Path,
    ) -> anyhow::Result<zseval::judge::JudgeOutcome> {
        anyhow::bail!(
            "no judge configured: pass --judge <file> or --no-judge (see judges/README.md)"
        )
    }
}

/// The mandatory-choice gate (spec `judge-selection`): a suite with at least
/// one rubric scenario must have an explicit judge decision. `Unspecified` is
/// fine when nothing needs grading (`has_rubric` false) — there is nothing to
/// decide — but is a loud, exit-2 usage error the moment a rubric exists,
/// naming both flags so the fix is obvious. Runs right after
/// discovery/loading and before any trial, backend setup, or preflight probe.
fn require_judge_decision(has_rubric: bool, choice: &JudgeChoice) -> anyhow::Result<()> {
    if has_rubric && matches!(choice, JudgeChoice::Unspecified) {
        anyhow::bail!(
            "this suite has at least one judge-graded scenario: pass --judge <file> to name a \
             ruler, or --no-judge to grade the deterministic asserts only (see judges/README.md)"
        );
    }
    Ok(())
}

/// Turns a resolved `JudgeChoice` into what the rest of `cmd_run`/
/// `cmd_regrade` need: the file to record, whether grading should be
/// skipped, and the `Judge` to grade with. Only ever called after
/// `require_judge_decision` has passed, so `Unspecified` here is only ever
/// the no-rubric case and is handled exactly like `NoJudge`.
///
/// When a real judge is resolved AND the suite actually has a rubric to
/// grade, this runs `LlmJudge::preflight()` before returning — before any
/// trial, per spec `judge-preflight` — so a broken judge (bad key, wrong
/// model, truncated output) fails loudly here rather than mid-suite. A
/// `--judge` given for a suite with no rubric at all skips the probe: there
/// is nothing to preflight when the judge will never actually be called.
fn judge_for(
    choice: JudgeChoice,
    has_rubric: bool,
) -> anyhow::Result<(Option<PathBuf>, bool, Box<dyn zseval::judge::Judge>)> {
    match choice {
        JudgeChoice::File(path, cfg) => {
            let judge = zseval::judge::LlmJudge::new(cfg);
            if has_rubric {
                judge.preflight()?;
            }
            Ok((Some(path), false, Box::new(judge)))
        }
        JudgeChoice::NoJudge | JudgeChoice::Unspecified => {
            Ok((None, true, Box::new(NoJudgeConfigured)))
        }
    }
}

fn cmd_run(rest: Vec<String>) -> anyhow::Result<ExitCode> {
    let f = parse_flags(
        rest,
        &[
            "trials",
            "tag",
            "zs-bin",
            "backend",
            "max-total-usd",
            "results",
            "target",
            "jobs",
            "judge",
            "prompts",
        ],
        &["no-judge", "json", "verbose"],
    )?;
    let choice = resolve_judge(&f)?;
    let path = f
        .positional
        .first()
        .ok_or_else(|| anyhow::anyhow!("run: missing <scenario-path>"))?;
    let scenarios = discover(Path::new(path))?;
    if scenarios.is_empty() {
        anyhow::bail!("no scenario.toml found under {path}");
    }
    let has_rubric = scenarios.iter().any(|s| s.judge.is_some());
    require_judge_decision(has_rubric, &choice)?;
    let (judge_path, no_judge, judge) = judge_for(choice, has_rubric)?;

    let targets: Vec<PathBuf> = f.get_all("target").into_iter().map(PathBuf::from).collect();
    let multi = targets.len() > 1;

    // mock replays canned artifacts and never reads a target config.toml, so
    // any `--target` under `--backend mock` is a usage error. Checked before
    // the --json/N>1 guards below so the caller sees the specific "mock
    // rejects --target" reason, not a generic multi-target complaint about a
    // combination mock could never honour anyway.
    if matches!(f.get("backend"), Some(b) if b.starts_with("mock=")) && !targets.is_empty() {
        anyhow::bail!(
            "--target is rejected for --backend mock: mock replays canned artifacts \
             and never reads a target config.toml"
        );
    }

    // `--prompts` is single-arity, the opposite of `--target`: a run evaluates
    // exactly one pack, so that a report's pack identity names one thing. Two
    // packs is two runs joined by `matrix` — which is also the only shape whose
    // columns this change's own one-variable rule can read.
    if f.count("prompts") > 1 {
        anyhow::bail!(
            "--prompts may be given at most once (unlike --target): a run evaluates exactly \
             one prompt pack; compare two packs with two runs and `zseval matrix` over their \
             reports"
        );
    }

    // mock replays canned artifacts and never constructs a zerostack
    // invocation or seeds a run directory, so a pack could not reach it —
    // accepting the flag would produce a report advertising a pack nothing
    // could have loaded. Same reasoning as the `--target` rejection above.
    if matches!(f.get("backend"), Some(b) if b.starts_with("mock=")) && f.get("prompts").is_some() {
        anyhow::bail!(
            "--prompts is rejected for --backend mock: mock replays canned artifacts and never \
             constructs a zerostack invocation, so it cannot load a pack"
        );
    }

    // Validate (and keep) the pack before any backend, budget, or trial
    // setup: a directory zerostack could never read must fail here, not
    // after a suite has already spent money. `Arc` so the one loaded pack is
    // shared, not reloaded, across every target in `run_over_targets`' loop.
    let prompt_pack: Option<std::sync::Arc<zseval::prompts::PromptPack>> = match f.get("prompts") {
        Some(dir) => Some(std::sync::Arc::new(zseval::prompts::PromptPack::load(
            Path::new(dir),
        )?)),
        None => None,
    };

    // N reports have no single JSON form (design.md: "`run --json` at N>1 is
    // a usage error"). Checked before backend/budget setup, so this is a
    // pure usage error rather than a partial run.
    if f.has("json") && multi {
        anyhow::bail!(
            "run --json accepts at most one --target ({} given): N reports have no single \
             JSON form; drop --json (the end-of-run table covers N>1) or run `zseval matrix \
             --json <report.json>...` over the resulting reports instead",
            targets.len()
        );
    }
    if multi {
        let target_refs: Vec<&Path> = targets.iter().map(PathBuf::as_path).collect();
        zseval::target::check_stem_collision(&target_refs)?;
    }

    zseval::backend::set_verbose(f.has("verbose"));

    let cfg = MultiTargetConfig {
        tag: f
            .get("tag")
            .map(String::from)
            .unwrap_or_else(|| auto_tag(path, f.get("target"), multi, f.get("prompts"))),
        no_judge,
        results_root: f
            .get("results")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("results")),
        max_total_usd: match f.get("max-total-usd") {
            Some(x) => Some(x.parse()?),
            None => None,
        },
        jobs: match f.get("jobs") {
            Some(j) => j.parse()?,
            None => 1,
        },
        judge_file: judge_path,
        trials_override: match f.get("trials") {
            Some(t) => Some(t.parse()?),
            None => None,
        },
    };

    let reports: Vec<Report> = match f.get("backend") {
        Some(b) if b.starts_with("mock=") => {
            // `--target` under mock is already rejected above.
            let backend: Box<dyn AgentBackend> = Box::new(Mock {
                fixture: PathBuf::from(b.trim_start_matches("mock=")),
            });
            let opts = RunOptions {
                target: None,
                trials_override: cfg.trials_override,
                tag: cfg.tag.clone(),
                no_judge: cfg.no_judge,
                results_root: cfg.results_root.clone(),
                max_total_usd: cfg.max_total_usd,
                jobs: cfg.jobs,
                judge_file: cfg.judge_file.clone(),
                multi_target: false,
            };
            vec![run_suite(
                &scenarios,
                backend.as_ref(),
                judge.as_ref(),
                &opts,
            )?]
        }
        Some("zs") | None => {
            if targets.is_empty() {
                anyhow::bail!(
                    "--target is required for --backend zs: pass a zerostack config.toml \
                     naming what to evaluate against"
                );
            }
            let bin = f
                .get("zs-bin")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("ZS_BIN").map(PathBuf::from))
                .ok_or_else(|| {
                    anyhow::anyhow!("need --zs-bin or ZS_BIN env (or --backend mock=<file>)")
                })?;
            run_over_targets(
                &scenarios,
                &targets,
                |t| {
                    Box::new(ZsCli {
                        bin: bin.clone(),
                        target: Some(t.to_path_buf()),
                        prompts: prompt_pack.clone(),
                    })
                },
                judge.as_ref(),
                &cfg,
            )?
        }
        Some(other) => anyhow::bail!("unknown backend '{other}'"),
    };

    if f.has("json") {
        // N>1 is rejected above, so exactly one report reaches here.
        println!("{}", serde_json::to_string_pretty(&reports[0])?);
    } else {
        print_run_report_summaries(&reports, &targets, multi, &cfg, &mut std::io::stderr())?;
    }

    // Most severe across columns, via each report's own `exit_code`. Note this
    // scores an *empty* (budget-truncated-to-zero) column 0, not 2: a budget cut
    // is a recorded fact (`budget_truncated`), not a harness error, so it must
    // not fail the run. `matrix` deliberately differs (an all-holes column exits
    // 2 there) — see design.md, "`run` and `matrix` diverge on a zero-scenario
    // column, deliberately".
    Ok(ExitCode::from(
        reports.iter().map(Report::exit_code).max().unwrap_or(0),
    ))
}

/// The non-`--json` end-of-run output: per-target summary lines, each
/// report's path, and — at N>1 — the scenario x target table built by the
/// same renderer `matrix` uses (target-matrix section 8). Everything here
/// writes only to `err`; the caller passes `stderr()` in production and a
/// `Vec<u8>` buffer in tests, which is how target-matrix 8.1 ("the table
/// lands on stderr while stdout stays clean") is verified without a
/// subprocess: this function has no way to reach stdout at all, by
/// construction, since it only ever receives one writer.
fn print_run_report_summaries(
    reports: &[Report],
    targets: &[PathBuf],
    multi: bool,
    cfg: &MultiTargetConfig,
    err: &mut impl std::io::Write,
) -> anyhow::Result<()> {
    for (i, report) in reports.iter().enumerate() {
        // Rates are undefined when nothing was gradable — show n/a rather
        // than 0, per line: an empty kind must not borrow the overall
        // report's gradable count to decide its own n/a (trustworthy-numbers
        // design D5).
        let rate = |v: f64, n_gradable: usize| {
            if n_gradable == 0 {
                "  n/a".to_string()
            } else {
                format!("{v:.3}")
            }
        };
        // Three lines, in this fixed order: the historical blended overall
        // renders last, since a number that averages expected-low capability
        // probes into contract regressions is the least interpretable of the
        // three (design D5).
        writeln!(
            err,
            "\nregression: {} scenarios ({} gradable) | pass@k {} | pass^k {}",
            report.summary.regression.n_scenarios,
            report.summary.regression.n_gradable,
            rate(
                report.summary.regression.pass_at_k,
                report.summary.regression.n_gradable
            ),
            rate(
                report.summary.regression.pass_hat_k,
                report.summary.regression.n_gradable
            ),
        )?;
        writeln!(
            err,
            "capability: {} scenarios ({} gradable) | pass@k {} | pass^k {}",
            report.summary.capability.n_scenarios,
            report.summary.capability.n_gradable,
            rate(
                report.summary.capability.pass_at_k,
                report.summary.capability.n_gradable
            ),
            rate(
                report.summary.capability.pass_hat_k,
                report.summary.capability.n_gradable
            ),
        )?;
        writeln!(
            err,
            "overall: {} scenarios ({} gradable) | pass@k {} | pass^k {} | \
             indeterminate {} scenario(s), {} trial(s) | ${:.4}",
            report.summary.n_scenarios,
            report.summary.n_gradable,
            rate(report.summary.pass_at_k, report.summary.n_gradable),
            rate(report.summary.pass_hat_k, report.summary.n_gradable),
            report.summary.indeterminate_scenarios,
            report.summary.indeterminate_trials,
            report.summary.total_cost_usd,
        )?;
        // The same derivation `run_suite` used to place this report (flat at
        // N=1, nested under the target's stem at N>1): computed once in
        // `runner::run_root`, reused here rather than re-derived.
        let run_root = zseval::runner::run_root(
            &cfg.results_root,
            &cfg.tag,
            multi,
            targets.get(i).map(|t| t.as_path()),
        )?;
        writeln!(err, "report: {}", run_root.join("report.json").display())?;
    }

    if multi {
        let report_refs: Vec<&Report> = reports.iter().collect();
        let m = zseval::matrix::build(&report_refs);
        writeln!(err, "\n{}", zseval::matrix::render_fixed_width(&m))?;
    }

    Ok(())
}

/// The parts of a multi-target `run` invocation that stay fixed across every
/// target in the loop — only `target` itself, the shrinking budget cap, and
/// `multi_target` (both computed by `run_over_targets`) vary per iteration.
struct MultiTargetConfig {
    tag: String,
    no_judge: bool,
    results_root: PathBuf,
    /// The whole invocation's budget (`--max-total-usd`), shared across every
    /// target rather than given to each one independently — see
    /// `run_over_targets`. `None` means unlimited.
    max_total_usd: Option<f64>,
    jobs: usize,
    judge_file: Option<PathBuf>,
    trials_override: Option<usize>,
}

/// Evaluate `scenarios` against every target in `targets`, sequentially, each
/// against a fresh backend from `make_backend`, under one shared budget
/// (target-matrix 4.3, design.md "Budget is one shared total; truncation is
/// marked"): a target's own cap is `max_total_usd - spent_so_far`, so what an
/// earlier target already spent comes out of what is left for the next one,
/// rather than every target getting its own full `--max-total-usd`.
/// `run_suite`'s existing per-scenario break (it stops before a scenario once
/// `spent >= cap`) needs no change for this: a cap clamped to `0.0` (spend
/// can overrun a cap that is only checked between scenarios, so the naive
/// subtraction can go negative) just makes that break fire before the
/// target's very first scenario, shutting a fully-out-of-budget target out
/// entirely.
///
/// `targets.len() > 1` decides `multi_target` on every `RunOptions` built
/// here — including the `targets.len() == 1` case, which is the ordinary
/// single-target `zs` run (only `--backend mock=` never reaches this
/// function).
fn run_over_targets(
    scenarios: &[Scenario],
    targets: &[PathBuf],
    make_backend: impl Fn(&Path) -> Box<dyn AgentBackend>,
    judge: &dyn zseval::judge::Judge,
    cfg: &MultiTargetConfig,
) -> anyhow::Result<Vec<Report>> {
    let multi_target = targets.len() > 1;
    let mut reports = Vec::with_capacity(targets.len());
    let mut spent_so_far = 0.0_f64;
    for target in targets {
        let backend = make_backend(target);
        let opts = RunOptions {
            target: Some(target.clone()),
            trials_override: cfg.trials_override,
            tag: cfg.tag.clone(),
            no_judge: cfg.no_judge,
            results_root: cfg.results_root.clone(),
            max_total_usd: cfg
                .max_total_usd
                .map(|total| (total - spent_so_far).max(0.0)),
            jobs: cfg.jobs,
            judge_file: cfg.judge_file.clone(),
            multi_target,
        };
        let report = run_suite(scenarios, backend.as_ref(), judge, &opts)?;
        spent_so_far += report.summary.total_cost_usd;
        reports.push(report);
    }
    Ok(reports)
}

/// Build a human-identifiable run tag: which scenarios, against what pack and
/// provider+model, and when — e.g.
/// `prompts_my-pack_anthropic-claude-sonnet-4-6_20260706-091936`. Semantic
/// tags like `main` are left to an explicit `--tag`; the auto name is always
/// descriptive.
///
/// `multi` is true when this invocation covers more than one `--target`: the
/// tag is then shared by every target (the results layout tells them apart
/// by stem instead — see `RunOptions::multi_target`), so the provider-model
/// segment is dropped here rather than appearing once in the tag and once
/// more as the stem. `pack`, by contrast, is held fixed across every target
/// in a multi-target run, so it stays in the tag even when `multi` is true —
/// it is what distinguishes one multi-target run from the next.
fn auto_tag(scenario_path: &str, target: Option<&str>, multi: bool, pack: Option<&str>) -> String {
    let suite = Path::new(scenario_path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("scenarios");
    // Read provider + model out of the target config.
    let (provider, model) = if multi {
        (None, None)
    } else {
        target
            .map(|p| zseval::target::peek(Path::new(p)))
            .unwrap_or((None, None))
    };
    let target_group: Vec<String> = [provider, model]
        .into_iter()
        .flatten()
        .map(|s| sanitize(&s))
        .collect();
    let pack_name = pack
        .and_then(|p| Path::new(p).file_name())
        .and_then(|s| s.to_str())
        .map(sanitize);
    let ts = zseval::util::compact_timestamp();

    let mut segments = vec![sanitize(suite)];
    segments.extend(pack_name);
    if !target_group.is_empty() {
        segments.push(target_group.join("-"));
    }
    segments.push(ts);
    segments.join("_")
}

/// Keep only filesystem-friendly characters for a tag segment.
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_') {
                c
            } else {
                '-'
            }
        })
        .collect()
}

fn cmd_compare(rest: Vec<String>) -> anyhow::Result<ExitCode> {
    let f = parse_flags(rest, &["threshold"], &["json"])?;
    let (base_p, cand_p) = match (f.positional.first(), f.positional.get(1)) {
        (Some(a), Some(b)) => (a, b),
        _ => anyhow::bail!("compare: need <baseline.json> <candidate.json>"),
    };
    let threshold: f64 = f.get("threshold").unwrap_or("0.05").parse()?;
    let base = load_report(Path::new(base_p))?;
    let cand = load_report(Path::new(cand_p))?;
    let c = compare(&base, &cand, threshold);
    if f.has("json") {
        println!("{}", serde_json::to_string_pretty(&c)?);
    } else {
        print_human(&c);
    }
    Ok(ExitCode::from(c.exit_code()))
}

fn cmd_explain(rest: Vec<String>) -> anyhow::Result<ExitCode> {
    let f = parse_flags(rest, &[], &[])?;
    let dir = f
        .positional
        .first()
        .ok_or_else(|| anyhow::anyhow!("explain: missing <trial-dir>"))?;
    let dir = Path::new(dir);
    let trial = std::fs::read_to_string(dir.join("trial.json"))?;
    println!("== trial.json ==\n{trial}");
    let sessions = dir.join("data").join("sessions");
    if let Ok(entries) = std::fs::read_dir(&sessions) {
        for e in entries.flatten() {
            println!("== session {} ==", e.path().display());
            match zseval::transcript::parse_file(&e.path()) {
                Ok(t) => {
                    for m in &t.messages {
                        let one_line = m.content.replace('\n', " ");
                        let shown: String = one_line.chars().take(200).collect();
                        println!("[{}] {}", m.role, shown);
                    }
                }
                Err(err) => println!("(unparseable: {err:#})"),
            }
        }
    }
    // `mode = "loop"` scenarios have no session file at all — their evidence
    // is the per-iteration records under data/loops/<uuid>/iter-NNNN.json
    // (see scenario::LoopCfg's doc). A missing/empty dir is silently a
    // no-op here, same as the sessions dir above.
    match zseval::transcript::read_loop_iterations(&dir.join("data")) {
        Ok(iters) if !iters.is_empty() => {
            for it in &iters {
                println!("== loop iteration {} ==", it.iteration);
                let response_head: String =
                    it.response.replace('\n', " ").chars().take(300).collect();
                println!("response: {response_head}");
                if let Some(v) = &it.validation_output {
                    let tail: String = v
                        .replace('\n', " ")
                        .chars()
                        .rev()
                        .take(300)
                        .collect::<String>()
                        .chars()
                        .rev()
                        .collect();
                    println!("validation_output (tail): {tail}");
                }
            }
        }
        Ok(_) => {}
        Err(err) => println!("(loop iterations unreadable: {err:#})"),
    }
    // Dump every captured per-turn log (zerostack trace + stderr), in turn
    // order, however many turns the scenario had. This is a separate process
    // from the run that produced `dir`, so there's no live RunArtifacts to
    // consult — discover_turn_artifacts reconstructs the same shape backend
    // would have handed the runner.
    for t in zseval::backend::discover_turn_artifacts(dir) {
        for p in [&t.zslog, &t.stderr] {
            if let Ok(s) = std::fs::read_to_string(p) {
                if !s.trim().is_empty() {
                    let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
                    println!("== {name} ==\n{s}");
                }
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_regrade(rest: Vec<String>) -> anyhow::Result<ExitCode> {
    let f = parse_flags(rest, &["judge"], &["no-judge", "json"])?;
    let choice = resolve_judge(&f)?;
    let sc_dir = f
        .positional
        .first()
        .ok_or_else(|| anyhow::anyhow!("regrade: missing <scenario-dir>"))?;
    let trial_dir = f
        .positional
        .get(1)
        .ok_or_else(|| anyhow::anyhow!("regrade: missing <trial-dir>"))?;
    let sc = zseval::scenario::Scenario::load(Path::new(sc_dir))?;
    let trial_dir = Path::new(trial_dir);
    let has_rubric = sc.judge.is_some();
    require_judge_decision(has_rubric, &choice)?;
    let (judge_path, no_judge, judge) = judge_for(choice, has_rubric)?;
    // The trial index is cosmetic (it only labels the returned TrialResult),
    // so a directory not named "trial-N" just grades as trial 0.
    let trial_idx = trial_dir
        .file_name()
        .and_then(|s| s.to_str())
        .and_then(|s| s.strip_prefix("trial-"))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let tr = zseval::runner::regrade(
        &sc,
        judge.as_ref(),
        judge_path.as_deref(),
        no_judge,
        trial_idx,
        trial_dir,
    )?;

    if f.has("json") {
        println!("{}", serde_json::to_string_pretty(&tr)?);
    } else {
        let mark = match tr.outcome {
            zseval::verdict::Final::Pass => "PASS",
            zseval::verdict::Final::Fail => "FAIL",
            zseval::verdict::Final::Indeterminate => "????",
        };
        eprintln!(
            "[{mark}] {} trial {}{}",
            sc.id,
            tr.trial,
            if tr.reasons.is_empty() {
                String::new()
            } else {
                format!(" — {}", tr.reasons.join("; "))
            }
        );
    }

    Ok(match tr.outcome {
        zseval::verdict::Final::Fail => ExitCode::from(1),
        zseval::verdict::Final::Indeterminate => ExitCode::from(2),
        zseval::verdict::Final::Pass => ExitCode::SUCCESS,
    })
}

fn cmd_list(rest: Vec<String>) -> anyhow::Result<ExitCode> {
    let f = parse_flags(rest, &[], &[])?;
    let root = f
        .positional
        .first()
        .map(String::as_str)
        .unwrap_or("scenarios");
    let scenarios = discover(Path::new(root))?;
    for s in &scenarios {
        println!(
            "{:<44} prompt={:<12} trials={}",
            s.id,
            s.prompt.as_deref().unwrap_or("-"),
            s.trials
        );
    }
    eprintln!("{} scenario(s)", scenarios.len());
    Ok(ExitCode::SUCCESS)
}

/// `matrix <report.json>...`: a pure renderer over existing reports
/// (target-matrix section 7). Makes no API calls and writes nothing to
/// disk — `matrix::build` (section 5) does the modelling; this command only
/// does the identity/overlap validation `build` deliberately leaves to its
/// caller (it has no file path to name in an error), and picks a renderer.
fn cmd_matrix(rest: Vec<String>) -> anyhow::Result<ExitCode> {
    let f = parse_flags(rest, &[], &["json", "markdown"])?;
    if f.has("json") && f.has("markdown") {
        anyhow::bail!("matrix: --json and --markdown are mutually exclusive");
    }
    if f.positional.is_empty() {
        anyhow::bail!("matrix: need one or more <report.json> paths");
    }

    let reports: Vec<(String, Report)> = f
        .positional
        .iter()
        .map(|p| Ok((p.clone(), load_report(Path::new(p))?)))
        .collect::<anyhow::Result<_>>()?;

    // A target-less report has no column identity — rejected on that field
    // alone, per design.md ("Incomparability is layered, and content-based"),
    // never by gating on `schema_version`.
    for (path, r) in &reports {
        if r.target.is_empty() {
            anyhow::bail!(
                "{path}: report has no target identity (empty `target` field): matrix needs \
                 column identity to render — see report-target"
            );
        }
    }

    // Zero shared scenarios with every other report given is a hard error
    // naming the offending file; partial overlap is fine (`build` renders
    // `-` holes for it).
    for (i, (path, r)) in reports.iter().enumerate() {
        if reports.len() < 2 {
            continue;
        }
        let own_ids: std::collections::HashSet<&str> =
            r.scenarios.iter().map(|s| s.id.as_str()).collect();
        let other_ids: std::collections::HashSet<&str> = reports
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .flat_map(|(_, (_, other))| other.scenarios.iter().map(|s| s.id.as_str()))
            .collect();
        if own_ids.is_disjoint(&other_ids) {
            anyhow::bail!(
                "{path}: shares no scenario id with any other report given: not comparable in \
                 one matrix"
            );
        }
    }

    let report_refs: Vec<&Report> = reports.iter().map(|(_, r)| r).collect();
    let m = zseval::matrix::build(&report_refs);

    if f.has("json") {
        println!("{}", serde_json::to_string_pretty(&m)?);
    } else if f.has("markdown") {
        println!("{}", zseval::matrix::render_markdown(&m));
    } else {
        println!("{}", zseval::matrix::render_fixed_width(&m));
    }

    // 0 when a table rendered, 2 when any column is fully ungradable, never
    // 1 (matrix compares columns, it does not gate a regression). Read the
    // rendered matrix, not `Report::exit_code`: a column budget-truncated to
    // zero scenarios has an empty `scenarios` and so scores exit_code 0, yet
    // it contributes nothing but holes — an all-holes column is the true
    // "fully ungradable" signal and catches that case too.
    let any_fully_ungradable = (0..m.columns.len()).any(|c| {
        m.rows
            .iter()
            .all(|row| row.cells[c] == zseval::matrix::Cell::Hole)
    });
    Ok(ExitCode::from(if any_fully_ungradable { 2 } else { 0 }))
}

#[cfg(test)]
mod judge_flag_tests {
    use super::*;

    fn flags(args: &[&str]) -> Flags {
        parse_flags(
            args.iter().map(|s| s.to_string()).collect(),
            &["judge"],
            &["no-judge"],
        )
        .unwrap()
    }

    /// `--judge` is deliberately single-arity, the opposite of `--target`.
    /// The matrix's premise is "everything but the target is fixed" — the
    /// ruler must not vary with the column, so two rulers is a usage error,
    /// not a last-one-wins silent pick.
    #[test]
    fn judge_given_twice_is_a_usage_error() {
        let f = flags(&[
            "--judge",
            "judges/sonnet.toml",
            "--judge",
            "judges/opus.toml",
        ]);
        let err = resolve_judge(&f).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("--judge"), "{msg}");
    }

    /// Naming a ruler and switching the judge off in the same breath is a
    /// contradiction — surface it rather than silently honouring one.
    #[test]
    fn judge_together_with_no_judge_is_a_usage_error() {
        let f = flags(&["--judge", "judges/opus.toml", "--no-judge"]);
        let err = resolve_judge(&f).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("--no-judge"), "{msg}");
    }

    /// Neither flag given resolves to `Unspecified` — distinct from
    /// `NoJudge` (design.md decision 4): a rubric suite must reject this
    /// state with a mandatory-choice error (section 4.2's gate in
    /// `cmd_run`/`cmd_regrade`), whereas a no-rubric suite treats it the
    /// same as `NoJudge`. This test only checks resolution, not the gate.
    #[test]
    fn omitting_both_flags_resolves_to_unspecified() {
        let f = flags(&[]);
        let choice = resolve_judge(&f).unwrap();
        assert!(matches!(choice, JudgeChoice::Unspecified));
    }

    /// `--judge` resolves relative to the caller's cwd, which under
    /// `cargo test` is the crate dir, not the repo root. `judges/opus.toml`
    /// still carries the OLD (`api_url`/`api_key_env`) schema as of this
    /// section — updating the shipped cards is section 5's job — so this
    /// points `--judge` at an inline fixture written in the new four-field
    /// schema instead of the real shipped file.
    #[test]
    fn a_judge_file_resolves_to_the_file_state_with_its_own_values() {
        let dir = std::env::temp_dir().join(format!(
            "zseval-judge-flag-test-opus-shaped-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("opus-shaped.toml");
        std::fs::write(
            &path,
            "provider = \"anthropic\"\nmodel = \"claude-opus-4-8\"\n\
             price_in_usd_per_mtok = 5.0\nprice_out_usd_per_mtok = 25.0\n",
        )
        .unwrap();
        let path_str = path.display().to_string();

        let f = flags(&["--judge", &path_str]);
        match resolve_judge(&f).unwrap() {
            JudgeChoice::File(got_path, cfg) => {
                assert_eq!(got_path, PathBuf::from(&path_str));
                assert_eq!(cfg.model, "claude-opus-4-8");
            }
            other => panic!("expected JudgeChoice::File, got {other:?}"),
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn no_judge_alone_resolves_to_the_no_judge_state() {
        let f = flags(&["--no-judge"]);
        let choice = resolve_judge(&f).unwrap();
        assert!(matches!(choice, JudgeChoice::NoJudge));
    }

    /// `--target` is repeatable (design.md); `get_all` must surface every
    /// occurrence in order, unlike `get`'s last-wins lookup.
    #[test]
    fn get_all_returns_every_occurrence_in_order() {
        let f = parse_flags(
            ["--target", "a", "--target", "b"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            &["target"],
            &[],
        )
        .unwrap();
        assert_eq!(f.get_all("target"), vec!["a", "b"]);
    }

    /// target-matrix 3.6: an N>1 run shares one tag across targets (the
    /// results layout tells them apart by stem), so the provider-model
    /// segment must not appear in the tag at all — otherwise it would show
    /// up once in the tag and once more as the nested stem.
    #[test]
    fn auto_tag_drops_the_provider_model_segment_when_multi() {
        let dir =
            std::env::temp_dir().join(format!("zseval-autotag-multi-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target_path = dir.join("opus.toml");
        std::fs::write(
            &target_path,
            "provider = \"anthropic\"\nmodel = \"claude-opus-4-8\"\n",
        )
        .unwrap();
        let target_str = target_path.display().to_string();

        let single = auto_tag("scenarios", Some(&target_str), false, None);
        assert!(single.contains("anthropic"), "{single}");
        assert!(single.contains("claude-opus-4-8"), "{single}");

        let multi = auto_tag("scenarios", Some(&target_str), true, None);
        assert!(!multi.contains("anthropic"), "{multi}");
        assert!(!multi.contains("claude-opus-4-8"), "{multi}");
        assert!(multi.starts_with("scenarios_"), "{multi}");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// prompts-pack 3.1: with a pack and no explicit `--tag`, the auto tag
    /// carries the pack directory's name alongside the existing suite and
    /// provider/model segments — two runs differing only by pack must be
    /// distinguishable by results directory name, not only by timestamp.
    #[test]
    fn auto_tag_includes_the_pack_directory_name() {
        let dir =
            std::env::temp_dir().join(format!("zseval-autotag-pack-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target_path = dir.join("opus.toml");
        std::fs::write(
            &target_path,
            "provider = \"anthropic\"\nmodel = \"claude-opus-4-8\"\n",
        )
        .unwrap();
        let target_str = target_path.display().to_string();
        let pack_dir = dir.join("my-pack");
        std::fs::create_dir_all(&pack_dir).unwrap();
        let pack_str = pack_dir.display().to_string();

        let tag = auto_tag("scenarios", Some(&target_str), false, Some(&pack_str));
        assert!(tag.contains("my-pack"), "{tag}");
        assert!(tag.contains("anthropic"), "{tag}");
        assert!(tag.contains("claude-opus-4-8"), "{tag}");
        assert!(tag.starts_with("scenarios_"), "{tag}");

        let without_pack = auto_tag("scenarios", Some(&target_str), false, None);
        assert!(!without_pack.contains("my-pack"), "{without_pack}");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// An explicit `--tag` is never touched by the pack: it is used exactly
    /// as passed, whether or not `--prompts` is also given. `auto_tag` is
    /// only ever consulted when no explicit tag exists (`cmd_run`'s
    /// `unwrap_or_else`), so this pins that short-circuit directly rather
    /// than asserting a negative about `auto_tag`'s own output.
    #[test]
    fn explicit_tag_is_used_verbatim_even_with_a_pack() {
        let f = parse_flags(
            ["--tag", "stock", "--prompts", "my-pack/"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            &["tag", "prompts"],
            &[],
        )
        .unwrap();
        let tag = f
            .get("tag")
            .map(String::from)
            .unwrap_or_else(|| auto_tag("scenarios", None, false, f.get("prompts")));
        assert_eq!(tag, "stock");
    }

    /// prompts-pack 3.2: `multi` already drops the provider/model segment
    /// (target-matrix 3.6) since the results layout tells targets apart by
    /// stem instead. The pack segment must survive that drop: it is held
    /// fixed across every target in a multi-target run and is exactly what
    /// distinguishes one multi-target run from the next.
    #[test]
    fn auto_tag_keeps_the_pack_segment_when_multi() {
        let dir = std::env::temp_dir().join(format!(
            "zseval-autotag-pack-multi-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let target_path = dir.join("opus.toml");
        std::fs::write(
            &target_path,
            "provider = \"anthropic\"\nmodel = \"claude-opus-4-8\"\n",
        )
        .unwrap();
        let target_str = target_path.display().to_string();
        let pack_dir = dir.join("my-pack");
        std::fs::create_dir_all(&pack_dir).unwrap();
        let pack_str = pack_dir.display().to_string();

        let multi = auto_tag("scenarios", Some(&target_str), true, Some(&pack_str));
        assert!(multi.contains("my-pack"), "{multi}");
        assert!(!multi.contains("anthropic"), "{multi}");
        assert!(!multi.contains("claude-opus-4-8"), "{multi}");
        assert!(multi.starts_with("scenarios_"), "{multi}");

        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod multi_target_tests {
    use super::*;
    use zseval::backend::Mock;

    fn scenario_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "zseval-multitarget-scenario-{name}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("scenario.toml"),
            format!("id = \"{name}\"\nkind = \"regression\"\ntask = \"say hi\"\nexpect = [\"final_contains done\"]\n"),
        )
        .unwrap();
        dir
    }

    /// A single-file Mock fixture (`backend.rs::Mock`'s legacy shape) whose
    /// session records `cost_usd` as its `total_cost` and `message` as the
    /// final assistant turn — replayed verbatim for every scenario/trial the
    /// backend is asked to run, so a suite of N trials against this fixture
    /// spends `N * cost_usd` and grades against `message`.
    fn mock_fixture_with_message(name: &str, cost_usd: f64, message: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "zseval-multitarget-fixture-{name}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.json");
        std::fs::write(
            &path,
            format!(
                r#"{{"id":"s","messages":[{{"role":"user","content":"hi"}},{{"role":"assistant","content":"{message}"}}],"total_input_tokens":1,"total_output_tokens":1,"total_cost":{cost_usd}}}"#
            ),
        )
        .unwrap();
        path
    }

    /// [`mock_fixture_with_message`] with the message the scenarios in this
    /// module's `scenario_dir` expect (`final_contains done`).
    fn mock_fixture(name: &str, cost_usd: f64) -> PathBuf {
        mock_fixture_with_message(name, cost_usd, "done")
    }

    /// A minimal on-disk target config.toml — `run_suite` copies whatever
    /// `RunOptions::target` names into the run-level `target.toml` (section
    /// 3.4), so a placeholder path with nothing behind it won't do.
    fn target_file(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "zseval-multitarget-target-{name}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{name}.toml"));
        std::fs::write(
            &path,
            "provider = \"anthropic\"\nmodel = \"claude-sonnet-4-6\"\n",
        )
        .unwrap();
        path
    }

    fn base_cfg(name: &str, max_total_usd: Option<f64>) -> MultiTargetConfig {
        MultiTargetConfig {
            tag: "t".to_string(),
            no_judge: true,
            results_root: std::env::temp_dir().join(format!(
                "zseval-multitarget-results-{name}-{}",
                std::process::id()
            )),
            max_total_usd,
            jobs: 1,
            judge_file: None,
            trials_override: Some(1),
        }
    }

    /// target-matrix 4.2: looping `run_over_targets` over two `--target`
    /// values produces two reports, one per target, in target order.
    #[test]
    fn run_over_targets_produces_one_report_per_target() {
        let sc_dir = scenario_dir("count");
        let scenarios = zseval::scenario::discover(&sc_dir).unwrap();
        let fixture = mock_fixture("count", 0.01);
        let targets = vec![target_file("count-a"), target_file("count-b")];
        let cfg = base_cfg("count", None);

        let reports = run_over_targets(
            &scenarios,
            &targets,
            |_t| {
                Box::new(Mock {
                    fixture: fixture.clone(),
                }) as Box<dyn AgentBackend>
            },
            &NoJudgeConfigured,
            &cfg,
        )
        .unwrap();

        assert_eq!(reports.len(), 2);
        assert_eq!(reports[0].summary.n_scenarios, 1);
        assert_eq!(reports[1].summary.n_scenarios, 1);

        std::fs::remove_dir_all(&sc_dir).ok();
        std::fs::remove_dir_all(fixture.parent().unwrap()).ok();
        std::fs::remove_dir_all(&cfg.results_root).ok();
        for t in &targets {
            std::fs::remove_dir_all(t.parent().unwrap()).ok();
        }
    }

    /// target-matrix 4.3: `--max-total-usd` is one shared total across
    /// targets, not a cap handed to each target independently. Target 1
    /// alone spends past the whole budget; target 2's shrunk cap
    /// (`total - spent`, clamped to 0) then shuts it out before its own
    /// first scenario — which an independent per-target cap of the same
    /// size would *not* do, since the per-scenario check only ever compares
    /// against what that one run has spent so far (target 2 would start at
    /// 0 < 5 and run its scenario too).
    #[test]
    fn budget_is_shared_across_targets_not_per_target() {
        let sc_dir = scenario_dir("budget");
        let scenarios = zseval::scenario::discover(&sc_dir).unwrap();
        let fixture = mock_fixture("budget", 6.0);
        let targets = vec![target_file("budget-a"), target_file("budget-b")];
        let cfg = base_cfg("budget", Some(5.0));

        let reports = run_over_targets(
            &scenarios,
            &targets,
            |_t| {
                Box::new(Mock {
                    fixture: fixture.clone(),
                }) as Box<dyn AgentBackend>
            },
            &NoJudgeConfigured,
            &cfg,
        )
        .unwrap();

        assert_eq!(reports.len(), 2);
        assert_eq!(
            reports[0].summary.n_scenarios, 1,
            "target 1 runs and spends the shared budget"
        );
        assert_eq!(
            reports[1].summary.n_scenarios, 0,
            "target 2 is shut out: its shrunk cap is 0 once target 1 exhausted the shared \
             total, not a fresh independent $5 cap"
        );

        std::fs::remove_dir_all(&sc_dir).ok();
        std::fs::remove_dir_all(fixture.parent().unwrap()).ok();
        std::fs::remove_dir_all(&cfg.results_root).ok();
        for t in &targets {
            std::fs::remove_dir_all(t.parent().unwrap()).ok();
        }
    }

    /// target-matrix 4.4: the exit code `cmd_run` reports across N targets is
    /// the most severe of the N reports' own `exit_code()` — here, one
    /// target's trial fails its deterministic assert (1) while the other
    /// passes (0), and the aggregate must surface the 1, never silently
    /// average or ignore it.
    #[test]
    fn aggregate_exit_code_is_1_when_any_target_has_a_failing_trial() {
        let sc_dir = scenario_dir("aggregate-fail");
        let scenarios = zseval::scenario::discover(&sc_dir).unwrap();
        let target_a = target_file("aggregate-fail-a");
        let target_b = target_file("aggregate-fail-b");
        let targets = vec![target_a.clone(), target_b.clone()];
        let passing_fixture = mock_fixture("aggregate-fail-pass", 0.01);
        // Does not contain "done", so `final_contains done` fails the assert.
        let failing_fixture = mock_fixture_with_message("aggregate-fail-fail", 0.01, "nope");
        let cfg = base_cfg("aggregate-fail", None);

        let reports = run_over_targets(
            &scenarios,
            &targets,
            |t| {
                let fixture = if t == target_a {
                    passing_fixture.clone()
                } else {
                    failing_fixture.clone()
                };
                Box::new(Mock { fixture }) as Box<dyn AgentBackend>
            },
            &NoJudgeConfigured,
            &cfg,
        )
        .unwrap();

        assert_eq!(reports[0].exit_code(), 0, "target a's trial passed");
        assert_eq!(reports[1].exit_code(), 1, "target b's trial failed");
        let aggregate = reports.iter().map(Report::exit_code).max().unwrap_or(0);
        assert_eq!(
            aggregate, 1,
            "the failing column must win, not be averaged away"
        );

        std::fs::remove_dir_all(&sc_dir).ok();
        std::fs::remove_dir_all(passing_fixture.parent().unwrap()).ok();
        std::fs::remove_dir_all(failing_fixture.parent().unwrap()).ok();
        std::fs::remove_dir_all(&cfg.results_root).ok();
        for t in &targets {
            std::fs::remove_dir_all(t.parent().unwrap()).ok();
        }
    }

    /// target-matrix 4.4: a target whose backend can't even produce a
    /// gradable transcript (here, a `Mock` fixture pointed at a file that
    /// does not exist — `backend.run` errors, which `run_trial` grades
    /// Indeterminate, never Fail) leaves that whole column with nothing
    /// gradable. The aggregate exit code must escalate to 2 (harness error),
    /// outranking a same-run target that passed cleanly.
    #[test]
    fn aggregate_exit_code_is_2_when_any_target_is_fully_ungradable() {
        let sc_dir = scenario_dir("aggregate-ungradable");
        let scenarios = zseval::scenario::discover(&sc_dir).unwrap();
        let target_a = target_file("aggregate-ungradable-a");
        let target_b = target_file("aggregate-ungradable-b");
        let targets = vec![target_a.clone(), target_b.clone()];
        let passing_fixture = mock_fixture("aggregate-ungradable-pass", 0.01);
        let missing_fixture = std::env::temp_dir().join(format!(
            "zseval-multitarget-missing-fixture-{}.json",
            std::process::id()
        ));
        let cfg = base_cfg("aggregate-ungradable", None);

        let reports = run_over_targets(
            &scenarios,
            &targets,
            |t| {
                let fixture = if t == target_a {
                    passing_fixture.clone()
                } else {
                    missing_fixture.clone()
                };
                Box::new(Mock { fixture }) as Box<dyn AgentBackend>
            },
            &NoJudgeConfigured,
            &cfg,
        )
        .unwrap();

        assert_eq!(reports[0].exit_code(), 0, "target a's trial passed");
        assert_eq!(
            reports[1].exit_code(),
            2,
            "target b's backend errored on every trial: nothing gradable"
        );
        let aggregate = reports.iter().map(Report::exit_code).max().unwrap_or(0);
        assert_eq!(
            aggregate, 2,
            "the fully-ungradable column must win, over a's clean 0"
        );

        std::fs::remove_dir_all(&sc_dir).ok();
        std::fs::remove_dir_all(passing_fixture.parent().unwrap()).ok();
        std::fs::remove_dir_all(&cfg.results_root).ok();
        for t in &targets {
            std::fs::remove_dir_all(t.parent().unwrap()).ok();
        }
    }

    /// target-matrix 8.1/8.2: an N>1 run's end-of-run table lands on the
    /// `err` writer (stderr in production) built by the same renderer
    /// `matrix` uses (`matrix::build` + `render_fixed_width`), never on
    /// stdout — `print_run_report_summaries` only ever receives one writer,
    /// so it has no way to reach a separate stdout buffer at all.
    #[test]
    fn multi_target_summary_renders_the_table_on_err_only() {
        let sc_dir = scenario_dir("table");
        let scenarios = zseval::scenario::discover(&sc_dir).unwrap();
        let fixture = mock_fixture("table", 0.01);
        let targets = vec![target_file("table-a"), target_file("table-b")];
        let cfg = base_cfg("table", None);

        let reports = run_over_targets(
            &scenarios,
            &targets,
            |_t| {
                Box::new(Mock {
                    fixture: fixture.clone(),
                }) as Box<dyn AgentBackend>
            },
            &NoJudgeConfigured,
            &cfg,
        )
        .unwrap();

        let mut err = Vec::new();
        print_run_report_summaries(&reports, &targets, true, &cfg, &mut err).unwrap();
        let err_text = String::from_utf8(err).unwrap();

        // "legend:" and the SPREAD/DRIFT caveat are markers only
        // `matrix::render_fixed_width` emits — proof the same renderer
        // `matrix` uses ran here, not just the per-report summary lines
        // (which also happen to mention the stems in their report: paths).
        assert!(err_text.contains("legend:"), "err: {err_text}");
        assert!(
            err_text.contains("SPREAD, DRIFT, and MULTI-VAR are display heuristics"),
            "err: {err_text}"
        );
        assert!(err_text.contains("table-a"), "err: {err_text}");
        assert!(err_text.contains("table-b"), "err: {err_text}");

        std::fs::remove_dir_all(&sc_dir).ok();
        std::fs::remove_dir_all(fixture.parent().unwrap()).ok();
        std::fs::remove_dir_all(&cfg.results_root).ok();
        for t in &targets {
            std::fs::remove_dir_all(t.parent().unwrap()).ok();
        }
    }
}

#[cfg(test)]
mod run_summary_tests {
    use super::*;
    use zseval::scenario::Kind;
    use zseval::verdict::{Final, Report, ReportMeta, ScenarioResult, TrialResult};

    fn trial(outcome: Final) -> TrialResult {
        TrialResult {
            trial: 0,
            outcome,
            reasons: vec![],
            asserts: vec![],
            judge: None,
            judge_file: String::new(),
            judge_hash: None,
            judge_model: None,
            input_tokens: 0,
            output_tokens: 0,
            judge_input_tokens: 0,
            judge_output_tokens: 0,
            cost_usd: 0.0,
            wall_secs: 0.0,
            tool_call_count: 0,
            run_dir: String::new(),
        }
    }

    fn scenario(id: &str, kind: Kind, trials: Vec<TrialResult>) -> ScenarioResult {
        let mut sr = ScenarioResult::from_trials(id.into(), trials);
        sr.kind = kind;
        sr
    }

    fn report(scenarios: Vec<ScenarioResult>) -> Report {
        Report::build(
            ReportMeta {
                tag: "t".into(),
                model: "m".into(),
                backend: "mock".into(),
                trials: 1,
                ..Default::default()
            },
            scenarios,
        )
    }

    fn cfg(name: &str) -> MultiTargetConfig {
        MultiTargetConfig {
            tag: "t".into(),
            no_judge: true,
            results_root: std::env::temp_dir().join(format!(
                "zseval-run-summary-results-{name}-{}",
                std::process::id()
            )),
            max_total_usd: None,
            jobs: 1,
            judge_file: None,
            trials_override: None,
        }
    }

    /// trustworthy-numbers 5.3: the human run summary prints three lines, in
    /// order: regression, capability, then the historical blended overall —
    /// each carrying that line's own scenario/gradable counts and rates.
    #[test]
    fn run_summary_prints_regression_capability_overall_in_order() {
        let r = report(vec![
            scenario("reg-1", Kind::Regression, vec![trial(Final::Pass)]),
            scenario("cap-1", Kind::Capability, vec![trial(Final::Pass)]),
        ]);

        let mut err = Vec::new();
        print_run_report_summaries(&[r], &[], false, &cfg("order"), &mut err).unwrap();
        let text = String::from_utf8(err).unwrap();

        let reg_pos = text.find("regression:").unwrap_or_else(|| {
            panic!("no regression: line in {text}");
        });
        let cap_pos = text.find("capability:").unwrap_or_else(|| {
            panic!("no capability: line in {text}");
        });
        let overall_pos = text.find("overall:").unwrap_or_else(|| {
            panic!("no overall: line in {text}");
        });
        assert!(reg_pos < cap_pos, "{text}");
        assert!(cap_pos < overall_pos, "{text}");
        assert!(text.contains("1 scenarios (1 gradable)"), "{text}");
    }

    /// trustworthy-numbers 5.3 / D5: a kind with nothing gradable renders its
    /// rates as `n/a` on that kind's own line — the existing `rate()`
    /// convention, now applied per line instead of only to the overall one.
    #[test]
    fn an_empty_kinds_line_renders_n_a() {
        let r = report(vec![scenario(
            "reg-1",
            Kind::Regression,
            vec![trial(Final::Pass)],
        )]);

        let mut err = Vec::new();
        print_run_report_summaries(&[r], &[], false, &cfg("empty-kind"), &mut err).unwrap();
        let text = String::from_utf8(err).unwrap();

        let cap_line = text
            .lines()
            .find(|l| l.starts_with("capability:"))
            .unwrap_or_else(|| panic!("no capability: line in {text}"));
        assert!(cap_line.contains("n/a"), "{cap_line}");
    }
}

#[cfg(test)]
mod matrix_cmd_tests {
    use super::*;
    use zseval::verdict::{Final, Report, ReportMeta, ScenarioResult, TrialResult};

    fn trial(outcome: Final) -> TrialResult {
        TrialResult {
            trial: 0,
            outcome,
            reasons: vec![],
            asserts: vec![],
            judge: None,
            judge_file: String::new(),
            judge_hash: None,
            judge_model: None,
            input_tokens: 0,
            output_tokens: 0,
            judge_input_tokens: 0,
            judge_output_tokens: 0,
            cost_usd: 0.01,
            wall_secs: 0.0,
            tool_call_count: 0,
            run_dir: String::new(),
        }
    }

    fn report(target: &str, tag: &str, scenarios: Vec<ScenarioResult>) -> Report {
        Report::build(
            ReportMeta {
                tag: tag.into(),
                model: format!("anthropic/{tag}"),
                backend: "zs".into(),
                trials: 1,
                target: target.into(),
                ..Default::default()
            },
            scenarios,
        )
    }

    /// A tempdir per test, holding one `report.json` per (name, report) pair
    /// written to disk — `cmd_matrix` reads real files by path, so tests
    /// need on-disk fixtures rather than in-memory `Report`s.
    struct Fixtures {
        dir: PathBuf,
    }

    impl Fixtures {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "zseval-matrix-cmd-test-{name}-{}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Fixtures { dir }
        }

        fn write(&self, name: &str, r: &Report) -> String {
            let path = self.dir.join(format!("{name}.json"));
            std::fs::write(&path, serde_json::to_string_pretty(r).unwrap()).unwrap();
            path.display().to_string()
        }
    }

    impl Drop for Fixtures {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.dir).ok();
        }
    }

    // 7.1 — matrix over report files renders and creates no files.
    #[test]
    fn matrix_renders_and_creates_no_files() {
        let fx = Fixtures::new("no-side-effects");
        let a = report(
            "targets/opus.toml",
            "run-a",
            vec![ScenarioResult::from_trials(
                "s".into(),
                vec![trial(Final::Pass)],
            )],
        );
        let path_a = fx.write("a", &a);

        let before: Vec<_> = std::fs::read_dir(&fx.dir).unwrap().collect();
        let code = cmd_matrix(vec![path_a]).unwrap();
        let after: Vec<_> = std::fs::read_dir(&fx.dir).unwrap().collect();

        assert_eq!(code, ExitCode::from(0));
        assert_eq!(before.len(), after.len(), "matrix must write no files");
    }

    // 7.1 — a report with no target identity exits 2 naming it.
    #[test]
    fn targetless_report_exits_2_naming_the_file() {
        let fx = Fixtures::new("targetless");
        let a = report(
            "",
            "run-a",
            vec![ScenarioResult::from_trials(
                "s".into(),
                vec![trial(Final::Pass)],
            )],
        );
        let path_a = fx.write("a", &a);

        let err = cmd_matrix(vec![path_a.clone()]).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains(&path_a), "{msg}");
    }

    // 7.1 — a report sharing no scenario id exits 2 naming it.
    #[test]
    fn zero_overlap_report_exits_2_naming_the_file() {
        let fx = Fixtures::new("zero-overlap");
        let a = report(
            "targets/opus.toml",
            "run-a",
            vec![ScenarioResult::from_trials(
                "apple".into(),
                vec![trial(Final::Pass)],
            )],
        );
        let b = report(
            "targets/sonnet.toml",
            "run-b",
            vec![ScenarioResult::from_trials(
                "mango".into(),
                vec![trial(Final::Pass)],
            )],
        );
        let path_a = fx.write("a", &a);
        let path_b = fx.write("b", &b);

        let err = cmd_matrix(vec![path_a.clone(), path_b]).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains(&path_a), "{msg}");
    }

    // 7.1 — partial overlap renders holes without erroring.
    #[test]
    fn partial_overlap_renders_holes_without_erroring() {
        let fx = Fixtures::new("partial-overlap");
        let a = report(
            "targets/opus.toml",
            "run-a",
            vec![
                ScenarioResult::from_trials("shared".into(), vec![trial(Final::Pass)]),
                ScenarioResult::from_trials("only-a".into(), vec![trial(Final::Pass)]),
            ],
        );
        let b = report(
            "targets/sonnet.toml",
            "run-b",
            vec![ScenarioResult::from_trials(
                "shared".into(),
                vec![trial(Final::Pass)],
            )],
        );
        let path_a = fx.write("a", &a);
        let path_b = fx.write("b", &b);

        let code = cmd_matrix(vec![path_a, path_b]).unwrap();
        assert_eq!(code, ExitCode::from(0));
    }

    // 7.5 — a low-scoring but rendered table still exits 0.
    #[test]
    fn low_scoring_table_still_exits_0() {
        let fx = Fixtures::new("low-scoring");
        let a = report(
            "targets/opus.toml",
            "run-a",
            vec![ScenarioResult::from_trials(
                "s".into(),
                vec![trial(Final::Fail), trial(Final::Fail)],
            )],
        );
        let path_a = fx.write("a", &a);

        let code = cmd_matrix(vec![path_a]).unwrap();
        assert_eq!(code, ExitCode::from(0));
    }

    // 7.5 — a fully-ungradable column exits 2.
    #[test]
    fn fully_ungradable_column_exits_2() {
        let fx = Fixtures::new("fully-ungradable");
        let a = report(
            "targets/opus.toml",
            "run-a",
            vec![ScenarioResult::from_trials(
                "s".into(),
                vec![trial(Final::Indeterminate)],
            )],
        );
        let path_a = fx.write("a", &a);

        let code = cmd_matrix(vec![path_a]).unwrap();
        assert_eq!(code, ExitCode::from(2));
    }
}
