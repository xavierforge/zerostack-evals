//! Harness self-tests. These run in CI on every PR with zero API cost:
//! they exercise the plumbing (parse -> assert -> verdict -> report) against
//! canned session fixtures that mirror zerostack's real session schema.

use std::path::{Path, PathBuf};

use zseval::asserts::Assert;
use zseval::backend::{AgentBackend, Mock, RunRoots, ZsCli};
use zseval::judge::{JudgeConfig, JudgeProvider, LlmJudge};
use zseval::prompts::PromptPack;
use zseval::runner::{regrade, run_suite, RunOptions};
use zseval::scenario::{discover, Scenario};
use zseval::seed;
use zseval::transcript;
use zseval::verdict::Final;

/// A valid ruler card for tests that need *some* `LlmJudge` in hand but never
/// actually call out to a network (they either pass `no_judge: true`, or the
/// scenario has no rubric to grade). `JudgeConfig` has no built-in default
/// (see its doc: no committed card, no ruler), so tests build one explicitly.
fn test_judge_cfg() -> JudgeConfig {
    JudgeConfig {
        provider: JudgeProvider::Anthropic,
        model: "claude-sonnet-4-6".into(),
        price_in_usd_per_mtok: 3.0,
        price_out_usd_per_mtok: 15.0,
    }
}

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
fn run_rejects_the_removed_model_flag() {
    // `--model` was a temporary override that lived in no file, so a report's
    // recorded target couldn't reproduce the run. A target file is now the
    // only way to say what is being evaluated: the flag must fail loudly
    // (usage error) rather than be silently accepted or ignored.
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_zseval"))
        .args([
            "run",
            scenarios_root().to_str().unwrap(),
            "--model",
            "claude-opus-4-8",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unknown flag '--model'"),
        "stderr: {stderr}"
    );
}

/// A minimal single-scenario suite with a judge rubric (`expect` empty,
/// `judge` set — `Scenario::load` requires at least one of the two).
fn rubric_scenario_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "zseval-test-rubric-suite-{name}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("scenario.toml"),
        "id = \"rubric-only\"\ntask = \"say hi\"\njudge = \"Did the agent say hi? Answer Yes/No/Unknown.\"\n",
    )
    .unwrap();
    dir
}

/// A minimal single-scenario suite with only a deterministic assert, no
/// rubric at all — a judge decision is never required for this one.
fn no_rubric_scenario_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "zseval-test-no-rubric-suite-{name}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("scenario.toml"),
        "id = \"no-rubric\"\ntask = \"say hi\"\nexpect = [\"tool_not_called write\"]\n",
    )
    .unwrap();
    dir
}

/// judge-selection: a rubric suite with neither `--judge` nor `--no-judge`
/// must fail fast (exit 2) before any trial, naming both flags — see
/// `openspec/changes/judge-provider-card/specs/judge-selection/spec.md`.
/// No `--backend`/`--zs-bin` is given: the gate must fire before backend
/// setup, so this would fail for an unrelated reason if the gate came later.
#[test]
fn run_on_a_rubric_suite_with_neither_judge_flag_exits_2_naming_both_flags() {
    let dir = rubric_scenario_dir("run-neither");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_zseval"))
        .args(["run", dir.to_str().unwrap()])
        .output()
        .unwrap();
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--judge"), "stderr: {stderr}");
    assert!(stderr.contains("--no-judge"), "stderr: {stderr}");
}

/// judge-selection: a suite with no rubric scenarios needs no judge
/// decision at all — `Unspecified` runs normally, same as if `--no-judge`
/// had been passed.
#[test]
fn run_on_a_no_rubric_suite_with_neither_judge_flag_runs_normally() {
    let dir = no_rubric_scenario_dir("run-neither");
    let results = std::env::temp_dir().join(format!(
        "zseval-test-no-rubric-results-{}",
        std::process::id()
    ));
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_zseval"))
        .args([
            "run",
            dir.to_str().unwrap(),
            "--backend",
            &format!("mock={}", fixture("session-ask-readonly.json").display()),
            "--results",
            results.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    std::fs::remove_dir_all(&dir).ok();
    let stderr = String::from_utf8_lossy(&out.stderr);
    std::fs::remove_dir_all(&results).ok();
    assert_ne!(out.status.code(), Some(2), "stderr: {stderr}");
}

/// judge-selection: explicit `--no-judge` on a rubric suite is honored — the
/// run proceeds and the skip is recorded, no judge key required.
#[test]
fn run_on_a_rubric_suite_with_explicit_no_judge_runs_with_skip_recorded() {
    let dir = rubric_scenario_dir("run-explicit-no-judge");
    let results = std::env::temp_dir().join(format!(
        "zseval-test-rubric-no-judge-results-{}",
        std::process::id()
    ));
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_zseval"))
        .args([
            "run",
            dir.to_str().unwrap(),
            "--backend",
            &format!("mock={}", fixture("session-ask-readonly.json").display()),
            "--results",
            results.to_str().unwrap(),
            "--no-judge",
            "--json",
        ])
        .output()
        .unwrap();
    std::fs::remove_dir_all(&dir).ok();
    assert_ne!(out.status.code(), Some(2));
    let stdout = String::from_utf8_lossy(&out.stdout);
    std::fs::remove_dir_all(&results).ok();
    assert!(
        stdout.contains("judge skipped (--no-judge)"),
        "stdout: {stdout}"
    );
}

/// judge-preflight / judge-selection: `regrade` mirrors the same mandatory
/// choice gate for a rubric scenario. The trial dir need not exist: the
/// gate must fire before `regrade` ever reads it.
#[test]
fn regrade_on_a_rubric_scenario_with_neither_judge_flag_exits_2_naming_both_flags() {
    let dir = rubric_scenario_dir("regrade-neither");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_zseval"))
        .args(["regrade", dir.to_str().unwrap(), "/no/such/trial-dir"])
        .output()
        .unwrap();
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--judge"), "stderr: {stderr}");
    assert!(stderr.contains("--no-judge"), "stderr: {stderr}");
}

/// A judge card naming a provider whose key we can deterministically ensure
/// is unset in the child process (`.env_remove`), regardless of what the
/// test-runner's own environment happens to carry.
fn unreachable_judge_card(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "zseval-test-preflight-judge-{name}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("judge.toml");
    std::fs::write(
        &path,
        "provider = \"gemini\"\nmodel = \"gemini-x\"\n\
         price_in_usd_per_mtok = 1.0\nprice_out_usd_per_mtok = 1.0\n",
    )
    .unwrap();
    path
}

/// A local "proxy" that accepts each TCP connection and immediately drops
/// it. Pointing the child's HTTP(S)_PROXY at this makes any outbound
/// request — here the judge's preflight dry-run — fail deterministically
/// offline, without giving the binary a routing override (the rig
/// `base_url` seam is deliberately code-only; see judge-provider-card
/// design.md). The counter proves the dry-run really died at this proxy
/// and not at the real provider endpoint with a garbage key.
fn refusing_proxy() -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let counter = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let thread_counter = counter.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            thread_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            drop(stream);
        }
    });
    (format!("http://{addr}"), counter)
}

/// judge-preflight: a rubric suite's judge whose key is unset must fail
/// preflight (naming `GEMINI_API_KEY`, the card's provider) before any
/// trial runs — checked concretely by asserting the results dir was never
/// created, not merely that the process exited 2.
#[test]
fn run_with_a_judge_whose_key_is_unset_fails_preflight_before_any_trial() {
    let dir = rubric_scenario_dir("preflight-run");
    let judge_path = unreachable_judge_card("run");
    let results = std::env::temp_dir().join(format!(
        "zseval-test-preflight-run-results-{}",
        std::process::id()
    ));

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_zseval"))
        .args([
            "run",
            dir.to_str().unwrap(),
            "--judge",
            judge_path.to_str().unwrap(),
            "--backend",
            &format!("mock={}", fixture("session-ask-readonly.json").display()),
            "--results",
            results.to_str().unwrap(),
        ])
        .env_remove("GEMINI_API_KEY")
        .output()
        .unwrap();

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(judge_path.parent().unwrap()).ok();
    let existed = results.exists();
    std::fs::remove_dir_all(&results).ok();

    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("GEMINI_API_KEY"), "stderr: {stderr}");
    assert!(
        !existed,
        "preflight must fail before any trial creates the results dir"
    );
}

/// judge-preflight: `regrade` runs the same presence check before touching
/// the trial dir. The trial dir doesn't exist at all — if `regrade` reached
/// it before preflight, the error would be about the missing trial dir, not
/// the missing key, so asserting the exact var name in stderr distinguishes
/// "preflight fired first" from "regrade failed for an unrelated reason".
#[test]
fn regrade_with_a_judge_whose_key_is_unset_fails_preflight_before_touching_the_trial() {
    let dir = rubric_scenario_dir("preflight-regrade");
    let judge_path = unreachable_judge_card("regrade");

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_zseval"))
        .args([
            "regrade",
            dir.to_str().unwrap(),
            "/no/such/trial-dir",
            "--judge",
            judge_path.to_str().unwrap(),
        ])
        .env_remove("GEMINI_API_KEY")
        .output()
        .unwrap();

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(judge_path.parent().unwrap()).ok();

    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("GEMINI_API_KEY"), "stderr: {stderr}");
}

/// judge-preflight: the *dry-run* half of preflight, at the CLI boundary.
/// The key is present (a dummy), so the presence check passes; the child's
/// proxy env routes the dry-run into a proxy that drops every connection,
/// so the probe itself fails — exit 2, before any trial creates the
/// results dir. Asserting the proxy saw ≥1 connection proves the failure
/// happened at our proxy, offline, not at the real endpoint.
#[test]
fn run_with_a_judge_whose_dry_run_fails_exits_2_before_any_trial() {
    let dir = rubric_scenario_dir("preflight-dryrun-run");
    let judge_path = unreachable_judge_card("dryrun-run");
    let results = std::env::temp_dir().join(format!(
        "zseval-test-preflight-dryrun-run-results-{}",
        std::process::id()
    ));
    let (proxy_url, counter) = refusing_proxy();

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_zseval"))
        .args([
            "run",
            dir.to_str().unwrap(),
            "--judge",
            judge_path.to_str().unwrap(),
            "--backend",
            &format!("mock={}", fixture("session-ask-readonly.json").display()),
            "--results",
            results.to_str().unwrap(),
        ])
        .env("GEMINI_API_KEY", "zseval-test-dummy-key")
        .env("HTTPS_PROXY", &proxy_url)
        .env("https_proxy", &proxy_url)
        .env("HTTP_PROXY", &proxy_url)
        .env("http_proxy", &proxy_url)
        .env("ALL_PROXY", &proxy_url)
        .env("all_proxy", &proxy_url)
        .env_remove("NO_PROXY")
        .env_remove("no_proxy")
        .output()
        .unwrap();

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(judge_path.parent().unwrap()).ok();
    let existed = results.exists();
    std::fs::remove_dir_all(&results).ok();

    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("could not complete a dry-run"),
        "stderr: {stderr}"
    );
    assert!(!stderr.contains("is not set"), "stderr: {stderr}");
    assert!(
        !existed,
        "preflight must fail before any trial creates the results dir"
    );
    assert!(
        counter.load(std::sync::atomic::Ordering::SeqCst) >= 1,
        "the dry-run must have died at the local proxy, not reached a real endpoint"
    );
}

/// judge-preflight: `regrade` runs the same dry-run probe before touching
/// the trial dir. The trial dir doesn't exist at all, so a dry-run failure
/// (not a missing-trial-dir error) proves preflight fired first.
#[test]
fn regrade_with_a_judge_whose_dry_run_fails_exits_2_before_touching_the_trial() {
    let dir = rubric_scenario_dir("preflight-dryrun-regrade");
    let judge_path = unreachable_judge_card("dryrun-regrade");
    let (proxy_url, counter) = refusing_proxy();

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_zseval"))
        .args([
            "regrade",
            dir.to_str().unwrap(),
            "/no/such/trial-dir",
            "--judge",
            judge_path.to_str().unwrap(),
        ])
        .env("GEMINI_API_KEY", "zseval-test-dummy-key")
        .env("HTTPS_PROXY", &proxy_url)
        .env("https_proxy", &proxy_url)
        .env("HTTP_PROXY", &proxy_url)
        .env("http_proxy", &proxy_url)
        .env("ALL_PROXY", &proxy_url)
        .env("all_proxy", &proxy_url)
        .env_remove("NO_PROXY")
        .env_remove("no_proxy")
        .output()
        .unwrap();

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(judge_path.parent().unwrap()).ok();

    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("could not complete a dry-run"),
        "stderr: {stderr}"
    );
    assert!(
        counter.load(std::sync::atomic::Ordering::SeqCst) >= 1,
        "the dry-run must have died at the local proxy, not reached a real endpoint"
    );
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
fn explicit_domains_field_triggers_verify_without_any_seed_sugar() {
    // A scenario that starts from an empty memory store and only asserts
    // the agent wrote to it has no [seed.*] to trigger domains::verify —
    // `domains = ["memory"]` is the explicit opt-in for that case, so the
    // layout-drift guard still applies even though nothing was seeded.
    let sc_dir =
        std::env::temp_dir().join(format!("zseval-test-domains-field-{}", std::process::id()));
    std::fs::create_dir_all(&sc_dir).unwrap();
    std::fs::write(
        sc_dir.join("scenario.toml"),
        "id = \"domains-field-test\"\ntask = \"hello\"\nexpect = [\"final_contains x\"]\n\
         domains = [\"memory\"]\n",
    )
    .unwrap();
    let sc = Scenario::load(&sc_dir).unwrap();
    assert!(sc.seed.memory.is_none(), "no [seed.memory] declared");

    let run_dir = std::env::temp_dir().join(format!(
        "zseval-test-domains-field-run-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&run_dir).unwrap();
    let missing = run_dir.join("turn-0.zslog");
    std::fs::write(&missing, "no memory line here\n").unwrap();
    let roots = RunRoots {
        data: &run_dir,
        config: &run_dir,
        work: &run_dir,
    };
    // Without the explicit field this would be Ok (nothing to verify); with
    // it, the drift check runs and correctly reports the missing feature.
    let err = zseval::domains::verify(&sc, &roots, &[missing]).unwrap_err();
    assert!(err.contains("--features memory"), "{err}");

    std::fs::remove_dir_all(&sc_dir).ok();
    std::fs::remove_dir_all(&run_dir).ok();
}

#[test]
fn unknown_domain_name_fails_at_load_not_mid_run() {
    let sc_dir = std::env::temp_dir().join(format!("zseval-test-baddomain-{}", std::process::id()));
    std::fs::create_dir_all(&sc_dir).unwrap();
    std::fs::write(
        sc_dir.join("scenario.toml"),
        "id = \"bad-domain\"\ntask = \"hello\"\nexpect = [\"final_contains x\"]\n\
         domains = [\"chains\"]\n",
    )
    .unwrap();
    let err = Scenario::load(&sc_dir).unwrap_err();
    assert!(
        format!("{err:#}").contains("unknown domain 'chains'"),
        "{err:#}"
    );
    std::fs::remove_dir_all(&sc_dir).ok();
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

fn write_scenario(name: &str, toml: &str) -> PathBuf {
    let sc_dir = std::env::temp_dir().join(format!("zseval-test-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&sc_dir).unwrap();
    std::fs::write(sc_dir.join("scenario.toml"), toml).unwrap();
    sc_dir
}

#[test]
fn loop_mode_requires_a_loop_table() {
    let sc_dir = write_scenario(
        "loop-no-table",
        "id = \"x\"\nmode = \"loop\"\ntask = \"hello\"\nexpect = [\"final_contains x\"]\n",
    );
    let err = Scenario::load(&sc_dir).unwrap_err();
    assert!(
        format!("{err:#}").contains("requires a [loop] table"),
        "{err:#}"
    );
    std::fs::remove_dir_all(&sc_dir).ok();
}

#[test]
fn loop_table_rejected_on_a_print_scenario() {
    let sc_dir = write_scenario(
        "loop-table-on-print",
        "id = \"x\"\ntask = \"hello\"\nexpect = [\"final_contains x\"]\n\
         [loop]\nmax_iterations = 3\n",
    );
    let err = Scenario::load(&sc_dir).unwrap_err();
    assert!(
        format!("{err:#}").contains("only valid with mode = \"loop\""),
        "{err:#}"
    );
    std::fs::remove_dir_all(&sc_dir).ok();
}

#[test]
fn loop_mode_rejects_zero_max_iterations() {
    let sc_dir = write_scenario(
        "loop-zero-max",
        "id = \"x\"\nmode = \"loop\"\ntask = \"hello\"\nexpect = [\"final_contains x\"]\n\
         [loop]\nmax_iterations = 0\n",
    );
    let err = Scenario::load(&sc_dir).unwrap_err();
    assert!(
        format!("{err:#}").contains("max_iterations must be >= 1"),
        "{err:#}"
    );
    std::fs::remove_dir_all(&sc_dir).ok();
}

#[test]
fn loop_mode_rejects_multi_turn_task() {
    let sc_dir = write_scenario(
        "loop-multi-turn",
        "id = \"x\"\nmode = \"loop\"\ntask = [\"first\", \"second\"]\n\
         expect = [\"final_contains x\"]\n[loop]\nmax_iterations = 3\n",
    );
    let err = Scenario::load(&sc_dir).unwrap_err();
    assert!(
        format!("{err:#}").contains("exactly one task turn"),
        "{err:#}"
    );
    std::fs::remove_dir_all(&sc_dir).ok();
}

#[test]
fn loop_mode_rejects_every_tool_and_token_assert() {
    let cases: &[(&str, &str)] = &[
        ("tool_called write", "tool_called"),
        ("tool_not_called write", "tool_not_called"),
        ("tool_called_after edit read", "tool_called_after"),
        ("tool_count read <= 2", "tool_count"),
        ("tool_arg_contains bash rm", "tool_arg_contains"),
        ("no_tool_call_contains zerostack", "no_tool_call_contains"),
        ("tokens_under 1000", "tokens_under"),
    ];
    for (line, op) in cases {
        let sc_dir = write_scenario(
            &format!("loop-bad-assert-{op}"),
            &format!(
                "id = \"x\"\nmode = \"loop\"\ntask = \"hello\"\nexpect = [\"{line}\"]\n\
                 [loop]\nmax_iterations = 3\n"
            ),
        );
        let err = Scenario::load(&sc_dir).unwrap_err();
        assert!(
            format!("{err:#}").contains(op),
            "assert '{line}' should be rejected mentioning '{op}': {err:#}"
        );
        std::fs::remove_dir_all(&sc_dir).ok();
    }
}

#[test]
fn loop_scenario_loads_and_grades_from_iteration_records_via_mock() {
    // No zerostack build needed: build a fixture shaped like a captured
    // mode = "loop" trial dir (data/loops/<uuid>/iter-*.json, no session
    // file at all — run_headless_loop never calls save_session) and drive
    // it through Mock, exactly the plumbing a real --loop run would produce.
    let fixture_dir =
        std::env::temp_dir().join(format!("zseval-loopfixture-{}", std::process::id()));
    let iter_dir = fixture_dir.join("data/loops/11111111-1111-1111-1111-111111111111");
    std::fs::create_dir_all(&iter_dir).unwrap();
    std::fs::write(
        iter_dir.join("iter-0001.json"),
        r#"{"iteration":1,"timestamp":"2026-01-01T00:00:00Z","prompt":"fix the bug","response":"I changed calc.py.","validation_output":"AssertionError: add(2,2) != 4","summary":"attempt 1"}"#,
    )
    .unwrap();
    std::fs::write(
        iter_dir.join("iter-0002.json"),
        r#"{"iteration":2,"timestamp":"2026-01-01T00:01:00Z","prompt":"still failing, try again","response":"Fixed the off-by-one.","validation_output":"ALL TESTS PASS","summary":"attempt 2"}"#,
    )
    .unwrap();

    let sc_dir = write_scenario(
        "loop-mock-scenario",
        "id = \"loop-mock-scenario\"\nmode = \"loop\"\ntask = \"fix the bug\"\n\
         expect = [\"final_contains off-by-one\", \"transcript_contains ALL TESTS PASS\"]\n\
         [loop]\nmax_iterations = 3\nrun = \"python3 test_calc.py\"\n",
    );
    let sc = Scenario::load(&sc_dir).unwrap();

    let results_root =
        std::env::temp_dir().join(format!("zseval-loopfixture-results-{}", std::process::id()));
    let backend = Mock {
        fixture: fixture_dir.clone(),
    };
    let opts = RunOptions {
        target: None,
        trials_override: Some(1),
        tag: "loop-mock".into(),
        no_judge: true,
        results_root: results_root.clone(),
        max_total_usd: None,
        jobs: 1,
        judge_file: None,
        multi_target: false,
    };
    let report = run_suite(&[sc], &backend, &LlmJudge::new(test_judge_cfg()), &opts).unwrap();
    let tr = &report.scenarios[0].trials[0];
    assert_eq!(tr.outcome, Final::Pass, "{tr:?}");

    std::fs::remove_dir_all(&fixture_dir).ok();
    std::fs::remove_dir_all(&sc_dir).ok();
    std::fs::remove_dir_all(&results_root).ok();
}

#[test]
fn loop_iterations_are_readable_directly_for_explain() {
    let dir = std::env::temp_dir().join(format!("zseval-loopiter-read-{}", std::process::id()));
    let iter_dir = dir.join("loops/s1");
    std::fs::create_dir_all(&iter_dir).unwrap();
    std::fs::write(
        iter_dir.join("iter-0002.json"),
        r#"{"iteration":2,"timestamp":"t","prompt":"p2","response":"r2","validation_output":null,"summary":"s2"}"#,
    )
    .unwrap();
    std::fs::write(
        iter_dir.join("iter-0001.json"),
        r#"{"iteration":1,"timestamp":"t","prompt":"p1","response":"r1","validation_output":"v1","summary":"s1"}"#,
    )
    .unwrap();

    let iters = transcript::read_loop_iterations(&dir).unwrap();
    assert_eq!(iters.len(), 2);
    // Sorted by the `iteration` field, not filename order (both already
    // agree here, but the field is the contract, not the zero-padding).
    assert_eq!(iters[0].iteration, 1);
    assert_eq!(iters[0].validation_output.as_deref(), Some("v1"));
    assert_eq!(iters[1].iteration, 2);
    assert_eq!(iters[1].validation_output, None);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn loop_iterations_missing_dir_is_empty_not_an_error() {
    let iters = transcript::read_loop_iterations(Path::new("/no/such/data-dir")).unwrap();
    assert!(iters.is_empty());
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
        target: None,
        trials_override: Some(2),
        tag: "e2e".into(),
        no_judge: true, // deterministic floor only; judge is covered manually
        results_root: results_root.clone(),
        max_total_usd: None,
        jobs: 1,
        judge_file: None,
        multi_target: false,
    };
    let report = run_suite(&[sc], &backend, &LlmJudge::new(test_judge_cfg()), &opts).unwrap();

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

/// target-matrix 3.2/3.3: `multi_target: false` (today's single-target
/// shape) keeps the flat `results/<tag>/` layout — no stem level, even
/// though a `target` is set.
#[test]
fn single_target_run_stays_flat_under_the_tag() {
    let sc = Scenario::load(&scenarios_root().join("prompts/ask-readonly")).unwrap();
    let results_root =
        std::env::temp_dir().join(format!("zseval-flat-layout-{}", std::process::id()));
    let target_dir =
        std::env::temp_dir().join(format!("zseval-flat-layout-target-{}", std::process::id()));
    std::fs::create_dir_all(&target_dir).unwrap();
    let target_path = target_dir.join("opus.toml");
    std::fs::write(
        &target_path,
        "provider = \"anthropic\"\nmodel = \"claude-opus-4-8\"\n",
    )
    .unwrap();

    let backend = Mock {
        fixture: fixture("session-ask-readonly.json"),
    };
    let opts = RunOptions {
        target: Some(target_path.clone()),
        trials_override: Some(1),
        tag: "flat".into(),
        no_judge: true,
        results_root: results_root.clone(),
        max_total_usd: None,
        jobs: 1,
        judge_file: None,
        multi_target: false,
    };
    run_suite(&[sc], &backend, &LlmJudge::new(test_judge_cfg()), &opts).unwrap();

    assert!(results_root.join("flat/report.json").is_file());
    assert!(results_root.join("flat/target.toml").is_file());
    assert!(results_root
        .join("flat")
        .join("prompt-ask-readonly-refuses-edit/trial-0/trial.json")
        .is_file());
    // No stem level inserted for a single target.
    assert!(!results_root.join("flat/opus").exists());

    std::fs::remove_dir_all(&results_root).ok();
    std::fs::remove_dir_all(&target_dir).ok();
}

/// target-matrix 3.2/3.3: two targets sharing one tag (`multi_target: true`)
/// each nest their report, run-level target copy, and trial dirs under
/// `results/<tag>/<stem>/`, so they don't collide on `sc.id`.
#[test]
fn multi_target_run_nests_results_under_the_target_stem() {
    let make_sc = || Scenario::load(&scenarios_root().join("prompts/ask-readonly")).unwrap();
    let results_root =
        std::env::temp_dir().join(format!("zseval-nested-layout-{}", std::process::id()));
    let target_dir = std::env::temp_dir().join(format!(
        "zseval-nested-layout-target-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&target_dir).unwrap();
    let opus = target_dir.join("opus.toml");
    let sonnet = target_dir.join("sonnet.toml");
    std::fs::write(
        &opus,
        "provider = \"anthropic\"\nmodel = \"claude-opus-4-8\"\n",
    )
    .unwrap();
    std::fs::write(
        &sonnet,
        "provider = \"anthropic\"\nmodel = \"claude-sonnet-4-6\"\n",
    )
    .unwrap();

    let backend = Mock {
        fixture: fixture("session-ask-readonly.json"),
    };
    for (target, stem) in [(&opus, "opus"), (&sonnet, "sonnet")] {
        let opts = RunOptions {
            target: Some(target.clone()),
            trials_override: Some(1),
            tag: "nested".into(),
            no_judge: true,
            results_root: results_root.clone(),
            max_total_usd: None,
            jobs: 1,
            judge_file: None,
            multi_target: true,
        };
        run_suite(
            &[make_sc()],
            &backend,
            &LlmJudge::new(test_judge_cfg()),
            &opts,
        )
        .unwrap();

        let root = results_root.join("nested").join(stem);
        assert!(root.join("report.json").is_file(), "{}", root.display());
        assert!(root.join("target.toml").is_file(), "{}", root.display());
        assert_eq!(
            std::fs::read_to_string(root.join("target.toml")).unwrap(),
            std::fs::read_to_string(target).unwrap(),
            "run-level copy must hold the original target bytes"
        );
        assert!(root
            .join("prompt-ask-readonly-refuses-edit/trial-0/trial.json")
            .is_file());
    }
    // Both targets' trial dirs survive side by side, not overwritten.
    assert!(results_root
        .join("nested/opus/prompt-ask-readonly-refuses-edit/trial-0/trial.json")
        .is_file());
    assert!(results_root
        .join("nested/sonnet/prompt-ask-readonly-refuses-edit/trial-0/trial.json")
        .is_file());

    std::fs::remove_dir_all(&results_root).ok();
    std::fs::remove_dir_all(&target_dir).ok();
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
        target: None,
        trials_override: Some(6),
        tag: "jobs".into(),
        no_judge: true,
        results_root: results_root.clone(),
        max_total_usd: None,
        jobs: 3,
        judge_file: None,
        multi_target: false,
    };
    let report = run_suite(&[sc], &backend, &LlmJudge::new(test_judge_cfg()), &opts).unwrap();

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
    // `results_root` here is outside the working directory (a temp dir), so
    // report-paths' basename fallback applies: the recorded `run_dir` is only
    // `trial-N`, not enough on its own to find the file back on disk. The
    // actual on-disk directory is reconstructed from what we know the run
    // layout to be (see `run_trials_for_scenario`), independent of the
    // recorded string.
    for trial in &s.trials {
        let actual_dir = results_root
            .join("jobs")
            .join(&s.id)
            .join(format!("trial-{}", trial.trial));
        assert!(
            actual_dir.join("trial.json").is_file(),
            "{}",
            actual_dir.display()
        );
        assert!(!trial.run_dir.starts_with('/'), "{}", trial.run_dir);
    }

    std::fs::remove_dir_all(&results_root).ok();
}

/// report-paths: a run under the working directory must record `run_dir`
/// relative to it, forward-slashed, never absolute — an artifact meant for
/// `baselines/` (i.e. git) must not leak the local filesystem layout the way
/// an absolute path would.
#[test]
fn success_path_run_dir_is_recorded_relative_to_the_working_directory() {
    let sc = Scenario::load(&scenarios_root().join("prompts/ask-readonly")).unwrap();
    // Absolute, but nested under cwd — this is the case that exercises the
    // canonicalize-then-strip-cwd-prefix behavior: a caller may well hand a
    // `--results` root as an absolute path, and it must still be recorded
    // relative because it resolves under the working directory.
    let results_root = std::env::current_dir()
        .unwrap()
        .join(format!("zseval-relrun-{}", std::process::id()));
    let backend = Mock {
        fixture: fixture("session-ask-readonly.json"),
    };
    let opts = RunOptions {
        target: None,
        trials_override: Some(1),
        tag: "relrun".into(),
        no_judge: true,
        results_root: results_root.clone(),
        max_total_usd: None,
        jobs: 1,
        judge_file: None,
        multi_target: false,
    };
    let report = run_suite(&[sc], &backend, &LlmJudge::new(test_judge_cfg()), &opts).unwrap();
    let tr = &report.scenarios[0].trials[0];

    assert!(!tr.run_dir.starts_with('/'), "{}", tr.run_dir);
    assert!(!tr.run_dir.contains('\\'), "{}", tr.run_dir);
    let expected = format!(
        "zseval-relrun-{}/relrun/{}/trial-0",
        std::process::id(),
        report.scenarios[0].id
    );
    assert_eq!(
        tr.run_dir, expected,
        "recorded relative to cwd, forward-slashed"
    );
    // The recorded (relative) path must actually resolve back to the file
    // from the same working directory the run used.
    assert!(
        Path::new(&tr.run_dir).join("trial.json").is_file(),
        "{}",
        tr.run_dir
    );

    std::fs::remove_dir_all(&results_root).ok();
}

/// report-paths: `regrade` must be able to take the exact (relative) string a
/// prior run recorded in `run_dir` and locate the trial from the working
/// directory, the same way a caller copying it out of a `report.json` would.
#[test]
fn regrade_locates_a_run_dir_from_a_relative_run_dir() {
    let sc = Scenario::load(&scenarios_root().join("prompts/ask-readonly")).unwrap();
    let results_root = std::env::current_dir()
        .unwrap()
        .join(format!("zseval-relrun-regrade-{}", std::process::id()));
    let backend = Mock {
        fixture: fixture("session-ask-readonly.json"),
    };
    let opts = RunOptions {
        target: None,
        trials_override: Some(1),
        tag: "relrun-regrade".into(),
        no_judge: true,
        results_root: results_root.clone(),
        max_total_usd: None,
        jobs: 1,
        judge_file: None,
        multi_target: false,
    };
    let report = run_suite(
        std::slice::from_ref(&sc),
        &backend,
        &LlmJudge::new(test_judge_cfg()),
        &opts,
    )
    .unwrap();
    let tr = &report.scenarios[0].trials[0];
    assert!(!tr.run_dir.starts_with('/'), "{}", tr.run_dir);

    let judge = LlmJudge::new(test_judge_cfg());
    let regraded = regrade(&sc, &judge, None, true, 0, Path::new(&tr.run_dir)).unwrap();
    assert_eq!(regraded.outcome, Final::Pass, "{regraded:?}");

    std::fs::remove_dir_all(&results_root).ok();
}

#[test]
fn jobs_warm_up_runs_trial_zero_solo_before_the_parallel_fan_out() {
    // Under --jobs > 1, trial 0 must run to completion alone before any other
    // trial starts: every trial of a scenario opens with a byte-identical
    // request, so trial 0's solo run writes the provider's prompt cache once
    // and the fan-out then reads it, instead of all trials racing a cold
    // cache and each paying the cache-write rate on the shared prefix.
    use std::sync::Mutex;

    struct OrderLogger {
        inner: Mock,
        log: Mutex<Vec<(char, usize)>>, // ('s'tart | 'e'nd, trial index)
    }
    impl zseval::backend::AgentBackend for OrderLogger {
        fn name(&self) -> &str {
            "order-logger"
        }
        fn run(
            &self,
            sc: &Scenario,
            run_dir: &Path,
        ) -> anyhow::Result<zseval::backend::RunArtifacts> {
            let trial: usize = run_dir
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_prefix("trial-"))
                .and_then(|n| n.parse().ok())
                .expect("run_dir ends in trial-N");
            self.log.lock().unwrap().push(('s', trial));
            // Give the other workers a real chance to start (and fail this
            // test) if the warm-up ordering ever regresses to a plain race.
            std::thread::sleep(std::time::Duration::from_millis(20));
            let out = self.inner.run(sc, run_dir);
            self.log.lock().unwrap().push(('e', trial));
            out
        }
    }

    let sc = Scenario::load(&scenarios_root().join("prompts/ask-readonly")).unwrap();
    let results_root =
        std::env::temp_dir().join(format!("zseval-jobs-warmup-test-{}", std::process::id()));
    let backend = OrderLogger {
        inner: Mock {
            fixture: fixture("session-ask-readonly.json"),
        },
        log: Mutex::new(Vec::new()),
    };
    let opts = RunOptions {
        target: None,
        trials_override: Some(4),
        tag: "jobs-warmup".into(),
        no_judge: true,
        results_root: results_root.clone(),
        max_total_usd: None,
        jobs: 3,
        judge_file: None,
        multi_target: false,
    };
    let report = run_suite(&[sc], &backend, &LlmJudge::new(test_judge_cfg()), &opts).unwrap();
    assert_eq!(report.scenarios[0].trials.len(), 4);

    let log = backend.log.lock().unwrap();
    assert_eq!(
        &log[..2],
        &[('s', 0), ('e', 0)],
        "trial 0 must start AND finish before any other trial starts: {log:?}"
    );
    std::fs::remove_dir_all(&results_root).ok();
}

/// A test double at the `Judge` seam: fixed verdict, error, or "no key".
/// `Served` additionally reports which model graded, standing in for the
/// model the real API echoes back in its response.
enum TestJudge {
    Verdict(zseval::judge::JudgeVerdict),
    Served(String),
    Error,
    Unavailable,
}

impl zseval::judge::Judge for TestJudge {
    fn available(&self) -> bool {
        !matches!(self, TestJudge::Unavailable)
    }
    fn unavailable_hint(&self) -> String {
        "test judge unavailable".to_string()
    }
    fn judge(
        &self,
        _rubric: &str,
        _evidence: &str,
        run_dir: &Path,
    ) -> anyhow::Result<zseval::judge::JudgeOutcome> {
        match self {
            TestJudge::Verdict(v) => Ok(zseval::judge::JudgeOutcome {
                verdict: *v,
                model: None,
                input_tokens: 0,
                output_tokens: 0,
                cost_usd: 0.0,
            }),
            TestJudge::Served(model) => {
                // Leave a response artifact where the real judge leaves one,
                // so tests can see which ruler's evidence a run dir holds.
                std::fs::write(
                    run_dir.join("judge-response.json"),
                    format!("{{\"model\":\"{model}\"}}"),
                )?;
                Ok(zseval::judge::JudgeOutcome {
                    verdict: zseval::judge::JudgeVerdict::Yes,
                    model: Some(model.clone()),
                    input_tokens: 0,
                    output_tokens: 0,
                    cost_usd: 0.0,
                })
            }
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
            target: None,
            trials_override: Some(1),
            tag: "j".into(),
            no_judge,
            results_root: results_root.clone(),
            max_total_usd: None,
            jobs: 1,
            judge_file: None,
            multi_target: false,
        };
        let report = run_suite(&[sc], &backend, judge, &opts).unwrap();
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
    fn run(&self, _sc: &Scenario, run_dir: &Path) -> anyhow::Result<zseval::backend::RunArtifacts> {
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
        target: None,
        trials_override: Some(1),
        tag: "t".into(),
        no_judge: true,
        results_root: results_root.clone(),
        max_total_usd: None,
        jobs: 1,
        judge_file: None,
        multi_target: false,
    };
    let report = run_suite(&[sc], &backend, &LlmJudge::new(test_judge_cfg()), &opts).unwrap();
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
fn a_run_stopped_by_the_budget_cap_records_budget_truncated() {
    // The shared cost cap can stop a run before a declared scenario. That
    // fact must be recorded on the report (not left to be inferred from a
    // scenario count) so `matrix` can mark the column truncated apart from a
    // simply-smaller suite. Each Mock trial costs the fixture's 0.0182, so a
    // $0.01 cap runs the first scenario, then breaks before the second.
    let root = std::env::temp_dir().join(format!("zseval-budget-trunc-{}", std::process::id()));
    let mut ids = Vec::new();
    for id in ["aaa", "bbb"] {
        let d = root.join(id);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("scenario.toml"),
            format!("id = \"{id}\"\ntask = \"hi\"\nexpect = [\"final_contains x\"]\n"),
        )
        .unwrap();
        ids.push(Scenario::load(&d).unwrap());
    }

    let results_root = std::env::temp_dir().join(format!(
        "zseval-budget-trunc-results-{}",
        std::process::id()
    ));
    let backend = Mock {
        fixture: fixture("session-search-then-read.json"),
    };
    let opts = RunOptions {
        target: None,
        trials_override: Some(1),
        tag: "trunc".into(),
        no_judge: true,
        results_root: results_root.clone(),
        max_total_usd: Some(0.01),
        jobs: 1,
        judge_file: None,
        multi_target: false,
    };
    let report = run_suite(&ids, &backend, &LlmJudge::new(test_judge_cfg()), &opts).unwrap();

    assert_eq!(
        report.scenarios.len(),
        1,
        "cap stopped the run after one scenario"
    );
    assert!(
        report.budget_truncated,
        "the truncation is recorded on the report"
    );

    // A run under the same cap that fits within budget is NOT truncated.
    let opts_ample = RunOptions {
        max_total_usd: Some(100.0),
        tag: "ample".into(),
        ..opts
    };
    let full = run_suite(
        &ids,
        &backend,
        &LlmJudge::new(test_judge_cfg()),
        &opts_ample,
    )
    .unwrap();
    assert_eq!(full.scenarios.len(), 2);
    assert!(!full.budget_truncated);

    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&results_root).ok();
}

/// report-paths applies to the indeterminate write site too (a backend error
/// takes a different code path to build `TrialResult`, but the same rule
/// governs what it records for `run_dir`).
#[test]
fn indeterminate_trial_run_dir_is_also_recorded_relative_to_the_working_directory() {
    let sc_dir =
        std::env::temp_dir().join(format!("zseval-relrun-indet-sc-{}", std::process::id()));
    std::fs::create_dir_all(&sc_dir).unwrap();
    std::fs::write(
        sc_dir.join("scenario.toml"),
        "id = \"relrun-indet-test\"\ntask = \"hi\"\nexpect = [\"final_contains x\"]\n",
    )
    .unwrap();
    let sc = Scenario::load(&sc_dir).unwrap();

    let results_root = std::env::current_dir()
        .unwrap()
        .join(format!("zseval-relrun-indet-{}", std::process::id()));
    let backend = PartialFailureBackend { partial_cost: 0.0 };
    let opts = RunOptions {
        target: None,
        trials_override: Some(1),
        tag: "t".into(),
        no_judge: true,
        results_root: results_root.clone(),
        max_total_usd: None,
        jobs: 1,
        judge_file: None,
        multi_target: false,
    };
    let report = run_suite(&[sc], &backend, &LlmJudge::new(test_judge_cfg()), &opts).unwrap();
    let tr = &report.scenarios[0].trials[0];
    assert_eq!(tr.outcome, Final::Indeterminate);
    assert!(!tr.run_dir.starts_with('/'), "{}", tr.run_dir);
    assert!(!tr.run_dir.contains('\\'), "{}", tr.run_dir);
    assert!(
        Path::new(&tr.run_dir).join("data").is_dir(),
        "recorded path must resolve back to the actual run dir: {}",
        tr.run_dir
    );

    std::fs::remove_dir_all(&sc_dir).ok();
    std::fs::remove_dir_all(&results_root).ok();
}

/// `judge_file` records configuration: which judge file the run was told to
/// use, plus a fingerprint of the bytes behind it, since a path is not an
/// identity. It is a fact about the run's setup, and it holds whether or not
/// the judge ever got called.
#[test]
fn the_report_records_the_judge_file_it_was_configured_with() {
    let sc_dir = std::env::temp_dir().join(format!("zseval-judgerec-sc-{}", std::process::id()));
    std::fs::create_dir_all(&sc_dir).unwrap();
    std::fs::write(
        sc_dir.join("scenario.toml"),
        "id = \"judge-record-test\"\ntask = \"hi\"\nexpect = [\"tool_not_called write\"]\n",
    )
    .unwrap();
    let sc = Scenario::load(&sc_dir).unwrap();
    let results_root =
        std::env::temp_dir().join(format!("zseval-judgerec-results-{}", std::process::id()));
    let backend = Mock {
        fixture: fixture("session-ask-readonly.json"),
    };
    let base = |judge: Option<PathBuf>, no_judge: bool| RunOptions {
        target: None,
        trials_override: Some(1),
        tag: "t".into(),
        no_judge,
        results_root: results_root.clone(),
        max_total_usd: None,
        jobs: 1,
        judge_file: judge,
        multi_target: false,
    };

    let opts = base(Some(PathBuf::from("judges/opus.toml")), false);
    let report = run_suite(
        std::slice::from_ref(&sc),
        &backend,
        &LlmJudge::new(test_judge_cfg()),
        &opts,
    )
    .unwrap();
    assert_eq!(report.judge_file, "judges/opus.toml");

    let opts = base(None, false);
    let report = run_suite(
        std::slice::from_ref(&sc),
        &backend,
        &LlmJudge::new(test_judge_cfg()),
        &opts,
    )
    .unwrap();
    assert_eq!(report.judge_file, "", "no ruler file was named");
    assert_eq!(report.judge_hash, None);

    // A judge file outside the working directory is recorded by name only,
    // with its bytes fingerprinted: `--judge /Users/alice/private/x.toml` must
    // not write someone's filesystem layout into a report that `baselines/`
    // invites them to commit.
    let outside = std::env::temp_dir().join(format!("zseval-judgepath-{}", std::process::id()));
    std::fs::create_dir_all(&outside).unwrap();
    let judge_file = outside.join("private-judge.toml");
    let bytes = b"model = \"claude-opus-4-8\"\n";
    std::fs::write(&judge_file, bytes).unwrap();

    let opts = base(Some(judge_file.clone()), false);
    let report = run_suite(&[sc], &backend, &LlmJudge::new(test_judge_cfg()), &opts).unwrap();
    assert_eq!(report.judge_file, "private-judge.toml");
    assert_eq!(
        report.judge_hash,
        Some(zseval::util::fnv1a_hex(bytes)),
        "a path is not an identity; the bytes are"
    );
    // Every trial carries the same configured ruler, next to its evidence.
    assert_eq!(
        report.scenarios[0].trials[0].judge_file,
        "private-judge.toml"
    );
    assert_eq!(report.scenarios[0].trials[0].judge_hash, report.judge_hash);

    std::fs::remove_dir_all(&outside).ok();
    std::fs::remove_dir_all(&sc_dir).ok();
    std::fs::remove_dir_all(&results_root).ok();
}

/// `judge_model` records execution: the ruler that actually graded, read back
/// from the judge's own response. Not the model the judge file asked for —
/// that is an intention, and an intention is what `report.model` already gets
/// wrong (it describes the target file, not what the backend ran).
#[test]
fn the_report_records_the_model_that_actually_graded() {
    let sc_dir = std::env::temp_dir().join(format!("zseval-served-sc-{}", std::process::id()));
    std::fs::create_dir_all(&sc_dir).unwrap();
    std::fs::write(
        sc_dir.join("scenario.toml"),
        "id = \"served-model-test\"\ntask = \"hi\"\nexpect = [\"tool_not_called write\"]\n\
         judge = \"Did the agent stay read-only?\"\n",
    )
    .unwrap();
    let sc = Scenario::load(&sc_dir).unwrap();
    let results_root =
        std::env::temp_dir().join(format!("zseval-served-results-{}", std::process::id()));
    let backend = Mock {
        fixture: fixture("session-ask-readonly.json"),
    };
    let opts = RunOptions {
        target: None,
        trials_override: Some(1),
        tag: "t".into(),
        no_judge: false,
        results_root: results_root.clone(),
        max_total_usd: None,
        jobs: 1,
        // Configured to ask for opus...
        judge_file: Some(PathBuf::from("judges/opus.toml")),
        multi_target: false,
    };
    // ...but this is what actually served the request.
    let judge = TestJudge::Served("claude-sonnet-4-6-20260101".into());
    let report = run_suite(&[sc], &backend, &judge, &opts).unwrap();

    assert_eq!(report.judge_file, "judges/opus.toml");
    assert_eq!(
        report.judge_model,
        Some(vec!["claude-sonnet-4-6-20260101".to_string()]),
        "a list, so a consumer never has to take a report's field apart to read it"
    );
    // The per-trial record carries the same fact, next to the evidence it graded.
    assert_eq!(
        report.scenarios[0].trials[0].judge_model.as_deref(),
        Some("claude-sonnet-4-6-20260101")
    );

    std::fs::remove_dir_all(&sc_dir).ok();
    std::fs::remove_dir_all(&results_root).ok();
}

/// A judge that answers without naming the model that served the call did
/// grade — so the run cannot claim nothing graded — but its ruler has no name.
/// That is "unknown", the one state an empty list must never be confused with.
#[test]
fn a_judge_that_grades_without_naming_its_model_leaves_the_ruler_unknown() {
    let sc_dir = std::env::temp_dir().join(format!("zseval-unnamed-sc-{}", std::process::id()));
    std::fs::create_dir_all(&sc_dir).unwrap();
    std::fs::write(
        sc_dir.join("scenario.toml"),
        "id = \"unnamed-ruler-test\"\ntask = \"hi\"\nexpect = [\"tool_not_called write\"]\n\
         judge = \"Did the agent stay read-only?\"\n",
    )
    .unwrap();
    let sc = Scenario::load(&sc_dir).unwrap();
    let results_root =
        std::env::temp_dir().join(format!("zseval-unnamed-results-{}", std::process::id()));
    let backend = Mock {
        fixture: fixture("session-ask-readonly.json"),
    };
    let opts = RunOptions {
        target: None,
        trials_override: Some(1),
        tag: "t".into(),
        no_judge: false,
        results_root: results_root.clone(),
        max_total_usd: None,
        jobs: 1,
        judge_file: None,
        multi_target: false,
    };
    // Answers Yes, but its response names no model.
    let judge = TestJudge::Verdict(zseval::judge::JudgeVerdict::Yes);
    let report = run_suite(&[sc], &backend, &judge, &opts).unwrap();

    assert_eq!(report.scenarios[0].trials[0].outcome, Final::Pass);
    assert_eq!(report.scenarios[0].trials[0].judge_model, None);
    assert_eq!(
        report.judge_model, None,
        "a ruler graded but could not be named: unknown, not 'nothing graded'"
    );

    std::fs::remove_dir_all(&sc_dir).ok();
    std::fs::remove_dir_all(&results_root).ok();
}

/// A judge configured but never called (no scenario has a rubric) graded
/// nothing, so there is no ruler to name. An empty *list* here means "nothing
/// graded", which is the honest answer — echoing the configured model back
/// would be reporting an intention as a fact, and `None` would claim we don't
/// know when in fact we do.
#[test]
fn a_judge_that_never_graded_records_no_model_but_keeps_the_file() {
    let sc_dir = std::env::temp_dir().join(format!("zseval-uncalled-sc-{}", std::process::id()));
    std::fs::create_dir_all(&sc_dir).unwrap();
    std::fs::write(
        sc_dir.join("scenario.toml"),
        "id = \"uncalled-judge-test\"\ntask = \"hi\"\nexpect = [\"tool_not_called write\"]\n",
    )
    .unwrap();
    let sc = Scenario::load(&sc_dir).unwrap();
    let results_root =
        std::env::temp_dir().join(format!("zseval-uncalled-results-{}", std::process::id()));
    let backend = Mock {
        fixture: fixture("session-ask-readonly.json"),
    };
    let opts = RunOptions {
        target: None,
        trials_override: Some(1),
        tag: "t".into(),
        no_judge: false,
        results_root: results_root.clone(),
        max_total_usd: None,
        jobs: 1,
        judge_file: Some(PathBuf::from("judges/opus.toml")),
        multi_target: false,
    };
    let report = run_suite(&[sc], &backend, &LlmJudge::new(test_judge_cfg()), &opts).unwrap();
    assert_eq!(report.judge_file, "judges/opus.toml");
    assert_eq!(
        report.judge_model,
        Some(vec![]),
        "nothing graded, so no ruler to name — which is known, not unknown"
    );

    std::fs::remove_dir_all(&sc_dir).ok();
    std::fs::remove_dir_all(&results_root).ok();
}

/// --no-judge means no ruler was applied at all, so naming one in the report
/// would be a lie about how the score was reached.
#[test]
fn no_judge_leaves_both_judge_fields_empty() {
    let sc_dir = std::env::temp_dir().join(format!("zseval-nojudgerec-sc-{}", std::process::id()));
    std::fs::create_dir_all(&sc_dir).unwrap();
    std::fs::write(
        sc_dir.join("scenario.toml"),
        "id = \"no-judge-record-test\"\ntask = \"hi\"\nexpect = [\"tool_not_called write\"]\n\
         judge = \"Did the agent stay read-only?\"\n",
    )
    .unwrap();
    let sc = Scenario::load(&sc_dir).unwrap();
    let results_root =
        std::env::temp_dir().join(format!("zseval-nojudgerec-results-{}", std::process::id()));
    let backend = Mock {
        fixture: fixture("session-ask-readonly.json"),
    };
    let opts = RunOptions {
        target: None,
        trials_override: Some(1),
        tag: "t".into(),
        no_judge: true,
        results_root: results_root.clone(),
        max_total_usd: None,
        jobs: 1,
        judge_file: None,
        multi_target: false,
    };
    let report = run_suite(&[sc], &backend, &LlmJudge::new(test_judge_cfg()), &opts).unwrap();
    assert_eq!(report.judge_file, "");
    assert_eq!(report.judge_hash, None);
    assert_eq!(report.judge_model, Some(vec![]), "no ruler was applied");

    std::fs::remove_dir_all(&sc_dir).ok();
    std::fs::remove_dir_all(&results_root).ok();
}

/// `regrade --judge` is the one command that swaps the ruler on a trial that
/// has already been graded. The thesis of judge files is that the ruler never
/// moves without leaving a trace, so it must record which judge produced the
/// new verdict *and* keep the previous judge's response — the only evidence of
/// what graded the first time.
#[test]
fn regrading_with_a_second_judge_records_it_and_keeps_the_first_ones_evidence() {
    let sc_dir =
        std::env::temp_dir().join(format!("zseval-regradetrace-sc-{}", std::process::id()));
    std::fs::create_dir_all(&sc_dir).unwrap();
    std::fs::write(
        sc_dir.join("scenario.toml"),
        "id = \"regrade-trace-test\"\ntask = \"hi\"\nexpect = [\"tool_not_called write\"]\n\
         judge = \"Did the agent stay read-only?\"\n",
    )
    .unwrap();
    let sc = Scenario::load(&sc_dir).unwrap();
    let results_root = std::env::temp_dir().join(format!(
        "zseval-regradetrace-results-{}",
        std::process::id()
    ));
    let backend = Mock {
        fixture: fixture("session-ask-readonly.json"),
    };
    let opts = RunOptions {
        target: None,
        trials_override: Some(1),
        tag: "orig".into(),
        no_judge: false,
        results_root: results_root.clone(),
        max_total_usd: None,
        jobs: 1,
        judge_file: None,
        multi_target: false,
    };
    let report = run_suite(
        std::slice::from_ref(&sc),
        &backend,
        &TestJudge::Served("first-ruler".into()),
        &opts,
    )
    .unwrap();
    assert_eq!(
        report.judge_model,
        Some(vec!["first-ruler".to_string()]),
        "{:?}",
        report.scenarios[0].trials[0].reasons
    );
    let run_dir = results_root
        .join("orig")
        .join("regrade-trace-test")
        .join("trial-0");

    // Re-score the frozen evidence with a different ruler, named by a file.
    let judge_dir =
        std::env::temp_dir().join(format!("zseval-regradetrace-j-{}", std::process::id()));
    std::fs::create_dir_all(&judge_dir).unwrap();
    let judge_file = judge_dir.join("second.toml");
    std::fs::write(&judge_file, b"model = \"second-ruler\"\n").unwrap();

    let tr = zseval::runner::regrade(
        &sc,
        &TestJudge::Served("second-ruler".into()),
        Some(&judge_file),
        false,
        0,
        &run_dir,
    )
    .unwrap();

    // The regraded trial names the ruler that produced it, so it can never be
    // read as having been graded by the one report.json still names.
    assert_eq!(tr.judge_model.as_deref(), Some("second-ruler"));
    assert_eq!(tr.judge_file, "second.toml");
    assert_eq!(
        tr.judge_hash,
        Some(zseval::util::fnv1a_hex(b"model = \"second-ruler\"\n"))
    );
    let persisted: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(run_dir.join("trial.json")).unwrap())
            .unwrap();
    assert_eq!(persisted["judge_file"], "second.toml");
    assert_eq!(persisted["judge_model"], "second-ruler");

    // The first judge's response survives, and the second's sits beside it
    // rather than on top of it.
    let first = std::fs::read_to_string(run_dir.join("judge-response.json")).unwrap();
    assert!(
        first.contains("first-ruler"),
        "the original judge's response was destroyed: {first}"
    );
    let regrade_responses: Vec<String> = std::fs::read_dir(&run_dir)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("regrade-"))
        .filter_map(|e| std::fs::read_to_string(e.path().join("judge-response.json")).ok())
        .collect();
    assert_eq!(regrade_responses.len(), 1, "{regrade_responses:?}");
    assert!(regrade_responses[0].contains("second-ruler"));

    std::fs::remove_dir_all(&judge_dir).ok();
    std::fs::remove_dir_all(&sc_dir).ok();
    std::fs::remove_dir_all(&results_root).ok();
}

/// A regrade that never calls a judge has no artifacts to protect, so it must
/// not litter the trial dir with an empty regrade folder.
#[test]
fn a_no_judge_regrade_leaves_no_regrade_artifacts_dir() {
    let sc_dir = std::env::temp_dir().join(format!("zseval-regradebare-sc-{}", std::process::id()));
    std::fs::create_dir_all(&sc_dir).unwrap();
    std::fs::write(
        sc_dir.join("scenario.toml"),
        "id = \"regrade-bare-test\"\ntask = \"hi\"\nexpect = [\"tool_not_called write\"]\n\
         judge = \"Did the agent stay read-only?\"\n",
    )
    .unwrap();
    let sc = Scenario::load(&sc_dir).unwrap();
    let results_root =
        std::env::temp_dir().join(format!("zseval-regradebare-results-{}", std::process::id()));
    let backend = Mock {
        fixture: fixture("session-ask-readonly.json"),
    };
    let opts = RunOptions {
        target: None,
        trials_override: Some(1),
        tag: "orig".into(),
        no_judge: true,
        results_root: results_root.clone(),
        max_total_usd: None,
        jobs: 1,
        judge_file: None,
        multi_target: false,
    };
    run_suite(
        std::slice::from_ref(&sc),
        &backend,
        &LlmJudge::new(test_judge_cfg()),
        &opts,
    )
    .unwrap();
    let run_dir = results_root
        .join("orig")
        .join("regrade-bare-test")
        .join("trial-0");

    zseval::runner::regrade(
        &sc,
        &LlmJudge::new(test_judge_cfg()),
        None,
        true,
        0,
        &run_dir,
    )
    .unwrap();

    let stray: Vec<_> = std::fs::read_dir(&run_dir)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("regrade-"))
        .collect();
    assert!(stray.is_empty(), "{stray:?}");

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
        target: None,
        trials_override: Some(1),
        tag: "orig".into(),
        no_judge: true,
        results_root: results_root.clone(),
        max_total_usd: None,
        jobs: 1,
        judge_file: None,
        multi_target: false,
    };
    let report = run_suite(&[sc], &backend, &LlmJudge::new(test_judge_cfg()), &opts).unwrap();
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

    let tr = zseval::runner::regrade(
        &tightened,
        &LlmJudge::new(test_judge_cfg()),
        None,
        true,
        0,
        &run_dir,
    )
    .unwrap();
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

    let sc_dir =
        std::env::temp_dir().join(format!("zseval-regrade-canon-sc-{}", std::process::id()));
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
    let tr = zseval::runner::regrade(
        &sc,
        &LlmJudge::new(test_judge_cfg()),
        None,
        true,
        0,
        &run_dir,
    )
    .unwrap();
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
    use zseval::verdict::{Report, ReportMeta, ScenarioResult, TrialResult};

    let meta = |tag: &str| ReportMeta {
        tag: tag.into(),
        model: "m".into(),
        backend: "b".into(),
        trials: 1,
        ..Default::default()
    };

    let mk = |tool_call_count| TrialResult {
        trial: 0,
        outcome: Final::Pass,
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
        tool_call_count,
        run_dir: String::new(),
    };

    let base = Report::build(
        meta("base"),
        vec![ScenarioResult::from_trials(
            "memory-search-then-read-when-needed".into(),
            vec![mk(2)],
        )],
    );
    let cand = Report::build(
        meta("cand"),
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
        meta("base"),
        vec![ScenarioResult::from_trials(
            "prompt-code-concise-answer".into(),
            vec![mk(0)],
        )],
    );
    let cand2 = Report::build(
        meta("cand"),
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

/// target-matrix section 2: `--target` becomes mandatory for the `zs`
/// backend — a missing target is a usage error (exit 2), fired before any
/// trial runs (no `--zs-bin`/`ZS_BIN` is given, so a later failure would be
/// for the wrong reason).
#[test]
fn run_with_zs_backend_and_no_target_exits_2_before_any_trial() {
    let dir = no_rubric_scenario_dir("zs-no-target");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_zseval"))
        .args(["run", dir.to_str().unwrap(), "--backend", "zs"])
        .env_remove("ZS_BIN")
        .output()
        .unwrap();
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--target"), "stderr: {stderr}");
}

/// target-matrix section 2: `--target` is rejected for the `mock` backend —
/// mock replays canned artifacts, so naming a target it would never read is
/// a usage error.
#[test]
fn run_with_mock_backend_and_target_exits_2() {
    let dir = no_rubric_scenario_dir("mock-with-target");
    let target_dir =
        std::env::temp_dir().join(format!("zseval-test-mock-target-{}", std::process::id()));
    std::fs::create_dir_all(&target_dir).unwrap();
    let target_path = target_dir.join("config.toml");
    std::fs::write(
        &target_path,
        "provider = \"anthropic\"\nmodel = \"claude-sonnet-4-6\"\n",
    )
    .unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_zseval"))
        .args([
            "run",
            dir.to_str().unwrap(),
            "--backend",
            &format!("mock={}", fixture("session-ask-readonly.json").display()),
            "--target",
            target_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&target_dir).ok();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--target"), "stderr: {stderr}");
}

/// target-matrix section 2: `--target` is unreachable for `mock`, so a mock
/// run's model is a fixed `"mock"` label, not derived from an absent target
/// (the old empty-target fallback label is now unreachable and deleted).
#[test]
fn a_completed_mock_run_records_model_mock() {
    let dir = no_rubric_scenario_dir("mock-model-label");
    let results = std::env::temp_dir().join(format!(
        "zseval-test-mock-model-results-{}",
        std::process::id()
    ));
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_zseval"))
        .args([
            "run",
            dir.to_str().unwrap(),
            "--backend",
            &format!("mock={}", fixture("session-ask-readonly.json").display()),
            "--results",
            results.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    std::fs::remove_dir_all(&dir).ok();
    let stdout = String::from_utf8_lossy(&out.stdout);
    std::fs::remove_dir_all(&results).ok();
    assert_ne!(out.status.code(), Some(2), "stdout: {stdout}");
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["model"], "mock", "stdout: {stdout}");
}

/// target-matrix 4.5: N>1 `--target` has no single JSON report to print, so
/// `run --json` is a usage error naming `zseval matrix --json` as the way to
/// render N reports as one table. This must fire before any backend/budget
/// setup (no `--zs-bin`/`ZS_BIN`, and the target files need not even exist),
/// so a later failure for a different reason can't be mistaken for this gate.
#[test]
fn run_json_with_more_than_one_target_exits_2_naming_matrix() {
    let dir = no_rubric_scenario_dir("json-multi-target");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_zseval"))
        .args([
            "run",
            dir.to_str().unwrap(),
            "--target",
            "a/opus.toml",
            "--target",
            "b/sonnet.toml",
            "--json",
        ])
        .env_remove("ZS_BIN")
        .output()
        .unwrap();
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("zseval matrix --json"), "stderr: {stderr}");
}

/// target-matrix 4.5: the N>1 `--json` gate is keyed on target *count*, not
/// on `--json` plus `--target` together — one `--target` with `--json` must
/// reach past it to the next check (no `--zs-bin`/`ZS_BIN`), never the
/// "matrix" usage error. Paired with `a_completed_mock_run_records_model_mock`
/// (N=1 via `--backend mock`, no `--target` at all) this covers both N=1
/// shapes; a live N=1 `zs --target --json` run needs a real `zerostack`
/// binary this harness does not have.
#[test]
fn run_json_with_exactly_one_target_does_not_trip_the_multi_target_gate() {
    let dir = no_rubric_scenario_dir("json-single-target");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_zseval"))
        .args([
            "run",
            dir.to_str().unwrap(),
            "--target",
            "a/opus.toml",
            "--json",
        ])
        .env_remove("ZS_BIN")
        .output()
        .unwrap();
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("zseval matrix --json"),
        "N=1 must not trip the multi-target JSON gate; stderr: {stderr}"
    );
    assert!(stderr.contains("--zs-bin"), "stderr: {stderr}");
}

/// target-matrix section 7: `matrix --json` is genuine end-to-end coverage
/// for the CLI wiring at main.rs — it exits 0, its stdout parses as JSON,
/// and the parsed value carries the matrix model itself (not just any
/// JSON), by asserting on a real field (`columns`).
#[test]
fn matrix_json_flag_exits_0_and_stdout_parses_as_the_matrix_model() {
    use zseval::verdict::{Final, Report, ReportMeta, ScenarioResult, TrialResult};

    let dir = std::env::temp_dir().join(format!("zseval-matrix-e2e-json-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let trial = TrialResult {
        trial: 0,
        outcome: Final::Pass,
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
    };
    let r = Report::build(
        ReportMeta {
            tag: "run-a".into(),
            model: "anthropic/opus".into(),
            backend: "zs".into(),
            trials: 1,
            target: "targets/opus.toml".into(),
            ..Default::default()
        },
        vec![ScenarioResult::from_trials("s".into(), vec![trial])],
    );
    let report_path = dir.join("a.json");
    std::fs::write(&report_path, serde_json::to_string_pretty(&r).unwrap()).unwrap();

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_zseval"))
        .args(["matrix", report_path.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    std::fs::remove_dir_all(&dir).ok();

    assert_eq!(out.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("stdout did not parse as JSON: {e}"));
    let columns = v["columns"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a `columns` array in the matrix model: {v}"));
    assert_eq!(columns.len(), 1, "matrix json: {v}");
}

/// target-matrix section 7: `matrix --markdown` is genuine end-to-end
/// coverage for the CLI wiring at main.rs:877-878 — `render_markdown` is
/// unit-tested in matrix.rs, but the flag itself was never exercised
/// through the binary until now.
#[test]
fn matrix_markdown_flag_exits_0_and_stdout_is_a_markdown_table() {
    use zseval::verdict::{Final, Report, ReportMeta, ScenarioResult, TrialResult};

    let dir =
        std::env::temp_dir().join(format!("zseval-matrix-e2e-markdown-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let trial = TrialResult {
        trial: 0,
        outcome: Final::Pass,
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
    };
    let r = Report::build(
        ReportMeta {
            tag: "run-a".into(),
            model: "anthropic/opus".into(),
            backend: "zs".into(),
            trials: 1,
            target: "targets/opus.toml".into(),
            ..Default::default()
        },
        vec![ScenarioResult::from_trials(
            "markdown-scenario".into(),
            vec![trial],
        )],
    );
    let report_path = dir.join("a.json");
    std::fs::write(&report_path, serde_json::to_string_pretty(&r).unwrap()).unwrap();

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_zseval"))
        .args(["matrix", report_path.to_str().unwrap(), "--markdown"])
        .output()
        .unwrap();
    std::fs::remove_dir_all(&dir).ok();

    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("|---|"), "stdout: {stdout}");
    assert!(stdout.contains("markdown-scenario"), "stdout: {stdout}");
}

/// target-matrix section 7: with no format flag, `matrix` renders the fixed
/// -width form, not markdown — the absence of the markdown separator row
/// distinguishes the two renderers through the actual CLI wiring.
#[test]
fn matrix_with_no_format_flag_renders_fixed_width_not_markdown() {
    use zseval::verdict::{Final, Report, ReportMeta, ScenarioResult, TrialResult};

    let dir =
        std::env::temp_dir().join(format!("zseval-matrix-e2e-default-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let trial = TrialResult {
        trial: 0,
        outcome: Final::Pass,
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
    };
    let r = Report::build(
        ReportMeta {
            tag: "run-a".into(),
            model: "anthropic/opus".into(),
            backend: "zs".into(),
            trials: 1,
            target: "targets/opus.toml".into(),
            ..Default::default()
        },
        vec![ScenarioResult::from_trials(
            "fixed-width-scenario".into(),
            vec![trial],
        )],
    );
    let report_path = dir.join("a.json");
    std::fs::write(&report_path, serde_json::to_string_pretty(&r).unwrap()).unwrap();

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_zseval"))
        .args(["matrix", report_path.to_str().unwrap()])
        .output()
        .unwrap();
    std::fs::remove_dir_all(&dir).ok();

    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.contains("|---|"), "stdout: {stdout}");
    assert!(stdout.contains("fixed-width-scenario"), "stdout: {stdout}");
}

/// target-matrix design decision: `matrix` treats an all-holes / empty column
/// as ungradable and exits 2. A report with a valid target but *zero*
/// scenarios yields an empty column that contributes nothing but holes, so the
/// matrix is ungradable even though the report's own `exit_code` would be 0.
#[test]
fn matrix_over_a_zero_scenario_report_exits_2() {
    use zseval::verdict::{Report, ReportMeta};

    let dir = std::env::temp_dir().join(format!("zseval-matrix-e2e-empty-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let r = Report::build(
        ReportMeta {
            tag: "run-a".into(),
            model: "anthropic/opus".into(),
            backend: "zs".into(),
            trials: 1,
            target: "targets/opus.toml".into(),
            ..Default::default()
        },
        vec![],
    );
    let report_path = dir.join("a.json");
    std::fs::write(&report_path, serde_json::to_string_pretty(&r).unwrap()).unwrap();

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_zseval"))
        .args(["matrix", report_path.to_str().unwrap()])
        .output()
        .unwrap();
    std::fs::remove_dir_all(&dir).ok();

    assert_eq!(
        out.status.code(),
        Some(2),
        "a zero-scenario column is ungradable"
    );
}

/// A prompt pack directory for the `--prompts` CLI tests: one `code.md`, plus
/// whatever extra entries a case needs to make illegal.
fn prompt_pack_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "zseval-test-prompt-pack-{name}-{}",
        std::process::id()
    ));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("code.md"), "# code\nemit MARKER on line one.\n").unwrap();
    dir
}

/// prompts-pack 1.4 (spec `prompts-pack-run`, "run accepts a single prompt
/// pack"): `--prompts` is single-arity, the opposite of `--target`. A run
/// evaluates exactly one pack; two packs is two runs joined by `matrix`. The
/// gate fires before anything reads the directories, so the paths given here
/// need not exist.
#[test]
fn run_with_two_prompts_flags_exits_2_naming_the_single_arity_rule() {
    let dir = no_rubric_scenario_dir("two-prompts");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_zseval"))
        .args([
            "run",
            dir.to_str().unwrap(),
            "--prompts",
            "packs/a",
            "--prompts",
            "packs/b",
        ])
        .env_remove("ZS_BIN")
        .output()
        .unwrap();
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--prompts"), "stderr: {stderr}");
    assert!(stderr.contains("at most once"), "stderr: {stderr}");
    assert!(stderr.contains("matrix"), "stderr: {stderr}");
}

/// prompts-pack 1.4 (spec `prompts-pack-run`, "--prompts is rejected under
/// the mock backend"): mock replays canned artifacts and never constructs a
/// zerostack invocation, so a pack could not possibly be read — accepting the
/// flag would produce a report advertising a pack nothing loaded. Mirrors the
/// existing `--target` rejection under mock.
#[test]
fn run_with_mock_backend_and_prompts_exits_2() {
    let dir = no_rubric_scenario_dir("mock-with-prompts");
    let pack = prompt_pack_dir("mock");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_zseval"))
        .args([
            "run",
            dir.to_str().unwrap(),
            "--backend",
            &format!("mock={}", fixture("session-ask-readonly.json").display()),
            "--prompts",
            pack.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&pack).ok();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--prompts"), "stderr: {stderr}");
    assert!(stderr.contains("mock"), "stderr: {stderr}");
}

/// prompts-pack 1.4 (spec `prompts-pack-run`, "a pack contains only files
/// zerostack will read"): a malformed pack is a usage error before any trial
/// spends money. No `--zs-bin`/`ZS_BIN` is given, so if the pack were
/// validated after backend setup this would fail for the wrong reason — the
/// message assertion is what tells the two apart.
#[test]
fn run_with_a_pack_containing_a_subdirectory_exits_2_before_any_trial() {
    let dir = no_rubric_scenario_dir("pack-subdir");
    let pack = prompt_pack_dir("subdir");
    std::fs::create_dir_all(pack.join("nested")).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_zseval"))
        .args([
            "run",
            dir.to_str().unwrap(),
            "--target",
            "targets/anthropic.toml",
            "--prompts",
            pack.to_str().unwrap(),
        ])
        .env_remove("ZS_BIN")
        .output()
        .unwrap();
    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&pack).ok();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("nested"), "stderr: {stderr}");
    assert!(stderr.contains("top-level *.md"), "stderr: {stderr}");
    assert!(
        !stderr.contains("--zs-bin"),
        "the pack must be validated before backend setup; stderr: {stderr}"
    );
}

/// prompts-pack 1.5, the control for the three rejections above: a *valid*
/// pack passes validation and the run reaches the next check (no
/// `--zs-bin`/`ZS_BIN`), so the exit-2s above are the pack gates firing
/// rather than `--prompts` being rejected out of hand.
#[test]
fn run_with_a_valid_pack_passes_validation_and_reaches_the_next_check() {
    let dir = no_rubric_scenario_dir("valid-pack");
    let pack = prompt_pack_dir("valid");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_zseval"))
        .args([
            "run",
            dir.to_str().unwrap(),
            "--target",
            "targets/anthropic.toml",
            "--prompts",
            pack.to_str().unwrap(),
        ])
        .env_remove("ZS_BIN")
        .output()
        .unwrap();
    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&pack).ok();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("prompt pack"),
        "a valid pack must not be rejected; stderr: {stderr}"
    );
    assert!(stderr.contains("--zs-bin"), "stderr: {stderr}");
}

/// prompts-pack 1.5, the second control: the same suite with no `--prompts`
/// at all still runs to completion and exits 0, so "exit 2" above means the
/// flag's own gates and not a suite that could never have passed.
#[test]
fn run_without_prompts_still_exits_0() {
    let dir = no_rubric_scenario_dir("no-prompts-control");
    let results = std::env::temp_dir().join(format!(
        "zseval-test-no-prompts-control-results-{}",
        std::process::id()
    ));
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_zseval"))
        .args([
            "run",
            dir.to_str().unwrap(),
            "--backend",
            &format!("mock={}", fixture("session-ask-readonly.json").display()),
            "--results",
            results.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    std::fs::remove_dir_all(&dir).ok();
    let stderr = String::from_utf8_lossy(&out.stderr);
    std::fs::remove_dir_all(&results).ok();
    assert_eq!(out.status.code(), Some(0), "stderr: {stderr}");
}

// ---------------------------------------------------------------------------
// prompts-pack 2: seeding the pack into every trial
// ---------------------------------------------------------------------------

/// Write an executable stub `--zs-bin` script (`.claude/skills/verify/SKILL.md`)
/// that, on every invocation, appends a listing of `.zerostack/prompts/` as
/// seen from its own working directory — the same directory `ZsCli::run`
/// sets via `cmd.current_dir` — to `listing_log`, then completes the call
/// with a canned session file so `run_print` doesn't error on a missing one.
fn write_stub_zs_bin(bin: &Path, listing_log: &Path) {
    let script = format!(
        "#!/usr/bin/env bash\nset -euo pipefail\n{{\n  if [ -d .zerostack/prompts ]; then\n    ls .zerostack/prompts | sort | paste -sd, -\n  else\n    echo '(none)'\n  fi\n}} >> \"{log}\"\nmkdir -p \"$ZS_DATA_DIR/sessions\"\ncp \"{fixture}\" \"$ZS_DATA_DIR/sessions/s.json\"\n",
        log = listing_log.display(),
        fixture = fixture("session-ask-readonly.json").display(),
    );
    std::fs::write(bin, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// A scratch dir for a `ZsCli` test, named after the test so parallel runs
/// don't collide.
fn scratch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("zseval-test-zscli-{name}-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// prompts-pack 2.1 (spec `prompts-pack-run`, "The pack is seeded into every
/// trial"): every trial gets its own `ZsCli::run` call with a fresh run_dir
/// (the trial loop lives in `runner::run_trials_for_scenario`, outside
/// `ZsCli` itself), so calling `run` twice with two run_dirs stands in for
/// two trials. The stub records what its own cwd looked like, proving the
/// pack lands where the child process actually looks, not merely somewhere
/// under the run_dir.
#[test]
fn zscli_seeds_the_pack_into_every_trial() {
    let pack_dir = scratch_dir("seed-pack-src");
    std::fs::write(pack_dir.join("code.md"), "code body\n").unwrap();
    std::fs::write(pack_dir.join("review.md"), "review body\n").unwrap();
    let pack = PromptPack::load(&pack_dir).unwrap();

    let stub = scratch_dir("seed-stub").join("fake-zs");
    let listing_log = scratch_dir("seed-log").join("listing.log");
    write_stub_zs_bin(&stub, &listing_log);

    let sc_dir = no_rubric_scenario_dir("seed-every-trial");
    let sc = Scenario::load(&sc_dir).unwrap();

    let backend = ZsCli {
        bin: stub,
        target: None,
        prompts: Some(std::sync::Arc::new(pack)),
    };

    for trial in 0..2 {
        let run_dir = scratch_dir(&format!("seed-run-{trial}"));
        backend.run(&sc, &run_dir).unwrap();
    }

    let listing = std::fs::read_to_string(&listing_log).unwrap();
    let lines: Vec<&str> = listing.lines().collect();
    assert_eq!(lines.len(), 2, "listing: {listing}");
    for line in lines {
        assert_eq!(line, "code.md,review.md", "listing: {listing}");
    }

    std::fs::remove_dir_all(&sc_dir).ok();
}

/// prompts-pack 2.2 (spec `prompts-pack-run`, "A scenario's own prompt seed
/// wins over the pack"): the pack is copied before `seed::apply`, so a
/// scenario placement with the same destination lands last and wins.
#[test]
fn scenario_seeded_prompt_overrides_the_pack() {
    let pack_dir = scratch_dir("precedence-pack-src");
    std::fs::write(pack_dir.join("code.md"), "pack body\n").unwrap();
    let pack = PromptPack::load(&pack_dir).unwrap();

    let sc_dir = scratch_dir("precedence-sc");
    std::fs::write(sc_dir.join("scenario-code.md"), "scenario body\n").unwrap();
    std::fs::write(
        sc_dir.join("scenario.toml"),
        "id = \"precedence\"\ntask = \"say hi\"\nexpect = [\"tool_not_called write\"]\n\n\
         [[files]]\nsrc = \"scenario-code.md\"\ndest = \"work:.zerostack/prompts/code.md\"\n",
    )
    .unwrap();
    let sc = Scenario::load(&sc_dir).unwrap();

    let stub = scratch_dir("precedence-stub").join("fake-zs");
    let listing_log = scratch_dir("precedence-log").join("listing.log");
    write_stub_zs_bin(&stub, &listing_log);

    let backend = ZsCli {
        bin: stub,
        target: None,
        prompts: Some(std::sync::Arc::new(pack)),
    };

    let run_dir = scratch_dir("precedence-run");
    backend.run(&sc, &run_dir).unwrap();

    let content =
        std::fs::read_to_string(run_dir.join("work/.zerostack/prompts/code.md")).unwrap();
    assert_eq!(content, "scenario body\n");

    std::fs::remove_dir_all(&sc_dir).ok();
}

/// prompts-pack 2.3 (spec `prompts-pack-run`, "Without the flag nothing is
/// seeded"): with no pack, `ZsCli::run` must not create
/// `work:.zerostack/prompts/` at all — a scenario placing its own file there
/// still can (that's a scenario concern, not the harness's).
#[test]
fn zscli_without_a_pack_creates_no_prompts_dir() {
    let sc_dir = no_rubric_scenario_dir("no-pack-no-dir");
    let sc = Scenario::load(&sc_dir).unwrap();

    let stub = scratch_dir("no-pack-stub").join("fake-zs");
    let listing_log = scratch_dir("no-pack-log").join("listing.log");
    write_stub_zs_bin(&stub, &listing_log);

    let backend = ZsCli {
        bin: stub,
        target: None,
        prompts: None,
    };

    let run_dir = scratch_dir("no-pack-run");
    backend.run(&sc, &run_dir).unwrap();

    assert!(!run_dir.join("work/.zerostack/prompts").exists());

    std::fs::remove_dir_all(&sc_dir).ok();
}

// ---------------------------------------------------------------------------
// prompts-pack 4: the pack's identity on the report
// ---------------------------------------------------------------------------

/// A `RunOptions` for a stub-`ZsCli` run: single trial, no judge (the
/// scenario's deterministic assert is the only ruler), scratch results dir.
fn pack_run_opts(tag: &str, results_root: PathBuf) -> RunOptions {
    RunOptions {
        target: None,
        trials_override: Some(1),
        tag: tag.into(),
        no_judge: true,
        results_root,
        max_total_usd: None,
        jobs: 1,
        judge_file: None,
        multi_target: false,
    }
}

/// prompts-pack 4.1 (spec `prompts-pack-identity`, "Report carries the pack's
/// identity"): a run with a pack records the working-directory-relative,
/// forward-slashed path, the fingerprint, and the sorted prompt names. The
/// identity comes from the backend that actually seeded the pack, derived
/// inside `run_suite`, so this drives a real `run_suite` against a stub bin
/// rather than building a `Report` by hand.
#[test]
fn a_pack_run_records_its_relative_path_hash_and_sorted_names() {
    // Under the working directory, so the recorded path is relative.
    let cwd = std::env::current_dir().unwrap();
    let pack_dir = cwd.join(format!("zseval-test-pack-identity-{}", std::process::id()));
    std::fs::remove_dir_all(&pack_dir).ok();
    std::fs::create_dir_all(&pack_dir).unwrap();
    std::fs::write(pack_dir.join("review.md"), "review body\n").unwrap();
    std::fs::write(pack_dir.join("code.md"), "code body\n").unwrap();
    let pack = PromptPack::load(&pack_dir).unwrap();
    let expected_hash = pack.fingerprint();

    let stub = scratch_dir("pack-id-stub").join("fake-zs");
    let listing_log = scratch_dir("pack-id-log").join("listing.log");
    write_stub_zs_bin(&stub, &listing_log);

    let sc_dir = no_rubric_scenario_dir("pack-identity");
    let sc = Scenario::load(&sc_dir).unwrap();
    let results_root = scratch_dir("pack-id-results");

    let backend = ZsCli {
        bin: stub,
        target: None,
        prompts: Some(std::sync::Arc::new(pack)),
    };
    let opts = pack_run_opts("pack-id", results_root.clone());
    let report = run_suite(&[sc], &backend, &LlmJudge::new(test_judge_cfg()), &opts).unwrap();

    let expected_path = format!("zseval-test-pack-identity-{}", std::process::id());
    assert_eq!(report.prompts_pack, expected_path);
    assert!(!report.prompts_pack.starts_with('/'), "{}", report.prompts_pack);
    assert_eq!(report.prompts_hash, expected_hash);
    assert_eq!(report.prompts_names, vec!["code", "review"]);

    std::fs::remove_dir_all(&pack_dir).ok();
    std::fs::remove_dir_all(&sc_dir).ok();
    std::fs::remove_dir_all(&results_root).ok();
}

/// prompts-pack 4.1 (spec `prompts-pack-identity`, the path rule): a pack
/// outside the working directory records its bare directory name, never an
/// absolute path — a report is meant to be committed into `baselines/`, so it
/// must not become a map of someone's filesystem.
#[test]
fn a_pack_outside_the_working_directory_records_its_bare_name() {
    let pack_dir = std::env::temp_dir().join(format!(
        "zseval-test-pack-outside-{}",
        std::process::id()
    ));
    std::fs::remove_dir_all(&pack_dir).ok();
    std::fs::create_dir_all(&pack_dir).unwrap();
    std::fs::write(pack_dir.join("code.md"), "code body\n").unwrap();
    let pack = PromptPack::load(&pack_dir).unwrap();

    let stub = scratch_dir("pack-out-stub").join("fake-zs");
    let listing_log = scratch_dir("pack-out-log").join("listing.log");
    write_stub_zs_bin(&stub, &listing_log);

    let sc_dir = no_rubric_scenario_dir("pack-outside");
    let sc = Scenario::load(&sc_dir).unwrap();
    let results_root = scratch_dir("pack-out-results");

    let backend = ZsCli {
        bin: stub,
        target: None,
        prompts: Some(std::sync::Arc::new(pack)),
    };
    let opts = pack_run_opts("pack-out", results_root.clone());
    let report = run_suite(&[sc], &backend, &LlmJudge::new(test_judge_cfg()), &opts).unwrap();

    let expected_name = format!("zseval-test-pack-outside-{}", std::process::id());
    assert_eq!(report.prompts_pack, expected_name);
    assert!(!report.prompts_pack.starts_with('/'), "{}", report.prompts_pack);

    std::fs::remove_dir_all(&pack_dir).ok();
    std::fs::remove_dir_all(&sc_dir).ok();
    std::fs::remove_dir_all(&results_root).ok();
}

/// prompts-pack 4.2 (spec `prompts-pack-identity`, "A run without a pack
/// records empties"): a `mock` run (which never carries a pack) records the
/// three fields empty on its report.
#[test]
fn a_run_without_a_pack_records_empty_pack_identity() {
    let sc_dir = no_rubric_scenario_dir("no-pack-identity");
    let sc = Scenario::load(&sc_dir).unwrap();
    let results_root = scratch_dir("no-pack-id-results");

    let backend = Mock {
        fixture: fixture("session-ask-readonly.json"),
    };
    let opts = pack_run_opts("no-pack-id", results_root.clone());
    let report = run_suite(&[sc], &backend, &LlmJudge::new(test_judge_cfg()), &opts).unwrap();

    assert_eq!(report.prompts_pack, "");
    assert_eq!(report.prompts_hash, "");
    assert!(report.prompts_names.is_empty());

    std::fs::remove_dir_all(&sc_dir).ok();
    std::fs::remove_dir_all(&results_root).ok();
}

// ---------------------------------------------------------------------------
// prompts-pack 6: resolve which prompt each scenario actually loaded
// ---------------------------------------------------------------------------

/// prompts-pack 6.5 (spec `prompts-pack-identity`, "Each scenario records the
/// prompt it actually loaded"): drive a mixed suite through a stub bin with a
/// one-prompt pack (`code.md`) and read the per-scenario `prompt_name` /
/// `prompt_source` back off the emitted report. Four scenarios span three
/// sources, so the fields are shown to discriminate rather than print one
/// constant: a declared name the pack provides (`pack`), a declared name it
/// lacks (`stock`), a scenario seeding its own same-named file (`scenario`),
/// and a no-prompt scenario whose derived `code` default the pack provides
/// (`pack`).
#[test]
fn a_mixed_suite_records_a_distinct_prompt_source_per_scenario() {
    use zseval::verdict::PromptSource;

    let pack_dir = scratch_dir("s6-pack-src");
    std::fs::write(pack_dir.join("code.md"), "pack code body\n").unwrap();
    let pack = PromptPack::load(&pack_dir).unwrap();

    let stub = scratch_dir("s6-stub").join("fake-zs");
    let listing_log = scratch_dir("s6-log").join("listing.log");
    write_stub_zs_bin(&stub, &listing_log);

    // A: declares `code`, which the pack provides -> pack.
    let a_dir = scratch_dir("s6-a");
    std::fs::write(
        a_dir.join("scenario.toml"),
        "id = \"declares-code\"\nprompt = \"code\"\ntask = \"say hi\"\n\
         expect = [\"tool_not_called write\"]\n",
    )
    .unwrap();

    // B: declares `ask`, which the pack lacks -> stock.
    let b_dir = scratch_dir("s6-b");
    std::fs::write(
        b_dir.join("scenario.toml"),
        "id = \"declares-ask\"\nprompt = \"ask\"\ntask = \"say hi\"\n\
         expect = [\"tool_not_called write\"]\n",
    )
    .unwrap();

    // C: declares `code` but seeds its own file for it -> scenario.
    let c_dir = scratch_dir("s6-c");
    std::fs::write(c_dir.join("scenario-code.md"), "scenario code body\n").unwrap();
    std::fs::write(
        c_dir.join("scenario.toml"),
        "id = \"seeds-own-code\"\nprompt = \"code\"\ntask = \"say hi\"\n\
         expect = [\"tool_not_called write\"]\n\n\
         [[files]]\nsrc = \"scenario-code.md\"\ndest = \"work:.zerostack/prompts/code.md\"\n",
    )
    .unwrap();

    // D: no prompt, no target -> derives `code`, which the pack provides -> pack.
    let d_dir = scratch_dir("s6-d");
    std::fs::write(
        d_dir.join("scenario.toml"),
        "id = \"no-prompt\"\ntask = \"say hi\"\nexpect = [\"tool_not_called write\"]\n",
    )
    .unwrap();

    let scenarios: Vec<Scenario> = [&a_dir, &b_dir, &c_dir, &d_dir]
        .iter()
        .map(|d| Scenario::load(d).unwrap())
        .collect();

    let results_root = scratch_dir("s6-results");
    let backend = ZsCli {
        bin: stub,
        target: None,
        prompts: Some(std::sync::Arc::new(pack)),
    };
    let opts = pack_run_opts("s6", results_root.clone());
    let report = run_suite(&scenarios, &backend, &LlmJudge::new(test_judge_cfg()), &opts).unwrap();

    let got = |id: &str| {
        let r = report.scenarios.iter().find(|r| r.id == id).unwrap();
        (r.prompt_name.clone(), r.prompt_source)
    };
    assert_eq!(got("declares-code"), ("code".into(), PromptSource::Pack));
    assert_eq!(got("declares-ask"), ("ask".into(), PromptSource::Stock));
    assert_eq!(got("seeds-own-code"), ("code".into(), PromptSource::Scenario));
    assert_eq!(got("no-prompt"), ("code".into(), PromptSource::Pack));

    let mut sources: Vec<String> = report
        .scenarios
        .iter()
        .map(|r| format!("{:?}", r.prompt_source))
        .collect();
    sources.sort();
    sources.dedup();
    assert!(sources.len() >= 2, "expected >=2 distinct sources: {sources:?}");

    for d in [&pack_dir, &a_dir, &b_dir, &c_dir, &d_dir, &results_root] {
        std::fs::remove_dir_all(d).ok();
    }
}
