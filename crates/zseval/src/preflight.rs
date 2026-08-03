//! The free half of a run's prerequisites, checked together before anything is
//! spent.
//!
//! A run stands on four legs — a zerostack binary to drive, a key for the
//! provider each `--target` names, a decision about who grades the subjective
//! layer, and (if a ruler was named) a judge that actually answers. The first
//! three cost nothing to check, so they are checked *together* here and
//! reported as one list: a first-day setup is usually missing more than one,
//! and discovering them one failed run at a time is the experience this gate
//! exists to delete. Only the fourth costs money, so it stays where it is —
//! behind this gate, run after everything free has passed.
//!
//! Two severities, deliberately distinct. A *problem* is a prerequisite that is
//! definitely absent, and any one of them refuses the run. A *warning* is
//! something this harness could not verify — an older binary that announces no
//! feature set, a target whose provider is a gateway zseval can't read a key
//! requirement out of — and never blocks: a check that cannot be made is not a
//! failed check, and treating missing information as a missing prerequisite
//! would make the gate wrong in the one direction it must not be.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::backend::AgentBackend;
use crate::target::TargetKey;

/// The zerostack build features this harness's scenarios need compiled in.
/// `memory` is the one that forces the issue — it is not one of zerostack's
/// default features, so a plain `cargo build` produces a binary whose memory
/// tools never register and whose memory scenarios can only grade
/// indeterminate. The other three are defaults today, listed anyway because
/// what this harness depends on should not silently change when upstream's
/// default set does.
pub const REQUIRED_FEATURES: &[&str] = &["memory", "mcp", "subagents", "loop"];

/// The rebuild every build complaint here ends with, named in full so it can be
/// pasted rather than reconstructed.
const REBUILD: &str = "rebuild it with `cargo build --all-features` in the zerostack checkout \
                       and point ZS_BIN (or --zs-bin) at the result";

/// Where judge files conventionally live, relative to the working directory.
pub const JUDGES_DIR: &str = "judges";

/// At most this many scenario ids are spelled out before the rest are counted.
const MAX_LISTED_IDS: usize = 5;

/// Everything the free checks found, collected rather than raised one at a
/// time. Built empty, handed to each leg in turn, and cashed in by `finish`.
#[derive(Debug, Default)]
pub struct Preflight {
    problems: Vec<String>,
    warnings: Vec<String>,
}

impl Preflight {
    pub fn new() -> Preflight {
        Preflight::default()
    }

    /// A prerequisite that is definitely missing: the run will not start.
    pub fn problem(&mut self, text: impl Into<String>) {
        self.problems.push(text.into());
    }

    /// Something that could not be verified. Says so and lets the run proceed.
    pub fn warn(&mut self, text: impl Into<String>) {
        self.warnings.push(text.into());
    }

    pub fn problems(&self) -> &[String] {
        &self.problems
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// The one aggregated report, or `None` when nothing is missing. The header
    /// leads with the fact that costs the reader most to be unsure about —
    /// nothing ran, nothing was spent — and the footer says the list is
    /// complete, so the reader fixes everything in one pass instead of
    /// discovering the next item on the next run.
    pub fn report(&self) -> Option<String> {
        if self.problems.is_empty() {
            return None;
        }
        let n = self.problems.len();
        let subject = if n == 1 {
            "1 prerequisite is".to_string()
        } else {
            format!("{n} prerequisites are")
        };
        let mut out =
            format!("cannot start this run: {subject} missing. Nothing ran and nothing was spent.");
        for (i, p) in self.problems.iter().enumerate() {
            out.push_str(&format!("\n\n  {}. {p}", i + 1));
        }
        out.push_str(
            "\n\nEvery check above is free and all of them ran, so this list is complete: fix \
             them together rather than one run at a time.",
        );
        Some(out)
    }

    /// Emit the warnings on `err`, then fail with the aggregated report if
    /// anything is missing. Warnings go out either way — they are context the
    /// reader needs whether or not the run proceeds.
    pub fn finish(self, err: &mut impl Write) -> anyhow::Result<()> {
        for w in &self.warnings {
            writeln!(err, "⚠ {w}")?;
        }
        match self.report() {
            Some(report) => anyhow::bail!(report),
            None => Ok(()),
        }
    }
}

/// The binary leg: which zerostack will be driven, does it run, and is it the
/// build this harness needs. Skipped entirely for `--backend mock`, which
/// replays canned artifacts and drives no binary at all.
///
/// `bin` is the already-resolved `--zs-bin`/`ZS_BIN` path, or `None` when
/// neither named one.
pub fn check_binary(pre: &mut Preflight, bin: Option<&Path>) {
    let Some(bin) = bin else {
        pre.problem(
            "no zerostack binary: neither --zs-bin nor the ZS_BIN environment variable names \
             one. Pass --zs-bin <path>, export ZS_BIN=<path>, or replay canned artifacts \
             instead with --backend mock=<file>",
        );
        return;
    };
    // The same probe the run itself makes to stamp identity on the report,
    // made here first so an unrunnable binary is named alongside everything
    // else missing rather than aborting the run on its own later.
    match (crate::backend::ZsCli {
        bin: bin.to_path_buf(),
        target: None,
        prompts: None,
    })
    .identity()
    {
        Ok(id) => check_features(pre, bin, id.features.as_deref()),
        Err(e) => pre.problem(format!("{e:#}")),
    }
}

/// Rule on the feature set a binary reported. Three outcomes, and the middle
/// one is the reason this is not a boolean: a build that announces its features
/// and is missing one this harness needs is a hard problem, a build that
/// announces a complete set passes, and a build that announces nothing at all
/// is unverifiable — a warning, because the alternative is refusing to run
/// against every binary that predates the banner.
fn check_features(pre: &mut Preflight, bin: &Path, features: Option<&[String]>) {
    let Some(features) = features else {
        pre.warn(format!(
            "{}: this build announces no feature set, so zseval cannot verify it is an \
             --all-features build. A build missing `memory` (not a zerostack default) grades \
             those scenarios indeterminate rather than failing them, so a run can look thin \
             for a reason that is not the agent's; if the numbers surprise you, {REBUILD}",
            bin.display()
        ));
        return;
    };
    let missing: Vec<&str> = REQUIRED_FEATURES
        .iter()
        .copied()
        .filter(|want| !features.iter().any(|have| have == want))
        .collect();
    if !missing.is_empty() {
        pre.problem(format!(
            "{}: this build is missing the feature(s) {} that this suite needs (it announces {}). \
             To fix, {REBUILD}",
            bin.display(),
            missing.join(", "),
            features.join(", "),
        ));
    }
}

/// The agent-key leg: every `--target` names a provider, and every provider
/// that needs a key needs it *before* the first target runs — a two-target run
/// must not spend its way through target 1 only to find target 2's key missing.
/// A key embedded in the target file itself is refused as its own problem: the
/// run would work, and that is the trap. Skipped entirely for `--backend
/// mock`, which calls no provider.
///
/// `get_env` is how the environment is read, so the rule (unset or empty is
/// missing) can be exercised without a test mutating the process's own
/// environment.
pub fn check_target_keys(
    pre: &mut Preflight,
    targets: &[PathBuf],
    get_env: impl Fn(&str) -> Option<String>,
) {
    if targets.is_empty() {
        pre.problem(
            "--target is required for --backend zs: pass a zerostack config.toml naming what \
             to evaluate against",
        );
        return;
    }
    for target in targets {
        // Checked before the variable itself: zerostack's own key resolution
        // falls back to a config-file `[api_keys]` table, so a key written
        // into the target would make the run work — and a target file lives
        // committed beside the suite, so a key that works from inside one is
        // a secret in git. Refused outright, whatever the environment holds.
        let embedded = crate::target::embedded_api_keys(target);
        if !embedded.is_empty() {
            pre.problem(format!(
                "{}: its [api_keys] table embeds a key value for {} — zerostack would accept \
                 that, but a target file lives committed beside the suite, so a key inside one \
                 is a secret in git. Delete it and export the provider's key variable instead",
                target.display(),
                embedded.join(", "),
            ));
        }
        match crate::target::key_requirement(target) {
            Ok(TargetKey::Required { provider, var }) => {
                let held = get_env(&var).filter(|v| !v.trim().is_empty());
                // With an embedded key on file, "unset" would misdescribe the
                // situation — zerostack would have run on the embedded key —
                // so the embedding problem above speaks for this target alone.
                if held.is_none() && embedded.is_empty() {
                    pre.problem(format!(
                        "{}: names provider '{provider}', whose key is read from {var} — that \
                         variable is unset or empty. Export it (`export {var}=…`) before running; \
                         the key belongs in the environment, never in the target file",
                        target.display()
                    ));
                }
            }
            // Nothing to check, and nothing worth saying: a keyless provider is
            // a normal target, not a degraded one.
            Ok(TargetKey::Keyless { .. }) => {}
            Ok(TargetKey::Undetermined { reason }) => pre.warn(format!(
                "{reason}. The run will proceed; zerostack itself will report a missing key if \
                 there is one"
            )),
            Err(e) => pre.problem(format!("{e:#}")),
        }
    }
}

/// The judge-decision leg's message: a suite with judge-graded scenarios is
/// refused until the caller says who grades them. Names the scenarios that
/// forced the choice (so "which ones?" needs no second command) and lists the
/// judge files sitting in `judges_dir` right now.
///
/// It never picks one. Which ruler grades a batch decides what the numbers
/// mean, so a default judge — even a helpfully-obvious single candidate — would
/// be the harness quietly answering an experimenter's question. Candidates are
/// offered; the choice stays with the caller.
pub fn judge_decision_needed(judge_graded_ids: &[&str], judges_dir: &Path) -> String {
    let n = judge_graded_ids.len();
    let shown: Vec<&str> = judge_graded_ids
        .iter()
        .copied()
        .take(MAX_LISTED_IDS)
        .collect();
    let listed = match n.saturating_sub(shown.len()) {
        0 => shown.join(", "),
        rest => format!("{}, and {rest} more", shown.join(", ")),
    };
    let subject = if n == 1 {
        "1 judge-graded scenario".to_string()
    } else {
        format!("{n} judge-graded scenarios")
    };
    format!(
        "this suite has {subject} ({listed}): pass --judge <file> to name a ruler, or --no-judge \
         to grade the deterministic asserts only. {}. See {}/README.md",
        judge_candidates(judges_dir),
        judges_dir.display(),
    )
}

/// The `--judge` candidates a caller could name, read out of `judges_dir` as it
/// is right now — a real listing, not a guess about what is committed.
fn judge_candidates(judges_dir: &Path) -> String {
    let mut found: Vec<String> = std::fs::read_dir(judges_dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .map(|p| p.display().to_string())
        .collect();
    found.sort();
    if found.is_empty() {
        return format!(
            "Judge files conventionally live in {}/, which holds none right now",
            judges_dir.display()
        );
    }
    format!(
        "Candidates in {}/: {} — zseval never picks one for you, because which ruler grades a \
         batch is what its scores mean",
        judges_dir.display(),
        found.join(", "),
    )
}

#[cfg(test)]
mod aggregation_tests {
    use super::*;

    #[test]
    fn nothing_missing_reports_nothing_and_still_emits_warnings() {
        let mut pre = Preflight::new();
        pre.warn("could not verify the build");
        assert!(pre.report().is_none());

        let mut err = Vec::new();
        pre.finish(&mut err).unwrap();
        let text = String::from_utf8(err).unwrap();
        assert_eq!(text, "⚠ could not verify the build\n");
    }

    /// Several missing legs at once come back as one numbered list under one
    /// header, in the order they were found — the whole point of the gate.
    #[test]
    fn every_problem_appears_once_in_one_numbered_report() {
        let mut pre = Preflight::new();
        pre.problem("no zerostack binary");
        pre.problem("ANTHROPIC_API_KEY is unset");
        pre.problem("this suite has 2 judge-graded scenarios");
        let report = pre.report().unwrap();

        assert!(report.starts_with("cannot start this run: 3 prerequisites are missing."));
        assert!(
            report.contains("Nothing ran and nothing was spent"),
            "{report}"
        );
        assert!(report.contains("\n  1. no zerostack binary"), "{report}");
        assert!(
            report.contains("\n  2. ANTHROPIC_API_KEY is unset"),
            "{report}"
        );
        assert!(
            report.contains("\n  3. this suite has 2 judge-graded scenarios"),
            "{report}"
        );
        assert!(report.contains("fix them together"), "{report}");
    }

    #[test]
    fn a_single_problem_reads_in_the_singular() {
        let mut pre = Preflight::new();
        pre.problem("no zerostack binary");
        let report = pre.report().unwrap();
        assert!(
            report.starts_with("cannot start this run: 1 prerequisite is missing."),
            "{report}"
        );
    }

    #[test]
    fn finish_fails_with_the_report_and_still_prints_the_warnings() {
        let mut pre = Preflight::new();
        pre.warn("build unverifiable");
        pre.problem("no zerostack binary");
        let mut err = Vec::new();
        let e = pre.finish(&mut err).unwrap_err();
        assert!(String::from_utf8(err)
            .unwrap()
            .contains("build unverifiable"));
        assert!(format!("{e:#}").contains("no zerostack binary"), "{e:#}");
    }
}

#[cfg(test)]
mod binary_leg_tests {
    use super::*;

    fn features(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn an_unnamed_binary_is_a_problem_naming_both_ways_to_name_one() {
        let mut pre = Preflight::new();
        check_binary(&mut pre, None);
        let p = pre.problems().join("\n");
        assert!(p.contains("--zs-bin"), "{p}");
        assert!(p.contains("ZS_BIN"), "{p}");
        assert!(p.contains("mock="), "{p}");
    }

    #[test]
    fn a_build_announcing_every_required_feature_passes_silently() {
        let mut pre = Preflight::new();
        let mut all = features(REQUIRED_FEATURES);
        all.push("git-worktree".into());
        check_features(&mut pre, Path::new("/bin/zs"), Some(&all));
        assert!(pre.problems().is_empty(), "{:?}", pre.problems());
        assert!(pre.warnings().is_empty(), "{:?}", pre.warnings());
    }

    /// The motivating case: a plain `cargo build` ships every default feature
    /// but not `memory`, and the scenarios that need it can only grade
    /// indeterminate. Hard problem, naming both the gap and the rebuild.
    #[test]
    fn a_build_missing_a_required_feature_is_a_problem_naming_the_rebuild() {
        let mut pre = Preflight::new();
        check_features(
            &mut pre,
            Path::new("/bin/zs"),
            Some(&features(&["mcp", "subagents", "loop", "git-worktree"])),
        );
        let p = pre.problems().join("\n");
        assert!(p.contains("memory"), "{p}");
        assert!(p.contains("cargo build --all-features"), "{p}");
        assert!(p.contains("/bin/zs"), "{p}");
        assert!(pre.warnings().is_empty(), "{:?}", pre.warnings());
    }

    /// A binary that says nothing about its features is unverifiable, not
    /// broken: warn, name the consequence, and let the run proceed.
    #[test]
    fn a_build_announcing_no_features_warns_and_does_not_block() {
        let mut pre = Preflight::new();
        check_features(&mut pre, Path::new("/bin/zs"), None);
        assert!(pre.problems().is_empty(), "{:?}", pre.problems());
        let w = pre.warnings().join("\n");
        assert!(w.contains("/bin/zs"), "{w}");
        assert!(w.contains("indeterminate"), "{w}");
        assert!(w.contains("cargo build --all-features"), "{w}");
    }

    /// A binary that cannot be probed at all is a problem, named by path — the
    /// probe's own error, not a paraphrase of it.
    #[test]
    fn an_unrunnable_binary_is_a_problem_naming_it() {
        let bin = std::env::temp_dir().join(format!(
            "zseval-preflight-missing-{}/does-not-exist",
            std::process::id()
        ));
        let mut pre = Preflight::new();
        check_binary(&mut pre, Some(&bin));
        let p = pre.problems().join("\n");
        assert!(p.contains(&bin.display().to_string()), "{p}");
    }
}

#[cfg(test)]
mod target_key_tests {
    use super::*;

    /// Write `contents` as a target file under a fresh per-test directory.
    fn target(dir: &Path, name: &str, contents: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, contents).unwrap();
        p
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "zseval-preflight-targets-{name}-{}",
            std::process::id()
        ));
        std::fs::remove_dir_all(&dir).ok();
        dir
    }

    /// The environment is injected, never read from the process: these tests
    /// must not care whether the developer running them has a real
    /// `ANTHROPIC_API_KEY` exported.
    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let owned: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name| {
            owned
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
        }
    }

    #[test]
    fn no_target_at_all_is_a_problem_naming_the_flag() {
        let mut pre = Preflight::new();
        check_target_keys(&mut pre, &[], env(&[]));
        assert!(pre.problems().join("\n").contains("--target"));
    }

    #[test]
    fn an_unset_key_names_the_file_the_provider_and_the_variable() {
        let dir = scratch("unset");
        let t = target(&dir, "anthropic.toml", "provider = \"anthropic\"\n");
        let mut pre = Preflight::new();
        check_target_keys(&mut pre, std::slice::from_ref(&t), env(&[]));
        std::fs::remove_dir_all(&dir).ok();

        let p = pre.problems().join("\n");
        assert!(p.contains(&t.display().to_string()), "{p}");
        assert!(p.contains("anthropic"), "{p}");
        assert!(p.contains("ANTHROPIC_API_KEY"), "{p}");
    }

    /// Exported-but-empty is the same failure as never exported — a common
    /// shape when a `.env` line lost its value.
    #[test]
    fn an_empty_key_is_treated_as_missing() {
        let dir = scratch("empty");
        let t = target(&dir, "anthropic.toml", "provider = \"anthropic\"\n");
        let mut pre = Preflight::new();
        check_target_keys(&mut pre, &[t], env(&[("ANTHROPIC_API_KEY", "   ")]));
        std::fs::remove_dir_all(&dir).ok();
        assert!(pre.problems().join("\n").contains("ANTHROPIC_API_KEY"));
    }

    #[test]
    fn a_set_key_passes_silently() {
        let dir = scratch("set");
        let t = target(&dir, "anthropic.toml", "provider = \"anthropic\"\n");
        let mut pre = Preflight::new();
        check_target_keys(&mut pre, &[t], env(&[("ANTHROPIC_API_KEY", "sk-ant-x")]));
        std::fs::remove_dir_all(&dir).ok();
        assert!(pre.problems().is_empty(), "{:?}", pre.problems());
        assert!(pre.warnings().is_empty(), "{:?}", pre.warnings());
    }

    /// The multi-target rule: every target is checked up front, so target 2's
    /// missing key cannot surface after target 1 has already spent money.
    #[test]
    fn every_target_is_checked_not_just_the_first() {
        let dir = scratch("multi");
        let a = target(&dir, "anthropic.toml", "provider = \"anthropic\"\n");
        let b = target(&dir, "openrouter.toml", "provider = \"openrouter\"\n");
        let c = target(&dir, "local.toml", "provider = \"ollama\"\n");
        let mut pre = Preflight::new();
        check_target_keys(
            &mut pre,
            &[a, b, c],
            env(&[("ANTHROPIC_API_KEY", "sk-ant-x")]),
        );
        std::fs::remove_dir_all(&dir).ok();

        // The first target's key is set and the third needs none, so exactly
        // the middle one is reported — and it is reported even though a target
        // before it was fine.
        assert_eq!(pre.problems().len(), 1, "{:?}", pre.problems());
        assert!(pre.problems()[0].contains("OPENROUTER_API_KEY"));
    }

    #[test]
    fn a_target_that_cannot_be_read_is_a_problem_naming_the_path() {
        let mut pre = Preflight::new();
        check_target_keys(&mut pre, &[PathBuf::from("/no/such/target.toml")], env(&[]));
        assert!(pre.problems().join("\n").contains("/no/such/target.toml"));
    }

    /// A key written into the target file is refused even when the environment
    /// would satisfy the check — the run would work, and that is the trap: a
    /// target file lives committed, so a working embedded key is a leaked one.
    #[test]
    fn a_key_embedded_in_the_target_file_is_refused_even_with_the_env_set() {
        let dir = scratch("embedded");
        let t = target(
            &dir,
            "anthropic.toml",
            "provider = \"anthropic\"\n[api_keys]\nanthropic = \"sk-ant-x\"\n",
        );
        let mut pre = Preflight::new();
        check_target_keys(
            &mut pre,
            std::slice::from_ref(&t),
            env(&[("ANTHROPIC_API_KEY", "sk-ant-x")]),
        );
        std::fs::remove_dir_all(&dir).ok();

        assert_eq!(pre.problems().len(), 1, "{:?}", pre.problems());
        assert!(
            pre.problems()[0].contains("[api_keys]"),
            "{:?}",
            pre.problems()
        );
        assert!(
            pre.problems()[0].contains("secret in git"),
            "{:?}",
            pre.problems()
        );
    }

    /// With the key on file and the environment empty, the embedding is the
    /// whole story: an "unset variable" line would misdescribe a run zerostack
    /// would happily have started on the embedded key.
    #[test]
    fn an_embedded_key_reports_the_embedding_not_a_missing_variable() {
        let dir = scratch("embedded-no-env");
        let t = target(
            &dir,
            "anthropic.toml",
            "provider = \"anthropic\"\n[api_keys]\nanthropic = \"sk-ant-x\"\n",
        );
        let mut pre = Preflight::new();
        check_target_keys(&mut pre, std::slice::from_ref(&t), env(&[]));
        std::fs::remove_dir_all(&dir).ok();

        assert_eq!(pre.problems().len(), 1, "{:?}", pre.problems());
        assert!(
            pre.problems()[0].contains("[api_keys]"),
            "{:?}",
            pre.problems()
        );
        assert!(
            !pre.problems()[0].contains("unset or empty"),
            "{:?}",
            pre.problems()
        );
    }

    #[test]
    fn an_undeterminable_target_warns_instead_of_blocking() {
        let dir = scratch("undetermined");
        let t = target(&dir, "bare.toml", "model = \"m\"\n");
        let mut pre = Preflight::new();
        check_target_keys(&mut pre, &[t], env(&[]));
        std::fs::remove_dir_all(&dir).ok();
        assert!(pre.problems().is_empty(), "{:?}", pre.problems());
        assert!(pre.warnings().join("\n").contains("provider"));
    }
}

#[cfg(test)]
mod judge_decision_tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "zseval-preflight-judges-{name}-{}",
            std::process::id()
        ));
        std::fs::remove_dir_all(&dir).ok();
        dir
    }

    #[test]
    fn the_message_names_the_judge_graded_scenarios_and_both_flags() {
        let msg = judge_decision_needed(&["prompts/ask", "tools/edit"], Path::new("no-such-dir"));
        assert!(msg.contains("2 judge-graded scenarios"), "{msg}");
        assert!(msg.contains("prompts/ask, tools/edit"), "{msg}");
        assert!(msg.contains("--judge"), "{msg}");
        assert!(msg.contains("--no-judge"), "{msg}");
    }

    /// A long list is truncated rather than dumped: five ids, then a count.
    #[test]
    fn more_than_five_scenarios_are_listed_as_five_and_a_count() {
        let ids = ["a", "b", "c", "d", "e", "f", "g"];
        let msg = judge_decision_needed(&ids, Path::new("no-such-dir"));
        assert!(msg.contains("7 judge-graded scenarios"), "{msg}");
        assert!(msg.contains("a, b, c, d, e, and 2 more"), "{msg}");
        assert!(!msg.contains(", f"), "{msg}");
    }

    #[test]
    fn one_scenario_reads_in_the_singular() {
        let msg = judge_decision_needed(&["only/one"], Path::new("no-such-dir"));
        assert!(msg.contains("1 judge-graded scenario ("), "{msg}");
    }

    /// The judge files actually sitting in `judges/` are offered as candidates
    /// — sorted, `.toml` only, and explicitly not chosen.
    #[test]
    fn candidates_are_the_toml_files_that_exist_right_now() {
        let dir = scratch("candidates");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("sonnet.toml"), "").unwrap();
        std::fs::write(dir.join("opus.toml"), "").unwrap();
        std::fs::write(dir.join("README.md"), "").unwrap();
        let msg = judge_decision_needed(&["s"], &dir);
        std::fs::remove_dir_all(&dir).ok();

        let opus = dir.join("opus.toml").display().to_string();
        let sonnet = dir.join("sonnet.toml").display().to_string();
        assert!(msg.contains(&opus), "{msg}");
        assert!(msg.contains(&sonnet), "{msg}");
        assert!(
            msg.find(&opus) < msg.find(&sonnet),
            "candidates are sorted: {msg}"
        );
        // Only the listing itself is checked for the non-judge file: the
        // message's trailer legitimately points at `judges/README.md`.
        let listing = msg.split("Candidates in").nth(1).unwrap();
        let listing = listing.split('—').next().unwrap();
        assert!(!listing.contains("README"), "{msg}");
        assert!(msg.contains("never picks one for you"), "{msg}");
    }

    /// A single candidate is still not a choice: the message offers it and
    /// stops. Nothing here may ever resolve to "so I used that one".
    #[test]
    fn a_lone_candidate_is_offered_never_selected() {
        let dir = scratch("lone");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("sonnet.toml"), "").unwrap();
        let msg = judge_decision_needed(&["s"], &dir);
        std::fs::remove_dir_all(&dir).ok();
        assert!(msg.contains("sonnet.toml"), "{msg}");
        assert!(msg.contains("never picks one for you"), "{msg}");
        assert!(msg.contains("--judge"), "{msg}");
    }

    #[test]
    fn a_missing_judges_directory_still_says_where_they_live() {
        let msg = judge_decision_needed(&["s"], Path::new("no-such-dir"));
        assert!(msg.contains("no-such-dir/"), "{msg}");
        assert!(msg.contains("holds none right now"), "{msg}");
    }
}
