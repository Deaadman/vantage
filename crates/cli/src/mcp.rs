//! `vantage mcp`: an MCP server on stdin and stdout, for the AI review
//! without an API key. The player's AI app (Claude Desktop, Claude Code and
//! others) starts it, fetches a match's facts and death screenshots, and
//! saves its review back in the same fixed format a key-based review uses.
//! The app's own plan pays for it. The shell sees the review appear.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use vantage_core::ai;
use vantage_core::coach::{Debrief, plural};

use crate::{Clips, ReviewData};

/// Newest first, the protocol versions this server speaks.
const PROTOCOLS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
/// Most screenshot bytes (base64) in one answer: apps cap a tool's answer
/// at about 1 MB, so the rest are fetched with `get_screenshots`.
const IMAGE_BUDGET: usize = 700_000;

/// What the app is told about the server when it connects.
const INSTRUCTIONS: &str = "Vantage reviews the player's own VALORANT matches from their replays. To do an AI review: \
     call get_match_for_review (the newest match unless the player names one; list_matches finds it), study the \
     screenshots against the facts, then call save_ai_review with JSON in the format it describes. Each match is \
     reviewed once.";

/// Serves until the app closes stdin.
pub fn serve(data_dir: &Path) -> Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    let mut server = Server {
        data_dir: data_dir.to_path_buf(),
        app: None,
    };
    for line in stdin.lock().lines() {
        let line = line.context("reading from the AI app")?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => server.handle(&msg),
            Err(e) => Some(error(Value::Null, -32700, &format!("not JSON: {e}"))),
        };
        if let Some(reply) = reply {
            writeln!(stdout, "{reply}")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

struct Server {
    data_dir: PathBuf,
    /// The app's name, from what it called itself when it connected.
    app: Option<String>,
}

impl Server {
    /// The answer to one message; notifications get none.
    fn handle(&mut self, msg: &Value) -> Option<Value> {
        let id = msg.get("id").cloned()?;
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let result = match msg["method"].as_str().unwrap_or_default() {
            "initialize" => {
                let client = params.pointer("/clientInfo/name").and_then(Value::as_str).unwrap_or("");
                self.app = Some(self.app_name(client));
                let asked = params["protocolVersion"].as_str().unwrap_or_default();
                let version = PROTOCOLS.iter().find(|v| **v == asked).unwrap_or(&PROTOCOLS[0]);
                json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "vantage", "title": crate::APP_NAME, "version": crate::VERSION},
                    "instructions": INSTRUCTIONS,
                })
            }
            "ping" => json!({}),
            "tools/list" => json!({"tools": tools()}),
            "tools/call" => {
                let name = params["name"].as_str().unwrap_or_default();
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                match self.call(name, &args) {
                    Ok(content) => json!({"content": content}),
                    Err(e) => json!({"content": [text(&format!("{e:#}"))], "isError": true}),
                }
            }
            other => return Some(error(id, -32601, &format!("Vantage doesn't know {other}"))),
        };
        Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
    }

    /// The app as the player knows it: from its own name, else from settings.
    fn app_name(&self, client: &str) -> String {
        ai::app_from_client(client).map_or_else(
            || {
                let cfg = vantage_core::config::Config::load(&self.data_dir).unwrap_or_default();
                match ai::app(cfg.ai_app.as_deref()) {
                    a if a.id != "other" => a.name.to_owned(),
                    _ if !client.is_empty() => client.to_owned(),
                    _ => "Your AI app".to_owned(),
                }
            },
            str::to_owned,
        )
    }

    fn call(&mut self, name: &str, args: &Value) -> Result<Vec<Value>> {
        let dir = &self.data_dir;
        let arg = |k: &str| args.get(k).and_then(Value::as_str).map(str::to_owned);
        match name {
            "list_matches" => {
                let limit = args["limit"].as_u64().unwrap_or(10).clamp(1, 50) as usize;
                Ok(vec![text(&list_matches(dir, limit))])
            }
            "get_match_for_review" => {
                let data = crate::load_review(dir, arg("match_id"))?;
                let id = data.debrief.match_id.clone();
                if ai::load(dir, &id).is_some() {
                    bail!(
                        "{} already has an AI review. It's saved and can't be rerun: the player can read it in \
                         Vantage with /review, on the AI tab.",
                        match_name(&data.debrief)
                    );
                }
                let shots = data.shots();
                if shots.is_empty() {
                    bail!("{}", no_shots(&data));
                }
                let app = self.app.clone().unwrap_or_else(|| self.app_name(""));
                ai::mark_asked(dir, &id, &app)?;
                let mut content = vec![text(&format!(
                    "{}\n\nWhen you're done, call save_ai_review with match_id \"{id}\", review set to your JSON \
                     object, and model set to the model you are (for example claude-opus-5-5). If it doesn't fit \
                     the format, the error says what to fix: fix it and save again.",
                    ai::prompt(&data.debrief, &shots)
                ))];
                let (images, rest) = images(&shots)?;
                content.extend(images);
                if !rest.is_empty() {
                    content.push(text(&format!(
                        "{} didn't fit in this answer. Fetch them with get_screenshots (match_id \"{id}\"): {}.",
                        plural(rest.len(), "screenshot"),
                        rest.join(", ")
                    )));
                }
                Ok(content)
            }
            "get_screenshots" => {
                let data = crate::load_review(dir, arg("match_id"))?;
                let wanted: Vec<String> = args["names"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect();
                let shots: Vec<ai::Shot> = data
                    .shots()
                    .into_iter()
                    .filter(|s| wanted.is_empty() || wanted.contains(&s.name))
                    .collect();
                if shots.is_empty() {
                    bail!("none of those are this match's screenshots");
                }
                let (mut content, rest) = images(&shots)?;
                if !rest.is_empty() {
                    content.push(text(&format!("Fetch the rest in another call: {}.", rest.join(", "))));
                }
                Ok(content)
            }
            "save_ai_review" => {
                let id = arg("match_id").context("match_id is missing")?;
                let answer = match &args["review"] {
                    Value::String(s) => s.clone(),
                    Value::Null => bail!("review is missing"),
                    v => v.to_string(),
                };
                let data = crate::load_review(dir, Some(id))?;
                let app = self.app.clone().unwrap_or_else(|| self.app_name(""));
                let saved = ai::save_from_app(
                    dir,
                    &data.debrief.match_id,
                    &answer,
                    &data.shots(),
                    &app,
                    arg("model").as_deref(),
                )
                .context("not saved")?;
                Ok(vec![text(&format!(
                    "Saved the review of {}, with {}. The player can read it in Vantage with /review, on the AI \
                     tab. Tell them in a sentence or two what you found most.",
                    match_name(&data.debrief),
                    plural(saved.output.findings.len(), "finding")
                ))])
            }
            other => bail!("Vantage has no tool called {other}"),
        }
    }
}

fn tools() -> Value {
    let match_id = json!({"type": "string", "description": "A match id from list_matches, or its first few characters. Leave it out for the newest match."});
    json!([
        {
            "name": "list_matches",
            "title": "List VALORANT matches",
            "description": "The player's VALORANT matches in Vantage, newest first: id, when, agent, map, result, mode, and whether it has an AI review yet.",
            "inputSchema": {"type": "object", "properties": {"limit": {"type": "integer", "description": "How many (default 10, at most 50)"}}},
            "annotations": {"readOnlyHint": true}
        },
        {
            "name": "get_match_for_review",
            "title": "Get a match to review",
            "description": "One match's facts and the screenshots of the player's deaths, with the format the review must be in. Call this to start an AI review; save it with save_ai_review.",
            "inputSchema": {"type": "object", "properties": {"match_id": match_id}}
        },
        {
            "name": "get_screenshots",
            "title": "Get more screenshots",
            "description": "Screenshots of a match that didn't fit in get_match_for_review's answer.",
            "inputSchema": {"type": "object", "properties": {
                "match_id": match_id,
                "names": {"type": "array", "items": {"type": "string"}, "description": "Screenshot names, e.g. R07.jpg"}
            }, "required": ["match_id", "names"]},
            "annotations": {"readOnlyHint": true}
        },
        {
            "name": "save_ai_review",
            "title": "Save the AI review",
            "description": "Saves your review of a match in Vantage, once. It must fit the vantage.ai-review/1 format from get_match_for_review; if it doesn't, the error says what to fix.",
            "inputSchema": {"type": "object", "properties": {
                "match_id": {"type": "string", "description": "The match's id, as get_match_for_review gave it"},
                "review": ai::json_schema(),
                "model": {"type": "string", "description": "The model you are, e.g. claude-opus-5-5"}
            }, "required": ["match_id", "review"]}
        }
    ])
}

fn text(t: &str) -> Value {
    json!({"type": "text", "text": t})
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// The screenshots as images, as many as fit in one answer (always at least
/// one), and the names of the rest.
fn images(shots: &[ai::Shot]) -> Result<(Vec<Value>, Vec<String>)> {
    let mut out = Vec::new();
    let mut rest = Vec::new();
    let mut used = 0;
    for s in shots {
        if !rest.is_empty() {
            rest.push(s.name.clone());
            continue;
        }
        let bytes = std::fs::read(&s.path).with_context(|| format!("reading {}", s.path.display()))?;
        let data = ai::base64(&bytes);
        if !out.is_empty() && used + data.len() > IMAGE_BUDGET {
            rest.push(s.name.clone());
            continue;
        }
        used += data.len();
        out.push(text(&s.name));
        out.push(json!({"type": "image", "data": data, "mimeType": "image/jpeg"}));
    }
    Ok((out, rest))
}

/// `Sova on Lotus, Mon 19 Oct 21:04`.
pub fn match_name(d: &Debrief) -> String {
    match d.played_at.as_deref() {
        Some(t) => format!("{} on {}, {}", d.agent, d.map, crate::short_when(t)),
        None => format!("{} on {}", d.agent, d.map),
    }
}

/// What to ask the AI app to review this match.
pub fn ask_line(d: &Debrief) -> String {
    let short: String = d.match_id.chars().take(8).collect();
    format!(
        "Review my VALORANT match {} (match {short}) with Vantage.",
        match_name(d)
    )
}

/// Why there are no screenshots to review.
fn no_shots(data: &ReviewData) -> String {
    match &data.clips {
        Clips::NotSet(_) => "There are no screenshots of this match: the player hasn't picked their clip folder. They \
                             can pick it in Vantage with /review, on the Clips tab."
            .into(),
        Clips::Set {
            shot_error: Some(e), ..
        } => format!("There are no screenshots of this match. {e}"),
        Clips::Set { waiting: Some(w), .. } => format!(
            "There are no screenshots of this match yet. {w} Tell the player to try again in a few minutes; \
             don't review the match without them."
        ),
        Clips::Set { .. } => "There are no screenshots of this match: none of the player's clips are of its deaths. \
                              Their clip app has to save a clip of each death."
            .into(),
    }
}

fn list_matches(dir: &Path, limit: usize) -> String {
    let matches = crate::my_matches(dir);
    if matches.is_empty() {
        return "No matches yet. The player downloads a replay in the VALORANT client and Vantage reads it.".into();
    }
    let reviewed = ai::saved_ids(dir);
    matches
        .iter()
        .take(limit)
        .map(|m| {
            let (res, _) = crate::review_view::result_style(m.won, m.lost);
            format!(
                "{} · {} · {} on {} · {res} {}-{} · {} · {}",
                m.id,
                m.played_at.as_deref().map_or_else(|| "?".into(), crate::short_when),
                m.agent.as_deref().unwrap_or("?"),
                m.map.as_deref().unwrap_or("?"),
                m.won,
                m.lost,
                m.mode.as_deref().unwrap_or("mode unknown"),
                if reviewed.contains(&m.id) {
                    "AI review saved"
                } else {
                    "no AI review yet"
                }
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The command an AI app runs: this exe, then `mcp`.
fn this_exe() -> Result<PathBuf> {
    std::env::current_exe().context("couldn't find vantage.exe")
}

/// Where Claude Desktop keeps its config: the usual install, then the
/// Microsoft Store one. The ones whose folder exists.
fn claude_desktop_configs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(d) = std::env::var_os("APPDATA") {
        out.push(PathBuf::from(d).join("Claude"));
    }
    if let Some(d) = std::env::var_os("LOCALAPPDATA")
        && let Ok(packages) = std::fs::read_dir(PathBuf::from(d).join("Packages"))
    {
        out.extend(
            packages
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().starts_with("Claude_"))
                .map(|e| e.path().join("LocalCache").join("Roaming").join("Claude")),
        );
    }
    out.into_iter()
        .filter(|d| d.is_dir())
        .map(|d| d.join("claude_desktop_config.json"))
        .collect()
}

/// `config` with Vantage among its MCP servers, everything else kept.
fn with_vantage(config: &str, exe: &Path) -> Result<String> {
    let mut v: Value = if config.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(config).context("it isn't valid JSON, so Vantage left it alone")?
    };
    let Some(root) = v.as_object_mut() else {
        bail!("it isn't a JSON object, so Vantage left it alone");
    };
    let servers = root.entry("mcpServers").or_insert_with(|| json!({}));
    let Some(servers) = servers.as_object_mut() else {
        bail!("its mcpServers isn't a JSON object, so Vantage left it alone");
    };
    servers.insert(
        "vantage".into(),
        json!({"command": exe.to_string_lossy(), "args": ["mcp"]}),
    );
    Ok(serde_json::to_string_pretty(&v)? + "\n")
}

/// Adds Vantage to Claude Desktop's MCP servers.
pub fn add_to_claude_desktop() -> Result<()> {
    let configs = claude_desktop_configs();
    if configs.is_empty() {
        bail!("Claude Desktop isn't installed, or hasn't been opened yet. Open it once, then pick it again.");
    }
    let exe = this_exe()?;
    for path in configs {
        let old = std::fs::read_to_string(&path).unwrap_or_default();
        let new = with_vantage(&old, &exe).with_context(|| format!("couldn't add Vantage to {}", path.display()))?;
        vantage_core::files::write_atomic(&path, new.as_bytes())?;
    }
    Ok(())
}

/// Adds Vantage to Claude Code's MCP servers, for every project.
pub fn add_to_claude_code() -> Result<()> {
    let exe = this_exe()?;
    let exe = exe.to_string_lossy();
    // The native install is claude.exe; the npm one is claude.cmd.
    let mut last = None;
    for claude in ["claude", "claude.cmd"] {
        let _ = Command::new(claude)
            .args(["mcp", "remove", "--scope", "user", "vantage"])
            .output();
        match Command::new(claude)
            .args(["mcp", "add", "--scope", "user", "vantage", "--", &exe, "mcp"])
            .output()
        {
            Ok(o) if o.status.success() => return Ok(()),
            Ok(o) => {
                let why = String::from_utf8_lossy(&o.stderr).trim().to_owned();
                bail!("claude mcp add didn't work: {why}");
            }
            Err(e) => last = Some(e),
        }
    }
    bail!(
        "Claude Code isn't installed ({}). Install it, or run: claude mcp add --scope user vantage -- vantage mcp",
        last.map_or_else(String::new, |e| e.to_string())
    )
}

/// Puts `text` on the clipboard.
pub fn copy(text: &str) -> Result<()> {
    // clip.exe reads UTF-16 when the text starts with its byte order mark.
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
    let mut child = Command::new("clip")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .context("couldn't reach the clipboard")?;
    child
        .stdin
        .take()
        .context("couldn't reach the clipboard")?
        .write_all(&bytes)?;
    if !child.wait()?.success() {
        bail!("couldn't reach the clipboard");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(server: &mut Server, id: u64, method: &str, params: Value) -> Value {
        server
            .handle(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .unwrap()
    }

    #[test]
    fn speaks_mcp() {
        let dir = std::env::temp_dir().join(format!("vantage-mcp-{}", std::process::id()));
        let mut s = Server {
            data_dir: dir.clone(),
            app: None,
        };
        let init = call(
            &mut s,
            1,
            "initialize",
            json!({"protocolVersion": "2025-03-26", "clientInfo": {"name": "claude-ai", "version": "1"}}),
        );
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(s.app.as_deref(), Some("Claude Desktop"));
        let newer = call(&mut s, 2, "initialize", json!({"protocolVersion": "2099-01-01"}));
        assert_eq!(newer["result"]["protocolVersion"], PROTOCOLS[0]);
        assert!(
            s.handle(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
                .is_none()
        );
        let names: Vec<String> = call(&mut s, 3, "tools/list", json!({}))["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(
            names,
            [
                "list_matches",
                "get_match_for_review",
                "get_screenshots",
                "save_ai_review"
            ]
        );
        let empty = call(&mut s, 4, "tools/call", json!({"name": "list_matches"}));
        assert!(
            empty["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .starts_with("No matches yet")
        );
        let bad = call(&mut s, 5, "tools/call", json!({"name": "nope"}));
        assert_eq!(bad["result"]["isError"], true);
        assert_eq!(call(&mut s, 6, "resources/list", json!({}))["error"]["code"], -32601);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn adds_itself_to_claude_desktop_keeping_the_rest() {
        let exe = Path::new(r"C:\Users\Player\AppData\Local\vantage\bin\vantage.exe");
        let fresh: Value = serde_json::from_str(&with_vantage("", exe).unwrap()).unwrap();
        assert_eq!(fresh["mcpServers"]["vantage"]["args"], json!(["mcp"]));
        let kept: Value = serde_json::from_str(
            &with_vantage(
                r#"{"theme": "dark", "mcpServers": {"files": {"command": "npx"}, "vantage": {"command": "old"}}}"#,
                exe,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(kept["theme"], "dark");
        assert_eq!(kept["mcpServers"]["files"]["command"], "npx");
        assert_eq!(kept["mcpServers"]["vantage"]["command"], exe.to_string_lossy().as_ref());
        assert!(with_vantage("{not json", exe).is_err());
        assert!(with_vantage(r#"{"mcpServers": []}"#, exe).is_err());
    }

    #[test]
    fn screenshots_beyond_the_budget_are_named_for_later() {
        let dir = std::env::temp_dir().join(format!("vantage-mcp-shots-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let shots: Vec<ai::Shot> = (1..=3)
            .map(|r| {
                let path = dir.join(format!("R0{r}.jpg"));
                std::fs::write(&path, vec![0u8; IMAGE_BUDGET / 2]).unwrap();
                ai::Shot {
                    name: format!("R0{r}.jpg"),
                    path,
                }
            })
            .collect();
        let (content, rest) = images(&shots).unwrap();
        assert_eq!(content.len(), 2, "a name and an image");
        assert_eq!(rest, ["R02.jpg", "R03.jpg"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
