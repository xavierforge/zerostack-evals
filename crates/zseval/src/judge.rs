//! LLM judge: binary Yes/No with an Unknown escape hatch, over a swappable
//! judge file.
//!
//! Which model grades, and at what price, is a `JudgeConfig` — a "ruler card"
//! read from a judge file (`--judge judges/opus.toml`). There is no built-in
//! default: a card is loaded from a file or not at all (see `resolve_judge` in
//! `main.rs`). **Changing the judge should be paired with re-checking a batch
//! against human labels**: the judge is the ruler, and a run graded by a
//! different model is not comparable to one graded by the old one just because
//! both say "pass". Every report records the judge file it was configured with
//! and, read back from the judge's own responses, the model that actually
//! graded, so a score can always be traced to the ruler that really produced
//! it (see `verdict::Report`).
//!
//! Keys never live in a judge file. A card names a `provider` (one of the
//! closed set on `JudgeProvider`); which env var holds that provider's key,
//! and which endpoint it's sent to, are decided by matching on the enum **in
//! code**, never by a field the file supplies (see `JudgeProvider::key_env`).
//! This is a structural fix, not a validation fence: an earlier design let a
//! judge file name an arbitrary `api_url` plus the *name* of an env var
//! (`api_key_env`) to send to it — a verified exfiltration vector, since any
//! PR-introduced file could point an existing secret at an arbitrary host.
//! Those two fields are now rejected outright
//! (`reject_removed_routing_fields`) rather than merely validated, so the
//! vulnerable shape cannot be expressed at all: a card can select *which*
//! provider's key is used, never *where* it goes.
//!
//! Transport (which HTTP client, which per-provider request/response shape)
//! runs on rig-core's thin completion path — no Agent machinery, no streaming,
//! just `completion_model(model).completion_request(prompt)
//! .temperature(0.0).max_tokens(1024).send()` per provider (see `judge()`
//! below).

use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use rig_core::client::CompletionClient;
use rig_core::completion::{AssistantContent, CompletionError, CompletionModel};
use rig_core::providers::{anthropic, gemini, openai, openrouter};
use serde::{Deserialize, Serialize};

/// Fixed request shape for every judge call, on every provider: temperature 0
/// for greedy, near-deterministic grading, and a token budget sized so
/// thinking models (OpenAI Responses reasoning tokens, Gemini 2.5 thoughts)
/// don't silently eat the whole visible answer. Neither is a card field:
/// swappable inputs are `provider`/`model`/prices only, matching the
/// ruler-card's closed shape.
const JUDGE_TEMPERATURE: f64 = 0.0;
const JUDGE_MAX_TOKENS: u64 = 1024;
/// How long to wait before the single transient retry. Short and fixed, not
/// exponential: this is a one-shot second chance for a rate limit or 5xx blip,
/// not a durable retry policy — a persistent failure is meant to surface
/// quickly as Indeterminate, not be masked by a long backoff loop.
const RETRY_BACKOFF: Duration = Duration::from_millis(200);

/// The fixed self-calibration question `LlmJudge::preflight` asks: a question
/// with a definite, knowable answer, run through the exact same prompt
/// template, `max_tokens`, and temperature-fallback path as a real judge call.
/// It exists purely to prove the chain works end to end (auth, model name,
/// response shape, output budget) — nobody reads *which* answer comes back,
/// only whether it's a parseable Yes/No at all (see
/// `LlmJudge::preflight_with_base_url`).
const PREFLIGHT_RUBRIC: &str =
    "This is a pre-flight self-check, not a real evaluation. Is one plus one equal to two? \
     Answer only Yes or No.";
/// The probe has no real trial evidence to show the judge — there is no
/// trial. This placeholder plays the evidence section's role in the shared
/// prompt template so the probe exercises the exact same prompt shape a real
/// judge call uses.
const PREFLIGHT_EVIDENCE: &str = "(no evidence: this is a connectivity self-check, not a trial)";

/// The bridge between the sync `Judge` trait (kept sync so trial worker
/// threads under `--jobs` stay plain `std::thread`s, see the trait doc) and
/// rig's async completion calls. One multi-thread runtime is built once and
/// shared across every judge call rather than spinning a fresh runtime per
/// call: judge calls happen concurrently under `--jobs`, and `block_on` on a
/// shared multi-thread runtime lets those calls truly overlap instead of
/// each thread paying for its own thread pool. Small worker count: this
/// runtime only ever drives judge HTTP calls, not CPU-bound work.
static JUDGE_RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

fn shared_runtime() -> &'static tokio::runtime::Runtime {
    JUDGE_RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("build shared judge tokio runtime")
    })
}

/// The closed set of judge providers rig can reach. This is the structural
/// half of the security fix: a judge file selects *which* provider's key is
/// used by naming one of these four variants, but routing — the base URL and
/// the env var a key is read from — is decided by matching on this enum **in
/// code**, never by a field a committed file can supply. A card can therefore
/// at most send a provider's own key to that provider; cross-pairing (provider
/// X's key to provider Y's endpoint, or any key to an arbitrary host) is not
/// representable. Adding a fifth provider means adding a variant and a `match`
/// arm here, in reviewed code — not adding a string a file can set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JudgeProvider {
    Anthropic,
    OpenAI,
    OpenRouter,
    Gemini,
}

impl JudgeProvider {
    /// The fixed env var this provider's key lives in. There is no field
    /// anywhere that can override this mapping — `provider` is the only
    /// input, and this `match` is the entire routing table. Matches
    /// zerostack's own provider env-var conventions.
    pub fn key_env(&self) -> &'static str {
        match self {
            JudgeProvider::Anthropic => "ANTHROPIC_API_KEY",
            JudgeProvider::OpenAI => "OPENAI_API_KEY",
            JudgeProvider::OpenRouter => "OPENROUTER_API_KEY",
            JudgeProvider::Gemini => "GEMINI_API_KEY",
        }
    }
}

/// What a judge file says: which model grades and at what price. Provider
/// selects the fixed routing (see `JudgeProvider::key_env`); it never carries
/// a network destination or an env var name itself — that is the point.
/// Deserialized straight from the judge file, so every field is required — a
/// file that omits `model` is a mistake, not a request to inherit the old
/// one. `deny_unknown_fields` makes a typo'd or unsupported key loud rather
/// than silently ignored. There is deliberately no `Default`: a judge is
/// loaded from a file or not at all (see `resolve_judge` in `main.rs`) — no
/// built-in ruler stands in when one isn't named.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JudgeConfig {
    pub provider: JudgeProvider,
    pub model: String,
    /// Per-million-token pricing (USD) for the tier `model` belongs to. Kept
    /// beside the model because they must move together: judge spend lands in
    /// a trial's `cost_usd`, so a model swapped without its prices would
    /// under- or over-report every run's cost.
    pub price_in_usd_per_mtok: f64,
    pub price_out_usd_per_mtok: f64,
}

impl JudgeConfig {
    /// Read a judge file. Every failure is loud: an unreadable file, a
    /// missing or misspelled field, a non-numeric or negative price, or one
    /// of the removed routing fields. A judge file that can't be trusted must
    /// stop the run (exit 2), never silently fall back to some other ruler —
    /// the whole point of naming one is knowing which ruler graded the batch.
    pub fn load(path: &Path) -> Result<JudgeConfig> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("read judge file {}", path.display()))?;
        let table: toml::Table = toml::from_str(&text)
            .with_context(|| format!("parse judge file {}", path.display()))?;
        reject_key_shaped_fields(&table)
            .with_context(|| format!("invalid judge file {}", path.display()))?;
        reject_removed_routing_fields(&table)
            .with_context(|| format!("invalid judge file {}", path.display()))?;
        let cfg: JudgeConfig = table
            .try_into()
            .with_context(|| format!("parse judge file {}", path.display()))?;
        cfg.validate()
            .with_context(|| format!("invalid judge file {}", path.display()))?;
        Ok(cfg)
    }

    /// What TOML's type system can't say: a price must be a real,
    /// non-negative number, and a judge with no model (or one that couldn't
    /// possibly name a real model) is not a judge.
    fn validate(&self) -> Result<()> {
        if self.model.trim().is_empty() {
            bail!("model must not be empty");
        }
        if self
            .model
            .chars()
            .any(|c| c.is_control() || c.is_whitespace())
        {
            bail!(
                "model must not contain whitespace or control characters, got {:?}",
                self.model
            );
        }
        for (field, price) in [
            ("price_in_usd_per_mtok", self.price_in_usd_per_mtok),
            ("price_out_usd_per_mtok", self.price_out_usd_per_mtok),
        ] {
            if !price.is_finite() || price < 0.0 {
                bail!("{field} must be a non-negative number, got {price}");
            }
        }
        Ok(())
    }
}

/// Field names that mean someone put the secret itself in a committed file.
/// `deny_unknown_fields` already rejects them, but "unknown field `api_key`"
/// reads as "this key is unsupported, find the supported spelling" — the
/// opposite of the rule. Named here so the error can state the rule instead.
const KEY_SHAPED_FIELDS: &[&str] = &[
    "api_key",
    "apikey",
    "key",
    "token",
    "api_token",
    "auth_token",
    "secret",
    "api_secret",
    "password",
    "authorization",
    "bearer_token",
];

fn reject_key_shaped_fields(table: &toml::Table) -> Result<()> {
    for name in table.keys() {
        let lowered = name.to_lowercase();
        if KEY_SHAPED_FIELDS.contains(&lowered.as_str()) {
            bail!(
                "`{name}` must not appear in a judge file: a judge file is committed and holds \
                 no secrets. Routing (including which key is used) is derived from `provider` \
                 alone, in code — a judge file never names a key or an env var. If this key was \
                 ever a real secret, rotate it."
            );
        }
    }
    Ok(())
}

/// Fields removed for security: naming either one used to let a committed file
/// select a network destination (`api_url`) or the env var a secret is read
/// from (`api_key_env`) — the verified exfiltration vector this crate's closed
/// provider set exists to close. Checked ahead of `deny_unknown_fields` (which
/// would otherwise just say "unknown field", reading as a typo rather than a
/// removed, unsafe capability) so the error states the real reason and the
/// fix. This is the regression guard for the structural invariant on
/// `JudgeProvider`: routing must derive from `provider` alone, in code, never
/// from a file field — case-insensitive, so `API_URL` doesn't sneak past.
fn reject_removed_routing_fields(table: &toml::Table) -> Result<()> {
    const REMOVED_ROUTING_FIELDS: &[&str] = &["api_url", "api_key_env"];
    let offending: Vec<&str> = table
        .keys()
        .filter(|name| REMOVED_ROUTING_FIELDS.contains(&name.to_lowercase().as_str()))
        .map(|s| s.as_str())
        .collect();
    if offending.is_empty() {
        return Ok(());
    }
    let pronoun = if offending.len() == 1 { "it" } else { "them" };
    let names = offending
        .iter()
        .map(|f| format!("`{f}`"))
        .collect::<Vec<_>>()
        .join(", ");
    bail!(
        "{names} removed for security: a committed judge file must never be able to name a \
         network destination or the env var a secret is read from. Routing (base URL and key \
         env var) is derived from `provider` alone, in code, and cannot be overridden. Delete \
         {pronoun} and keep only `provider`, `model`, `price_in_usd_per_mtok`, and \
         `price_out_usd_per_mtok`. If a key was ever exposed via this file, rotate it."
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JudgeVerdict {
    Yes,
    No,
    Unknown,
}

/// A judge call's verdict plus the token usage that produced it — the usage
/// is real API spend (a live provider call per trial) that would otherwise be
/// invisible outside `judge-response.json`; kept separate from the agent's
/// own `input_tokens`/`output_tokens` since it's eval overhead, not agent
/// behavior, and must never count against a scenario's `max_total_tokens`.
#[derive(Debug, Clone)]
pub struct JudgeOutcome {
    pub verdict: JudgeVerdict,
    /// The model that actually served this call, as the API reported it —
    /// the ruler that really graded, which is not necessarily the one the
    /// judge config asked for (the request's model is an intention: names
    /// are resolved server-side, and a stale or aliased one can come back as
    /// something else). `None` when the response didn't say, which reports
    /// an unknown ruler rather than echoing the config's claim back as fact.
    pub model: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Estimated from the judge config's per-mtok prices, not
    /// provider-reported — the judge API response carries no cost field.
    pub cost_usd: f64,
}

/// The seam the runner grades through: same shape as `AgentBackend`, so the
/// verdict-mapping paths (No -> Fail, Unknown/error/no-key -> Indeterminate)
/// are testable without a key or a network. `LlmJudge` is the real referee.
/// `Sync` for the same reason as `AgentBackend: Sync` — shared across trial
/// worker threads under `--jobs`. `LlmJudge` holds only owned data, as does
/// a test double (see `harness.rs`'s `TestJudge`), so both are Sync.
pub trait Judge: Sync {
    /// Whether this judge can grade right now (e.g. its API key is set).
    fn available(&self) -> bool;
    /// A short hint for why this judge isn't available right now, embedded
    /// in the runner's backstop error when a rubric needs a judge that isn't
    /// ready (see `runner::grade_trial`). Never hard-codes a single
    /// provider's env var: a real `LlmJudge` names whichever var its own
    /// configured `provider` actually reads (the regression this guards
    /// against is the same one `available()` guards against — see its own
    /// doc and `judge_availability_tests`).
    fn unavailable_hint(&self) -> String;
    fn judge(&self, rubric: &str, evidence: &str, run_dir: &Path) -> Result<JudgeOutcome>;
}

/// The LLM referee, grading with whatever `JudgeConfig` it was built with
/// (see module doc). No `Default`: there is no built-in ruler, matching
/// `JudgeConfig`.
#[derive(Debug, Clone)]
pub struct LlmJudge {
    pub cfg: JudgeConfig,
}

impl LlmJudge {
    pub fn new(cfg: JudgeConfig) -> Self {
        LlmJudge { cfg }
    }

    /// Fail-fast checks run once before any trial spends money on grading: (a)
    /// the configured provider's key env var must be set, naming the exact
    /// variable and the `--no-judge` escape when it isn't; (b) a live dry-run,
    /// in the exact judge shape (same prompt template, `JUDGE_MAX_TOKENS`, and
    /// temperature-fallback path as a real judge call — see
    /// `preflight_with_base_url`), must come back with a parseable Yes/No
    /// verdict. This catches bad auth, a mistyped model, temperature
    /// rejection, and thinking-budget truncation before trial 1, rather than
    /// discovering them mid-suite as an Indeterminate trial.
    ///
    /// Takes no `run_dir`: the probe is not a trial, so it writes no
    /// `judge-request.json` / `judge-response.json`, and its cost has no
    /// report row to land in — this method's `Result<()>` is structurally
    /// incapable of carrying cost or token data back to its caller
    /// (`main.rs`), which is how "probe cost excluded from the report" holds
    /// without special-casing accounting anywhere else.
    pub fn preflight(&self) -> Result<()> {
        self.preflight_with_base_url(None)
    }

    /// `preflight()`'s real body, parameterized by an optional base-URL
    /// override — the same test-only seam as `judge_with_base_url` (see its
    /// doc): production code only ever calls `preflight()` above, which
    /// always passes `None`; tests point this at a local mock server.
    fn preflight_with_base_url(&self, base_url: Option<&str>) -> Result<()> {
        self.cfg.validate().context("invalid judge config")?;

        let key_env = self.cfg.provider.key_env();
        if !have_api_key(key_env) {
            bail!(
                "judge preflight failed: {key_env} is not set. Set it before running a \
                 judge-graded suite (e.g. `export {key_env}=...`), or pass --no-judge to skip \
                 grading entirely."
            );
        }
        let key = std::env::var(key_env).with_context(|| format!("{key_env} not set"))?;

        let prompt = build_prompt(PREFLIGHT_RUBRIC, PREFLIGHT_EVIDENCE);
        let (result, _record) =
            shared_runtime().block_on(run_judge_call(&self.cfg, &key, &prompt, base_url));

        let outcome = result.map_err(|e| {
            // Scrubbed before it reaches the message: this is printed to the
            // user at exit 2, and a Gemini transport failure would otherwise
            // render `...generateContent?key=<GEMINI_API_KEY>` (see
            // `scrub_provider_error`).
            anyhow::anyhow!(
                "judge preflight failed: the configured judge could not complete a dry-run \
                 call ({}). Check the provider, model, and key in the judge file, or pass \
                 --no-judge to skip grading.",
                scrub_provider_error(&e.to_string())
            )
        })?;

        match parse_verdict(&outcome.text) {
            JudgeVerdict::Yes | JudgeVerdict::No => Ok(()),
            JudgeVerdict::Unknown => bail!(
                "judge preflight failed: the dry-run response had no parseable verdict (got \
                 {:?}). The judge is not reliably producing verdicts — check the model and its \
                 output budget, or pass --no-judge to skip grading.",
                outcome.text
            ),
        }
    }
}

impl Judge for LlmJudge {
    fn available(&self) -> bool {
        have_api_key(self.cfg.provider.key_env())
    }
    fn unavailable_hint(&self) -> String {
        format!("is {} set?", self.cfg.provider.key_env())
    }
    fn judge(&self, rubric: &str, evidence: &str, run_dir: &Path) -> Result<JudgeOutcome> {
        judge(&self.cfg, rubric, evidence, run_dir)
    }
}

/// Whether the key a *given* env var names is set. Takes the var name rather
/// than assuming Anthropic's: a judge pointed at another provider is not
/// available just because an unrelated key happens to be in the env — that
/// would report a ruler as ready and then fail (or worse, send the wrong
/// provider's secret) at call time. Callers pass `cfg.provider.key_env()`, the
/// only source of truth for which var a given config's provider uses.
fn have_api_key(api_key_env: &str) -> bool {
    std::env::var(api_key_env)
        .map(|k| !k.is_empty())
        .unwrap_or(false)
}

/// The public entry point: grade once, against the provider's real default
/// endpoint. The only difference from `judge_with_base_url` is `base_url:
/// None` — every provider then routes to its own official default. There is no
/// config path to anything else.
pub fn judge(
    cfg: &JudgeConfig,
    rubric: &str,
    transcript_text: &str,
    run_dir: &Path,
) -> Result<JudgeOutcome> {
    judge_with_base_url(cfg, rubric, transcript_text, run_dir, None)
}

/// The one prompt template every judge call uses — a real grading call
/// (`judge_with_base_url`, `rubric`/`transcript_text` filled in from a
/// scenario's evidence) and `LlmJudge::preflight`'s dry-run (the fixed
/// self-calibration question filled in instead) alike, so a preflight pass
/// is genuine evidence about the real call shape, not a different code path
/// that happens to also hit the network.
fn build_prompt(rubric: &str, evidence: &str) -> String {
    format!(
        "You are a strict evaluator for an agent-behavior eval.\n\
         Question:\n{rubric}\n\n\
         Answer with exactly one word: Yes, No, or Unknown \
         (Unknown if the evidence is insufficient to decide).\n\n\
         --- Evidence ---\n{evidence}\n"
    )
}

/// The real implementation, parameterized by an optional base-URL override.
/// `base_url` is a **test-only seam**, never a config field: production
/// code only ever calls `judge()` above, which always passes `None`. Tests
/// pass `Some(mock_addr)` to point the rig client at a local
/// `std::net::TcpListener` responder instead of the real network — this is
/// the seam every task in this section's offline tests goes through (see
/// `mock_server` below), while the structural invariant (routing derives
/// from `provider` alone, in code) stays intact: a committed judge file has
/// no way to reach this parameter.
fn judge_with_base_url(
    cfg: &JudgeConfig,
    rubric: &str,
    transcript_text: &str,
    run_dir: &Path,
    base_url: Option<&str>,
) -> Result<JudgeOutcome> {
    // A `JudgeConfig` can also be built in Rust, bypassing `load()`. The
    // checks that guard which secret can be reached must hold for every
    // config that reaches a live call, not only for the ones that came from
    // a file.
    cfg.validate().context("invalid judge config")?;

    let key_env = cfg.provider.key_env();
    let key = std::env::var(key_env).with_context(|| format!("{key_env} not set"))?;

    let prompt = build_prompt(rubric, transcript_text);

    // The sync `Judge` trait bridges into rig's async completion call on the
    // shared runtime (see its doc, and `shared_runtime` above) — one
    // `block_on` per judge call, which can run concurrently with others
    // under `--jobs` because the runtime is multi-thread and shared.
    let (result, record) = shared_runtime().block_on(run_judge_call(cfg, &key, &prompt, base_url));

    // The request record is written regardless of outcome: it documents
    // what was actually sent (including a temperature fallback, if one
    // happened) even when every attempt ultimately failed, which is useful
    // evidence for diagnosing why.
    std::fs::write(
        run_dir.join("judge-request.json"),
        serde_json::to_vec_pretty(&record).context("serialize judge-request.json")?,
    )
    .with_context(|| format!("write {}", run_dir.join("judge-request.json").display()))?;

    // Flatten to a scrubbed message rather than carrying the raw error as an
    // anyhow `source`: the runner renders this with `{e:#}` into `trial.json`
    // / `report.json`, and a retained source would re-print the unscrubbed
    // chain (key and all) regardless of what this top-level message says.
    let outcome = result.map_err(|e| {
        anyhow::anyhow!(
            "judge call failed: {}",
            scrub_provider_error(&e.to_string())
        )
    })?;

    std::fs::write(
        run_dir.join("judge-response.json"),
        serde_json::to_vec_pretty(&outcome.raw_response_json)
            .context("serialize judge-response.json")?,
    )
    .with_context(|| format!("write {}", run_dir.join("judge-response.json").display()))?;

    let cost_usd = (outcome.input_tokens as f64 / 1_000_000.0) * cfg.price_in_usd_per_mtok
        + (outcome.output_tokens as f64 / 1_000_000.0) * cfg.price_out_usd_per_mtok;

    Ok(JudgeOutcome {
        verdict: parse_verdict(&outcome.text),
        model: outcome.served_model,
        input_tokens: outcome.input_tokens,
        output_tokens: outcome.output_tokens,
        cost_usd,
    })
}

/// One completion call's result, provider-agnostic: `choice` and `usage` are
/// already normalized across all four providers by rig's `CompletionResponse`
/// (only `raw_response`'s *type* differs per provider), so this struct is the
/// single shape every match arm in `send_completion` converges on.
struct CompletionOutcome {
    /// The judge's visible answer text, concatenated from every text block
    /// in `choice` (in practice always exactly one for this thin, no-tool
    /// completion path).
    text: String,
    input_tokens: u64,
    output_tokens: u64,
    /// The served model, read from `raw_response_json` at
    /// `served_model_field(provider)` — `None` when the response didn't carry
    /// it (see that function's doc).
    served_model: Option<String>,
    /// `raw_response`, serialized generically via `serde_json::to_value`
    /// (every provider's `CompletionModel::Response` is `Serialize`) — this
    /// becomes `judge-response.json` verbatim.
    raw_response_json: serde_json::Value,
}

/// Runs one completion call against an already-built provider model and
/// normalizes its result into a `CompletionOutcome`. Generic over `M` because
/// each provider's `Client::completion_model` returns a different concrete
/// type; this is the one place that logic is shared, so a fifth provider only
/// needs a new `send_completion` match arm, not a new copy of this function.
async fn run_model<M>(
    model: M,
    prompt: &str,
    temperature: Option<f64>,
    served_model_field: &'static str,
) -> Result<CompletionOutcome, CompletionError>
where
    M: CompletionModel,
{
    let mut req = model
        .completion_request(prompt)
        .max_tokens(JUDGE_MAX_TOKENS);
    if let Some(t) = temperature {
        req = req.temperature(t);
    }
    let resp = req.send().await?;

    let text = resp
        .choice
        .iter()
        .filter_map(|c| match c {
            AssistantContent::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect::<String>();

    let raw_response_json =
        serde_json::to_value(&resp.raw_response).unwrap_or(serde_json::Value::Null);
    let served_model = raw_response_json
        .get(served_model_field)
        .and_then(|v| v.as_str())
        .map(str::to_string);

    Ok(CompletionOutcome {
        text,
        input_tokens: resp.usage.input_tokens,
        output_tokens: resp.usage.output_tokens,
        served_model,
        raw_response_json,
    })
}

/// Which field of a provider's raw response names the model that actually
/// served the request. Confirmed by reading rig 0.40's response types
/// directly, not guessed: Anthropic
/// (`providers::anthropic::completion::CompletionResponse::model`), OpenAI's
/// Responses API
/// (`providers::openai::responses_api::CompletionResponse::model`), and
/// OpenRouter (`providers::openrouter::completion::CompletionResponse::model`)
/// all expose a top-level `model` field. Gemini's `GenerateContentResponse`
/// instead has `model_version: Option<String>` under `#[serde(rename_all =
/// "camelCase")]`, so its wire field is `modelVersion` — and it really is
/// optional there, unlike the other three, which is why the absent-field
/// fallback (`run_model` above defaulting to `None`, the same unknown-ruler
/// tri-state as a missing `model`) matters most for Gemini.
fn served_model_field(provider: JudgeProvider) -> &'static str {
    match provider {
        JudgeProvider::Anthropic | JudgeProvider::OpenAI | JudgeProvider::OpenRouter => "model",
        JudgeProvider::Gemini => "modelVersion",
    }
}

/// Builds the provider's rig client (explicit key, default base URL unless
/// `base_url` overrides it for a test) and runs one completion call. This is
/// the only function that names a provider's rig client type — the routing
/// invariant, that a destination and a key var derive from `provider` alone,
/// lives here and in `JudgeProvider::key_env` alone.
async fn send_completion(
    cfg: &JudgeConfig,
    key: &str,
    prompt: &str,
    temperature: Option<f64>,
    base_url: Option<&str>,
) -> Result<CompletionOutcome, CompletionError> {
    let served_field = served_model_field(cfg.provider);
    match cfg.provider {
        JudgeProvider::Anthropic => {
            let mut builder = anthropic::Client::builder().api_key(key);
            if let Some(u) = base_url {
                builder = builder.base_url(u);
            }
            let client = builder.build()?;
            let model = client.completion_model(&cfg.model);
            run_model(model, prompt, temperature, served_field).await
        }
        JudgeProvider::OpenAI => {
            // The Responses API client is OpenAI's default (`openai::Client`)
            // — Chat Completions is a deferred, OpenAI-compatible future, not
            // this path.
            let mut builder = openai::Client::builder().api_key(key);
            if let Some(u) = base_url {
                builder = builder.base_url(u);
            }
            let client = builder.build()?;
            let model = client.completion_model(&cfg.model);
            run_model(model, prompt, temperature, served_field).await
        }
        JudgeProvider::OpenRouter => {
            let mut builder = openrouter::Client::builder().api_key(key);
            if let Some(u) = base_url {
                builder = builder.base_url(u);
            }
            let client = builder.build()?;
            let model = client.completion_model(&cfg.model);
            run_model(model, prompt, temperature, served_field).await
        }
        JudgeProvider::Gemini => {
            let mut builder = gemini::Client::builder().api_key(key);
            if let Some(u) = base_url {
                builder = builder.base_url(u);
            }
            let client = builder.build()?;
            let model = client.completion_model(&cfg.model);
            run_model(model, prompt, temperature, served_field).await
        }
    }
}

/// Whether a completion error is the provider rejecting the temperature
/// parameter: a 400 whose body mentions "temperature". Error-driven, not a
/// model-name list — reasoning models across providers reject the param in
/// different ways, but all as a 400 naming the offending field.
fn is_temperature_rejected(err: &CompletionError) -> bool {
    let is_400 = err.provider_response_status().map(|s| s.as_u16()) == Some(400);
    let mentions_temperature = err
        .provider_response_body()
        .map(|b| b.to_lowercase().contains("temperature"))
        .unwrap_or(false);
    is_400 && mentions_temperature
}

/// Whether a completion error is clearly transient: a rate limit (429), a
/// server error (5xx), or a transport-level failure with no HTTP status at all
/// (connection refused, reset, timeout — rig surfaces these as
/// `CompletionError::HttpError` wrapping a non-status variant). Everything
/// else — including a status this doesn't recognize — is treated as not
/// transient: the classification is deliberately conservative, so an
/// unclassifiable error is never retried.
fn is_transient(err: &CompletionError) -> bool {
    match err.provider_response_status() {
        Some(status) => {
            let code = status.as_u16();
            code == 429 || (500..600).contains(&code)
        }
        None => matches!(err, CompletionError::HttpError(_)),
    }
}

/// Redact the query string of every URL in a rendered provider/transport
/// error, before that text becomes a string that is persisted, printed, or
/// recorded in an artifact. The module invariant is that a key never leaves
/// the process except to its provider's endpoint, and the error path is a way
/// out: rig-core 0.40's Gemini client authenticates by putting the key in the
/// URL query (`...:generateContent?key=<GEMINI_API_KEY>`, `client.rs:83`), and
/// reqwest's error `Display` appends the full, unredacted URL — so a transport
/// failure (DNS, TLS, timeout, connection refused: exactly the flaky cases the
/// retry path above exists for) renders the key in cleartext through
/// `CompletionError`'s own `Display`. Every rig error is funneled through here
/// at the one point it is turned into a message (`judge_with_base_url` and
/// `preflight_with_base_url` below), so the invariant survives the error path
/// too. The whole query string is dropped, not just `key=`-shaped parameters:
/// it is simpler, and nothing downstream needs a failed request's query to
/// diagnose it. A URL with no query, and text with no URL, pass through
/// unchanged. Only rendering is affected — the raw `CompletionError` still
/// reaches `is_transient`/`is_temperature_rejected` unscrubbed, so retry
/// classification is untouched.
fn scrub_provider_error(text: &str) -> String {
    const SCHEMES: [&str; 2] = ["https://", "http://"];
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some((offset, scheme_len)) = next_url(rest, &SCHEMES) {
        out.push_str(&rest[..offset]);
        let url_and_after = &rest[offset..];
        // The URL runs to the first character that cannot sit inside one as
        // reqwest renders it (`... for url (https://...)`): whitespace, or a
        // delimiter the surrounding message brackets it with.
        let end = url_and_after[scheme_len..]
            .find(|c: char| {
                c.is_whitespace() || matches!(c, '(' | ')' | '"' | '\'' | '<' | '>' | '|' | '`')
            })
            .map(|i| i + scheme_len)
            .unwrap_or(url_and_after.len());
        let url = &url_and_after[..end];
        match url.find('?') {
            Some(q) => {
                out.push_str(&url[..=q]);
                out.push_str("<redacted>");
            }
            None => out.push_str(url),
        }
        rest = &url_and_after[end..];
    }
    out.push_str(rest);
    out
}

/// The earliest scheme match in `s`, paired with that scheme's length, or
/// `None` when the text holds no URL. Neither scheme is a prefix of the other,
/// so at most one matches at any given index; the earliest offset wins.
fn next_url(s: &str, schemes: &[&str]) -> Option<(usize, usize)> {
    schemes
        .iter()
        .filter_map(|scheme| s.find(scheme).map(|i| (i, scheme.len())))
        .min_by_key(|(i, _)| *i)
}

/// The reconstructed request record written to `judge-request.json` — not the
/// literal bytes sent to the provider (rig doesn't expose those), but enough
/// to know what was asked for: which provider and model, whether temperature 0
/// was sent or omitted (and why), the fixed token budget, and the exact
/// prompt.
#[derive(Serialize)]
struct RequestRecord {
    provider: JudgeProvider,
    model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature_omitted: Option<&'static str>,
    max_tokens: u64,
    prompt: String,
}

/// Orchestrates one judge call end to end: temperature-0 attempt, a
/// temperature-fallback retry if the provider rejects the parameter, then a
/// single transient retry with a short fixed backoff if whatever attempt is
/// left standing failed with a clearly transient error. Returns the final
/// result alongside the request record describing what was actually sent, so
/// the caller can write `judge-request.json` even when every attempt failed.
async fn run_judge_call(
    cfg: &JudgeConfig,
    key: &str,
    prompt: &str,
    base_url: Option<&str>,
) -> (Result<CompletionOutcome, CompletionError>, RequestRecord) {
    let mut temperature = Some(JUDGE_TEMPERATURE);
    let mut temperature_omitted: Option<&'static str> = None;

    let mut result = send_completion(cfg, key, prompt, temperature, base_url).await;

    if let Err(err) = &result {
        if is_temperature_rejected(err) {
            temperature = None;
            temperature_omitted = Some("temperature omitted (provider rejected)");
            result = send_completion(cfg, key, prompt, temperature, base_url).await;
        }
    }

    if let Err(err) = &result {
        if is_transient(err) {
            tokio::time::sleep(RETRY_BACKOFF).await;
            result = send_completion(cfg, key, prompt, temperature, base_url).await;
        }
    }

    let record = RequestRecord {
        provider: cfg.provider,
        model: cfg.model.clone(),
        temperature,
        temperature_omitted,
        max_tokens: JUDGE_MAX_TOKENS,
        prompt: prompt.to_string(),
    };
    (result, record)
}

/// The judge's visible text, parsed by its first alphanumeric word,
/// case-insensitively: `yes` -> Yes, `no` -> No, anything else (including a
/// hedge like "Not sure" or an empty string) -> Unknown. Pure — a plain
/// `&str` in, `JudgeVerdict` out — so it's testable without a key, a mock
/// server, or any particular response JSON shape. This used to live inside a
/// JSON-navigating `parse_judge_response(v: &serde_json::Value, cfg:
/// &JudgeConfig)` written against one provider's wire shape
/// (`v["content"][0]["text"]`, Anthropic's Messages API); rig's
/// `CompletionResponse` normalizes `choice` into plain text across all four
/// providers before this ever runs, so that JSON navigation (and the
/// per-provider cost/served-model logic that used to live beside it) is gone
/// — see `run_model` and `judge_with_base_url` above for where that moved.
fn parse_verdict(text: &str) -> JudgeVerdict {
    let lowered = text.to_lowercase();
    let first = lowered
        .split(|c: char| !c.is_alphanumeric())
        .find(|w| !w.is_empty())
        .unwrap_or("");
    match first {
        "yes" => JudgeVerdict::Yes,
        "no" => JudgeVerdict::No,
        _ => JudgeVerdict::Unknown,
    }
}

#[cfg(test)]
mod parse_verdict_tests {
    use super::*;

    #[test]
    fn yes_no_unknown_map_correctly_and_ignore_case_and_punctuation() {
        for (text, want) in [
            ("Yes", JudgeVerdict::Yes),
            ("no.", JudgeVerdict::No),
            ("Not sure", JudgeVerdict::Unknown),
            ("", JudgeVerdict::Unknown),
        ] {
            assert_eq!(parse_verdict(text), want, "{text}");
        }
    }
}

#[cfg(test)]
mod served_model_field_tests {
    use super::*;

    #[test]
    fn each_provider_names_its_own_served_model_field() {
        assert_eq!(served_model_field(JudgeProvider::Anthropic), "model");
        assert_eq!(served_model_field(JudgeProvider::OpenAI), "model");
        assert_eq!(served_model_field(JudgeProvider::OpenRouter), "model");
        assert_eq!(served_model_field(JudgeProvider::Gemini), "modelVersion");
    }
}

#[cfg(test)]
mod scrub_provider_error_tests {
    use super::*;

    /// The core property: a URL carrying a credential in its query string
    /// comes out with the whole query gone, whether the secret is the last
    /// parameter or in the middle — the exact shapes rig-core 0.40's Gemini
    /// client builds (`?key=...` and `?alt=sse&key=...`).
    #[test]
    fn a_key_bearing_query_string_is_redacted_wherever_the_key_sits() {
        for input in [
            "error for url (https://host/path?alt=sse&key=SECRETVALUE)",
            "error for url (https://host/path?key=SECRETVALUE&alt=sse)",
            "https://generativelanguage.googleapis.com/v1beta/models/m:generateContent?key=SECRETVALUE",
        ] {
            let out = scrub_provider_error(input);
            assert!(!out.contains("SECRETVALUE"), "{input} -> {out}");
            assert!(out.contains("<redacted>"), "{input} -> {out}");
        }
    }

    /// A URL with no query string is left exactly as-is: there is nothing to
    /// hide, and mangling the host/path would only make a transport error
    /// harder to read.
    #[test]
    fn a_url_without_a_query_string_passes_through_unchanged() {
        let input = "connection refused for url (https://api.anthropic.com/v1/messages)";
        assert_eq!(scrub_provider_error(input), input);
    }

    /// Text with no URL at all is untouched — most provider errors (a 401
    /// body, a rate-limit message) carry no URL, and scrubbing must not
    /// rewrite them.
    #[test]
    fn text_without_a_url_passes_through_unchanged() {
        let input = "HttpError: ProviderError: 401 invalid x-api-key";
        assert_eq!(scrub_provider_error(input), input);
    }

    /// Only the query is dropped; the host and path survive, so a scrubbed
    /// error still says which endpoint failed.
    #[test]
    fn the_host_and_path_survive_redaction() {
        let out =
            scrub_provider_error("https://host:8443/v1beta/m:generateContent?key=SECRETVALUE");
        assert_eq!(out, "https://host:8443/v1beta/m:generateContent?<redacted>");
    }

    /// Every URL in a message is scrubbed, not just the first.
    #[test]
    fn multiple_urls_are_each_redacted() {
        let out =
            scrub_provider_error("tried http://a/x?key=AAA then https://b/y?key=BBB and gave up");
        assert!(!out.contains("AAA"), "{out}");
        assert!(!out.contains("BBB"), "{out}");
    }
}

#[cfg(test)]
mod judge_provider_tests {
    use super::*;

    #[derive(Debug, Deserialize)]
    struct Wrapper {
        provider: JudgeProvider,
    }

    #[test]
    fn all_four_providers_deserialize_from_their_lowercase_names() {
        for (s, want) in [
            ("anthropic", JudgeProvider::Anthropic),
            ("openai", JudgeProvider::OpenAI),
            ("openrouter", JudgeProvider::OpenRouter),
            ("gemini", JudgeProvider::Gemini),
        ] {
            let w: Wrapper = toml::from_str(&format!("provider = \"{s}\"")).unwrap();
            assert_eq!(w.provider, want, "{s}");
        }
    }

    /// A provider outside the closed set must fail loudly rather than being
    /// silently accepted as some free-form string — the whole point of a
    /// closed enum instead of a `String` field.
    #[test]
    fn an_unknown_provider_name_is_rejected() {
        let err = toml::from_str::<Wrapper>("provider = \"someother\"").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("someother"), "{msg}");
    }

    #[test]
    fn key_env_returns_the_four_fixed_names() {
        assert_eq!(JudgeProvider::Anthropic.key_env(), "ANTHROPIC_API_KEY");
        assert_eq!(JudgeProvider::OpenAI.key_env(), "OPENAI_API_KEY");
        assert_eq!(JudgeProvider::OpenRouter.key_env(), "OPENROUTER_API_KEY");
        assert_eq!(JudgeProvider::Gemini.key_env(), "GEMINI_API_KEY");
    }
}

#[cfg(test)]
mod judge_config_tests {
    use super::*;

    /// Load a judge file written from `contents`, cleaning the temp dir up
    /// before the caller can assert (and so panic) on the result.
    fn load_judge(name: &str, contents: &str) -> Result<JudgeConfig> {
        let dir =
            std::env::temp_dir().join(format!("zseval-judge-cfg-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("judge.toml");
        std::fs::write(&p, contents).unwrap();
        let got = JudgeConfig::load(&p);
        std::fs::remove_dir_all(&dir).ok();
        got
    }

    /// A judge file with every required field of the current (four-field)
    /// schema, as a starting point for tests that make exactly one thing
    /// wrong.
    fn valid_judge() -> String {
        "provider = \"anthropic\"\nmodel = \"m\"\n\
         price_in_usd_per_mtok = 3.0\nprice_out_usd_per_mtok = 15.0\n"
            .to_string()
    }

    /// A card loads for every supported provider, carrying the file's model
    /// and prices back verbatim.
    #[test]
    fn a_valid_card_loads_for_every_supported_provider() {
        for (name, want) in [
            ("anthropic", JudgeProvider::Anthropic),
            ("openai", JudgeProvider::OpenAI),
            ("openrouter", JudgeProvider::OpenRouter),
            ("gemini", JudgeProvider::Gemini),
        ] {
            let toml = format!(
                "provider = \"{name}\"\nmodel = \"m-{name}\"\n\
                 price_in_usd_per_mtok = 1.5\nprice_out_usd_per_mtok = 7.5\n"
            );
            let cfg = load_judge(&format!("provider-{name}"), &toml).unwrap();
            assert_eq!(cfg.provider, want, "{name}");
            assert_eq!(cfg.model, format!("m-{name}"));
            assert_eq!(cfg.price_in_usd_per_mtok, 1.5);
            assert_eq!(cfg.price_out_usd_per_mtok, 7.5);
        }
    }

    /// Inline fixture shaped like `judges/opus.toml`, independent of the real
    /// file on disk. `a_shipped_judge_card_loads_with_the_new_schema` below
    /// is the regression guard against the real files drifting from this
    /// shape; this test pins the shape itself without touching the
    /// filesystem.
    #[test]
    fn a_card_shaped_like_the_future_opus_toml_loads_with_its_own_model_and_prices() {
        let toml = "provider = \"anthropic\"\nmodel = \"claude-opus-4-8\"\n\
                     price_in_usd_per_mtok = 5.0\nprice_out_usd_per_mtok = 25.0\n";
        let cfg = load_judge("opus-shaped", toml).unwrap();
        assert_eq!(cfg.model, "claude-opus-4-8");
        assert_eq!(cfg.price_in_usd_per_mtok, 5.0);
        assert_eq!(cfg.price_out_usd_per_mtok, 25.0);
    }

    /// The harness's actual regression guard on the shipped ruler cards: every
    /// card in `judges/` must load under the current four-field schema.
    /// Iterates the real directory (not an inline fixture) so a card left on
    /// the old `api_url`/`api_key_env` shape, or missing a required field,
    /// fails this test rather than only being caught the first time someone
    /// runs `zseval` with `--judge`.
    #[test]
    fn every_shipped_judge_card_loads_under_the_current_schema() {
        let judges_dir = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../judges"));
        let mut checked = 0;
        for entry in std::fs::read_dir(judges_dir).expect("read judges/ dir") {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            let cfg = JudgeConfig::load(&path)
                .unwrap_or_else(|e| panic!("{} failed to load: {e:#}", path.display()));
            assert!(
                !cfg.model.is_empty(),
                "{}: model must not be empty",
                path.display()
            );
            checked += 1;
        }
        assert!(
            checked >= 2,
            "expected at least sonnet.toml and opus.toml under {}",
            judges_dir.display()
        );
    }

    #[test]
    fn a_missing_file_is_a_loud_error() {
        let err = JudgeConfig::load(Path::new("/no/such/judge.toml")).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("/no/such/judge.toml"), "{msg}");
    }

    /// A judge file that doesn't say what model to grade with must fail
    /// loudly, never quietly inherit some other model.
    #[test]
    fn a_missing_field_is_a_loud_error() {
        let err = load_judge(
            "missing-field",
            "provider = \"anthropic\"\n\
             price_in_usd_per_mtok = 3.0\nprice_out_usd_per_mtok = 15.0\n",
        )
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("model"), "{msg}");
    }

    /// A provider outside the closed set must fail loudly, listing (or at
    /// least naming) the offending value rather than being silently accepted.
    #[test]
    fn a_provider_outside_the_closed_set_is_a_loud_error() {
        let toml = valid_judge().replace("\"anthropic\"", "\"someother\"");
        let err = load_judge("bad-provider", &toml).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("someother"), "{msg}");
    }

    #[test]
    fn a_non_numeric_price_is_a_loud_error() {
        let toml = valid_judge().replace("3.0", "\"three dollars\"");
        assert!(load_judge("non-numeric", &toml).is_err());
    }

    #[test]
    fn a_negative_price_is_a_loud_error() {
        // Parses as TOML but is nonsense: a negative price would credit the
        // budget cap for every judge call.
        let toml = valid_judge().replace("= 3.0", "= -3.0");
        let err = load_judge("negative", &toml).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("price_in_usd_per_mtok"), "{msg}");
    }

    /// TOML itself has `nan`/`inf`/`-inf` float literals, so a non-finite
    /// price parses fine as an f64 — `validate()` is the only thing that
    /// catches it.
    #[test]
    fn a_non_finite_price_is_a_loud_error() {
        for literal in ["nan", "inf", "-inf"] {
            let toml = valid_judge().replace("3.0", literal);
            let err = load_judge("non-finite", &toml).unwrap_err();
            let msg = format!("{err:#}");
            assert!(msg.contains("price_in_usd_per_mtok"), "{literal}: {msg}");
        }
    }

    #[test]
    fn an_empty_model_is_a_loud_error() {
        let toml = valid_judge().replace("model = \"m\"", "model = \"\"");
        let err = load_judge("empty-model", &toml).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("model"), "{msg}");
    }

    #[test]
    fn a_model_with_whitespace_or_control_characters_is_a_loud_error() {
        for bad in ["bad model", "bad\tmodel", "bad\nmodel"] {
            let toml = valid_judge().replace("model = \"m\"", &format!("model = {bad:?}"));
            let err = load_judge("bad-model-chars", &toml).unwrap_err();
            let msg = format!("{err:#}");
            assert!(msg.contains("model"), "{bad:?}: {msg}");
        }
    }

    /// A key not in the schema is a typo, not a comment — swallowing it
    /// would let `--judge` look applied while grading with the old value.
    #[test]
    fn an_unknown_field_is_a_loud_error() {
        let toml = valid_judge() + "temperture = 0.5\n";
        assert!(load_judge("unknown-field", &toml).is_err());
    }

    /// `deny_unknown_fields` alone answers "unknown field `api_key`", which
    /// reads as a spelling problem. The rule is that the key never goes in the
    /// file at all, so the error must state that rule.
    #[test]
    fn a_key_shaped_field_names_the_rule_it_breaks() {
        for field in ["api_key", "key", "token", "secret", "API_KEY"] {
            let toml = valid_judge() + &format!("{field} = \"sk-ant-oops\"\n");
            let err = load_judge("key-shaped", &toml).unwrap_err();
            let msg = format!("{err:#}");
            assert!(msg.contains("no secrets"), "{field}: {msg}");
        }
    }

    /// The verified exfiltration vector this change closes: a card naming
    /// both removed routing fields alongside otherwise-valid data must be
    /// rejected — loading never reaches network code at all, so "before any
    /// network activity" holds by construction — with an error naming the
    /// fields and the security reason, not a generic "unknown field".
    #[test]
    fn the_verified_legacy_attack_file_is_rejected_with_the_security_rationale() {
        let toml = valid_judge()
            + "api_url = \"https://evil.example/v1/messages\"\n\
               api_key_env = \"GITHUB_TOKEN\"\n";
        let err = load_judge("legacy-attack", &toml).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("api_url"), "{msg}");
        assert!(msg.contains("api_key_env"), "{msg}");
        assert!(msg.contains("security"), "{msg}");
        assert!(msg.contains("provider"), "{msg}");
    }

    /// Each removed routing field is rejected on its own with the same
    /// targeted error, case-insensitively, not only when paired together.
    #[test]
    fn each_removed_routing_field_is_rejected_on_its_own() {
        for extra in [
            "api_url = \"https://evil.example/v1/messages\"\n",
            "api_key_env = \"GITHUB_TOKEN\"\n",
            "API_URL = \"https://evil.example/v1/messages\"\n",
        ] {
            let toml = valid_judge() + extra;
            let err = load_judge("legacy-attack-alone", &toml).unwrap_err();
            let msg = format!("{err:#}");
            assert!(msg.contains("security"), "{extra}: {msg}");
            assert!(msg.contains("provider"), "{extra}: {msg}");
        }
    }
}

#[cfg(test)]
mod judge_availability_tests {
    use super::*;

    /// Availability follows the exact var name given, not merely "some var is
    /// set somewhere". Read-only against vars this process is already
    /// guaranteed (or guaranteed not) to have: `set_var` is not thread-safe,
    /// and these tests run alongside other code touching the env.
    #[test]
    fn availability_follows_the_configured_env_var() {
        assert!(have_api_key("PATH"), "a var that is set reads as available");
        let unset = format!("ZSEVAL_TEST_JUDGE_KEY_{}", std::process::id());
        assert!(!have_api_key(&unset), "an unset var reads as unavailable");
    }

    /// The regression this guards against: `available()` silently keying off
    /// one hard-coded var (e.g. always `ANTHROPIC_API_KEY`) regardless of
    /// which provider the config actually names. For every provider,
    /// availability must equal `have_api_key` applied to *that* provider's
    /// own `key_env()` — a fixed lookup that ignored `cfg.provider` would
    /// report a ruler as ready and then fail (or send the wrong provider's
    /// secret) at call time.
    #[test]
    fn availability_follows_the_configured_providers_own_env_var() {
        for provider in [
            JudgeProvider::Anthropic,
            JudgeProvider::OpenAI,
            JudgeProvider::OpenRouter,
            JudgeProvider::Gemini,
        ] {
            let cfg = JudgeConfig {
                provider,
                model: "m".into(),
                price_in_usd_per_mtok: 1.0,
                price_out_usd_per_mtok: 1.0,
            };
            assert_eq!(
                LlmJudge::new(cfg).available(),
                have_api_key(provider.key_env()),
                "{provider:?}"
            );
        }
    }
}

#[cfg(test)]
mod shared_runtime_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};

    /// The `Judge` trait stays sync so trial worker threads under `--jobs`
    /// stay plain `std::thread`s; `shared_runtime()` is the bridge each of
    /// those threads calls `block_on` through. This is the guard for that
    /// concurrency claim: several OS threads must be able to `block_on` on
    /// the *same* shared runtime at the same time, with none of them
    /// panicking or deadlocking the others out.
    #[test]
    fn concurrent_block_on_calls_from_multiple_threads_all_complete() {
        const THREADS: usize = 8;
        // Barrier forces every thread into `block_on` at (as close as
        // possible to) the same instant, rather than one at a time.
        let barrier = Arc::new(Barrier::new(THREADS));
        let completed = Arc::new(AtomicUsize::new(0));

        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                let barrier = Arc::clone(&barrier);
                let completed = Arc::clone(&completed);
                std::thread::spawn(move || {
                    barrier.wait();
                    let result = shared_runtime().block_on(async { 1 + 1 });
                    assert_eq!(result, 2);
                    completed.fetch_add(1, Ordering::SeqCst);
                })
            })
            .collect();

        for h in handles {
            h.join()
                .expect("thread panicked while block_on-ing the shared runtime");
        }
        assert_eq!(completed.load(Ordering::SeqCst), THREADS);
    }
}

/// A minimal hand-rolled HTTP/1.1 responder for offline judge-transport tests:
/// the local mock server the test-only rig `base_url` seam points at. Not a
/// general HTTP server: reads one full request (headers, then exactly
/// `Content-Length` more body bytes) per accepted connection and replies with
/// the next canned `(status, body)` in order, closing the connection afterward
/// so the next call opens a fresh one — no keep-alive bookkeeping needed. No
/// new dev-dependency: `std::net::TcpListener` is enough for canned
/// status/body responses, and rig's client only needs plain HTTP (no TLS) once
/// `base_url` points at `http://127.0.0.1:<port>`.
#[cfg(test)]
mod mock_server {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};

    pub struct MockServer {
        pub base_url: String,
    }

    impl MockServer {
        /// Serves `responses` in order, one per accepted connection, then
        /// stops (the listener is dropped when the spawned thread's loop
        /// ends). A test expecting exactly `n` calls passes exactly `n`
        /// responses; a call beyond that gets a real "connection refused"
        /// once the listener is gone, rather than hanging.
        pub fn start(responses: Vec<(u16, String)>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock listener");
            let addr = listener.local_addr().expect("mock listener local_addr");
            std::thread::spawn(move || {
                for (status, body) in responses {
                    let Ok((mut stream, _)) = listener.accept() else {
                        break;
                    };
                    read_request(&mut stream);
                    let reason = reason_phrase(status);
                    let response = format!(
                        "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len(),
                    );
                    let _ = stream.write_all(response.as_bytes());
                    let _ = stream.flush();
                }
            });
            MockServer {
                base_url: format!("http://{addr}"),
            }
        }
    }

    fn reason_phrase(status: u16) -> &'static str {
        match status {
            200 => "OK",
            400 => "Bad Request",
            401 => "Unauthorized",
            429 => "Too Many Requests",
            500 => "Internal Server Error",
            503 => "Service Unavailable",
            _ => "Error",
        }
    }

    /// Reads the header block, then exactly `Content-Length` more bytes of
    /// body — good enough for the small JSON bodies rig sends; not a general
    /// parser (no chunked transfer-encoding, no pipelining).
    fn read_request(stream: &mut TcpStream) {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        let header_end = loop {
            let n = stream.read(&mut chunk).expect("read request headers");
            if n == 0 {
                return;
            }
            buf.extend_from_slice(&chunk[..n]);
            if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
                break pos + 4;
            }
        };
        let headers = String::from_utf8_lossy(&buf[..header_end]);
        let content_length: usize = headers
            .lines()
            .find_map(|l| {
                let (name, value) = l.split_once(':')?;
                if name.eq_ignore_ascii_case("content-length") {
                    value.trim().parse().ok()
                } else {
                    None
                }
            })
            .unwrap_or(0);
        while buf.len() < header_end + content_length {
            let n = stream.read(&mut chunk).expect("read request body");
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
    }

    fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack.windows(needle.len()).position(|w| w == needle)
    }
}

/// Offline tests for the full `judge_with_base_url` seam: the test-only rig
/// `base_url` points at a local mock server, covering probe success/failure,
/// temperature fallback, and retry. Anthropic is used as the representative
/// provider throughout — its rig raw response shape is a direct,
/// well-understood JSON structure, which keeps these tests focused on the
/// transport/retry/artifact behavior under test rather than on hand-rolling
/// four providers' distinct wire shapes; the other three providers'
/// client-construction and served-model-field code paths are exercised by
/// `served_model_field_tests` (pure) and by compiling against rig's own types.
#[cfg(test)]
mod judge_transport_tests {
    use super::mock_server::MockServer;
    use super::*;
    use std::sync::Mutex;

    /// `std::env::set_var` is process-global and, on the rustc this repo
    /// pins, `unsafe`; this crate's other tests avoid touching real env vars
    /// entirely (see `judge_availability_tests`). These tests are the
    /// exception — `judge()` must read a provider key from the environment —
    /// so they serialize on this lock and always restore the var afterward.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvVarGuard {
        name: &'static str,
        previous: Option<String>,
    }

    impl EnvVarGuard {
        /// An obviously-bogus value: these tests never reach a real network,
        /// so there is no key to protect, only presence to satisfy.
        fn set(name: &'static str, value: &str) -> Self {
            let previous = std::env::var(name).ok();
            unsafe { std::env::set_var(name, value) };
            EnvVarGuard { name, previous }
        }
    }

    impl EnvVarGuard {
        /// Ensures the var is unset for the guard's lifetime, restoring
        /// whatever was there before on drop — the presence-check test needs
        /// a *deterministically unset* var, not merely "whatever this
        /// process happens to have".
        fn unset(name: &'static str) -> Self {
            let previous = std::env::var(name).ok();
            unsafe { std::env::remove_var(name) };
            EnvVarGuard { name, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(v) => unsafe { std::env::set_var(self.name, v) },
                None => unsafe { std::env::remove_var(self.name) },
            }
        }
    }

    fn temp_run_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "zseval-judge-transport-{}-{name}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn test_cfg() -> JudgeConfig {
        JudgeConfig {
            provider: JudgeProvider::Anthropic,
            model: "claude-sonnet-4-6".into(),
            price_in_usd_per_mtok: 3.0,
            price_out_usd_per_mtok: 15.0,
        }
    }

    /// A minimal, valid Anthropic Messages API response body: exactly the
    /// fields `rig_core::providers::anthropic::completion::CompletionResponse`
    /// requires to deserialize — `content` (tagged `"type": "text"`), `id`,
    /// `model`, `role`, `stop_reason`, `stop_sequence`, and `usage`.
    fn anthropic_success_body(text: &str, model: &str, input: u64, output: u64) -> String {
        serde_json::json!({
            "id": "msg_test",
            "type": "message",
            "model": model,
            "role": "assistant",
            "content": [{"type": "text", "text": text}],
            "stop_reason": "end_turn",
            "stop_sequence": serde_json::Value::Null,
            "usage": {
                "input_tokens": input,
                "output_tokens": output,
                "cache_read_input_tokens": serde_json::Value::Null,
                "cache_creation_input_tokens": serde_json::Value::Null,
            },
        })
        .to_string()
    }

    fn anthropic_error_body(message: &str) -> String {
        serde_json::json!({
            "type": "error",
            "error": {"type": "invalid_request_error", "message": message},
        })
        .to_string()
    }

    /// The seam's offline success path — a verdict comes back from the mock
    /// server and cost math prices usage at the card's rates into `cost_usd`.
    #[test]
    fn success_path_through_the_seam_returns_a_verdict_and_prices_cost_at_the_cards_rates() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _key = EnvVarGuard::set("ANTHROPIC_API_KEY", "sk-ant-test-bogus-not-real");
        let server = MockServer::start(vec![(
            200,
            anthropic_success_body("Yes", "claude-sonnet-4-6-20260101", 1_000_000, 1_000_000),
        )]);
        let run_dir = temp_run_dir("success");
        let cfg = test_cfg();

        let outcome = judge_with_base_url(
            &cfg,
            "Did the agent do the thing?",
            "evidence text",
            &run_dir,
            Some(&server.base_url),
        )
        .expect("judge call should succeed against the mock server");

        assert_eq!(outcome.verdict, JudgeVerdict::Yes);
        assert!(
            (outcome.cost_usd - (cfg.price_in_usd_per_mtok + cfg.price_out_usd_per_mtok)).abs()
                < 1e-9,
            "{}",
            outcome.cost_usd
        );
        std::fs::remove_dir_all(&run_dir).ok();
    }

    /// A provider 400 naming `temperature` is retried once without it, and the
    /// omission is recorded in `judge-request.json`.
    #[test]
    fn a_provider_rejecting_temperature_is_retried_without_it_and_the_omission_is_recorded() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _key = EnvVarGuard::set("ANTHROPIC_API_KEY", "sk-ant-test-bogus-not-real");
        let server = MockServer::start(vec![
            (
                400,
                anthropic_error_body("temperature: Extra inputs are not permitted"),
            ),
            (200, anthropic_success_body("Yes", "claude-x", 10, 2)),
        ]);
        let run_dir = temp_run_dir("temp-fallback");

        let outcome = judge_with_base_url(
            &test_cfg(),
            "rubric",
            "evidence",
            &run_dir,
            Some(&server.base_url),
        )
        .expect("should succeed on the retry without temperature");
        assert_eq!(outcome.verdict, JudgeVerdict::Yes);

        let request_json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(run_dir.join("judge-request.json")).unwrap())
                .unwrap();
        assert!(request_json.get("temperature").is_none(), "{request_json}");
        assert_eq!(
            request_json["temperature_omitted"],
            "temperature omitted (provider rejected)"
        );
        std::fs::remove_dir_all(&run_dir).ok();
    }

    /// A single transient (429) blip does not cost a verdict — the retry
    /// succeeds and the trial records it normally.
    #[test]
    fn a_rate_limit_is_retried_once_and_succeeds() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _key = EnvVarGuard::set("ANTHROPIC_API_KEY", "sk-ant-test-bogus-not-real");
        let server = MockServer::start(vec![
            (429, anthropic_error_body("rate limited")),
            (200, anthropic_success_body("No", "claude-x", 5, 1)),
        ]);
        let run_dir = temp_run_dir("retry-success");

        let outcome = judge_with_base_url(
            &test_cfg(),
            "rubric",
            "evidence",
            &run_dir,
            Some(&server.base_url),
        )
        .expect("should succeed after one transient retry");
        assert_eq!(outcome.verdict, JudgeVerdict::No);
        std::fs::remove_dir_all(&run_dir).ok();
    }

    /// Persistent failure (both the attempt and its single retry fail)
    /// surfaces the error rather than retrying again.
    #[test]
    fn a_persistent_rate_limit_surfaces_as_an_error_after_exactly_one_retry() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _key = EnvVarGuard::set("ANTHROPIC_API_KEY", "sk-ant-test-bogus-not-real");
        let server = MockServer::start(vec![
            (429, anthropic_error_body("rate limited")),
            (429, anthropic_error_body("still rate limited")),
        ]);
        let run_dir = temp_run_dir("retry-fail");

        let err = judge_with_base_url(
            &test_cfg(),
            "rubric",
            "evidence",
            &run_dir,
            Some(&server.base_url),
        )
        .expect_err("should surface the error after exhausting the single retry");
        let msg = format!("{err:#}").to_lowercase();
        assert!(msg.contains("rate"), "{msg}");
        std::fs::remove_dir_all(&run_dir).ok();
    }

    /// A non-retryable error (401) is not retried at all — exactly one
    /// response is queued, so a second attempt would hit a closed listener.
    #[test]
    fn a_non_retryable_error_is_not_retried() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _key = EnvVarGuard::set("ANTHROPIC_API_KEY", "sk-ant-test-bogus-not-real");
        let server = MockServer::start(vec![(401, anthropic_error_body("invalid x-api-key"))]);
        let run_dir = temp_run_dir("no-retry-401");

        let err = judge_with_base_url(
            &test_cfg(),
            "rubric",
            "evidence",
            &run_dir,
            Some(&server.base_url),
        )
        .expect_err("401 should not be retried");
        let msg = format!("{err:#}").to_lowercase();
        assert!(
            msg.contains("401") || msg.contains("unauthorized") || msg.contains("api-key"),
            "{msg}"
        );
        std::fs::remove_dir_all(&run_dir).ok();
    }

    /// `judge-request.json` and `judge-response.json` record the reconstructed
    /// request and the served model read from the raw response — a fact, not
    /// an echo of the card's configured model.
    #[test]
    fn artifacts_record_the_request_and_the_served_model_from_the_raw_response() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _key = EnvVarGuard::set("ANTHROPIC_API_KEY", "sk-ant-test-bogus-not-real");
        let server = MockServer::start(vec![(
            200,
            anthropic_success_body("Yes", "claude-sonnet-4-6-20260101-served", 7, 3),
        )]);
        let run_dir = temp_run_dir("artifacts");
        let cfg = test_cfg(); // asks for claude-sonnet-4-6

        let outcome = judge_with_base_url(
            &cfg,
            "the rubric",
            "the evidence",
            &run_dir,
            Some(&server.base_url),
        )
        .expect("mock success");

        assert_eq!(
            outcome.model.as_deref(),
            Some("claude-sonnet-4-6-20260101-served")
        );
        assert_ne!(
            outcome.model.as_deref(),
            Some(cfg.model.as_str()),
            "must not echo back the card's configured model"
        );

        let request_json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(run_dir.join("judge-request.json")).unwrap())
                .unwrap();
        assert_eq!(request_json["provider"], "anthropic");
        assert_eq!(request_json["model"], cfg.model);
        assert_eq!(request_json["temperature"], JUDGE_TEMPERATURE);
        assert_eq!(request_json["max_tokens"], JUDGE_MAX_TOKENS);
        assert!(request_json["prompt"]
            .as_str()
            .unwrap()
            .contains("the rubric"));

        let response_json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(run_dir.join("judge-response.json")).unwrap())
                .unwrap();
        assert_eq!(response_json["model"], "claude-sonnet-4-6-20260101-served");

        std::fs::remove_dir_all(&run_dir).ok();
    }

    // -----------------------------------------------------------------
    // `LlmJudge::preflight()` — same mock-server seam, no run_dir (a probe is
    // not a trial: it writes no artifacts, see `preflight`'s doc).
    // -----------------------------------------------------------------

    /// The key-presence check runs before any network call, so it needs no
    /// mock server at all — an unset var must fail, naming the exact
    /// configured provider's env var.
    #[test]
    fn preflight_fails_before_any_call_when_the_key_is_unset_naming_the_exact_var() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _key = EnvVarGuard::unset("ANTHROPIC_API_KEY");
        let err = LlmJudge::new(test_cfg())
            .preflight_with_base_url(None)
            .expect_err("unset key must fail preflight");
        let msg = format!("{err:#}");
        assert!(msg.contains("ANTHROPIC_API_KEY"), "{msg}");
    }

    /// A successful dry-run (parseable Yes/No) passes preflight — same
    /// prompt template, max_tokens, and temperature-fallback path as a real
    /// judge call, through the same offline seam.
    #[test]
    fn preflight_succeeds_when_the_dry_run_returns_a_parseable_verdict() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _key = EnvVarGuard::set("ANTHROPIC_API_KEY", "sk-ant-test-bogus-not-real");
        let server = MockServer::start(vec![(
            200,
            anthropic_success_body("Yes", "claude-sonnet-4-6-20260101", 12, 1),
        )]);
        LlmJudge::new(test_cfg())
            .preflight_with_base_url(Some(&server.base_url))
            .expect("a parseable Yes must pass preflight");
    }

    /// A dry-run response with no parseable verdict word (e.g. thinking
    /// exhausted the budget, or an off-topic hedge) must reject up front,
    /// explaining that the judge produced no verdict.
    #[test]
    fn preflight_fails_when_the_dry_run_produces_no_parseable_verdict() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _key = EnvVarGuard::set("ANTHROPIC_API_KEY", "sk-ant-test-bogus-not-real");
        let server = MockServer::start(vec![(
            200,
            anthropic_success_body(
                "I would need more context to say anything at all here.",
                "claude-sonnet-4-6-20260101",
                12,
                1,
            ),
        )]);
        let err = LlmJudge::new(test_cfg())
            .preflight_with_base_url(Some(&server.base_url))
            .expect_err("no parseable Yes/No must fail preflight");
        let msg = format!("{err:#}").to_lowercase();
        assert!(msg.contains("verdict"), "{msg}");
    }

    /// A dry-run call that errors outright (bad auth, unknown model — here
    /// simulated as a 401) must exit the same way: preflight relays the
    /// provider error rather than treating it as a pass.
    #[test]
    fn preflight_fails_when_the_dry_run_call_itself_errors() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _key = EnvVarGuard::set("ANTHROPIC_API_KEY", "sk-ant-test-bogus-not-real");
        let server = MockServer::start(vec![(401, anthropic_error_body("invalid x-api-key"))]);
        let err = LlmJudge::new(test_cfg())
            .preflight_with_base_url(Some(&server.base_url))
            .expect_err("a provider error must fail preflight");
        let msg = format!("{err:#}").to_lowercase();
        assert!(
            msg.contains("401") || msg.contains("unauthorized") || msg.contains("api-key"),
            "{msg}"
        );
    }

    /// Probe cost must never land in the report. `preflight_with_base_url`
    /// returns `Result<()>` — there is no cost or token field to hand back to
    /// the caller even with exaggerated usage counts — and, unlike a real
    /// `judge()` call, it takes no `run_dir`, so it never writes
    /// `judge-request.json` / `judge-response.json` for anything downstream to
    /// read a cost from. This is the concrete guard for that property.
    #[test]
    fn a_successful_preflight_probe_returns_no_cost_and_writes_no_artifacts() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _key = EnvVarGuard::set("ANTHROPIC_API_KEY", "sk-ant-test-bogus-not-real");
        // Deliberately huge usage: if cost accounting could leak through at
        // all, it would be obvious against numbers this size.
        let server = MockServer::start(vec![(
            200,
            anthropic_success_body("Yes", "claude-sonnet-4-6-20260101", 5_000_000, 5_000_000),
        )]);
        let run_dir = temp_run_dir("preflight-no-artifacts");

        LlmJudge::new(test_cfg())
            .preflight_with_base_url(Some(&server.base_url))
            .expect("preflight should succeed");

        // No run_dir was ever passed to the probe, so nothing was written
        // here (or anywhere) — unlike a real judge() call, which always
        // writes both artifacts alongside its cost.
        assert!(!run_dir.join("judge-request.json").exists());
        assert!(!run_dir.join("judge-response.json").exists());
        std::fs::remove_dir_all(&run_dir).ok();
    }

    /// The security regression this fix exists for: a Gemini transport failure
    /// must not surface the API key. Gemini (unlike the other three providers)
    /// authenticates via a `?key=<GEMINI_API_KEY>` URL query, and reqwest's
    /// error `Display` echoes the full URL — so a real connection failure
    /// renders the key through `CompletionError`'s `Display`. This drives
    /// `judge_with_base_url` (a Gemini card pointed at a closed port, forcing
    /// exactly that failure) and asserts the surfaced error, rendered the way
    /// the runner writes it into `trial.json` (`{e:#}`, which walks the whole
    /// anyhow chain), carries no key. The positive `<redacted>` check proves
    /// the scrub actually fired on a rendered URL, not that the URL simply
    /// never appeared.
    #[test]
    fn a_gemini_transport_failure_does_not_leak_the_key_into_the_surfaced_error() {
        const SECRET: &str = "SECRETVALUE-gemini-leak-canary";
        let _lock = ENV_LOCK.lock().unwrap();
        let _key = EnvVarGuard::set("GEMINI_API_KEY", SECRET);

        // A port that was just bound and released: connecting to it fails, and
        // that transport failure is what renders the key-bearing URL.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind to grab a port");
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);

        let cfg = JudgeConfig {
            provider: JudgeProvider::Gemini,
            model: "gemini-2.5-flash".into(),
            price_in_usd_per_mtok: 1.0,
            price_out_usd_per_mtok: 1.0,
        };
        let run_dir = temp_run_dir("gemini-leak");

        let err = judge_with_base_url(&cfg, "rubric", "evidence", &run_dir, Some(&base_url))
            .expect_err("a closed port must fail the judge call");
        let rendered = format!("{err:#}");
        assert!(
            !rendered.contains(SECRET),
            "key leaked into error: {rendered}"
        );
        assert!(
            rendered.contains("<redacted>"),
            "expected a scrubbed URL query in the error: {rendered}"
        );
        std::fs::remove_dir_all(&run_dir).ok();
    }
}
