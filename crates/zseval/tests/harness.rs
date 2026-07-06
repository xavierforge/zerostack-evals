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
fn domain_dispatch_is_the_single_entry_point() {
    // "Which domains exist" must live in domains/mod.rs only: the core calls
    // domains::{validate, expand, verify} and never names a specific
    // subsystem. This exercises all three dispatch functions on a
    // [seed.memory] scenario and on a domain-free scenario.
    let sc_dir = std::env::temp_dir().join(format!("zseval-test-dispatch-{}", std::process::id()));
    let fixtures = sc_dir.join("_fixtures");
    std::fs::create_dir_all(&fixtures).unwrap();
    std::fs::write(
        sc_dir.join("scenario.toml"),
        r#"
id = "dispatch-test"
task = "hello"
expect = ["final_contains x"]

[seed.memory]
long_term = "_fixtures/MEMORY.md"
"#,
    )
    .unwrap();
    std::fs::write(fixtures.join("MEMORY.md"), "prefers tabs").unwrap();
    let sc = Scenario::load(&sc_dir).unwrap();

    // validate: happy path is Ok (the fail-fast path is covered by
    // load_fails_fast_on_missing_memory_note_fixture).
    zseval::domains::validate(&sc).unwrap();

    // expand: memory sugar becomes generic placements rooted at config:.
    let run_dir =
        std::env::temp_dir().join(format!("zseval-test-dispatchrun-{}", std::process::id()));
    let (data, config, work) = (
        run_dir.join("data"),
        run_dir.join("config"),
        run_dir.join("work"),
    );
    for d in [&data, &config, &work] {
        std::fs::create_dir_all(d).unwrap();
    }
    let ctx = RunRoots {
        data: &data,
        config: &config,
        work: &work,
    };
    let placements = zseval::domains::expand(&sc, &ctx).unwrap();
    assert_eq!(placements.len(), 1);
    assert_eq!(placements[0].dest, config.join("agent/memory/MEMORY.md"));

    // verify: dispatches to the memory drift check only when the scenario
    // seeds memory; a domain-free scenario is always Ok.
    let roots = ctx;
    let missing = run_dir.join("turn-0.zslog"); // zslog without a memory-open line
    std::fs::write(&missing, "no memory line here\n").unwrap();
    let err = zseval::domains::verify(&sc, &roots, std::slice::from_ref(&missing)).unwrap_err();
    assert!(err.contains("--features memory"), "{err}");

    let mut no_domain = sc.clone();
    no_domain.seed.memory = None;
    assert!(zseval::domains::verify(&no_domain, &roots, &[missing]).is_ok());

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
fn scenario_load_stores_parsed_asserts() {
    // The "every expect line is valid" invariant is enforced by the type,
    // not by convention: load parses once and stores the Asserts, so the
    // runner never re-parses (and never needs a "validated at load" panic).
    let sc_dir = std::env::temp_dir().join(format!("zseval-test-parsed-{}", std::process::id()));
    std::fs::create_dir_all(&sc_dir).unwrap();
    std::fs::write(
        sc_dir.join("scenario.toml"),
        r#"
id = "parsed-asserts"
task = "hello"
expect = ["tool_not_called write", "final_max_lines 4"]
"#,
    )
    .unwrap();
    let sc = Scenario::load(&sc_dir).unwrap();
    assert_eq!(sc.asserts.len(), sc.expect.len());
    assert_eq!(sc.asserts[0], Assert::ToolNotCalled("write".into()));
    assert_eq!(sc.asserts[1], Assert::FinalMaxLines(4));
    std::fs::remove_dir_all(&sc_dir).ok();
}

#[test]
fn scenario_content_hash_changes_with_the_file_and_is_stable_otherwise() {
    // `compare` uses this to warn when a scenario's own definition changed
    // between a baseline and a candidate run (AGENTS.md's "don't move the
    // ruler" guardrail, made machine-checkable).
    let sc_dir = std::env::temp_dir().join(format!("zseval-test-hash-{}", std::process::id()));
    std::fs::create_dir_all(&sc_dir).unwrap();
    let toml = |task: &str| {
        format!("id = \"hash-test\"\ntask = \"{task}\"\nexpect = [\"final_contains x\"]\n")
    };
    std::fs::write(sc_dir.join("scenario.toml"), toml("hello")).unwrap();
    let a = Scenario::load(&sc_dir).unwrap();
    let a2 = Scenario::load(&sc_dir).unwrap();
    assert!(!a.content_hash.is_empty());
    assert_eq!(
        a.content_hash, a2.content_hash,
        "reloading unchanged file must hash the same"
    );

    std::fs::write(sc_dir.join("scenario.toml"), toml("goodbye")).unwrap();
    let b = Scenario::load(&sc_dir).unwrap();
    assert_ne!(a.content_hash, b.content_hash);

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
        target: None,
        trials_override: Some(2),
        tag: "e2e".into(),
        no_judge: true, // deterministic floor only; judge is covered manually
        results_root: results_root.clone(),
        max_total_usd: None,
        jobs: 1,
    };
    let report = run_suite(vec![sc], &backend, &zseval::judge::LlmJudge, &opts).unwrap();

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
fn jobs_greater_than_one_grades_every_trial_and_keeps_them_in_order() {
    // Trials run on worker threads under --jobs finish in whatever order the
    // OS schedules them; run_trials_for_scenario must still hand back
    // exactly `trials` results, one per index, sorted — same shape a caller
    // would get from the sequential (jobs=1) path, just faster wall-clock.
    let sc_dir = scenarios_root().join("prompts/ask-readonly");
    let sc = Scenario::load(&sc_dir).unwrap();
    let results_root =
        std::env::temp_dir().join(format!("zseval-jobs-test-{}", std::process::id()));

    let backend = Mock {
        fixture: fixture("session-ask-readonly.json"),
    };
    let opts = RunOptions {
        model: None,
        target: None,
        trials_override: Some(6),
        tag: "jobs".into(),
        no_judge: true,
        results_root: results_root.clone(),
        max_total_usd: None,
        jobs: 3,
    };
    let report = run_suite(vec![sc], &backend, &zseval::judge::LlmJudge, &opts).unwrap();

    let s = &report.scenarios[0];
    assert_eq!(s.trials.len(), 6);
    assert_eq!(
        s.trials.iter().map(|t| t.trial).collect::<Vec<_>>(),
        vec![0, 1, 2, 3, 4, 5],
        "trials must come back sorted by index regardless of thread completion order"
    );
    assert!(s.trials.iter().all(|t| t.outcome == Final::Pass), "{s:?}");
    // Every trial got its own isolated run_dir with a persisted trial.json —
    // the concurrent path must not let two workers collide on one directory.
    for trial in &s.trials {
        assert!(
            Path::new(&trial.run_dir).join("trial.json").is_file(),
            "{}",
            trial.run_dir
        );
    }

    std::fs::remove_dir_all(&results_root).ok();
}

/// A test double at the `Judge` seam: fixed verdict, error, or "no key".
enum TestJudge {
    Verdict(zseval::judge::JudgeVerdict),
    Error,
    Unavailable,
}

impl zseval::judge::Judge for TestJudge {
    fn available(&self) -> bool {
        !matches!(self, TestJudge::Unavailable)
    }
    fn judge(
        &self,
        _rubric: &str,
        _evidence: &str,
        _run_dir: &Path,
    ) -> anyhow::Result<zseval::judge::JudgeOutcome> {
        match self {
            TestJudge::Verdict(v) => Ok(zseval::judge::JudgeOutcome {
                verdict: *v,
                input_tokens: 0,
                output_tokens: 0,
                cost_usd: 0.0,
            }),
            _ => anyhow::bail!("judge transport exploded"),
        }
    }
}

#[test]
fn judge_verdicts_map_to_final_outcomes() {
    // The judge's four failure/verdict paths each map to the right Final:
    // No -> Fail, Unknown -> Indeterminate, transport error -> Indeterminate,
    // unavailable (no key) -> Indeterminate. Only reachable through the Judge
    // seam — the real LlmJudge needs a key and a network.
    use zseval::judge::JudgeVerdict;

    let sc_dir = std::env::temp_dir().join(format!("zseval-test-judge-{}", std::process::id()));
    std::fs::create_dir_all(&sc_dir).unwrap();
    std::fs::write(
        sc_dir.join("scenario.toml"),
        r#"
id = "judge-mapping"
task = "hello"
judge = "Did the agent answer the question?"
"#,
    )
    .unwrap();

    let run = |judge: &dyn zseval::judge::Judge, no_judge: bool| {
        let sc = Scenario::load(&sc_dir).unwrap();
        let results_root = std::env::temp_dir().join(format!(
            "zseval-judge-run-{}-{no_judge}-{:p}",
            std::process::id(),
            judge
        ));
        let backend = Mock {
            fixture: fixture("session-ask-readonly.json"),
        };
        let opts = RunOptions {
            model: None,
            target: None,
            trials_override: Some(1),
            tag: "j".into(),
            no_judge,
            results_root: results_root.clone(),
            max_total_usd: None,
        jobs: 1,
        };
        let report = run_suite(vec![sc], &backend, judge, &opts).unwrap();
        std::fs::remove_dir_all(&results_root).ok();
        report.scenarios[0].trials[0].clone()
    };

    let t = run(&TestJudge::Verdict(JudgeVerdict::Yes), false);
    assert_eq!(t.outcome, Final::Pass, "{:?}", t.reasons);

    let t = run(&TestJudge::Verdict(JudgeVerdict::No), false);
    assert_eq!(t.outcome, Final::Fail);
    assert!(
        t.reasons.iter().any(|r| r.contains("judge: No")),
        "{:?}",
        t.reasons
    );

    let t = run(&TestJudge::Verdict(JudgeVerdict::Unknown), false);
    assert_eq!(t.outcome, Final::Indeterminate);

    let t = run(&TestJudge::Error, false);
    assert_eq!(t.outcome, Final::Indeterminate);
    assert!(
        t.reasons.iter().any(|r| r.contains("judge error")),
        "{:?}",
        t.reasons
    );

    let t = run(&TestJudge::Unavailable, false);
    assert_eq!(t.outcome, Final::Indeterminate);
    assert!(
        t.reasons.iter().any(|r| r.contains("not available")),
        "{:?}",
        t.reasons
    );

    // --no-judge skips even an unavailable judge: deterministic floor only.
    let t = run(&TestJudge::Unavailable, true);
    assert_eq!(t.outcome, Final::Pass, "{:?}", t.reasons);
    assert!(
        t.reasons.iter().any(|r| r.contains("judge skipped")),
        "{:?}",
        t.reasons
    );

    std::fs::remove_dir_all(&sc_dir).ok();
}

/// A backend that writes a partial session (as if some turns completed and
/// spent real money) before failing — simulating a timeout mid-run.
struct PartialFailureBackend {
    partial_cost: f64,
}

impl zseval::backend::AgentBackend for PartialFailureBackend {
    fn name(&self) -> &str {
        "partial-failure"
    }
    fn run(
        &self,
        _sc: &Scenario,
        _model: Option<&str>,
        run_dir: &Path,
    ) -> anyhow::Result<zseval::backend::RunArtifacts> {
        let sessions = run_dir.join("data").join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(
            sessions.join("partial.json"),
            format!(
                r#"{{"id":"p","messages":[],"total_cost":{}}}"#,
                self.partial_cost
            ),
        )
        .unwrap();
        anyhow::bail!("simulated timeout after turn 1")
    }
}

#[test]
fn indeterminate_trial_recovers_cost_already_spent_before_the_failure() {
    // A timeout after turn 1 of 3 already burned real API cost — an
    // indeterminate trial that reports $0 spent would let --max-total-usd
    // silently undercount, letting a run blow through the budget the caller
    // set. Recovery is best-effort: it scans whatever session JSON exists
    // under the run_dir even though the backend itself errored.
    let sc_dir = std::env::temp_dir().join(format!("zseval-costrecover-sc-{}", std::process::id()));
    std::fs::create_dir_all(&sc_dir).unwrap();
    std::fs::write(
        sc_dir.join("scenario.toml"),
        "id = \"cost-recover-test\"\ntask = \"hi\"\nexpect = [\"final_contains x\"]\n",
    )
    .unwrap();
    let sc = Scenario::load(&sc_dir).unwrap();

    let results_root =
        std::env::temp_dir().join(format!("zseval-costrecover-results-{}", std::process::id()));
    let backend = PartialFailureBackend {
        partial_cost: 0.0275,
    };
    let opts = RunOptions {
        model: None,
        target: None,
        trials_override: Some(1),
        tag: "t".into(),
        no_judge: true,
        results_root: results_root.clone(),
        max_total_usd: None,
        jobs: 1,
    };
    let report = run_suite(vec![sc], &backend, &zseval::judge::LlmJudge, &opts).unwrap();
    let tr = &report.scenarios[0].trials[0];
    assert_eq!(tr.outcome, Final::Indeterminate);
    assert!(
        (tr.cost_usd - 0.0275).abs() < 1e-9,
        "expected recovered cost ~0.0275, got {}",
        tr.cost_usd
    );

    std::fs::remove_dir_all(&sc_dir).ok();
    std::fs::remove_dir_all(&results_root).ok();
}

#[test]
fn regrade_regrades_existing_artifacts_without_driving_the_agent() {
    // The whole point of `regrade`: after editing an assert to be stricter,
    // re-grading a previously captured trial dir must reflect the new
    // assert against the *same*, unchanged evidence — no backend involved.
    let sc_dir = std::env::temp_dir().join(format!("zseval-regrade-sc-{}", std::process::id()));
    std::fs::create_dir_all(&sc_dir).unwrap();
    let write_scenario = |expect: &str| {
        std::fs::write(
            sc_dir.join("scenario.toml"),
            format!("id = \"regrade-test\"\ntask = \"hi\"\nexpect = [\"{expect}\"]\n"),
        )
        .unwrap();
    };
    write_scenario("tool_not_called write");
    let sc = Scenario::load(&sc_dir).unwrap();

    let results_root =
        std::env::temp_dir().join(format!("zseval-regrade-results-{}", std::process::id()));
    let backend = Mock {
        fixture: fixture("session-ask-readonly.json"),
    };
    let opts = RunOptions {
        model: None,
        target: None,
        trials_override: Some(1),
        tag: "orig".into(),
        no_judge: true,
        results_root: results_root.clone(),
        max_total_usd: None,
        jobs: 1,
    };
    let report = run_suite(vec![sc], &backend, &zseval::judge::LlmJudge, &opts).unwrap();
    assert_eq!(report.scenarios[0].trials[0].outcome, Final::Pass);
    let run_dir = results_root
        .join("orig")
        .join("regrade-test")
        .join("trial-0");
    assert!(run_dir.join("trial.json").is_file());

    // Tighten the assert to one the frozen artifacts can't satisfy, without
    // touching the artifacts themselves.
    write_scenario("tool_called write");
    let tightened = Scenario::load(&sc_dir).unwrap();

    let tr =
        zseval::runner::regrade(&tightened, &zseval::judge::LlmJudge, true, 0, &run_dir).unwrap();
    assert_eq!(tr.outcome, Final::Fail, "{:?}", tr.reasons);

    // trial.json on disk reflects the regrade, so `explain` sees the update.
    let persisted: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(run_dir.join("trial.json")).unwrap())
            .unwrap();
    assert_eq!(persisted["outcome"], "fail");

    std::fs::remove_dir_all(&sc_dir).ok();
    std::fs::remove_dir_all(&results_root).ok();
}

#[test]
fn regrade_canonicalizes_run_dir_so_memory_drift_check_does_not_false_positive() {
    // `ZsCli::run` canonicalizes run_dir before computing the memory layout
    // root it seeds and before zerostack ever sees ZS_CONFIG_DIR — so a
    // captured zslog's "memory open: root=..." line always records a
    // canonical path. `regrade` re-scores that same run_dir from a fresh
    // process and must canonicalize the same way, or a caller passing a
    // relative/non-canonical path (macOS: /tmp -> /private/tmp; or simply
    // `zseval regrade ... results/tag/.../trial-0/` off the command line)
    // would make an unrelated path-form mismatch look like real drift.
    use zseval::domains::memory::project_slug;

    let run_dir = std::env::temp_dir().join(format!("zseval-regrade-canon-{}", std::process::id()));
    std::fs::create_dir_all(run_dir.join("data/sessions")).unwrap();
    std::fs::create_dir_all(run_dir.join("config")).unwrap();
    std::fs::create_dir_all(run_dir.join("work")).unwrap();

    // What a real ZsCli run would have recorded: root/project computed from
    // the *canonical* form of this run_dir, exactly as `domains::memory`
    // expects (see layout_root/project_slug in domains/memory.rs).
    let canon = std::fs::canonicalize(&run_dir).unwrap();
    let expected_root = canon.join("config").join("agent").join("memory");
    let expected_project = project_slug(&canon.join("work"));
    std::fs::write(
        run_dir.join("turn-0.zslog"),
        format!(
            "2026-01-01T00:00:00Z DEBUG zerostack: memory open: root={}, project={expected_project}\n",
            expected_root.display()
        ),
    )
    .unwrap();
    std::fs::write(
        run_dir.join("data/sessions/session.json"),
        r#"{"id":"s","messages":[{"role":"user","content":"hi"},{"role":"assistant","content":"done"}]}"#,
    )
    .unwrap();

    let sc_dir = std::env::temp_dir().join(format!("zseval-regrade-canon-sc-{}", std::process::id()));
    std::fs::create_dir_all(&sc_dir).unwrap();
    std::fs::write(
        sc_dir.join("scenario.toml"),
        "id = \"regrade-canon-test\"\ntask = \"hi\"\nexpect = [\"final_contains done\"]\n[seed.memory]\n",
    )
    .unwrap();
    let sc = Scenario::load(&sc_dir).unwrap();

    // Passed as-is (not pre-canonicalized by the caller) — regrade must do
    // it internally, the same way `zseval regrade <scenario> <trial-dir>`
    // would be invoked from a shell with a relative or symlinked path.
    let tr = zseval::runner::regrade(&sc, &zseval::judge::LlmJudge, true, 0, &run_dir).unwrap();
    assert_eq!(
        tr.outcome,
        Final::Pass,
        "expected a clean pass, got indeterminate/fail via: {:?}",
        tr.reasons
    );

    std::fs::remove_dir_all(&run_dir).ok();
    std::fs::remove_dir_all(&sc_dir).ok();
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
        judge_input_tokens: 0,
        judge_output_tokens: 0,
        cost_usd: 0.0,
        wall_secs: 0.0,
        tool_call_count: 0,
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
fn compare_warns_when_tool_call_evidence_drops_to_zero() {
    // The exact failure mode that let every tool_called assert pass
    // vacuously for months: a scenario whose pass rate looks unchanged, but
    // whose evidence channel (tool_call_count) silently went to zero.
    use zseval::compare::compare;
    use zseval::verdict::{Report, ScenarioResult, TrialResult};

    let mk = |tool_call_count| TrialResult {
        trial: 0,
        outcome: Final::Pass,
        reasons: vec![],
        asserts: vec![],
        judge: None,
        input_tokens: 0,
        output_tokens: 0,
        judge_input_tokens: 0,
        judge_output_tokens: 0,
        cost_usd: 0.0,
        wall_secs: 0.0,
        tool_call_count,
        run_dir: String::new(),
    };

    let base = Report::build(
        "base".into(),
        "m".into(),
        "b".into(),
        1,
        vec![ScenarioResult::from_trials(
            "memory-search-then-read-when-needed".into(),
            vec![mk(2)],
        )],
    );
    let cand = Report::build(
        "cand".into(),
        "m".into(),
        "b".into(),
        1,
        vec![ScenarioResult::from_trials(
            "memory-search-then-read-when-needed".into(),
            vec![mk(0)],
        )],
    );

    let c = compare(&base, &cand, 0.05);
    assert_eq!(
        c.evidence_warnings,
        vec!["memory-search-then-read-when-needed".to_string()]
    );
    // Pass rate didn't move (both trials Pass), so this must NOT also be
    // reported as a regression — it's a distinct signal.
    assert!(c.regressions.is_empty());

    // A scenario with zero tool calls on both sides (e.g. a pure text-answer
    // scenario) must never false-positive.
    let base2 = Report::build(
        "base".into(),
        "m".into(),
        "b".into(),
        1,
        vec![ScenarioResult::from_trials(
            "prompt-code-concise-answer".into(),
            vec![mk(0)],
        )],
    );
    let cand2 = Report::build(
        "cand".into(),
        "m".into(),
        "b".into(),
        1,
        vec![ScenarioResult::from_trials(
            "prompt-code-concise-answer".into(),
            vec![mk(0)],
        )],
    );
    assert!(compare(&base2, &cand2, 0.05).evidence_warnings.is_empty());
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
