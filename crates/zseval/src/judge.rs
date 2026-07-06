//! LLM judge: binary Yes/No with an Unknown escape hatch, pinned model.
//!
//! The judge model is pinned here so grading is stable across runs; changing
//! it should be paired with re-checking a batch against human labels.
//!
//! Transport is a `curl` subprocess: no extra crate dependency, and the
//! request/response is inspectable in the run dir. The interface is just
//! `judge() -> JudgeOutcome`; swap in an HTTP client later if desired.

use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde::Serialize;

pub const JUDGE_MODEL: &str = "claude-sonnet-4-6";
const API_URL: &str = "https://api.anthropic.com/v1/messages";
/// Sonnet-tier per-million-token pricing (USD), pinned alongside
/// `JUDGE_MODEL` so judge spend shows up in a trial's `cost_usd` instead of
/// being invisible outside `judge-response.json` — update these together if
/// `JUDGE_MODEL` ever changes to a different-priced tier.
const JUDGE_INPUT_USD_PER_MTOK: f64 = 3.0;
const JUDGE_OUTPUT_USD_PER_MTOK: f64 = 15.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JudgeVerdict {
    Yes,
    No,
    Unknown,
}

/// A judge call's verdict plus the token usage that produced it — the usage
/// is real API spend (a curl subprocess per trial) that would otherwise be
/// invisible outside `judge-response.json`; kept separate from the agent's
/// own `input_tokens`/`output_tokens` since it's eval overhead, not agent
/// behavior, and must never count against a scenario's `max_total_tokens`.
#[derive(Debug, Clone, Copy)]
pub struct JudgeOutcome {
    pub verdict: JudgeVerdict,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Estimated from `JUDGE_INPUT_USD_PER_MTOK`/`JUDGE_OUTPUT_USD_PER_MTOK`,
    /// not provider-reported — the judge API response carries no cost field.
    pub cost_usd: f64,
}

/// The seam the runner grades through: same shape as `AgentBackend`, so the
/// verdict-mapping paths (No -> Fail, Unknown/error/no-key -> Indeterminate)
/// are testable without a key or a network. `LlmJudge` is the real referee.
pub trait Judge {
    /// Whether this judge can grade right now (e.g. its API key is set).
    fn available(&self) -> bool;
    fn judge(&self, rubric: &str, evidence: &str, run_dir: &Path) -> Result<JudgeOutcome>;
}

/// The pinned-model LLM referee (see module doc).
pub struct LlmJudge;

impl Judge for LlmJudge {
    fn available(&self) -> bool {
        have_api_key()
    }
    fn judge(&self, rubric: &str, evidence: &str, run_dir: &Path) -> Result<JudgeOutcome> {
        judge(rubric, evidence, run_dir)
    }
}

pub fn have_api_key() -> bool {
    std::env::var("ANTHROPIC_API_KEY")
        .map(|k| !k.is_empty())
        .unwrap_or(false)
}

pub fn judge(rubric: &str, transcript_text: &str, run_dir: &Path) -> Result<JudgeOutcome> {
    let key = std::env::var("ANTHROPIC_API_KEY").context("ANTHROPIC_API_KEY not set")?;

    let prompt = format!(
        "You are a strict evaluator for an agent-behavior eval.\n\
         Question:\n{rubric}\n\n\
         Answer with exactly one word: Yes, No, or Unknown \
         (Unknown if the evidence is insufficient to decide).\n\n\
         --- Evidence ---\n{transcript_text}\n"
    );

    let body = serde_json::json!({
        "model": JUDGE_MODEL,
        "max_tokens": 8,
        // Grading must be stable across runs, not just the model pinned —
        // the API defaults to temperature 1.0, which would let the same
        // rubric flip verdicts on the same evidence.
        "temperature": 0,
        "messages": [{"role": "user", "content": prompt}],
    });
    let body_path = run_dir.join("judge-request.json");
    std::fs::write(&body_path, serde_json::to_vec(&body)?)?;

    let out = Command::new("curl")
        .arg("-sS")
        .arg("--max-time")
        .arg("60")
        .arg(API_URL)
        .arg("-H")
        .arg(format!("x-api-key: {key}"))
        .arg("-H")
        .arg("anthropic-version: 2023-06-01")
        .arg("-H")
        .arg("content-type: application/json")
        .arg("-d")
        .arg(format!("@{}", body_path.display()))
        .output()
        .context("spawn curl (is curl installed?)")?;

    if !out.status.success() {
        bail!("curl failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    std::fs::write(run_dir.join("judge-response.json"), &out.stdout)?;

    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).context("judge response json")?;
    if let Some(err) = v.get("error") {
        bail!("anthropic api error: {err}");
    }
    Ok(parse_judge_response(&v))
}

/// Pure parse of a `POST /v1/messages` response body into a `JudgeOutcome` —
/// split out from `judge()` so the verdict/cost math is testable without a
/// key, curl, or the network.
fn parse_judge_response(v: &serde_json::Value) -> JudgeOutcome {
    let text = v["content"][0]["text"]
        .as_str()
        .unwrap_or("")
        .to_lowercase();
    // Match the first whole word, stripped of punctuation, so a hedge like
    // "Not sure" reads as Unknown rather than being caught by a "no" prefix.
    let first = text
        .split(|c: char| !c.is_alphanumeric())
        .find(|w| !w.is_empty())
        .unwrap_or("");
    let verdict = match first {
        "yes" => JudgeVerdict::Yes,
        "no" => JudgeVerdict::No,
        _ => JudgeVerdict::Unknown,
    };
    // Best-effort: a missing/reshaped `usage` block still yields a verdict,
    // it just under-reports judge spend rather than failing the trial over it.
    let input_tokens = v["usage"]["input_tokens"].as_u64().unwrap_or(0);
    let output_tokens = v["usage"]["output_tokens"].as_u64().unwrap_or(0);
    let cost_usd = (input_tokens as f64 / 1_000_000.0) * JUDGE_INPUT_USD_PER_MTOK
        + (output_tokens as f64 / 1_000_000.0) * JUDGE_OUTPUT_USD_PER_MTOK;
    JudgeOutcome {
        verdict,
        input_tokens,
        output_tokens,
        cost_usd,
    }
}

#[cfg(test)]
mod parse_response_tests {
    use super::*;

    #[test]
    fn yes_no_unknown_map_correctly_and_ignore_case_and_punctuation() {
        for (text, want) in [
            ("Yes", JudgeVerdict::Yes),
            ("no.", JudgeVerdict::No),
            ("Not sure", JudgeVerdict::Unknown),
            ("", JudgeVerdict::Unknown),
        ] {
            let v = serde_json::json!({"content": [{"text": text}]});
            assert_eq!(parse_judge_response(&v).verdict, want, "{text}");
        }
    }

    #[test]
    fn usage_tokens_convert_to_cost_at_the_pinned_sonnet_rate() {
        let v = serde_json::json!({
            "content": [{"text": "Yes"}],
            "usage": {"input_tokens": 1_000_000, "output_tokens": 1_000_000},
        });
        let o = parse_judge_response(&v);
        assert_eq!(o.input_tokens, 1_000_000);
        assert_eq!(o.output_tokens, 1_000_000);
        assert!(
            (o.cost_usd - (JUDGE_INPUT_USD_PER_MTOK + JUDGE_OUTPUT_USD_PER_MTOK)).abs() < 1e-9,
            "{}",
            o.cost_usd
        );
    }

    #[test]
    fn missing_usage_block_yields_zero_cost_not_an_error() {
        let v = serde_json::json!({"content": [{"text": "Yes"}]});
        let o = parse_judge_response(&v);
        assert_eq!(o.input_tokens, 0);
        assert_eq!(o.output_tokens, 0);
        assert_eq!(o.cost_usd, 0.0);
    }
}
