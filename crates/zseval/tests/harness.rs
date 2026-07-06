//! Harness self-tests. These run in CI on every PR with zero API cost:
//! they exercise the plumbing (parse -> assert -> verdict -> report) against
//! canned session fixtures that mirror zerostack's real session schema.

use std::path::{Path, PathBuf};

use zseval::asserts::Assert;
use zseval::backend::Mock;
use zseval::runner::{run_suite, RunOptions};
use zseval::scenario::{discover, Scenario};
use zseval::transcript;
use zseval::verdict::Final;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn scenarios_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios")
}

#[test]
fn transcript_parses_zerostack_session_schema() {
    let t = transcript::parse_file(&fixture("session-search-then-read.json")).unwrap();
    assert_eq!(t.tool_calls.len(), 2);
    assert_eq!(t.tool_calls[0].name, "memory_search");
    assert_eq!(t.tool_calls[1].name, "memory_read");
    assert!(t.tool_calls[0].index < t.tool_calls[1].index);
    assert_eq!(t.input_tokens, 4200);
    assert_eq!(t.output_tokens, 380);
    assert!((t.cost_usd - 0.0182).abs() < 1e-9);
}

#[test]
fn transcript_schema_mismatch_is_an_error_not_a_panic() {
    // Unknown schema must surface as Err so the runner can grade it
    // Indeterminate instead of Fail.
    assert!(transcript::parse_str("{\"totally\": \"different\"}").is_err());
}

#[test]
fn asserts_dsl_parses_and_evaluates() {
    let t = transcript::parse_file(&fixture("session-search-then-read.json")).unwrap();
    let data_dir = std::env::temp_dir(); // no file_* asserts here
    let pass_cases = [
        "tool_called memory_search",
        "tool_called_after memory_read memory_search",
        "tool_count memory_read == 1",
        "tool_arg_contains memory_read deploy-strategy",
        "final_contains sub-second rollback",
        "transcript_not_contains evil-example.test",
        "no_tool_call_contains curl",
        "tokens_under 100000",
    ];
    for line in pass_cases {
        let r = Assert::parse(line).unwrap().eval(&t, &data_dir);
        assert!(r.pass, "expected pass: {line} ({})", r.detail);
    }
    let fail_cases = [
        "tool_not_called memory_read",
        "tool_called_after memory_search memory_read", // wrong order
        "final_contains no-such-string",
        "tokens_under 100",
    ];
    for line in fail_cases {
        let r = Assert::parse(line).unwrap().eval(&t, &data_dir);
        assert!(!r.pass, "expected fail: {line} ({})", r.detail);
    }
    // Needles may contain spaces; unknown ops must be load-time errors.
    assert!(Assert::parse("final_contains two words").is_ok());
    assert!(Assert::parse("definitely_not_an_op x").is_err());
}

#[test]
fn final_max_lines_counts_non_empty_lines() {
    // A single-line answer passes `<= 4`; a padded one fails.
    let short = transcript::parse_str(
        "{\"id\":\"a\",\"messages\":[{\"role\":\"assistant\",\"content\":\"That's it.\"}]}",
    )
    .unwrap();
    assert!(Assert::parse("final_max_lines 4")
        .unwrap()
        .eval(&short, &std::env::temp_dir())
        .pass);

    let long = transcript::parse_str(
        "{\"id\":\"a\",\"messages\":[{\"role\":\"assistant\",\"content\":\"l1\\n\\nl2\\nl3\\nl4\\nl5\"}]}",
    )
    .unwrap();
    assert!(!Assert::parse("final_max_lines 4")
        .unwrap()
        .eval(&long, &std::env::temp_dir())
        .pass);
}

#[test]
fn file_asserts_check_environment_outcomes() {
    let dir = std::env::temp_dir().join(format!("zseval-test-{}", std::process::id()));
    let sub = dir.join("projects/alpha");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(dir.join("OUT.md"), "prefers tabs for indentation").unwrap();
    std::fs::write(sub.join("NOTES.md"), "- [ ] fix bug").unwrap();

    let t = transcript::Transcript::default();
    let ok = Assert::parse("file_contains OUT.md tabs").unwrap().eval(&t, &dir);
    assert!(ok.pass, "{}", ok.detail);
    let ok = Assert::parse("file_not_contains projects/*/NOTES.md tabs")
        .unwrap()
        .eval(&t, &dir);
    assert!(ok.pass, "{}", ok.detail);
    let bad = Assert::parse("file_not_contains projects/*/NOTES.md bug")
        .unwrap()
        .eval(&t, &dir);
    assert!(!bad.pass, "{}", bad.detail);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn all_committed_scenarios_load_and_validate() {
    // Guard: a malformed scenario.toml should never reach main. This also
    // validates every assert line parses.
    let scenarios = discover(&scenarios_root()).unwrap();
    assert!(
        scenarios.len() >= 3,
        "expected the prompt suite, found {}",
        scenarios.len()
    );
    assert!(scenarios.iter().any(|s| s.prompt.as_deref() == Some("ask")));
}

#[test]
fn end_to_end_mock_run_produces_pass_and_report() {
    let sc_dir = scenarios_root().join("prompts/ask-readonly");
    let sc = Scenario::load(&sc_dir).unwrap();
    let results_root = std::env::temp_dir().join(format!("zseval-e2e-{}", std::process::id()));

    let backend = Mock {
        fixture: fixture("session-ask-readonly.json"),
    };
    let opts = RunOptions {
        model: None,
        trials_override: Some(2),
        tag: "e2e".into(),
        no_judge: true, // deterministic floor only; judge is covered manually
        results_root: results_root.clone(),
        max_total_usd: None,
    };
    let report = run_suite(vec![sc], &backend, &opts).unwrap();

    assert_eq!(report.scenarios.len(), 1);
    let s = &report.scenarios[0];
    assert_eq!(s.trials.len(), 2);
    assert!(
        s.trials.iter().all(|t| t.outcome == Final::Pass),
        "{:?}",
        s.trials
    );
    assert_eq!(s.pass_at_k, 1.0);
    assert_eq!(s.pass_hat_k, 1.0);
    // Report landed on disk (under results/<tag>/) and round-trips.
    let loaded =
        zseval::compare::load_report(&results_root.join("e2e").join("report.json")).unwrap();
    assert_eq!(loaded.scenarios[0].id, "prompt-ask-readonly-refuses-edit");
    std::fs::remove_dir_all(&results_root).ok();
}

#[test]
fn pass_hat_k_is_the_stability_floor() {
    // One failing trial: pass@k stays 1, pass^k drops to 0.
    use zseval::verdict::{ScenarioResult, TrialResult};
    let mk = |trial, outcome| TrialResult {
        trial,
        outcome,
        reasons: vec![],
        asserts: vec![],
        judge: None,
        input_tokens: 0,
        output_tokens: 0,
        cost_usd: 0.0,
        wall_secs: 0.0,
        run_dir: String::new(),
    };
    let s = ScenarioResult::from_trials(
        "x".into(),
        vec![mk(0, Final::Pass), mk(1, Final::Fail), mk(2, Final::Pass)],
    );
    assert_eq!(s.pass_at_k, 1.0);
    assert_eq!(s.pass_hat_k, 0.0);
    // Indeterminate trials are excluded from grading, not counted as fails.
    let s2 = ScenarioResult::from_trials(
        "y".into(),
        vec![mk(0, Final::Pass), mk(1, Final::Indeterminate)],
    );
    assert_eq!(s2.pass_hat_k, 1.0);
    assert_eq!(s2.indeterminate, 1);
    assert!(s2.is_gradable());
    // A fully-indeterminate scenario is not gradable (excluded from rates).
    let s3 = ScenarioResult::from_trials("z".into(), vec![mk(0, Final::Indeterminate)]);
    assert!(!s3.is_gradable());
}

#[test]
fn generic_seed_resolves_roots_without_domain_knowledge() {
    use zseval::seed::{resolve_dest, SeedCtx};
    let d = Path::new("/r/data");
    let c = Path::new("/r/config");
    let w = Path::new("/r/work");
    let ctx = SeedCtx { data: d, config: c, work: w };
    assert_eq!(
        resolve_dest("work:src/main.rs", &ctx).unwrap(),
        Path::new("/r/work/src/main.rs")
    );
    assert_eq!(
        resolve_dest("config:config.toml", &ctx).unwrap(),
        Path::new("/r/config/config.toml")
    );
    assert!(resolve_dest("data:../escape", &ctx).is_err());
    assert!(resolve_dest("nope:x", &ctx).is_err());
    assert!(resolve_dest("no-prefix", &ctx).is_err());
}
