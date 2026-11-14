//! Things the persona can do besides talk: hand research to Claude Code,
//! read and control Home Assistant. Each is off unless the config sets it
//! up. Web search is a server tool and lives in llm.rs.
//!
//! Actions that reach outside the app (Claude Code, Home Assistant service
//! calls) pass a confirmation gate enforced here in code, not just in the
//! prompt: the exact call must have been proposed on the previous turn and
//! the user's latest message must be a plain yes.

use crate::persona::Persona;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const CURL: &str = "/usr/bin/curl";
const KILL: &str = "/usr/bin/kill";

const CLAUDE_TIMEOUT: Duration = Duration::from_secs(180);
const CLAUDE_OUT_MAX: usize = 64 * 1024;

const HA_TIMEOUT: Duration = Duration::from_secs(20);
const HA_OUT_MAX: usize = 8 * 1024 * 1024;

/// Longest tool result handed back to the model.
const RESULT_MAX: usize = 6000;

/// Service domains the persona may never call, even with a yes.
const HA_BLOCKED: &[&str] = &["hassio", "recorder", "system_log", "logger", "backup", "shell_command"];

/// Home Assistant connection, from the config.
#[derive(Clone, Debug)]
pub struct HaConf {
    /// `https://host[:port]`, already checked.
    pub url: String,
    /// Optional curl `--pinnedpubkey` hash. With a pin, curl checks only the
    /// server's public key (for a server whose certificate can't pass normal
    /// checks); without one, normal TLS verification applies.
    pub pin: Option<String>,
    /// File holding a long-lived access token.
    pub token_file: PathBuf,
}

fn claude_def() -> Value {
    json!({
            "name": "ask_claude_code",
            "description": "Hand a research task to Claude Code, a coding agent on the user's computer. \
It can read files under their home folder (code projects, configs, notes) and search the web, but it \
cannot change anything. Use it for questions about their files, projects or system. It can take up to \
three minutes. Only call it after the user has said yes to it: propose it first.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "task": { "type": "string", "description": "The full, self-contained task in plain English." }
                },
                "required": ["task"]
            }
    })
}

fn ha_defs() -> [Value; 2] {
    [
        json!({
            "name": "ha_states",
            "description": "Read Home Assistant: find entities and their current state (lights, \
switches, sensors, climate, locks, and so on). Read-only, no confirmation needed.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "search": { "type": "string", "description": "Words to match in the entity id or friendly name, for example \"garage door\" or \"sensor.office\". Empty lists how many entities each domain has." }
                },
                "required": ["search"]
            }
        }),
        json!({
            "name": "ha_call_service",
            "description": "Control Home Assistant by calling a service, for example light.turn_on or \
switch.toggle. Look up the exact entity id with ha_states first. Only call it after the user has \
said yes to this exact action: propose it first.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "domain": { "type": "string" },
                    "service": { "type": "string" },
                    "entity_id": { "type": "string" },
                    "data": { "type": "object", "description": "Extra service data, for example {\"brightness_pct\": 40}." }
                },
                "required": ["domain", "service", "entity_id"]
            }
        }),
    ]
}

fn switch_def() -> Value {
    let who: Vec<String> = Persona::ALL.iter().map(|p| format!("{} is {}", p.tag(), p.default_name())).collect();
    let tags: Vec<&str> = Persona::ALL.iter().map(|p| p.tag()).collect();
    json!({
        "name": "switch_persona",
        "description": format!(
            "Switch to another persona when the user asks: {}. The app saves the choice and restarts \
as that persona after your reply.",
            who.join(", ")
        ),
        "input_schema": {
            "type": "object",
            "properties": {
                "persona": { "type": "string", "enum": tags }
            },
            "required": ["persona"]
        }
    })
}

/// Short line for the window while a tool runs.
pub fn label(name: &str) -> &'static str {
    match name {
        "ask_claude_code" => "Asking Claude Code...",
        "ha_states" | "ha_call_service" => "Checking Home Assistant...",
        "switch_persona" => "Switching...",
        _ => "Checking...",
    }
}

fn gated(name: &str) -> bool {
    matches!(name, "ask_claude_code" | "ha_call_service")
}

/// A plain yes, without a no or a wait mixed in.
pub fn is_yes(text: &str) -> bool {
    let t: String = text
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '\'' { c } else { ' ' })
        .collect();
    let words: Vec<&str> = t.split_whitespace().collect();
    if words.is_empty() || words.len() > 8 {
        return false;
    }
    const NO: &[&str] = &["no", "nope", "not", "don't", "dont", "wait", "stop", "cancel", "never", "hold"];
    if words.iter().any(|w| NO.contains(w)) {
        return false;
    }
    let joined = words.join(" ");
    const YES: &[&str] = &[
        "yes", "yeah", "yep", "yup", "sure", "ok", "okay", "confirm", "confirmed", "affirmative",
        "do it", "go ahead", "go for it", "please do", "sounds good",
    ];
    YES.iter().any(|y| joined == *y || joined.starts_with(&format!("{y} ")))
}

/// Gate decision for one gated call.
pub fn allowed(prev: Option<&(String, Value)>, name: &str, input: &Value, user_text: &str) -> bool {
    prev.is_some_and(|(n, i)| n == name && i == input) && is_yes(user_text)
}

pub struct Tools {
    /// Gated call proposed during the last turn, waiting for a yes.
    pending: Mutex<Option<(String, Value)>>,
    cancel: Arc<AtomicBool>,
    claude: Option<PathBuf>,
    ha: Option<HaConf>,
    current: Persona,
    /// Persona to restart as, set by switch_persona.
    switch: Mutex<Option<Persona>>,
}

/// Per-turn view: what was pending when the turn began, and what the user said.
pub struct Turn<'a> {
    tools: &'a Tools,
    prev: Option<(String, Value)>,
    user_text: String,
}

impl Tools {
    pub fn new(cancel: Arc<AtomicBool>, claude: Option<PathBuf>, ha: Option<HaConf>, current: Persona) -> Self {
        Self { pending: Mutex::new(None), cancel, claude, ha, current, switch: Mutex::new(None) }
    }

    /// The persona switch_persona asked for, if any. Clears it.
    pub fn take_switch(&self) -> Option<Persona> {
        self.switch.lock().ok().and_then(|mut s| s.take())
    }

    /// Which optional tools are set up.
    pub fn set(&self) -> crate::persona::ToolSet {
        crate::persona::ToolSet { claude_code: self.claude.is_some(), home_assistant: self.ha.is_some() }
    }

    /// Definitions for the tools that are set up.
    pub fn defs(&self) -> Vec<Value> {
        let mut v = vec![switch_def()];
        if self.claude.is_some() {
            v.push(claude_def());
        }
        if self.ha.is_some() {
            v.extend(ha_defs());
        }
        v
    }

    /// Start a turn. Whatever was pending is only good for this one turn.
    pub fn turn(&self, user_text: &str) -> Turn<'_> {
        let prev = self.pending.lock().ok().and_then(|mut p| p.take());
        Turn { tools: self, prev, user_text: user_text.to_string() }
    }
}

impl Turn<'_> {
    /// Run one client tool call; returns (result text, is_error).
    pub fn defs(&self) -> Vec<Value> {
        self.tools.defs()
    }

    pub fn run(&self, name: &str, input: &Value) -> (String, bool) {
        let set_up = match name {
            "ask_claude_code" => self.tools.claude.is_some(),
            "ha_states" | "ha_call_service" => self.tools.ha.is_some(),
            "switch_persona" => true,
            _ => false,
        };
        if !set_up {
            return (format!("{name} is not available."), true);
        }
        if gated(name) && !allowed(self.prev.as_ref(), name, input, &self.user_text) {
            if let Ok(mut p) = self.tools.pending.lock() {
                *p = Some((name.to_string(), input.clone()));
            }
            return (
                "Not run: the user has not confirmed this yet. Tell them in one short sentence what you \
want to do and ask if that's okay. If they say yes, call this tool again with exactly the same input."
                    .into(),
                true,
            );
        }
        let r = match name {
            "ask_claude_code" => self.claude_code(input),
            "ha_states" => self.ha_states(input),
            "ha_call_service" => self.ha_call(input),
            "switch_persona" => self.switch_to(input),
            _ => Err(format!("unknown tool {name}")),
        };
        match r {
            Ok(t) => (clip(&t, RESULT_MAX), false),
            Err(e) => {
                eprintln!("rex-static: tool {name}: {e}");
                (format!("Failed: {}", clip(&e, 500)), true)
            }
        }
    }

    fn switch_to(&self, input: &Value) -> Result<String, String> {
        let Some(to) = input["persona"].as_str().and_then(Persona::from_tag) else {
            let tags: Vec<&str> = Persona::ALL.iter().map(|p| p.tag()).collect();
            return Err(format!("persona must be one of {}", tags.join(", ")));
        };
        if to == self.tools.current {
            return Ok(format!("Already {}. Nothing to switch.", to.tag()));
        }
        crate::config::save_persona(to)?;
        if let Ok(mut s) = self.tools.switch.lock() {
            *s = Some(to);
        }
        Ok(format!(
            "Saved. The app restarts as {} after your reply. Say a one-line goodbye.",
            to.default_name()
        ))
    }

    fn claude_code(&self, input: &Value) -> Result<String, String> {
        let task = input["task"].as_str().unwrap_or("").trim();
        if task.is_empty() {
            return Err("empty task".into());
        }
        let prompt = format!(
            "{task}\n\nYou are answering for a voice assistant. Reply in plain text, under 150 words, \
no markdown. Don't change any files."
        );
        let claude = self.tools.claude.as_ref().ok_or("Claude Code is not set up")?;
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| "/".into());
        let mut cmd = Command::new(claude);
        cmd.args([
            "-p",
            "--allowedTools",
            "Read Grep Glob WebSearch",
            "--disallowedTools",
            "Bash Edit Write NotebookEdit WebFetch",
        ])
        .current_dir(home)
        // Use the user's own Claude Code login, not this app's API key.
        .env_remove("ANTHROPIC_API_KEY");
        let (ok, out) = run(cmd, prompt.as_bytes(), CLAUDE_TIMEOUT, CLAUDE_OUT_MAX, &self.tools.cancel)?;
        let out = String::from_utf8_lossy(&out).trim().to_string();
        if !ok {
            return Err(format!("Claude Code failed: {out}"));
        }
        Ok(out)
    }

    fn ha_get(&self, path: &str) -> Result<Vec<u8>, String> {
        self.ha(path, None)
    }

    /// One HA API request through curl. The token goes in on stdin, never
    /// on the command line.
    fn ha(&self, path: &str, body: Option<&Value>) -> Result<Vec<u8>, String> {
        let conf = self.tools.ha.as_ref().ok_or("Home Assistant is not set up")?;
        let token = std::fs::read_to_string(&conf.token_file)
            .map_err(|_| format!("no Home Assistant token in {}", conf.token_file.display()))?;
        let token = token.trim();
        if token.is_empty() || token.contains(['\r', '\n']) {
            return Err("bad Home Assistant token".into());
        }
        let header = format!("Authorization: Bearer {token}\n");
        let mut cmd = Command::new(CURL);
        cmd.args(["-q", "-sS", "--proto", "=https"]);
        if let Some(pin) = &conf.pin {
            cmd.args(["-k", "--pinnedpubkey", pin]);
        }
        cmd.args(["--max-time", &HA_TIMEOUT.as_secs().to_string()])
            .args(["--max-filesize", &HA_OUT_MAX.to_string()])
            .args(["-H", "@-", "-w", "\n%{http_code}"]);
        if let Some(b) = body {
            cmd.args(["-H", "Content-Type: application/json", "--data-binary", &b.to_string()]);
        }
        cmd.arg(format!("{}{path}", conf.url));
        let (ok, out) = run(cmd, header.as_bytes(), HA_TIMEOUT + Duration::from_secs(5), HA_OUT_MAX, &self.tools.cancel)?;
        if !ok {
            return Err("can't reach Home Assistant (network or TLS: bad certificate, or the key no longer matches the pin)".into());
        }
        let cut = out.iter().rposition(|&b| b == b'\n').ok_or("bad curl output")?;
        let code = String::from_utf8_lossy(&out[cut + 1..]).trim().to_string();
        let body = out[..cut].to_vec();
        if code != "200" && code != "201" {
            return Err(format!("Home Assistant said HTTP {code}: {}", clip(&String::from_utf8_lossy(&body), 200)));
        }
        Ok(body)
    }

    fn ha_states(&self, input: &Value) -> Result<String, String> {
        let raw = self.ha_get("/api/states")?;
        let v: Value = serde_json::from_slice(&raw).map_err(|e| format!("bad JSON: {e}"))?;
        Ok(format_states(&v, input["search"].as_str().unwrap_or("")))
    }

    fn ha_call(&self, input: &Value) -> Result<String, String> {
        let domain = input["domain"].as_str().unwrap_or("");
        let service = input["service"].as_str().unwrap_or("");
        let entity = input["entity_id"].as_str().unwrap_or("");
        if !ident(domain) || !ident(service) {
            return Err("bad domain or service".into());
        }
        if !entity.split(',').map(str::trim).all(|e| matches!(e.split_once('.'), Some((a, b)) if ident(a) && ident(b))) {
            return Err("bad entity_id".into());
        }
        if HA_BLOCKED.contains(&domain)
            || (domain == "homeassistant" && !matches!(service, "turn_on" | "turn_off" | "toggle"))
        {
            return Err(format!("{domain}.{service} is off limits"));
        }
        let mut body = match &input["data"] {
            Value::Object(m) => Value::Object(m.clone()),
            Value::Null => json!({}),
            _ => return Err("data must be an object".into()),
        };
        body["entity_id"] = json!(entity);
        let raw = self.ha(&format!("/api/services/{domain}/{service}"), Some(&body))?;
        let changed = serde_json::from_slice::<Value>(&raw)
            .ok()
            .and_then(|v| v.as_array().map(|a| format_list(a)))
            .unwrap_or_default();
        Ok(if changed.is_empty() { "Done.".into() } else { format!("Done. Now:\n{changed}") })
    }
}

fn ident(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn clip(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... (cut)", &s[..end])
}

fn state_line(e: &Value) -> String {
    let id = e["entity_id"].as_str().unwrap_or("?");
    let name = e["attributes"]["friendly_name"].as_str().unwrap_or("");
    let state = e["state"].as_str().unwrap_or("?");
    let unit = e["attributes"]["unit_of_measurement"].as_str().unwrap_or("");
    format!("{id} ({name}) = {state}{}{unit}", if unit.is_empty() { "" } else { " " })
}

fn format_list(a: &[Value]) -> String {
    a.iter().take(10).map(state_line).collect::<Vec<_>>().join("\n")
}

pub fn format_states(v: &Value, search: &str) -> String {
    let Some(all) = v.as_array() else { return "No states.".into() };
    let terms: Vec<String> = search.to_lowercase().split_whitespace().map(String::from).collect();
    if terms.is_empty() {
        let mut counts: std::collections::BTreeMap<&str, usize> = Default::default();
        for e in all {
            if let Some((d, _)) = e["entity_id"].as_str().and_then(|i| i.split_once('.')) {
                *counts.entry(d).or_default() += 1;
            }
        }
        return counts.iter().map(|(d, n)| format!("{d}: {n}")).collect::<Vec<_>>().join(", ");
    }
    let hits: Vec<&Value> = all
        .iter()
        .filter(|e| {
            let hay = format!(
                "{} {}",
                e["entity_id"].as_str().unwrap_or(""),
                e["attributes"]["friendly_name"].as_str().unwrap_or("")
            )
            .to_lowercase()
            .replace('_', " ");
            terms.iter().all(|t| hay.contains(&t.replace('_', " ")))
        })
        .collect();
    if hits.is_empty() {
        return format!("No entities match \"{search}\".");
    }
    let mut s = hits.iter().take(40).map(|e| state_line(e)).collect::<Vec<_>>().join("\n");
    if hits.len() > 40 {
        s.push_str(&format!("\n...and {} more. Search more narrowly.", hits.len() - 40));
    }
    s
}

/// Run a child in its own process group with stdin fed and stdout capped.
/// Kills the whole group on timeout or cancel. Returns (exit ok, stdout).
fn run(
    mut cmd: Command,
    stdin: &[u8],
    timeout: Duration,
    cap: usize,
    cancel: &AtomicBool,
) -> Result<(bool, Vec<u8>), String> {
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|e| format!("spawn: {e}"))?;
    let pid = child.id();
    let mut sin = child.stdin.take().ok_or("no stdin")?;
    let data = stdin.to_vec();
    thread::spawn(move || {
        let _ = sin.write_all(&data);
    });
    let sout = child.stdout.take().ok_or("no stdout")?;
    let reader = thread::spawn(move || {
        let mut buf = Vec::new();
        let mut sout = sout;
        let _ = (&mut sout).take(cap as u64 + 1).read_to_end(&mut buf);
        // Drain the rest so an over-long child doesn't block on a full pipe.
        let _ = std::io::copy(&mut sout, &mut std::io::sink());
        buf
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) => {}
            Err(e) => return Err(format!("wait: {e}")),
        }
        if start.elapsed() > timeout || cancel.load(Ordering::Relaxed) {
            let _ = Command::new(KILL).args(["-KILL", "--", &format!("-{pid}")]).status();
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        thread::sleep(Duration::from_millis(100));
    };
    let out = reader.join().unwrap_or_default();
    let Some(status) = status else {
        return Err(if cancel.load(Ordering::Relaxed) { "cancelled".into() } else { "timed out".into() });
    };
    if out.len() > cap {
        return Err("output too large".into());
    }
    Ok((status.success(), out))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Points nowhere real: tests must never reach a live Home Assistant.
    fn test_ha() -> HaConf {
        HaConf {
            url: "https://ha.invalid".into(),
            pin: None,
            token_file: PathBuf::from("/nonexistent/rex-test-token"),
        }
    }

    #[test]
    fn only_set_up_tools_exist() {
        let c = Arc::new(AtomicBool::new(false));
        let none = Tools::new(c.clone(), None, None, Persona::Male);
        let names: Vec<_> = none.defs().iter().map(|d| d["name"].as_str().unwrap().to_string()).collect();
        assert_eq!(names, ["switch_persona"]);
        let (msg, err) = none.turn("yes").run("ha_states", &json!({"search": ""}));
        assert!(err && msg.contains("not available"));
        let (_, err) = none.turn("yes").run("ask_claude_code", &json!({"task": "x"}));
        assert!(err);
        let ha = Tools::new(c.clone(), None, Some(test_ha()), Persona::Male);
        let names: Vec<_> = ha.defs().iter().map(|d| d["name"].as_str().unwrap().to_string()).collect();
        assert_eq!(names, ["switch_persona", "ha_states", "ha_call_service"]);
        let both = Tools::new(c, Some("/nonexistent/claude".into()), Some(test_ha()), Persona::Male);
        assert_eq!(both.defs().len(), 4);
        for d in both.defs() {
            assert!(!d.to_string().contains("Sam"));
        }
    }

    #[test]
    fn switch_persona_saves_and_queues() {
        let tools = Tools::new(Arc::new(AtomicBool::new(false)), None, None, Persona::Male);
        let (msg, err) = tools.turn("hi").run("switch_persona", &json!({"persona": "male"}));
        assert!(!err && msg.contains("Already"));
        assert_eq!(tools.take_switch(), None);
        let (_, err) = tools.turn("hi").run("switch_persona", &json!({"persona": "robot"}));
        assert!(err);
        let (msg, err) = tools.turn("hi").run("switch_persona", &json!({"persona": "female"}));
        assert!(!err, "{msg}");
        assert_eq!(tools.take_switch(), Some(Persona::Female));
        assert_eq!(tools.take_switch(), None);
        let saved = std::fs::read_to_string(crate::config::path()).unwrap();
        assert!(saved.contains("persona = female"));
    }

    #[test]
    fn yes_words() {
        for y in ["Yes", "yeah.", "Sure, go ahead", "ok", "Okay!", "do it", "go ahead please", "Yep"] {
            assert!(is_yes(y), "{y}");
        }
        for n in ["no", "yes but wait", "not yet", "don't", "maybe", "", "what lights are on", "okay no", "yes stop"] {
            assert!(!is_yes(n), "{n}");
        }
    }

    #[test]
    fn gate_needs_same_call_and_yes() {
        let call = json!({"domain": "light", "service": "turn_on", "entity_id": "light.office"});
        let prev = ("ha_call_service".to_string(), call.clone());
        assert!(allowed(Some(&prev), "ha_call_service", &call, "yes"));
        assert!(!allowed(Some(&prev), "ha_call_service", &call, "no"));
        assert!(!allowed(None, "ha_call_service", &call, "yes"));
        let other = json!({"domain": "lock", "service": "unlock", "entity_id": "lock.front"});
        assert!(!allowed(Some(&prev), "ha_call_service", &other, "yes"));
        assert!(!allowed(Some(&prev), "ask_claude_code", &call, "yes"));
    }

    #[test]
    fn pending_lasts_one_turn() {
        let tools = Tools::new(Arc::new(AtomicBool::new(false)), None, Some(test_ha()), Persona::Male);
        let call = json!({"domain": "light", "service": "turn_on", "entity_id": "light.x"});
        // Turn 1: not confirmed, gets parked. Never reaches the network.
        let (msg, err) = tools.turn("turn on the light").run("ha_call_service", &call);
        assert!(err && msg.starts_with("Not run"));
        // Turn 2: something else said; pending is dropped.
        drop(tools.turn("actually, what time is it"));
        // Turn 3: a yes now has nothing to confirm, so it parks again.
        let (msg, err) = tools.turn("yes").run("ha_call_service", &call);
        assert!(err && msg.starts_with("Not run"));
    }

    #[test]
    fn service_validation_blocks_before_network() {
        let tools = Tools::new(Arc::new(AtomicBool::new(false)), None, Some(test_ha()), Persona::Male);
        let t = Turn { tools: &tools, prev: None, user_text: String::new() };
        let bad = [
            json!({"domain": "hassio", "service": "host_shutdown", "entity_id": "a.b"}),
            json!({"domain": "homeassistant", "service": "restart", "entity_id": "a.b"}),
            json!({"domain": "light/../x", "service": "turn_on", "entity_id": "light.a"}),
            json!({"domain": "light", "service": "turn_on", "entity_id": "light"}),
            json!({"domain": "light", "service": "turn_on", "entity_id": "light.a", "data": [1]}),
        ];
        for b in bad {
            let e = t.ha_call(&b).unwrap_err();
            assert!(!e.contains("token"), "{e}");
        }
    }

    #[test]
    fn states_search() {
        let v = json!([
            {"entity_id": "light.office_lamp", "state": "on", "attributes": {"friendly_name": "Office Lamp"}},
            {"entity_id": "sensor.office_temp", "state": "21.5", "attributes": {"friendly_name": "Office Temp", "unit_of_measurement": "°C"}},
            {"entity_id": "cover.garage_door", "state": "closed", "attributes": {"friendly_name": "Garage Door"}},
        ]);
        let s = format_states(&v, "office");
        assert!(s.contains("light.office_lamp (Office Lamp) = on"));
        assert!(s.contains("= 21.5 °C"));
        assert!(!s.contains("garage"));
        assert!(format_states(&v, "garage door").contains("closed"));
        assert_eq!(format_states(&v, ""), "cover: 1, light: 1, sensor: 1");
        assert!(format_states(&v, "kitchen").starts_with("No entities"));
    }

    #[test]
    fn run_kills_on_timeout_and_caps() {
        let c = AtomicBool::new(false);
        let mut cmd = Command::new("/usr/bin/sleep");
        cmd.arg("30");
        let t = Instant::now();
        assert_eq!(run(cmd, b"", Duration::from_millis(300), 100, &c).unwrap_err(), "timed out");
        assert!(t.elapsed() < Duration::from_secs(5));
        let mut cmd = Command::new("/usr/bin/cat");
        cmd.arg("-");
        let (ok, out) = run(cmd, b"hello", Duration::from_secs(5), 100, &c).unwrap();
        assert!(ok && out == b"hello");
        let mut cmd = Command::new("/usr/bin/cat");
        cmd.arg("-");
        assert!(run(cmd, &[b'x'; 500], Duration::from_secs(5), 100, &c).is_err());
    }
}
