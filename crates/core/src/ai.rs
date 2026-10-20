//! The optional AI review: the match's facts and the death screenshots go to
//! the player's own AI (Claude, ChatGPT or Gemini with their API key, or a
//! model on this PC through Ollama). The answer must fit the fixed format
//! `valreview.ai-review/1`, and it is saved once per match: it can't be
//! rerun, so every match is judged the same way, once.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::coach::Debrief;

pub const FORMAT_VERSION: u32 = 1;
pub const SCHEMA: &str = "valreview.ai-review/1";
const TIMEOUT: Duration = Duration::from_secs(300);
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
    ("save", "saving"),
];

pub fn category_label(c: &str) -> &str {
    CATEGORIES.iter().find(|(k, _)| *k == c).map_or(c, |(_, l)| l)
}

/// The format, field by field, as `/review` shows it.
pub const FORMAT: [(&str, &str); 13] = [
    ("schema", "\"valreview.ai-review/1\""),
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
         valreview has already worked out everything the replay data shows (listed below). Add only what the \
         screenshots show and the data can't: where the player stood, which angles could see them, where their \
         crosshair was before the fight, how they peeked. Write in plain, second-person English with no statistics jargon.\n\n",
    );
    s.push_str("Answer with JSON only, in exactly this format (valreview.ai-review/1):\n");
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
        s.push_str("What valreview already found:\n");
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
pub fn run(data_dir: &Path, provider: &Provider, key: Option<&str>, model: Option<&str>, d: &Debrief, shots: &[Shot]) -> Result<Saved> {
    let path = saved_path(data_dir, &d.match_id);
    if path.is_file() {
        bail!("This match already has an AI review. It's saved and can't be rerun.");
    }
    let key = match (provider.key, key.map(str::trim).filter(|k| !k.is_empty())) {
        (Some(what), None) => bail!("No {what} set. Add it in /settings → Config → AI review."),
        (_, k) => k,
    };
    let model = model.filter(|m| !m.trim().is_empty()).unwrap_or(provider.model);
    let images: Vec<String> = shots
        .iter()
        .map(|s| std::fs::read(&s.path).map(|b| base64(&b)).with_context(|| format!("reading {}", s.path.display())))
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
    let saved = Saved {
        output,
        meta: Meta {
            provider: if provider.local {
                format!("{} (on this PC)", provider.name)
            } else {
                format!("{} ({model})", provider.name)
            },
            model: model.to_owned(),
            saved_at: crate::time::now_rfc3339(),
            format: FORMAT_VERSION,
        },
    };
    std::fs::create_dir_all(path.parent().expect("has a parent"))?;
    std::fs::write(&path, serde_json::to_string_pretty(&saved)?).with_context(|| format!("writing {}", path.display()))?;
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
fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_base64() {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
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
