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
use zseval::verdict::Final;

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
  zseval run <scenario-path> [--target config.toml] [--model M] [--trials N]
             [--tag T] [--zs-bin PATH] [--backend zs|mock=<session.json>]
             [--no-judge] [--max-total-usd X] [--results DIR] [--json] [--verbose]
  zseval compare <baseline.json> <candidate.json> [--threshold 0.05] [--json]
  zseval explain <trial-dir>
  zseval list [scenarios-root]

  --target is a zerostack config.toml (provider + model) seeded into each run's
  isolated config dir — the reproducible way to pick what you evaluate against.
  Put the API key in an env var (not the file); it is passed through to zerostack.
  --model overrides the target's model for a quick one-off.

ENV:
  ZS_BIN             default path to the zerostack binary
  <PROVIDER>_API_KEY the target provider's key (e.g. ANTHROPIC_API_KEY, OPENROUTER_API_KEY)
  ANTHROPIC_API_KEY  also required for LLM-judge scenarios (skip with --no-judge)

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
}

fn cmd_run(rest: Vec<String>) -> anyhow::Result<ExitCode> {
    let f = parse_flags(
        rest,
        &[
            "model",
            "trials",
            "tag",
            "zs-bin",
            "backend",
            "max-total-usd",
            "results",
            "target",
        ],
        &["no-judge", "json", "verbose"],
    )?;
    let path = f
        .positional
        .first()
        .ok_or_else(|| anyhow::anyhow!("run: missing <scenario-path>"))?;
    let scenarios = discover(Path::new(path))?;
    if scenarios.is_empty() {
        anyhow::bail!("no scenario.toml found under {path}");
    }

    let backend: Box<dyn AgentBackend> = match f.get("backend") {
        Some(b) if b.starts_with("mock=") => Box::new(Mock {
            fixture: PathBuf::from(b.trim_start_matches("mock=")),
        }),
        Some("zs") | None => {
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
        // No default: when unset, zerostack uses its own configured provider +
        // model. Forcing a model here would override the user's setup.
        model: f.get("model").map(String::from),
        trials_override: match f.get("trials") {
            Some(t) => Some(t.parse()?),
            None => None,
        },
        tag: f
            .get("tag")
            .map(String::from)
            .unwrap_or_else(|| auto_tag(path, f.get("target"), f.get("model"))),
        no_judge: f.has("no-judge"),
        results_root: f
            .get("results")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("results")),
        max_total_usd: match f.get("max-total-usd") {
            Some(x) => Some(x.parse()?),
            None => None,
        },
    };

    let report = run_suite(scenarios, backend.as_ref(), &opts)?;

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

    let any_fail = report
        .scenarios
        .iter()
        .any(|s| s.trials.iter().any(|t| t.outcome == Final::Fail));
    Ok(if any_fail {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}

/// Build a human-identifiable run tag: which scenarios, against what
/// provider+model, and when — e.g.
/// `prompts_anthropic-claude-sonnet-4-6_20260706-091936`. Semantic tags like
/// `main` are left to an explicit `--tag`; the auto name is always descriptive.
fn auto_tag(scenario_path: &str, target: Option<&str>, model_override: Option<&str>) -> String {
    let suite = Path::new(scenario_path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("scenarios");
    // Read provider + model out of the target config (a --model flag overrides).
    let (mut provider, mut model) = (None, model_override.map(String::from));
    if let Some(p) = target {
        if let Ok(text) = std::fs::read_to_string(p) {
            #[derive(serde::Deserialize)]
            struct Peek {
                provider: Option<String>,
                model: Option<String>,
            }
            if let Ok(peek) = toml::from_str::<Peek>(&text) {
                provider = peek.provider;
                model = model.or(peek.model);
            }
        }
    }
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
    Ok(if c.regressions.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
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
