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
use zseval::scenario::discover;

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
  zseval run <scenario-path> [--target config.toml] [--trials N]
             [--tag T] [--zs-bin PATH] [--backend zs|mock=<session.json>]
             [--judge judges/opus.toml] [--no-judge] [--max-total-usd X]
             [--results DIR] [--jobs N] [--json] [--verbose]
  zseval compare <baseline.json> <candidate.json> [--threshold 0.05] [--json]
  zseval explain <trial-dir>
  zseval list [scenarios-root]
  zseval regrade <scenario-dir> <trial-dir> [--judge F] [--no-judge] [--json]

  --target is a zerostack config.toml (provider + model) seeded into each run's
  isolated config dir — the reproducible way to pick what you evaluate against.
  Put the API key in an env var (not the file); it is passed through to zerostack.

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
    /// repeatable (design.md: "Repeatable `--target`"), so a caller that
    /// needs all of them (not just the last-wins single value) reaches for
    /// this instead. Not yet called from `cmd_run` — section 4 wires the
    /// multi-target loop over it; kept here now, beside `get`/`count`, so
    /// this section only adds the flag primitive, not the loop.
    #[allow(dead_code)]
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

    let backend: Box<dyn AgentBackend> = match f.get("backend") {
        Some(b) if b.starts_with("mock=") => {
            if f.get("target").is_some() {
                anyhow::bail!(
                    "--target is rejected for --backend mock: mock replays canned artifacts \
                     and never reads a target config.toml"
                );
            }
            Box::new(Mock {
                fixture: PathBuf::from(b.trim_start_matches("mock=")),
            })
        }
        Some("zs") | None => {
            if f.get("target").is_none() {
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
            Box::new(ZsCli {
                bin,
                target: f.get("target").map(PathBuf::from),
            })
        }
        Some(other) => anyhow::bail!("unknown backend '{other}'"),
    };

    zseval::backend::set_verbose(f.has("verbose"));

    let opts = RunOptions {
        target: f.get("target").map(PathBuf::from),
        trials_override: match f.get("trials") {
            Some(t) => Some(t.parse()?),
            None => None,
        },
        tag: f
            .get("tag")
            .map(String::from)
            .unwrap_or_else(|| auto_tag(path, f.get("target"), false)),
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
        // Repeated `--target` (target-matrix section 4) is not yet wired into
        // `cmd_run`'s single-target call here, so this invocation is always
        // N=1 for now.
        multi_target: false,
    };

    let report = run_suite(scenarios, backend.as_ref(), judge.as_ref(), &opts)?;

    if f.has("json") {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        // Rates are undefined when nothing was gradable — show n/a rather than 0.
        let rate = |v: f64| {
            if report.summary.n_gradable == 0 {
                "  n/a".to_string()
            } else {
                format!("{v:.3}")
            }
        };
        eprintln!(
            "\n{} scenarios ({} gradable) | pass@k {} | pass^k {} | \
             indeterminate {} scenario(s), {} trial(s) | ${:.4}",
            report.summary.n_scenarios,
            report.summary.n_gradable,
            rate(report.summary.pass_at_k),
            rate(report.summary.pass_hat_k),
            report.summary.indeterminate_scenarios,
            report.summary.indeterminate_trials,
            report.summary.total_cost_usd,
        );
        eprintln!(
            "report: {}/report.json",
            opts.results_root.join(&opts.tag).display()
        );
    }

    Ok(ExitCode::from(report.exit_code()))
}

/// Build a human-identifiable run tag: which scenarios, against what
/// provider+model, and when — e.g.
/// `prompts_anthropic-claude-sonnet-4-6_20260706-091936`. Semantic tags like
/// `main` are left to an explicit `--tag`; the auto name is always descriptive.
///
/// `multi` is true when this invocation covers more than one `--target`: the
/// tag is then shared by every target (the results layout tells them apart
/// by stem instead — see `RunOptions::multi_target`), so the provider-model
/// segment is dropped here rather than appearing once in the tag and once
/// more as the stem.
fn auto_tag(scenario_path: &str, target: Option<&str>, multi: bool) -> String {
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
    let ts = zseval::util::compact_timestamp();
    if target_group.is_empty() {
        format!("{}_{ts}", sanitize(suite))
    } else {
        format!("{}_{}_{ts}", sanitize(suite), target_group.join("-"))
    }
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

        let single = auto_tag("scenarios", Some(&target_str), false);
        assert!(single.contains("anthropic"), "{single}");
        assert!(single.contains("claude-opus-4-8"), "{single}");

        let multi = auto_tag("scenarios", Some(&target_str), true);
        assert!(!multi.contains("anthropic"), "{multi}");
        assert!(!multi.contains("claude-opus-4-8"), "{multi}");
        assert!(multi.starts_with("scenarios_"), "{multi}");

        std::fs::remove_dir_all(&dir).ok();
    }
}
