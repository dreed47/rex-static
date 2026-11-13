//! Claude Messages API over raw HTTP (Rust has no official SDK).

use crate::memory::Turn;
use serde_json::{Value, json};
use std::process::Command;
use std::time::Duration;

/// Model used when the config doesn't name one.
pub const DEFAULT_MODEL: &str = "claude-haiku-5-5";
const URL: &str = "https://api.anthropic.com/v1/messages";
const MAX_RESPONSE_BYTES: u64 = 2 * 1024 * 1024;
/// Web search tool version for a model. Newer models get the
/// dynamic-filtering one; Haiku and older models only have the basic one.
fn web_search(model: &str) -> &'static str {
    const NEW: &[&str] = &[
        "claude-fable-",
        "claude-mythos-",
        "claude-opus-5",
        "claude-opus-4-8",
        "claude-opus-4-7",
        "claude-opus-4-6",
        "claude-sonnet-5",
        "claude-sonnet-4-6",
    ];
    if NEW.iter().any(|p| model.starts_with(p)) { "web_search_20260209" } else { "web_search_20250305" }
}
/// Most API round trips per turn (tool calls plus pause_turn resumes).
const MAX_ROUNDS: usize = 8;

/// API key from `ANTHROPIC_API_KEY`, else the keyring entry
/// `service=rex-static key=anthropic`.
pub fn load_key() -> Option<String> {
    if let Ok(k) = std::env::var("ANTHROPIC_API_KEY") {
        let k = k.trim();
        if !k.is_empty() {
            return Some(k.to_string());
        }
    }
    let out = Command::new("/usr/bin/secret-tool")
        .args(["lookup", "service", "rex-static", "key", "anthropic"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let k = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!k.is_empty()).then_some(k)
}

pub enum Reply {
    Text(String),
    Refused,
}

#[derive(Clone)]
pub struct Claude {
    agent: ureq::Agent,
    key: String,
    model: String,
}

/// Merge same-role neighbours and drop leading assistant turns so the
/// API gets a clean user/assistant alternation that starts with the user.
pub fn normalize(turns: &[Turn]) -> Vec<Value> {
    let mut out: Vec<(String, String)> = Vec::new();
    for t in turns {
        if out.is_empty() && t.role != "user" {
            continue;
        }
        match out.last_mut() {
            Some((role, text)) if *role == t.role => {
                text.push('\n');
                text.push_str(&t.text);
            }
            _ => out.push((t.role.clone(), t.text.clone())),
        }
    }
    out.into_iter()
        .map(|(role, text)| json!({ "role": role, "content": text }))
        .collect()
}

impl Claude {
    pub fn new(key: String, model: String) -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(90)))
            .http_status_as_error(false)
            .https_only(true)
            .build();
        Self { agent: config.into(), key, model }
    }

    /// Plain question, no tools (used for the notes rewrite).
    pub fn ask(&self, system: &str, turns: &[Turn], max_tokens: u32) -> Result<Reply, String> {
        self.converse(system, turns, max_tokens, None, &|_| {})
    }

    /// Conversation turn with tools: web search runs on Anthropic's side,
    /// client tools run here through `tools`. `on_tool` gets a short label
    /// whenever a client tool starts.
    pub fn converse(
        &self,
        system: &str,
        turns: &[Turn],
        max_tokens: u32,
        tools: Option<&crate::tools::Turn>,
        on_tool: &dyn Fn(&str),
    ) -> Result<Reply, String> {
        let mut messages = normalize(turns);
        if messages.is_empty() {
            return Err("nothing to send".into());
        }
        let mut said: Vec<String> = Vec::new();
        for _ in 0..MAX_ROUNDS {
            let mut body = json!({
                "model": self.model,
                "max_tokens": max_tokens,
                "output_config": { "effort": "low" },
                "thinking": { "type": "disabled" },
                "system": system,
                "messages": messages,
            });
            if let Some(t) = tools {
                let mut defs = vec![json!({ "type": web_search(&self.model), "name": "web_search", "max_uses": 3 })];
                defs.extend(t.defs());
                body["tools"] = Value::Array(defs);
            }
            let v = self.post(&body)?;
            if v["stop_reason"] == "refusal" {
                return Ok(Reply::Refused);
            }
            let content = v["content"].as_array().cloned().unwrap_or_default();
            said.extend(texts(&content));
            match v["stop_reason"].as_str() {
                Some("pause_turn") => {
                    messages.push(json!({ "role": "assistant", "content": content }));
                }
                Some("tool_use") if tools.is_some() => {
                    let tools = tools.unwrap();
                    let mut results = Vec::new();
                    for b in content.iter().filter(|b| b["type"] == "tool_use") {
                        let name = b["name"].as_str().unwrap_or("");
                        on_tool(crate::tools::label(name));
                        let (out, is_error) = tools.run(name, &b["input"]);
                        results.push(json!({
                            "type": "tool_result",
                            "tool_use_id": b["id"],
                            "content": out,
                            "is_error": is_error,
                        }));
                    }
                    messages.push(json!({ "role": "assistant", "content": content }));
                    messages.push(json!({ "role": "user", "content": results }));
                }
                other => {
                    let text = said.join(" ").trim().to_string();
                    if text.is_empty() {
                        return Err(format!("empty reply (stop_reason {})", other.unwrap_or("?")));
                    }
                    return Ok(Reply::Text(text));
                }
            }
        }
        Err("too many tool rounds".into())
    }

    fn post(&self, body: &Value) -> Result<Value, String> {
        let mut resp = self
            .agent
            .post(URL)
            .header("x-api-key", &self.key)
            .header("anthropic-version", "2023-06-01")
            .send_json(body)
            .map_err(|e| format!("network: {e}"))?;
        let status = resp.status();
        let v: Value = resp
            .body_mut()
            .with_config()
            .limit(MAX_RESPONSE_BYTES)
            .read_json()
            .map_err(|e| format!("bad response ({status}): {e}"))?;
        if !status.is_success() {
            let msg = v["error"]["message"].as_str().unwrap_or("unknown error");
            return Err(format!("API {}: {msg}", status.as_u16()));
        }
        Ok(v)
    }
}

/// Text blocks of one response, trimmed, empties dropped.
fn texts(content: &[Value]) -> Vec<String> {
    content
        .iter()
        .filter(|b| b["type"] == "text")
        .filter_map(|b| b["text"].as_str())
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(role: &str, text: &str) -> Turn {
        Turn { role: role.into(), text: text.into() }
    }

    #[test]
    fn normalize_alternates() {
        let v = normalize(&[
            t("assistant", "orphan"),
            t("user", "a"),
            t("user", "b"),
            t("assistant", "c"),
            t("user", "d"),
        ]);
        assert_eq!(v.len(), 3);
        assert_eq!(v[0]["content"], "a\nb");
        assert_eq!(v[1]["role"], "assistant");
        assert_eq!(v[2]["content"], "d");
    }

    #[test]
    fn texts_skip_tool_blocks() {
        let c = vec![
            json!({"type": "text", "text": " Let me check. "}),
            json!({"type": "server_tool_use", "id": "s1", "name": "web_search", "input": {}}),
            json!({"type": "web_search_tool_result", "tool_use_id": "s1", "content": []}),
            json!({"type": "tool_use", "id": "t1", "name": "ha_states", "input": {}}),
            json!({"type": "text", "text": "  "}),
        ];
        assert_eq!(texts(&c), vec!["Let me check.".to_string()]);
    }
}
