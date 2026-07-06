//! Harness self-tests. These run in CI on every PR with zero API cost:
//! they exercise the plumbing (parse -> assert -> verdict -> report) against
//! canned session fixtures that mirror zerostack's real session schema.

use std::path::{Path, PathBuf};

use zseval::asserts::Assert;
use zseval::backend::{Mock, RunRoots};
use zseval::runner::{run_suite, RunOptions};
use zseval::scenario::{discover, Scenario};
use zseval::seed;
use zseval::transcript;
use zseval::verdict::Final;

/// A `RunRoots` where all three roots are the same dir — for tests that don't
/// care about root separation (only about the assert logic itself).
fn flat_roots(dir: &Path) -> RunRoots<'_> {
    RunRoots {
        data: dir,
        config: dir,
        work: dir,
    }
}

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
    let roots = flat_roots(&data_dir);
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
        let r = Assert::parse(line).unwrap().eval(&t, &roots);
        assert!(r.pass, "expected pass: {line} ({})", r.detail);
    }
    let fail_cases = [
        "tool_not_called memory_read",
        "tool_called_after memory_search memory_read", // wrong order
        "final_contains no-such-string",
        "tokens_under 100",
    ];
    for line in fail_cases {
        let r = Assert::parse(line).unwrap().eval(&t, &roots);
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
    let tmp = std::env::temp_dir();
    let roots = flat_roots(&tmp);
    assert!(
        Assert::parse("final_max_lines 4")
            .unwrap()
            .eval(&short, &roots)
            .pass
    );

    let long = transcript::parse_str(
        "{\"id\":\"a\",\"messages\":[{\"role\":\"assistant\",\"content\":\"l1\\n\\nl2\\nl3\\nl4\\nl5\"}]}",
    )
    .unwrap();
    assert!(
        !Assert::parse("final_max_lines 4")
            .unwrap()
            .eval(&long, &roots)
            .pass
    );
}

#[test]
fn file_asserts_check_environment_outcomes() {
    let dir = std::env::temp_dir().join(format!("zseval-test-{}", std::process::id()));
    let sub = dir.join("projects/alpha");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(dir.join("OUT.md"), "prefers tabs for indentation").unwrap();
    std::fs::write(sub.join("NOTES.md"), "- [ ] fix bug").unwrap();

    let t = transcript::Transcript::default();
    let roots = flat_roots(&dir);
    let ok = Assert::parse("file_contains OUT.md tabs")
        .unwrap()
        .eval(&t, &roots);
    assert!(ok.pass, "{}", ok.detail);
    let ok = Assert::parse("file_not_contains projects/*/NOTES.md tabs")
        .unwrap()
        .eval(&t, &roots);
    assert!(ok.pass, "{}", ok.detail);
    let bad = Assert::parse("file_not_contains projects/*/NOTES.md bug")
        .unwrap()
        .eval(&t, &roots);
    assert!(!bad.pass, "{}", bad.detail);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn file_asserts_resolve_data_config_work_prefixes_independently() {
    // Regression guard: a `config:` path must never accidentally resolve
    // against the `data:` (or `work:`) root, and vice versa.
    let base = std::env::temp_dir().join(format!("zseval-test-roots-{}", std::process::id()));
    let data = base.join("data");
    let config = base.join("config");
    let work = base.join("work");
    for d in [&data, &config, &work] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::create_dir_all(config.join("agent/memory")).unwrap();
    std::fs::write(data.join("marker.md"), "in-data").unwrap();
    std::fs::write(config.join("agent/memory/MEMORY.md"), "in-config").unwrap();
    std::fs::write(work.join("hello.py"), "in-work").unwrap();
    let roots = RunRoots {
        data: &data,
        config: &config,
        work: &work,
    };
    let t = transcript::Transcript::default();

    assert!(
        Assert::parse("file_contains marker.md in-data")
            .unwrap()
            .eval(&t, &roots)
            .pass
    );
    assert!(
        !Assert::parse("file_contains marker.md in-config")
            .unwrap()
            .eval(&t, &roots)
            .pass
    );
    assert!(
        Assert::parse("file_contains config:agent/memory/MEMORY.md in-config")
            .unwrap()
            .eval(&t, &roots)
            .pass
    );
    assert!(
        !Assert::parse("file_contains config:agent/memory/MEMORY.md in-data")
            .unwrap()
            .eval(&t, &roots)
            .pass
    );
    assert!(
        Assert::parse("file_contains work:hello.py in-work")
            .unwrap()
            .eval(&t, &roots)
            .pass
    );
    std::fs::remove_dir_all(&base).ok();
}

#[test]
fn memory_seed_sugar_expands_to_config_rooted_placements() {
    // A scenario declaring [seed.memory] should place MEMORY.md and notes
    // under <config>/agent/memory/, scoped by the same project_slug
    // zerostack itself derives from the working directory — never under
    // data:memory/ (the stale layout the old deferred/ design assumed).
    let sc_dir = std::env::temp_dir().join(format!("zseval-test-memseed-{}", std::process::id()));
    let fixtures = sc_dir.join("_fixtures");
    std::fs::create_dir_all(&fixtures).unwrap();
    std::fs::write(
        sc_dir.join("scenario.toml"),
        r#"
id = "memory-seed-test"
task = "hello"
expect = ["final_contains x"]

[seed.memory]
long_term = "_fixtures/MEMORY.md"
notes = [{ name = "deploy-strategy", file = "_fixtures/deploy.md" }]
"#,
    )
    .unwrap();
    std::fs::write(fixtures.join("MEMORY.md"), "prefers tabs").unwrap();
    std::fs::write(fixtures.join("deploy.md"), "blue-green on fly.io").unwrap();

    let sc = Scenario::load(&sc_dir).unwrap();
    assert!(sc.seed.memory.is_some());

    let run_dir = std::env::temp_dir().join(format!("zseval-test-memrun-{}", std::process::id()));
    let data = run_dir.join("data");
    let config = run_dir.join("config");
    let work = run_dir.join("work");
    for d in [&data, &config, &work] {
        std::fs::create_dir_all(d).unwrap();
    }
    let ctx = RunRoots {
        data: &data,
        config: &config,
        work: &work,
    };
    seed::apply(&sc, &ctx).unwrap();

    let expected_project = zseval::domains::memory::project_slug(&work);
    let memory_md = config.join("agent/memory/MEMORY.md");
    let note = config
        .join("agent/memory/projects")
        .join(&expected_project)
        .join("notes/deploy-strategy.md");
    assert!(memory_md.is_file(), "expected {}", memory_md.display());
    assert!(note.is_file(), "expected {}", note.display());
    assert_eq!(std::fs::read_to_string(&memory_md).unwrap(), "prefers tabs");
    assert_eq!(
        std::fs::read_to_string(&note).unwrap(),
        "blue-green on fly.io"
    );

    std::fs::remove_dir_all(&sc_dir).ok();
    std::fs::remove_dir_all(&run_dir).ok();
}

#[test]
fn load_fails_fast_on_missing_files_fixture() {
    // A typo'd [[files]] src should fail at load time (zseval list / the
    // very start of a run), not mid-run as a burned-API-call indeterminate.
    let sc_dir = std::env::temp_dir().join(format!("zseval-test-badfile-{}", std::process::id()));
    std::fs::create_dir_all(&sc_dir).unwrap();
    std::fs::write(
        sc_dir.join("scenario.toml"),
        r#"
id = "bad-files-fixture"
task = "hello"
expect = ["final_contains x"]

[[files]]
src = "_fixtures/does-not-exist.py"
dest = "work:hello.py"
"#,
    )
    .unwrap();
    let err = Scenario::load(&sc_dir).unwrap_err();
    assert!(format!("{err:#}").contains("does-not-exist.py"), "{err:#}");
    std::fs::remove_dir_all(&sc_dir).ok();
}

#[test]
fn load_fails_fast_on_missing_memory_note_fixture() {
    // Same fail-fast guarantee for [seed.memory] note/long_term fixtures.
    let sc_dir = std::env::temp_dir().join(format!("zseval-test-badmem-{}", std::process::id()));
    std::fs::create_dir_all(&sc_dir).unwrap();
    std::fs::write(
        sc_dir.join("scenario.toml"),
        r#"
id = "bad-memory-fixture"
task = "hello"
expect = ["final_contains x"]

[seed.memory]
notes = [{ name = "ghost", file = "_fixtures/ghost.md" }]
"#,
    )
    .unwrap();
    let err = Scenario::load(&sc_dir).unwrap_err();
    assert!(format!("{err:#}").contains("ghost"), "{err:#}");
    std::fs::remove_dir_all(&sc_dir).ok();
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
    use zseval::backend::RunRoots;
    use zseval::seed::resolve_dest;
    let d = Path::new("/r/data");
    let c = Path::new("/r/config");
    let w = Path::new("/r/work");
    let ctx = RunRoots {
        data: d,
        config: c,
        work: w,
    };
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
