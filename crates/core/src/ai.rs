//! The optional AI review: the match's facts and the death screenshots go to
//! the player's own AI (Claude, ChatGPT or Gemini with their API key, a
//! model on this PC through Ollama, or the AI app they already use, which
//! asks Vantage's MCP server for the match). The answer must fit the fixed
//! format `vantage.ai-review/1`, and it is saved once per match: it can't be
//! rerun, so every match is judged the same way, once.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::coach::Debrief;

pub const FORMAT_VERSION: u32 = 1;
pub const SCHEMA: &str = "vantage.ai-review/1";
/// Long enough for a slow model to write its longest answer: a request that
/// times out has usually been charged for already.
const TIMEOUT: Duration = Duration::from_secs(600);
const OLLAMA: &str = "http://localhost:11434";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Provider {
    /// What `config.toml` calls it.
    pub id: &'static str,
    pub name: &'static str,
    /// What the key is called, or `None` when it needs none.
    pub key: Option<&'static str>,
    pub model: &'static str,
    /// Runs on this PC: nothing leaves the computer.
    pub local: bool,
}

pub const PROVIDERS: [Provider; 4] = [
    Provider {
        id: "anthropic",
        name: "Claude",
        key: Some("Anthropic API key"),
        model: "claude-sonnet-5-5",
        local: false,
    },
    Provider {
        id: "openai",
        name: "ChatGPT",
        key: Some("OpenAI API key"),
        model: "gpt-5",
        local: false,
    },
    Provider {
        id: "gemini",
        name: "Gemini",
        key: Some("Google AI Studio API key"),
        model: "gemini-2.5-flash",
        local: false,
    },
    Provider {
        id: "ollama",
        name: "Ollama",
        key: None,
        model: "gemma3",
        local: true,
    },
];

pub fn provider(id: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().find(|p| p.id == id)
}

/// What `config.toml` calls the AI app route: no key, the app asks Vantage.
pub const APP: &str = "app";

/// An AI app that runs Vantage's MCP server (`vantage mcp`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct App {
    /// What `config.toml` calls it.
    pub id: &'static str,
    pub name: &'static str,
}

pub const APPS: [App; 3] = [
    App {
        id: "claude-desktop",
        name: "Claude Desktop",
    },
    App {
        id: "claude-code",
        name: "Claude Code",
    },
    App {
        id: "other",
        name: "your AI app",
    },
];

/// The configured app, or "your AI app" when it's unknown.
pub fn app(id: Option<&str>) -> &'static App {
    APPS.iter().find(|a| Some(a.id) == id).unwrap_or(&APPS[2])
}

/// The app's name from what it calls itself when it connects, for the saved review.
pub fn app_from_client(client: &str) -> Option<&'static str> {
    match client.to_lowercase().as_str() {
        "claude-ai" | "claude desktop" | "claude-desktop" => Some("Claude Desktop"),
        "claude-code" | "claude code" => Some("Claude Code"),
        _ => None,
    }
}

/// A model to pick for a provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Model {
    /// What the API calls it, and `config.toml` stores.
    pub id: String,
    pub name: String,
    pub about: String,
}

/// Claude's models when its list can't be fetched (no key yet, or offline).
const CLAUDE_MODELS: [&str; 4] = [
    "claude-fable-5-1",
    "claude-opus-5-5",
    "claude-sonnet-5-5",
    "claude-haiku-5-5",
];

/// A model's name as people say it: `claude-sonnet-5-5` is `Sonnet 5.5`.
/// Other providers' ids are their names.
pub fn model_name(id: &str) -> String {
    let Some(rest) = id.strip_prefix("claude-") else {
        return id.to_owned();
    };
    let mut parts = rest.split('-').filter(|p| p.len() < 8);
    let Some(family) = parts.next() else {
        return id.to_owned();
    };
    let mut name: String = family.chars().take(1).flat_map(char::to_uppercase).collect();
    name.push_str(&family[family.chars().next().map_or(0, char::len_utf8)..]);
    let version: Vec<&str> = parts.collect();
    if !version.is_empty() {
        name.push(' ');
        name.push_str(&version.join("."));
    }
    name
}

/// The model `config.toml` asks for, or the provider's default.
pub fn chosen_model<'a>(p: &'a Provider, configured: Option<&'a str>) -> &'a str {
    configured.map(str::trim).filter(|m| !m.is_empty()).unwrap_or(p.model)
}

/// The models `p` offers, from its own list so new ones show up without an
/// update: the ones that can read images, and for Ollama, those on this PC.
pub fn models(p: &Provider, key: Option<&str>) -> Result<Vec<Model>> {
    let key = key.unwrap_or_default();
    let model = |id: &str, name: String, about: String| Model {
        id: id.to_owned(),
        name,
        about: if id == p.model {
            format!("{about} · the default").trim_start_matches(" · ").to_owned()
        } else {
            about
        },
    };
    if key.is_empty() && !p.local {
        return Ok(fallback_models(p));
    }
    let mut out: Vec<Model> = match p.id {
        "anthropic" => {
            let v = get(
                "https://api.anthropic.com/v1/models?limit=100",
                &[("x-api-key", key), ("anthropic-version", "2023-06-01")],
            )?;
            v["data"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|m| m["id"].as_str())
                .map(|id| model(id, model_name(id), id.to_owned()))
                .collect()
        }
        "openai" => {
            let auth = format!("Bearer {key}");
            let v = get("https://api.openai.com/v1/models", &[("authorization", &auth)])?;
            let skip = [
                "audio",
                "realtime",
                "tts",
                "transcribe",
                "image",
                "search",
                "embedding",
                "instruct",
            ];
            let mut ids: Vec<&str> = v["data"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|m| m["id"].as_str())
                .filter(|id| id.starts_with("gpt-") && !skip.iter().any(|s| id.contains(s)))
                .collect();
            ids.sort_unstable_by(|a, b| b.cmp(a));
            ids.into_iter()
                .map(|id| model(id, id.to_owned(), String::new()))
                .collect()
        }
        "gemini" => {
            let v = get(
                "https://generativelanguage.googleapis.com/v1beta/models?pageSize=200",
                &[("x-goog-api-key", key)],
            )?;
            v["models"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|m| {
                    m["supportedGenerationMethods"]
                        .as_array()
                        .is_some_and(|a| a.iter().any(|x| x == "generateContent"))
                })
                .filter_map(|m| {
                    let id = m["name"].as_str()?.trim_start_matches("models/");
                    id.starts_with("gemini").then(|| {
                        let name = m["displayName"].as_str().unwrap_or(id).to_owned();
                        model(id, name, id.to_owned())
                    })
                })
                .collect()
        }
        "ollama" => {
            let v = get(&format!("{OLLAMA}/api/tags"), &[]).map_err(|_| anyhow!("Ollama isn't running on this PC"))?;
            let mut out = Vec::new();
            for m in v["models"].as_array().into_iter().flatten() {
                let Some(id) = m["name"].as_str() else { continue };
                // Only models that can read the screenshots.
                let Ok(show) = post(&format!("{OLLAMA}/api/show"), &[], &json!({"model": id})) else {
                    continue;
                };
                if !show["capabilities"]
                    .as_array()
                    .is_some_and(|c| c.iter().any(|x| x == "vision"))
                {
                    continue;
                }
                let gb = m["size"].as_f64().unwrap_or(0.0) / 1e9;
                out.push(model(id, id.to_owned(), format!("{gb:.1} GB · installed")));
            }
            if out.is_empty() {
                bail!(
                    "no model on this PC can read screenshots. `ollama pull {}` gets one",
                    p.model
                );
            }
            out
        }
        other => bail!("unknown AI provider {other}"),
    };
    if out.is_empty() {
        out = fallback_models(p);
    }
    Ok(out)
}

/// The models to offer when the provider's list can't be fetched.
pub fn fallback_models(p: &Provider) -> Vec<Model> {
    let ids: Vec<&str> = match p.id {
        "anthropic" => CLAUDE_MODELS.to_vec(),
        _ => vec![p.model],
    };
    ids.into_iter()
        .map(|id| Model {
            id: id.to_owned(),
            name: model_name(id),
            about: if id == p.model {
                format!("{id} · the default")
            } else {
                id.to_owned()
            },
        })
        .collect()
}

/// The categories a finding may use: what only screenshots can show, then the habits.
pub const CATEGORIES: [(&str, &str); 12] = [
    ("exposure", "two angles"),
    ("position", "position"),
    ("crosshair", "crosshair"),
    ("peek", "peek"),
    ("awareness", "awareness"),
    ("untraded", "trading"),
    ("first-in", "going in first"),
    ("moving", "moving"),
    ("repeek", "re-peek"),
    ("ult", "ult"),
    ("reload", "reload"),
    ("clutch", "clutch"),
];

pub fn category_label(c: &str) -> &str {
    CATEGORIES.iter().find(|(k, _)| *k == c).map_or(c, |(_, l)| l)
}

/// The format, field by field, as `/review` shows it.
pub const FORMAT: [(&str, &str); 13] = [
    ("schema", "\"vantage.ai-review/1\""),
    ("summary", "text, at most 2 sentences"),
    ("findings", "1 to 5, most costly first, each with:"),
    ("  category", "one of the categories below"),
    ("  title", "at most 50 characters"),
    ("  rounds", "round numbers"),
    ("  screenshots", "file names from this match, e.g. R07.jpg"),
    ("  saw", "what the screenshots show"),
    ("  why", "why it cost the round"),
    ("  try", "one thing to do next match"),
    ("  confidence", "high, medium or low"),
    ("keep", "0 to 3 things done well: title, rounds, screenshots, saw"),
    ("unclear", "screenshots it couldn't judge, each with a reason"),
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Finding {
    pub category: String,
    pub title: String,
    pub rounds: Vec<u32>,
    pub screenshots: Vec<String>,
    pub saw: String,
    pub why: String,
    #[serde(rename = "try")]
    pub try_this: String,
    pub confidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Keep {
    pub title: String,
    pub rounds: Vec<u32>,
    pub screenshots: Vec<String>,
    pub saw: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Unclear {
    pub screenshot: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Output {
    pub schema: String,
    pub summary: String,
    pub findings: Vec<Finding>,
    pub keep: Vec<Keep>,
    pub unclear: Vec<Unclear>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Meta {
    /// `Claude (claude-sonnet-5-5)` or `Ollama (on this PC)`.
    pub provider: String,
    pub model: String,
    /// RFC 3339, UTC.
    pub saved_at: String,
    pub format: u32,
    /// The AI app that saved it through MCP, e.g. `Claude Desktop`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Saved {
    pub output: Output,
    pub meta: Meta,
}

pub fn saved_path(data_dir: &Path, match_id: &str) -> PathBuf {
    data_dir.join("ai").join(format!("{match_id}.json"))
}

pub fn load(data_dir: &Path, match_id: &str) -> Option<Saved> {
    let text = std::fs::read_to_string(saved_path(data_dir, match_id)).ok()?;
    serde_json::from_str(&text).ok()
}

/// How long after an AI app fetched a match it still counts as reviewing it.
pub const REVIEWING_FOR: Duration = Duration::from_secs(20 * 60);

/// Set while an AI app reviews the match: holds the app's name.
fn asked_path(data_dir: &Path, match_id: &str) -> PathBuf {
    data_dir.join("ai").join(format!("{match_id}.asked"))
}

/// An AI app fetched the match to review it.
pub fn mark_asked(data_dir: &Path, match_id: &str, app: &str) -> Result<()> {
    crate::files::write_atomic(&asked_path(data_dir, match_id), app.as_bytes())
}

/// The matches an AI app is reviewing now: (match id, app, since when).
pub fn reviewing(data_dir: &Path) -> Vec<(String, String, std::time::SystemTime)> {
    let Ok(dir) = std::fs::read_dir(data_dir.join("ai")) else {
        return vec![];
    };
    let mut out = Vec::new();
    for e in dir.flatten() {
        let path = e.path();
        let (Some(id), Some("asked")) = (
            path.file_stem().and_then(|s| s.to_str()),
            path.extension().and_then(|s| s.to_str()),
        ) else {
            continue;
        };
        let Ok(since) = e.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        if since.elapsed().is_ok_and(|t| t > REVIEWING_FOR) || saved_path(data_dir, id).is_file() {
            continue;
        }
        let app = std::fs::read_to_string(&path).unwrap_or_default();
        out.push((id.to_owned(), app.trim().to_owned(), since));
    }
    out.sort();
    out
}

/// The ids of the matches with a saved review.
pub fn saved_ids(data_dir: &Path) -> Vec<String> {
    let Ok(dir) = std::fs::read_dir(data_dir.join("ai")) else {
        return vec![];
    };
    dir.flatten()
        .filter_map(|e| {
            let path = e.path();
            (path.extension().and_then(|s| s.to_str()) == Some("json"))
                .then(|| path.file_stem()?.to_str().map(str::to_owned))
                .flatten()
        })
        .collect()
}

/// The JSON Schema of the answer, for providers that can be held to one.
pub fn json_schema() -> Value {
    let strings = json!({"type": "array", "items": {"type": "string"}});
    let ints = json!({"type": "array", "items": {"type": "integer"}});
    let cats: Vec<&str> = CATEGORIES.iter().map(|(k, _)| *k).collect();
    json!({
        "type": "object",
        "properties": {
            "schema": {"type": "string", "enum": [SCHEMA]},
            "summary": {"type": "string"},
            "findings": {"type": "array", "items": {
                "type": "object",
                "properties": {
                    "category": {"type": "string", "enum": cats},
                    "title": {"type": "string"},
                    "rounds": ints,
                    "screenshots": strings,
                    "saw": {"type": "string"},
                    "why": {"type": "string"},
                    "try": {"type": "string"},
                    "confidence": {"type": "string", "enum": ["high", "medium", "low"]}
                },
                "required": ["category", "title", "rounds", "screenshots", "saw", "why", "try", "confidence"],
                "additionalProperties": false
            }},
            "keep": {"type": "array", "items": {
                "type": "object",
                "properties": {"title": {"type": "string"}, "rounds": ints, "screenshots": strings, "saw": {"type": "string"}},
                "required": ["title", "rounds", "screenshots", "saw"],
                "additionalProperties": false
            }},
            "unclear": {"type": "array", "items": {
                "type": "object",
                "properties": {"screenshot": {"type": "string"}, "reason": {"type": "string"}},
                "required": ["screenshot", "reason"],
                "additionalProperties": false
            }}
        },
        "required": ["schema", "summary", "findings", "keep", "unclear"],
        "additionalProperties": false
    })
}

/// One screenshot sent with the review.
pub struct Shot {
    pub name: String,
    pub path: PathBuf,
}

/// What the AI is asked: the format, then the match as the review knows it.
pub fn prompt(d: &Debrief, shots: &[Shot]) -> String {
    let mut s = String::new();
    s.push_str(
        "You are reviewing one VALORANT match for the player who played it, from screenshots of their deaths. \
         Vantage has already worked out everything the replay data shows (listed below). Add only what the \
         screenshots show and the data can't: where the player stood, which angles could see them, where their \
         crosshair was before the fight, how they peeked. Write in plain, second-person English with no statistics jargon.\n\n",
    );
    s.push_str("Answer with JSON only, in exactly this format (vantage.ai-review/1):\n");
    for (k, v) in FORMAT {
        s.push_str(&format!("- {k}: {v}\n"));
    }
    s.push_str(&format!(
        "Categories: {}.\nUse only screenshot names from the list below. Put a screenshot you can't judge in \"unclear\".\n\n",
        CATEGORIES.iter().map(|(k, _)| *k).collect::<Vec<_>>().join(", ")
    ));
    s.push_str(&format!(
        "The match: {} on {}, {} {}-{}, {} kills, {} deaths.\n{}\n\n",
        d.agent,
        d.map,
        d.result(),
        d.won,
        d.lost,
        d.kills,
        d.deaths_n,
        d.summary
    ));
    if !d.fixes.is_empty() {
        s.push_str("What Vantage already found:\n");
        for f in d.fixes.iter().take(5) {
            s.push_str(&format!("- {}: {}\n", f.title, f.short));
        }
        s.push('\n');
    }
    s.push_str("Screenshots (each is 3 s before, 1 s before and the moment of the death, left to right, unless it is a single frame):\n");
    for shot in shots {
        let round: u32 = shot.name.get(1..3).and_then(|r| r.parse().ok()).unwrap_or(0);
        let death = d.deaths.iter().find(|x| x.round == round);
        s.push_str(&format!(
            "- {}: round {round}. {}\n",
            shot.name,
            death.map_or_else(String::new, |x| x.sentence())
        ));
    }
    s
}

/// Checks an answer against the format; the error says what's wrong, for the retry.
pub fn validate(text: &str, shots: &[Shot]) -> Result<Output> {
    let trimmed = text.trim();
    let body = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .map_or(trimmed, |s| s.trim_end_matches("```"))
        .trim();
    let o: Output = serde_json::from_str(body).context("the answer isn't JSON in the format")?;
    if o.schema != SCHEMA {
        bail!("schema must be {SCHEMA:?}");
    }
    if o.findings.is_empty() || o.findings.len() > 5 {
        bail!("findings must have 1 to 5 entries");
    }
    if o.keep.len() > 3 {
        bail!("keep must have at most 3 entries");
    }
    let known = |n: &String| shots.iter().any(|s| &s.name == n);
    for f in &o.findings {
        if !CATEGORIES.iter().any(|(k, _)| *k == f.category) {
            bail!("unknown category {:?}", f.category);
        }
        if !["high", "medium", "low"].contains(&f.confidence.as_str()) {
            bail!("confidence must be high, medium or low");
        }
        if f.title.chars().count() > 60 {
            bail!("titles must be at most 50 characters");
        }
        if let Some(bad) = f.screenshots.iter().find(|n| !known(n)) {
            bail!("{bad} is not one of this match's screenshots");
        }
    }
    Ok(o)
}

/// Asks `provider`, checks the answer (asking once more if it doesn't fit), and saves it.
pub fn run(
    data_dir: &Path,
    provider: &Provider,
    key: Option<&str>,
    model: Option<&str>,
    d: &Debrief,
    shots: &[Shot],
) -> Result<Saved> {
    // Only a review that reads back blocks a new one; a damaged file doesn't.
    if load(data_dir, &d.match_id).is_some() {
        bail!("This match already has an AI review. It's saved and can't be rerun.");
    }
    let key = match (provider.key, key.map(str::trim).filter(|k| !k.is_empty())) {
        (Some(what), None) => bail!("No {what} set. Add it in /settings → Config → AI review."),
        (_, k) => k,
    };
    let model = chosen_model(provider, model);
    let images: Vec<String> = shots
        .iter()
        .map(|s| {
            std::fs::read(&s.path)
                .map(|b| base64(&b))
                .with_context(|| format!("reading {}", s.path.display()))
        })
        .collect::<Result<_>>()?;
    let text = prompt(d, shots);
    let mut answer = ask(provider, key, model, &text, &images)?;
    let output = match validate(&answer, shots) {
        Ok(o) => o,
        Err(e) => {
            let again = format!(
                "{text}\n\nYour previous answer didn't fit the format ({e:#}). Answer again with JSON only, exactly in the format."
            );
            answer = ask(provider, key, model, &again, &images)?;
            validate(&answer, shots).context("the AI's answer didn't fit the format twice, so nothing was saved")?
        }
    };
    let label = if provider.local {
        format!("{} (on this PC)", provider.name)
    } else {
        format!("{} ({model})", provider.name)
    };
    save(data_dir, &d.match_id, output, label, model, None)
}

/// Checks the answer an AI app sent through MCP, and saves it. The error
/// says what to fix, and the app can send it again.
pub fn save_from_app(
    data_dir: &Path,
    match_id: &str,
    answer: &str,
    shots: &[Shot],
    app: &str,
    model: Option<&str>,
) -> Result<Saved> {
    if load(data_dir, match_id).is_some() {
        bail!("This match already has an AI review. It's saved and can't be rerun.");
    }
    let output = validate(answer, shots)?;
    let model = model.map(str::trim).filter(|m| !m.is_empty());
    let label = match model {
        Some(m) => format!("{app} ({}) through MCP", model_name(m)),
        None => format!("{app} through MCP"),
    };
    let saved = save(data_dir, match_id, output, label, model.unwrap_or(""), Some(app))?;
    let _ = std::fs::remove_file(asked_path(data_dir, match_id));
    Ok(saved)
}

fn save(
    data_dir: &Path,
    match_id: &str,
    output: Output,
    provider: String,
    model: &str,
    app: Option<&str>,
) -> Result<Saved> {
    let saved = Saved {
        output,
        meta: Meta {
            provider,
            model: model.to_owned(),
            saved_at: crate::time::now_rfc3339(),
            format: FORMAT_VERSION,
            app: app.map(str::to_owned),
        },
    };
    crate::files::write_atomic(
        &saved_path(data_dir, match_id),
        serde_json::to_string_pretty(&saved)?.as_bytes(),
    )?;
    Ok(saved)
}

fn post(url: &str, headers: &[(&str, &str)], body: &Value) -> Result<Value> {
    let mut req = ureq::post(url).header("content-type", "application/json");
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let mut resp = req
        .config()
        .timeout_global(Some(TIMEOUT))
        .http_status_as_error(false)
        .build()
        .send(body.to_string())
        .map_err(|e| anyhow!("couldn't reach {}: {e}", host(url)))?;
    let status = resp.status().as_u16();
    let text = resp.body_mut().read_to_string().context("reading the answer")?;
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if !(200..300).contains(&status) {
        let msg = v
            .pointer("/error/message")
            .or_else(|| v.pointer("/error"))
            .and_then(Value::as_str)
            .map_or_else(|| text.chars().take(200).collect(), str::to_owned);
        bail!("{} answered {status}: {msg}", host(url));
    }
    Ok(v)
}

fn get(url: &str, headers: &[(&str, &str)]) -> Result<Value> {
    let mut req = ureq::get(url);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let mut resp = req
        .config()
        .timeout_global(Some(Duration::from_secs(15)))
        .http_status_as_error(false)
        .build()
        .call()
        .map_err(|e| anyhow!("couldn't reach {}: {e}", host(url)))?;
    let status = resp.status().as_u16();
    let text = resp.body_mut().read_to_string().context("reading the answer")?;
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if !(200..300).contains(&status) {
        let msg = v
            .pointer("/error/message")
            .or_else(|| v.pointer("/error"))
            .and_then(Value::as_str)
            .map_or_else(|| text.chars().take(200).collect(), str::to_owned);
        bail!("{} answered {status}: {msg}", host(url));
    }
    Ok(v)
}

fn host(url: &str) -> &str {
    url.split('/').nth(2).unwrap_or(url)
}

fn ask(p: &Provider, key: Option<&str>, model: &str, prompt: &str, images: &[String]) -> Result<String> {
    let key = key.unwrap_or_default();
    match p.id {
        "anthropic" => {
            let mut content: Vec<Value> = images
                .iter()
                .map(|b| json!({"type": "image", "source": {"type": "base64", "media_type": "image/jpeg", "data": b}}))
                .collect();
            content.push(json!({"type": "text", "text": prompt}));
            let v = post(
                "https://api.anthropic.com/v1/messages",
                &[
                    ("x-api-key", key),
                    ("anthropic-version", "2023-06-01"),
                    ("anthropic-beta", "server-side-fallback-2026-07-01"),
                ],
                &json!({
                    "model": model,
                    "max_tokens": 16000,
                    "fallbacks": "default",
                    "output_config": {"format": {"type": "json_schema", "schema": json_schema()}},
                    "messages": [{"role": "user", "content": content}]
                }),
            )?;
            if v["stop_reason"] == "refusal" {
                bail!("Claude declined to review this match");
            }
            v["content"]
                .as_array()
                .and_then(|c| c.iter().find(|b| b["type"] == "text"))
                .and_then(|b| b["text"].as_str())
                .map(str::to_owned)
                .context("Claude's answer had no text")
        }
        "openai" => {
            let mut content: Vec<Value> = vec![json!({"type": "text", "text": prompt})];
            content.extend(
                images
                    .iter()
                    .map(|b| json!({"type": "image_url", "image_url": {"url": format!("data:image/jpeg;base64,{b}")}})),
            );
            let auth = format!("Bearer {key}");
            let v = post(
                "https://api.openai.com/v1/chat/completions",
                &[("authorization", &auth)],
                &json!({
                    "model": model,
                    "messages": [{"role": "user", "content": content}],
                    "response_format": {"type": "json_schema", "json_schema": {"name": "ai_review", "strict": true, "schema": json_schema()}}
                }),
            )?;
            v.pointer("/choices/0/message/content")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .context("ChatGPT's answer had no text")
        }
        "gemini" => {
            let mut parts: Vec<Value> = images
                .iter()
                .map(|b| json!({"inline_data": {"mime_type": "image/jpeg", "data": b}}))
                .collect();
            parts.push(json!({"text": prompt}));
            let url = format!("https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent");
            let v = post(
                &url,
                &[("x-goog-api-key", key)],
                &json!({
                    "contents": [{"role": "user", "parts": parts}],
                    "generationConfig": {"responseMimeType": "application/json"}
                }),
            )?;
            v.pointer("/candidates/0/content/parts/0/text")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .context("Gemini's answer had no text")
        }
        "ollama" => {
            let v = post(
                &format!("{OLLAMA}/api/chat"),
                &[],
                &json!({
                    "model": model,
                    "stream": false,
                    "format": json_schema(),
                    "messages": [{"role": "user", "content": prompt, "images": images}]
                }),
            )
            .map_err(|e| anyhow!("{e:#}. Is Ollama running, with `ollama pull {model}` done?"))?;
            v.pointer("/message/content")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .context("Ollama's answer had no text")
        }
        other => bail!("unknown AI provider {other}"),
    }
}

/// Standard base64 with padding.
pub fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n =
            (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_without_a_key_are_the_usual_ones() {
        for id in ["anthropic", "openai", "gemini"] {
            let p = provider(id).unwrap();
            for key in [None, Some("")] {
                let got = models(p, key).unwrap();
                assert_eq!(got, fallback_models(p), "{id}");
                assert!(got.iter().any(|m| m.id == p.model && m.about.ends_with("the default")));
            }
        }
        assert_eq!(chosen_model(provider("openai").unwrap(), Some("  ")), "gpt-5");
    }

    #[test]
    fn model_names() {
        assert_eq!(model_name("claude-sonnet-5-5"), "Sonnet 5.5");
        assert_eq!(model_name("claude-fable-5-1"), "Fable 5.1");
        assert_eq!(model_name("claude-sonnet-4-5-20250929"), "Sonnet 4.5");
        assert_eq!(model_name("gpt-5"), "gpt-5");
        let claude = provider("anthropic").unwrap();
        assert_eq!(chosen_model(claude, None), "claude-sonnet-5-5");
        assert_eq!(chosen_model(claude, Some("claude-opus-5-5")), "claude-opus-5-5");
        let offline = fallback_models(claude);
        assert_eq!(offline.len(), 4);
        assert_eq!(offline[2].about, "claude-sonnet-5-5 · the default");
    }

    #[test]
    fn encodes_base64() {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
    }

    #[test]
    fn an_ai_app_reviews_once_and_stops_counting_as_reviewing() {
        let dir = std::env::temp_dir().join(format!("vantage-ai-app-{}", std::process::id()));
        let shots = [Shot {
            name: "R07.jpg".into(),
            path: "R07.jpg".into(),
        }];
        let answer = json!({
            "schema": SCHEMA, "summary": "s",
            "findings": [{"category": "peek", "title": "t", "rounds": [7], "screenshots": ["R07.jpg"],
                "saw": "a", "why": "b", "try": "c", "confidence": "low"}],
            "keep": [], "unclear": []
        })
        .to_string();
        mark_asked(&dir, "m1", "Claude Desktop").unwrap();
        let now = reviewing(&dir);
        assert_eq!((now[0].0.as_str(), now[0].1.as_str()), ("m1", "Claude Desktop"));
        assert!(save_from_app(&dir, "m1", "{}", &shots, "Claude Desktop", None).is_err());
        assert_eq!(reviewing(&dir).len(), 1, "a wrong answer leaves it reviewing");
        let saved = save_from_app(&dir, "m1", &answer, &shots, "Claude Desktop", Some("claude-opus-5-5")).unwrap();
        assert_eq!(saved.meta.provider, "Claude Desktop (Opus 5.5) through MCP");
        assert_eq!(load(&dir, "m1").unwrap().meta.app.as_deref(), Some("Claude Desktop"));
        assert!(reviewing(&dir).is_empty());
        assert_eq!(saved_ids(&dir), ["m1"]);
        assert!(save_from_app(&dir, "m1", &answer, &shots, "Claude Desktop", None).is_err());
        assert_eq!(app(Some("claude-code")).name, "Claude Code");
        assert_eq!(app(None).name, "your AI app");
        assert_eq!(app_from_client("claude-ai"), Some("Claude Desktop"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn answers_must_fit_the_format() {
        let shots = [Shot {
            name: "R07.jpg".into(),
            path: "R07.jpg".into(),
        }];
        let ok = json!({
            "schema": SCHEMA, "summary": "s",
            "findings": [{"category": "exposure", "title": "t", "rounds": [7], "screenshots": ["R07.jpg"],
                "saw": "a", "why": "b", "try": "c", "confidence": "high"}],
            "keep": [], "unclear": []
        });
        assert!(validate(&ok.to_string(), &shots).is_ok());
        assert!(validate(&format!("```json\n{ok}\n```"), &shots).is_ok());
        let mut bad = ok.clone();
        bad["findings"][0]["screenshots"] = json!(["R99.jpg"]);
        assert!(validate(&bad.to_string(), &shots).is_err());
        let mut bad = ok;
        bad["findings"][0]["category"] = json!("vibes");
        assert!(validate(&bad.to_string(), &shots).is_err());
        assert!(validate("no", &shots).is_err());
    }
}
