//! Things the persona can do besides talk: hand research to Claude Code,
//! read and control Home Assistant, read and draft Gmail, read and change
//! Google Calendar. Each is off unless the config sets it up. Web search is
//! a server tool and lives in llm.rs.
//!
//! Actions that reach outside the app (Claude Code, Home Assistant service
//! calls) pass a confirmation gate enforced here in code, not just in the
//! prompt: the exact call must have been proposed on the previous turn and
//! the user's latest message must be a plain yes. A yes with "always" in
//! it is remembered (see prefs.rs), so that action stops asking: for Claude
//! Code the whole tool, for Home Assistant one service such as
//! light.turn_on. Calendar changes always ask; "always" isn't remembered
//! for them.
//!
//! Once a turn has read text other people wrote (mail, calendar, web
//! search, Claude Code's findings), that text could carry instructions
//! aimed at the model. From then on in that turn every action with an
//! effect needs a fresh yes, and remembered "always" grants don't count.

use crate::google::Google;
use crate::persona::Persona;
use crate::prefs::Prefs;
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

/// How long the Home Assistant entity list in the prompt stays fresh.
const INDEX_MAX_AGE: Duration = Duration::from_secs(10 * 60);
/// Most entity-list text put in the prompt.
const INDEX_MAX: usize = 12 * 1024;
/// Domains listed in the prompt, in order: things to control first, then
/// things to read.
const INDEX_DOMAINS: &[&str] = &[
    "light", "switch", "fan", "cover", "lock", "climate", "media_player", "scene", "script",
    "input_boolean", "vacuum", "valve", "humidifier", "water_heater", "button", "siren",
    "alarm_control_panel", "binary_sensor", "sensor", "weather", "person",
];

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
three minutes. Call it straight away, without asking: the app asks the user itself for a yes.",
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
switch.toggle. Use the entity id from the entity list in your instructions, or find it with \
ha_states. Call it straight away, without asking: the app asks the user itself when a change needs a yes.",
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

fn prefs_defs() -> [Value; 3] {
    [
        json!({
            "name": "set_brief",
            "description": "Turn brief mode on or off when the user asks you to be brief, less wordy or \
shorter, or to give more detail by default. The app remembers it.",
            "input_schema": {
                "type": "object",
                "properties": { "on": { "type": "boolean" } },
                "required": ["on"]
            }
        }),
        json!({
            "name": "set_we",
            "description": "Choose how you refer to yourself. Call it whenever the user asks you to say we \
or I (speech recognition may write I as eye or aye); just agreeing doesn't change anything. On means \
we, us and our; off means I, me and my. The app remembers it for every persona and uses it in its \
built-in lines too.",
            "input_schema": {
                "type": "object",
                "properties": { "on": { "type": "boolean" } },
                "required": ["on"]
            }
        }),
        json!({
            "name": "forget_permissions",
            "description": "Forget every action the user said 'always' to, so you ask first again. Use \
it when they ask you to ask first again or to stop doing things without asking.",
            "input_schema": { "type": "object", "properties": {} }
        }),
    ]
}

fn google_defs() -> [Value; 8] {
    let time = "RFC 3339 with the UTC offset from your instructions, for example 2026-11-25T14:00:00-06:00, \
or a plain date like 2026-11-25 for an all-day event (the end date is the day after the last day)";
    [
        json!({
            "name": "morning_update",
            "description": "Gather the user's morning update at any time of day: the weather from \
their station, today's calendar and important unread mail. Call it when they ask for their morning \
update, a briefing or a rundown of their day, then speak it as the result says. Read-only.",
            "input_schema": { "type": "object", "properties": {} }
        }),
        json!({
            "name": "gmail_search",
            "description": "Search the user's Gmail. Takes Gmail search syntax, for example \
\"is:unread newer_than:1d\" or \"from:anne subject:lunch\". Returns id, date, sender, subject and a \
snippet for each message. Read-only.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "max": { "type": "integer", "description": "How many messages, 1 to 20. Default 10." }
                },
                "required": ["query"]
            }
        }),
        json!({
            "name": "gmail_read",
            "description": "Read one Gmail message in full, by the id gmail_search gave. Read-only.",
            "input_schema": {
                "type": "object",
                "properties": { "id": { "type": "string" } },
                "required": ["id"]
            }
        }),
        json!({
            "name": "gmail_draft",
            "description": "Save an email as a draft in the user's Gmail Drafts folder. It is never \
sent: the user reviews and sends it from Gmail. For a reply, give reply_to, the id of the message \
being answered, so the draft joins that thread.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "to": { "type": "string", "description": "Email address(es), comma-separated." },
                    "subject": { "type": "string" },
                    "body": { "type": "string", "description": "Plain text." },
                    "reply_to": { "type": "string" }
                },
                "required": ["to", "subject", "body"]
            }
        }),
        json!({
            "name": "calendar_events",
            "description": "List events on the user's Google Calendar between two times. Returns each \
event's event_id, start, end, title and place. Read-only.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "from": { "type": "string", "description": "RFC 3339 with offset, for example 2026-11-25T00:00:00-06:00." },
                    "to": { "type": "string", "description": "RFC 3339 with offset." }
                },
                "required": ["from", "to"]
            }
        }),
        json!({
            "name": "calendar_add",
            "description": "Add an event to the user's Google Calendar. Call it straight away, without \
asking: the app asks the user itself for a yes.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "summary": { "type": "string", "description": "The title." },
                    "start": { "type": "string", "description": time },
                    "end": { "type": "string", "description": time },
                    "location": { "type": "string" },
                    "description": { "type": "string" }
                },
                "required": ["summary", "start", "end"]
            }
        }),
        json!({
            "name": "calendar_update",
            "description": "Change or move an event on the user's Google Calendar, by the event_id \
calendar_events gave. Give only the fields to change. Call it straight away, without asking: the app \
asks the user itself for a yes.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "event_id": { "type": "string" },
                    "summary": { "type": "string" },
                    "start": { "type": "string", "description": time },
                    "end": { "type": "string", "description": time },
                    "location": { "type": "string" },
                    "description": { "type": "string" }
                },
                "required": ["event_id"]
            }
        }),
        json!({
            "name": "calendar_delete",
            "description": "Delete an event from the user's Google Calendar, by the event_id \
calendar_events gave. Call it straight away, without asking: the app asks the user itself for a yes.",
            "input_schema": {
                "type": "object",
                "properties": { "event_id": { "type": "string" } },
                "required": ["event_id"]
            }
        }),
    ]
}

/// Short line for the window while a tool runs.
pub fn label(name: &str) -> &'static str {
    match name {
        "ask_claude_code" => "Asking Claude Code...",
        "ha_states" | "ha_call_service" => "Checking Home Assistant...",
        "switch_persona" => "Switching...",
        "gmail_search" | "gmail_read" => "Checking mail...",
        "morning_update" => "Checking your day...",
        "gmail_draft" => "Writing a draft...",
        "calendar_events" | "calendar_add" | "calendar_update" | "calendar_delete" => "Checking calendar...",
        _ => "Checking...",
    }
}

/// Home Assistant domains whose changes need a yes: doors, locks and
/// alarms, plus scripts and automations, which can do any of those.
const HA_RISKY: &[&str] = &["lock", "alarm_control_panel", "cover", "valve", "script", "automation"];

/// Whether a call needs the user's yes. Home Assistant changes need one only
/// when the service or any target entity is in a risky domain (so
/// homeassistant.turn_on on a cover still asks).
fn gated(name: &str, input: &Value) -> bool {
    match name {
        "ask_claude_code" | "calendar_add" | "calendar_update" | "calendar_delete" => true,
        "ha_call_service" => {
            let domain = input["domain"].as_str().unwrap_or("");
            let entities = input["entity_id"].as_str().unwrap_or("");
            HA_RISKY.contains(&domain)
                || entities.split(',').any(|e| HA_RISKY.contains(&e.trim().split('.').next().unwrap_or("")))
        }
        _ => false,
    }
}

/// Tools that only read, or (drafts) change nothing outside the user's own
/// account. Everything else has an effect, and needs a yes once the turn
/// has read text other people wrote.
fn side_effect(name: &str) -> bool {
    !matches!(
        name,
        "ha_states" | "gmail_search" | "gmail_read" | "gmail_draft" | "calendar_events" | "morning_update"
            | "forget_permissions"
    )
}

/// Tools whose results carry text other people wrote.
fn taints(name: &str) -> bool {
    matches!(name, "gmail_search" | "gmail_read" | "calendar_events" | "morning_update" | "ask_claude_code")
}

/// A plain yes, without a no or a wait mixed in.
pub fn is_yes(text: &str) -> bool {
    let t: String = text
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '\'' { c } else { ' ' })
        .collect();
    let words: Vec<&str> = t.split_whitespace().collect();
    if words.is_empty() || words.len() > 12 {
        return false;
    }
    const NO: &[&str] = &["no", "nope", "not", "don't", "dont", "wait", "stop", "cancel", "never", "hold"];
    if words.iter().any(|w| NO.contains(w)) {
        return false;
    }
    let joined = words.join(" ");
    const YES: &[&str] = &[
        "yes", "yeah", "yep", "yup", "sure", "ok", "okay", "confirm", "confirmed", "affirmative",
        "do it", "go ahead", "go for it", "please do", "sounds good", "absolutely", "alright", "all right",
        "do that", "correct", "please",
    ];
    YES.iter().any(|y| joined == *y || joined.starts_with(&format!("{y} ")))
}

/// A yes that should be remembered: "always", "yes always", "always do
/// that". Not "always ask first", and nothing with a no in it.
pub fn is_always(text: &str) -> bool {
    let t: String = text
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '\'' { c } else { ' ' })
        .collect();
    let words: Vec<&str> = t.split_whitespace().collect();
    const NO: &[&str] = &[
        "no", "nope", "not", "don't", "dont", "wait", "stop", "cancel", "never", "hold", "ask", "first", "check",
    ];
    !words.is_empty() && words.len() <= 8 && words.contains(&"always") && !words.iter().any(|w| NO.contains(w))
}

/// What makes two gated calls the same action. Claude Code only reads, so
/// any task counts. A Home Assistant change is its service and entities,
/// in any order; a calendar change is its event. Other details may differ,
/// because the parked call is what runs.
fn call_key(name: &str, input: &Value) -> Option<String> {
    match name {
        "ask_claude_code" | "calendar_add" | "switch_persona" | "set_brief" | "set_we" => Some(name.to_string()),
        "calendar_update" | "calendar_delete" => Some(format!("{name}:{}", input["event_id"].as_str()?.trim())),
        "ha_call_service" => {
            let (d, s) = (input["domain"].as_str()?, input["service"].as_str()?);
            let mut e: Vec<String> = input["entity_id"]
                .as_str()?
                .split(',')
                .map(|x| x.trim().to_lowercase())
                .filter(|x| !x.is_empty())
                .collect();
            e.sort();
            e.dedup();
            Some(format!("{}.{}:{}", d.trim().to_lowercase(), s.trim().to_lowercase(), e.join(",")))
        }
        _ => None,
    }
}

/// Gate decision for one gated call: the input to run, if the user just
/// said yes to a matching call parked last turn. For Home Assistant that
/// is the parked input, the one the user heard about, not the model's
/// retyped copy. Claude Code takes the new task, since it only reads.
pub fn confirmed(prev: &[(String, Value)], name: &str, input: &Value, user_text: &str) -> Option<Value> {
    if !(is_yes(user_text) || is_always(user_text)) {
        return None;
    }
    let key = call_key(name, input)?;
    let (_, parked) = prev.iter().find(|(n, i)| n == name && call_key(n, i).as_ref() == Some(&key))?;
    Some(if name == "ask_claude_code" { input.clone() } else { parked.clone() })
}

/// What a "yes always" to this call remembers, if anything.
fn grant_key(name: &str, input: &Value) -> Option<String> {
    match name {
        "ask_claude_code" => Some(name.to_string()),
        "ha_call_service" => {
            let (d, s) = (input["domain"].as_str()?, input["service"].as_str()?);
            (ident(d) && ident(s)).then(|| format!("ha:{d}.{s}"))
        }
        _ => None,
    }
}

/// How a remembered key reads in the prompt.
pub fn grant_label(key: &str) -> String {
    match key.strip_prefix("ha:") {
        Some(svc) => format!("{svc} in Home Assistant"),
        None if key == "ask_claude_code" => "asking Claude Code".into(),
        None => key.to_string(),
    }
}

pub struct Tools {
    /// Gated calls proposed during the last turn, waiting for a yes.
    pending: Mutex<Vec<(String, Value)>>,
    cancel: Arc<AtomicBool>,
    claude: Option<PathBuf>,
    ha: Option<HaConf>,
    current: Persona,
    /// Persona to restart as, set by switch_persona.
    switch: Mutex<Option<Persona>>,
    prefs: Mutex<Prefs>,
    /// Brief mode from the config, until the user picks one by voice.
    brief_default: bool,
    /// Home Assistant entity list for the prompt, refreshed in the background.
    index: Arc<Mutex<Index>>,
    /// Whether the config turns Google on.
    google: bool,
    /// The Google sign-in, loaded from the keyring on first use.
    google_conn: Mutex<Option<Arc<Google>>>,
}

/// Cached Home Assistant entity list.
#[derive(Default)]
struct Index {
    text: String,
    fetched: Option<Instant>,
    busy: bool,
}

/// Per-turn view: what was pending when the turn began, and what the user said.
pub struct Turn<'a> {
    tools: &'a Tools,
    prev: Vec<(String, Value)>,
    user_text: String,
    /// Set once this turn has read text other people wrote.
    tainted: AtomicBool,
}

impl Tools {
    pub fn new(
        cancel: Arc<AtomicBool>,
        claude: Option<PathBuf>,
        ha: Option<HaConf>,
        current: Persona,
        brief_default: bool,
    ) -> Self {
        let prefs = Mutex::new(Prefs::load(&crate::prefs::path()));
        Self {
            pending: Mutex::new(Vec::new()),
            cancel,
            claude,
            ha,
            current,
            switch: Mutex::new(None),
            prefs,
            brief_default,
            index: Arc::new(Mutex::new(Index::default())),
            google: false,
            google_conn: Mutex::new(None),
        }
    }

    /// Turns the Gmail and Calendar tools on.
    pub fn with_google(mut self, on: bool) -> Self {
        self.google = on;
        self
    }

    /// The Google sign-in. Loading is retried on each call until it works,
    /// so a keyring that was locked at startup doesn't need a restart.
    fn google(&self) -> Result<Arc<Google>, String> {
        let mut g = self.google_conn.lock().map_err(|_| "google lock")?;
        if let Some(c) = g.as_ref() {
            return Ok(c.clone());
        }
        let c = Arc::new(Google::load()?);
        *g = Some(c.clone());
        Ok(c)
    }

    /// Use a prefs file of the test's own, so parallel tests don't share one.
    #[cfg(test)]
    fn with_prefs(mut self, name: &str) -> Self {
        let p = crate::prefs::test_path(name);
        let _ = std::fs::remove_file(&p);
        self.prefs = Mutex::new(Prefs::load(&p));
        self
    }

    /// Whether replies should be brief.
    pub fn brief(&self) -> bool {
        self.prefs.lock().ok().and_then(|p| p.brief()).unwrap_or(self.brief_default)
    }

    /// Whether the persona says "we" instead of "I".
    pub fn we(&self) -> bool {
        self.prefs.lock().is_ok_and(|p| p.we())
    }

    /// The app's own question about the calls held this turn, if any:
    /// "Shall I delete "Dentist", Thursday, November 26, 2 PM to 3 PM, from your calendar?"
    pub fn question(&self) -> Option<String> {
        let held = self.pending.lock().ok()?.clone();
        if held.is_empty() {
            return None;
        }
        let parts: Vec<String> = held.iter().map(|(n, i)| self.describe(n, i)).collect();
        Some(format!("Shall I {}?", join_and(&parts)))
    }

    /// Deal with the user's reply to the app's question. A yes runs the
    /// held calls exactly as parked, with no model round and no lookup.
    pub fn answer(&self, user_text: &str, on_tool: &dyn Fn(&str)) -> Answer {
        let held = self.pending.lock().map(|mut p| std::mem::take(&mut *p)).unwrap_or_default();
        if held.is_empty() {
            return Answer::Nothing;
        }
        if is_no(user_text) {
            return Answer::No;
        }
        if !(is_yes(user_text) || is_always(user_text)) {
            return Answer::Other;
        }
        let t = Turn { tools: self, prev: held.clone(), user_text: user_text.to_string(), tainted: AtomicBool::new(false) };
        // Claude Code last: its findings taint the turn.
        let mut order: Vec<&(String, Value)> = held.iter().filter(|(n, _)| n != "ask_claude_code").collect();
        order.extend(held.iter().filter(|(n, _)| n == "ask_claude_code"));
        let mut ran = Vec::new();
        for (name, input) in order {
            on_tool(label(name));
            let (out, err) = t.run(name, input);
            ran.push(Ran { name: name.clone(), input: input.clone(), out, err });
        }
        // Nothing run here may stay parked.
        if let Ok(mut p) = self.pending.lock() {
            p.clear();
        }
        Answer::Ran(ran)
    }

    /// One held call in plain words. Calendar edits look the event up so
    /// the user hears its title and time, not an id.
    fn describe(&self, name: &str, input: &Value) -> String {
        let s = |k: &str| input[k].as_str().map(str::trim).filter(|v| !v.is_empty());
        let quoted = |t: &str| format!("\"{}\"", short(t, 80));
        match name {
            "calendar_add" => {
                let when = s("start").zip(s("end")).and_then(|(a, b)| spoken_span(a, b));
                let mut d = format!("add {}", quoted(s("summary").unwrap_or("an event")));
                if let Some(w) = when {
                    d.push_str(&format!(", {w},"));
                }
                d.push_str(" to your calendar");
                d
            }
            "calendar_update" | "calendar_delete" => {
                let ev = if self.google { self.google().and_then(|g| g.calendar_get(input)).ok() } else { None };
                let title = ev.as_ref().and_then(|e| e["summary"].as_str()).map(quoted).unwrap_or_else(|| "the event".into());
                let old = ev.as_ref().and_then(event_span).map(|w| format!(", {w},")).unwrap_or_default();
                if name == "calendar_delete" {
                    return format!("delete {title}{old} from your calendar");
                }
                let mut changes = Vec::new();
                if s("start").is_some() || s("end").is_some() {
                    let (os, oe) = ev.as_ref().map(|e| (event_time(e, "start"), event_time(e, "end"))).unwrap_or_default();
                    let ns = s("start").map(str::to_string).or(os);
                    let ne = s("end").map(str::to_string).or(oe);
                    match ns.zip(ne).and_then(|(a, b)| spoken_span(&a, &b)) {
                        Some(w) => changes.push(format!("move it to {w}")),
                        None => changes.push("change its time".into()),
                    }
                }
                if let Some(t) = s("summary") {
                    changes.push(format!("rename it {}", quoted(t)));
                }
                if s("location").is_some() {
                    changes.push("change its place".into());
                }
                if s("description").is_some() {
                    changes.push("change its notes".into());
                }
                if changes.is_empty() {
                    changes.push("change it".into());
                }
                format!("change {title}{old} on your calendar: {}", join_and(&changes))
            }
            "ask_claude_code" => format!("ask Claude Code: {}", quoted(s("task").unwrap_or("that task"))),
            "ha_call_service" => {
                let (d, sv) = (s("domain").unwrap_or(""), s("service").unwrap_or(""));
                let e: Vec<String> =
                    s("entity_id").unwrap_or("").split(',').map(str::trim).filter(|x| !x.is_empty()).map(String::from).collect();
                let ok = ident(d)
                    && ident(sv)
                    && !e.is_empty()
                    && e.iter().all(|x| x.split_once('.').is_some_and(|(a, b)| ident(a) && ident(b)));
                if ok { format!("{} {}", sv.replace('_', " "), join_and(&e)) } else { "do that in Home Assistant".into() }
            }
            "switch_persona" => match s("persona") {
                Some(p) if ident(&p.replace('-', "_")) => format!("switch to {p}"),
                _ => "switch persona".into(),
            },
            "set_brief" => format!("turn brief mode {}", if input["on"] == true { "on" } else { "off" }),
            "set_we" => format!("say {} from now on", if input["on"] == true { "we" } else { "I" }),
            _ => "do that".into(),
        }
    }

    /// Actions the user said "always" to, as prompt text.
    pub fn always_labels(&self) -> Vec<String> {
        let Ok(p) = self.prefs.lock() else { return Vec::new() };
        p.always()
            .filter(|k| match *k {
                "ask_claude_code" => self.claude.is_some(),
                k => !k.starts_with("ha:") || self.ha.is_some(),
            })
            .map(grant_label)
            .collect()
    }

    /// The persona switch_persona asked for, if any. Clears it.
    pub fn take_switch(&self) -> Option<Persona> {
        self.switch.lock().ok().and_then(|mut s| s.take())
    }

    /// Which optional tools are set up.
    pub fn set(&self) -> crate::persona::ToolSet {
        crate::persona::ToolSet {
            claude_code: self.claude.is_some(),
            home_assistant: self.ha.is_some(),
            google: self.google,
        }
    }

    /// Definitions for the tools that are set up.
    pub fn defs(&self) -> Vec<Value> {
        let [brief, we, forget] = prefs_defs();
        let mut v = vec![switch_def(), brief, we];
        if self.claude.is_some() || self.ha.is_some() {
            v.push(forget);
        }
        if self.claude.is_some() {
            v.push(claude_def());
        }
        if self.ha.is_some() {
            v.extend(ha_defs());
        }
        if self.google {
            v.extend(google_defs());
        }
        v
    }

    /// Start a turn. Whatever was pending is only good for this one turn.
    pub fn turn(&self, user_text: &str) -> Turn<'_> {
        let prev = self.pending.lock().map(|mut p| std::mem::take(&mut *p)).unwrap_or_default();
        Turn { tools: self, prev, user_text: user_text.to_string(), tainted: AtomicBool::new(false) }
    }

    /// Home Assistant entities as prompt text (empty until the first fetch
    /// lands). Starts a background refresh when the list is missing or old,
    /// so a slow or down Home Assistant never holds up a reply.
    pub fn ha_index(&self) -> String {
        let Some(conf) = self.ha.clone() else { return String::new() };
        let Ok(mut ix) = self.index.lock() else { return String::new() };
        let stale = ix.fetched.is_none_or(|t| t.elapsed() > INDEX_MAX_AGE);
        if stale && !ix.busy {
            ix.busy = true;
            let index = self.index.clone();
            thread::spawn(move || {
                let r = ha_request(&conf, "/api/states", None, &AtomicBool::new(false));
                let text = r.ok().and_then(|raw| serde_json::from_slice::<Value>(&raw).ok()).map(|v| format_index(&v));
                if let Ok(mut ix) = index.lock() {
                    ix.busy = false;
                    match text {
                        Some(t) => {
                            ix.text = t;
                            ix.fetched = Some(Instant::now());
                        }
                        // Try again next turn, but keep the old list meanwhile.
                        None => eprintln!("rex-static: can't fetch the Home Assistant entity list"),
                    }
                }
            });
        }
        ix.text.clone()
    }
}

impl Turn<'_> {
    /// Run one client tool call; returns (result text, is_error).
    pub fn defs(&self) -> Vec<Value> {
        self.tools.defs()
    }

    /// Marks the turn as having read text other people wrote (llm.rs calls
    /// this after a web search).
    pub fn taint(&self) {
        self.tainted.store(true, Ordering::Relaxed);
    }

    /// The app's question about calls held so far, if any.
    pub fn question(&self) -> Option<String> {
        self.tools.question()
    }

    pub fn run(&self, name: &str, input: &Value) -> (String, bool) {
        let set_up = match name {
            "ask_claude_code" => self.tools.claude.is_some(),
            "gmail_search" | "gmail_read" | "gmail_draft" | "calendar_events" | "calendar_add"
            | "calendar_update" | "calendar_delete" | "morning_update" => self.tools.google,
            "ha_states" | "ha_call_service" => self.tools.ha.is_some(),
            "switch_persona" | "set_brief" | "set_we" => true,
            "forget_permissions" => self.tools.claude.is_some() || self.tools.ha.is_some(),
            _ => false,
        };
        if !set_up {
            return (format!("{name} is not available."), true);
        }
        let mut input = input;
        let yes;
        let tainted = self.tainted.load(Ordering::Relaxed);
        let own_gate = gated(name, input);
        if own_gate || (tainted && side_effect(name)) {
            let key = grant_key(name, input);
            let granted =
                !tainted && key.as_ref().is_some_and(|k| self.tools.prefs.lock().is_ok_and(|p| p.has(k)));
            if !granted {
                match confirmed(&self.prev, name, input, &self.user_text) {
                    Some(v) => {
                        yes = v;
                        input = &yes;
                    }
                    None => {
                        if let Ok(mut p) = self.tools.pending.lock() {
                            p.push((name.to_string(), input.clone()));
                        }
                        return (
                            "Not run yet: the app is holding this call and asks the user for a yes itself. \
Don't ask them, and don't call it again."
                                .into(),
                            true,
                        );
                    }
                }
            }
            let key = grant_key(name, input);
            if own_gate && !granted && is_always(&self.user_text) {
                if let (Some(k), Ok(mut p)) = (key, self.tools.prefs.lock()) {
                    if let Err(e) = p.grant(&k) {
                        eprintln!("rex-static: prefs: {e}");
                    }
                }
            }
        }
        let r = match name {
            "ask_claude_code" => self.claude_code(input),
            "ha_states" => self.ha_states(input),
            "ha_call_service" => self.ha_call(input),
            "switch_persona" => self.switch_to(input),
            "set_brief" => self.set_brief(input),
            "set_we" => self.set_we(input),
            "forget_permissions" => self.forget(),
            "gmail_search" => self.tools.google().and_then(|g| {
                let q = input["query"].as_str().unwrap_or("").trim();
                if q.is_empty() || q.len() > 500 {
                    return Err("query must be 1 to 500 characters".into());
                }
                g.gmail_search(q, input["max"].as_u64().unwrap_or(10))
            }),
            "gmail_read" => self.tools.google().and_then(|g| g.gmail_read(input["id"].as_str().unwrap_or("").trim())),
            "gmail_draft" => self.tools.google().and_then(|g| {
                g.gmail_draft(
                    input["to"].as_str().unwrap_or(""),
                    input["subject"].as_str().unwrap_or(""),
                    input["body"].as_str().unwrap_or(""),
                    input["reply_to"].as_str().map(str::trim).filter(|r| !r.is_empty()),
                )
            }),
            "calendar_events" => self.tools.google().and_then(|g| {
                let (from, to) = (input["from"].as_str().unwrap_or("").trim(), input["to"].as_str().unwrap_or("").trim());
                if !rfc3339ish(from) || !rfc3339ish(to) {
                    return Err("from and to must be RFC 3339 times with an offset".into());
                }
                g.calendar_events(from, to)
            }),
            "calendar_add" => self.tools.google().and_then(|g| g.calendar_add(input)),
            "calendar_update" => self.tools.google().and_then(|g| g.calendar_update(input)),
            "calendar_delete" => self.tools.google().and_then(|g| g.calendar_delete(input)),
            "morning_update" => crate::schedule::brief_now(),
            _ => Err(format!("unknown tool {name}")),
        };
        if taints(name) && r.is_ok() {
            self.taint();
        }
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

    fn set_brief(&self, input: &Value) -> Result<String, String> {
        let on = input["on"].as_bool().ok_or("on must be true or false")?;
        self.tools.prefs.lock().map_err(|_| "prefs lock")?.set_brief(on)?;
        Ok(if on { "Brief mode on. Keep replies to one short sentence." } else { "Brief mode off." }.into())
    }

    fn set_we(&self, input: &Value) -> Result<String, String> {
        let on = input["on"].as_bool().ok_or("on must be true or false")?;
        self.tools.prefs.lock().map_err(|_| "prefs lock")?.set_we(on)?;
        Ok(if on {
            "Saved. From now on, refer to yourself as we, us and our, never I, me or my, starting with this reply."
        } else {
            "Saved. From now on, refer to yourself as I, me and my, starting with this reply."
        }
        .into())
    }

    fn forget(&self) -> Result<String, String> {
        self.tools.prefs.lock().map_err(|_| "prefs lock")?.forget_all()?;
        Ok("Forgotten. You'll ask first again before Claude Code or Home Assistant changes.".into())
    }

    fn claude_code(&self, input: &Value) -> Result<String, String> {
        let task = input["task"].as_str().unwrap_or("").trim();
        if task.is_empty() {
            return Err("empty task".into());
        }
        let words = if self.tools.brief() { 50 } else { 150 };
        let prompt = format!(
            "{task}\n\nYou are answering for a voice assistant. Reply in plain text, under {words} words, \
no markdown. Don't change any files. You have no shell, and nobody can answer questions or grant \
permissions during this run, so never ask for any: use the tools you have (read, search files, web \
search), then answer with what you found. If you can't find it, say so in one sentence."
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

    fn ha(&self, path: &str, body: Option<&Value>) -> Result<Vec<u8>, String> {
        let conf = self.tools.ha.as_ref().ok_or("Home Assistant is not set up")?;
        ha_request(conf, path, body, &self.tools.cancel)
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

/// One HA API request through curl. The token goes in on stdin, never on
/// the command line.
fn ha_request(conf: &HaConf, path: &str, body: Option<&Value>, cancel: &AtomicBool) -> Result<Vec<u8>, String> {
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
    let (ok, out) = run(cmd, header.as_bytes(), HA_TIMEOUT + Duration::from_secs(5), HA_OUT_MAX, cancel)?;
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

/// Loose check for an RFC 3339 time: digits, a T, and only time characters.
fn rfc3339ish(s: &str) -> bool {
    (16..=40).contains(&s.len())
        && s.contains('T')
        && s.chars().all(|c| c.is_ascii_digit() || matches!(c, 'T' | 'Z' | '-' | '+' | ':' | '.'))
}

/// What came of the user's reply to the app's question.
pub enum Answer {
    /// Nothing was held.
    Nothing,
    /// A plain no: the held calls are dropped.
    No,
    /// Something else: the held calls are dropped and the model takes the turn.
    Other,
    /// A yes: the held calls ran.
    Ran(Vec<Ran>),
}

/// One held call run after a yes.
pub struct Ran {
    pub name: String,
    pub input: Value,
    pub out: String,
    pub err: bool,
}

impl Answer {
    /// The app's own reply, unless the model has results to read out
    /// (Claude Code's findings).
    pub fn reply(ran: &[Ran]) -> Option<String> {
        if ran.iter().any(|r| r.name == "ask_claude_code" && !r.err) {
            return None;
        }
        let failed: Vec<&Ran> = ran.iter().filter(|r| r.err).collect();
        Some(match failed.len() {
            0 => "Done.".into(),
            n => {
                let why = failed[0].out.strip_prefix("Failed: ").unwrap_or(&failed[0].out);
                let rest = if n < ran.len() { "The rest is done, but " } else { "Sorry, " };
                format!("{rest}that didn't work: {}", short(why, 160))
            }
        })
    }

    /// The runs as an assistant tool_use turn plus its tool_result turn,
    /// for the model to read out.
    pub fn messages(ran: &[Ran]) -> Vec<Value> {
        let id = |i: usize| format!("toolu_app_{i}");
        let uses: Vec<Value> = ran
            .iter()
            .enumerate()
            .map(|(i, r)| json!({"type": "tool_use", "id": id(i), "name": r.name, "input": r.input}))
            .collect();
        let results: Vec<Value> = ran
            .iter()
            .enumerate()
            .map(|(i, r)| json!({"type": "tool_result", "tool_use_id": id(i), "content": r.out, "is_error": r.err}))
            .collect();
        vec![json!({"role": "assistant", "content": uses}), json!({"role": "user", "content": results})]
    }
}

/// A plain no to the app's question, and nothing more.
pub fn is_no(text: &str) -> bool {
    let t: String = text
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '\'' { c } else { ' ' })
        .collect();
    let joined = t.split_whitespace().collect::<Vec<_>>().join(" ");
    const NO: &[&str] = &[
        "no", "nope", "nah", "no thanks", "no thank you", "cancel", "cancel it", "cancel that", "don't", "dont",
        "no don't", "don't do it", "don't do that", "never mind", "nevermind", "stop", "forget it", "no no",
    ];
    NO.contains(&joined.as_str())
}

fn join_and(parts: &[String]) -> String {
    match parts {
        [] => String::new(),
        [a] => a.clone(),
        [a @ .., b] => format!("{} and {b}", a.join(", ")),
    }
}

/// One line, at most `max` characters.
fn short(s: &str, max: usize) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() <= max { s } else { format!("{}...", s.chars().take(max).collect::<String>()) }
}

fn event_time(e: &Value, k: &str) -> Option<String> {
    e[k]["dateTime"].as_str().or(e[k]["date"].as_str()).map(str::to_string)
}

fn event_span(e: &Value) -> Option<String> {
    spoken_span(&event_time(e, "start")?, &event_time(e, "end")?)
}

/// "2026-11-26" or "2026-11-26T14:30:00-06:00" as (days since 1970, hour
/// and minute if any). Wall time as written; the offset is ignored.
pub(crate) fn parse_when(s: &str) -> Option<(i64, Option<(u32, u32)>)> {
    let b = s.as_bytes();
    if b.len() < 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<u32>().ok();
    let (y, m, d) = (n(0..4)?, n(5..7)?, n(8..10)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let days = days_from_civil(y as i64, m, d);
    if b.len() == 10 {
        return Some((days, None));
    }
    if b[10] != b'T' || b.len() < 16 || b[13] != b':' {
        return None;
    }
    let (h, mi) = (n(11..13)?, n(14..16)?);
    (h < 24 && mi < 60).then_some((days, Some((h, mi))))
}

pub(crate) fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

pub(crate) fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { yoe + era * 400 + 1 } else { yoe + era * 400 }, m, d)
}

/// "Thursday, November 26".
pub(crate) fn spoken_day(days: i64) -> String {
    const WD: [&str; 7] = ["Thursday", "Friday", "Saturday", "Sunday", "Monday", "Tuesday", "Wednesday"];
    const MO: [&str; 12] = [
        "January", "February", "March", "April", "May", "June", "July", "August", "September", "October",
        "November", "December",
    ];
    let (_, m, d) = civil_from_days(days);
    format!("{}, {} {d}", WD[days.rem_euclid(7) as usize], MO[m as usize - 1])
}

/// "2 PM", "2:30 PM", "12 AM".
pub(crate) fn spoken_clock((h, m): (u32, u32)) -> String {
    let h12 = if h % 12 == 0 { 12 } else { h % 12 };
    let ap = if h < 12 { "AM" } else { "PM" };
    if m == 0 { format!("{h12} {ap}") } else { format!("{h12}:{m:02} {ap}") }
}

/// An event's start and end in spoken words. All-day ends are exclusive,
/// as Google stores them.
fn spoken_span(start: &str, end: &str) -> Option<String> {
    let (sd, st) = parse_when(start)?;
    let (ed, et) = parse_when(end)?;
    Some(match (st, et) {
        (None, None) if ed <= sd + 1 => format!("{}, all day", spoken_day(sd)),
        (None, None) => format!("{} to {}, all day", spoken_day(sd), spoken_day(ed - 1)),
        (Some(a), Some(b)) if sd == ed => format!("{}, {} to {}", spoken_day(sd), spoken_clock(a), spoken_clock(b)),
        (Some(a), Some(b)) => {
            format!("{} at {} to {} at {}", spoken_day(sd), spoken_clock(a), spoken_day(ed), spoken_clock(b))
        }
        _ => return None,
    })
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

/// Entity ids and names for the prompt, so the persona can call a service
/// without searching first. No states: those go stale, so reads still use
/// ha_states. Stops at INDEX_MAX, dropping the read-only domains first.
pub fn format_index(v: &Value) -> String {
    let Some(all) = v.as_array() else { return String::new() };
    let mut out = String::new();
    for d in INDEX_DOMAINS {
        let mut lines: Vec<String> = all
            .iter()
            .filter_map(|e| {
                let id = e["entity_id"].as_str()?;
                if id.split_once('.')?.0 != *d || e["state"] == "unavailable" {
                    return None;
                }
                let name: String = e["attributes"]["friendly_name"].as_str().unwrap_or("").chars().take(60).collect();
                Some(if name.is_empty() { id.to_string() } else { format!("{id} ({name})") })
            })
            .collect();
        lines.sort();
        for l in lines {
            if out.len() + l.len() + 2 > INDEX_MAX {
                return out;
            }
            if !out.is_empty() {
                out.push_str("; ");
            }
            out.push_str(&l);
        }
    }
    out
}

/// Whether a search word is in the text, forgiving plurals ("lights" finds
/// "Light", "switches" finds "Switch").
fn has_word(hay: &str, term: &str) -> bool {
    hay.contains(term)
        || (term.len() > 3 && term.ends_with('s') && hay.contains(&term[..term.len() - 1]))
        || (term.len() > 4 && term.ends_with("es") && hay.contains(&term[..term.len() - 2]))
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
            terms.iter().all(|t| has_word(&hay, &t.replace('_', " ")))
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
    fn morning_update_needs_google_and_only_reads() {
        let c = Arc::new(AtomicBool::new(false));
        let off = Tools::new(c.clone(), None, None, Persona::Male, false);
        let (msg, err) = off.turn("morning update").run("morning_update", &json!({}));
        assert!(err && msg.contains("not available"));
        let on = Tools::new(c, None, None, Persona::Male, false).with_google(true);
        assert!(on.defs().iter().any(|d| d["name"] == "morning_update"));
        assert!(taints("morning_update") && !side_effect("morning_update"));
        assert!(!gated("morning_update", &json!({})));
    }

    #[test]
    fn only_set_up_tools_exist() {
        let c = Arc::new(AtomicBool::new(false));
        let none = Tools::new(c.clone(), None, None, Persona::Male, false);
        let names: Vec<_> = none.defs().iter().map(|d| d["name"].as_str().unwrap().to_string()).collect();
        assert_eq!(names, ["switch_persona", "set_brief", "set_we"]);
        let (msg, err) = none.turn("yes").run("ha_states", &json!({"search": ""}));
        assert!(err && msg.contains("not available"));
        let (_, err) = none.turn("yes").run("ask_claude_code", &json!({"task": "x"}));
        assert!(err);
        let ha = Tools::new(c.clone(), None, Some(test_ha()), Persona::Male, false);
        let names: Vec<_> = ha.defs().iter().map(|d| d["name"].as_str().unwrap().to_string()).collect();
        assert_eq!(names, ["switch_persona", "set_brief", "set_we", "forget_permissions", "ha_states", "ha_call_service"]);
        let both = Tools::new(c, Some("/nonexistent/claude".into()), Some(test_ha()), Persona::Male, false);
        assert_eq!(both.defs().len(), 7);
        for d in both.defs() {
            assert!(!d.to_string().contains("Sam"));
        }
    }

    #[test]
    fn switch_persona_saves_and_queues() {
        let tools = Tools::new(Arc::new(AtomicBool::new(false)), None, None, Persona::Male, false);
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
        for y in ["absolutely", "yes please turn on the kitchen lights for me now", "please", "do that"] {
            assert!(is_yes(y), "{y}");
        }
        for n in ["no", "yes but wait", "not yet", "don't", "maybe", "", "what lights are on", "okay no", "yes stop"] {
            assert!(!is_yes(n), "{n}");
        }
    }

    #[test]
    fn gate_needs_same_call_and_yes() {
        let call = json!({"domain": "light", "service": "turn_on", "entity_id": "light.office"});
        let prev = vec![("ha_call_service".to_string(), call.clone())];
        assert_eq!(confirmed(&prev, "ha_call_service", &call, "yes"), Some(call.clone()));
        assert!(confirmed(&prev, "ha_call_service", &call, "no").is_none());
        assert!(confirmed(&[], "ha_call_service", &call, "yes").is_none());
        let other = json!({"domain": "lock", "service": "unlock", "entity_id": "lock.front"});
        assert!(confirmed(&prev, "ha_call_service", &other, "yes").is_none());
        let other_light = json!({"domain": "light", "service": "turn_on", "entity_id": "light.kitchen"});
        assert!(confirmed(&prev, "ha_call_service", &other_light, "yes").is_none());
        assert!(confirmed(&prev, "ask_claude_code", &call, "yes").is_none());
    }

    #[test]
    fn yes_runs_the_parked_call_not_the_retyped_one() {
        let parked = json!({"domain": "light", "service": "turn_on", "entity_id": "light.a, light.b",
            "data": {"brightness_pct": 40}});
        let prev = vec![
            ("ha_call_service".to_string(), json!({"domain": "fan", "service": "turn_on", "entity_id": "fan.x"})),
            ("ha_call_service".to_string(), parked.clone()),
        ];
        // Retyped with entities reordered and the data changed or dropped.
        let retyped = json!({"domain": "light", "service": "turn_on", "entity_id": "light.b,light.a"});
        assert_eq!(confirmed(&prev, "ha_call_service", &retyped, "yes please"), Some(parked.clone()));
        let retyped = json!({"domain": "light", "service": "turn_on", "entity_id": "light.a,light.b",
            "data": {"brightness_pct": 100}});
        assert_eq!(confirmed(&prev, "ha_call_service", &retyped, "ok"), Some(parked));
        // Both parked calls can be confirmed by the one yes.
        let fan = json!({"domain": "fan", "service": "turn_on", "entity_id": "fan.x", "data": {}});
        assert!(confirmed(&prev, "ha_call_service", &fan, "yes").is_some());
    }

    #[test]
    fn one_yes_is_enough() {
        let tools = Tools::new(Arc::new(AtomicBool::new(false)), None, Some(test_ha()), Persona::Male, false)
            .with_prefs("one-yes");
        let call = json!({"domain": "lock", "service": "unlock", "entity_id": "lock.x"});
        let (msg, err) = tools.turn("unlock the door").run("ha_call_service", &call);
        assert!(err && msg.starts_with("Not run"));
        // The yes passes the gate (then fails at the fake token).
        let retyped = json!({"domain": "lock", "service": "unlock", "entity_id": "lock.x", "data": {}});
        let (msg, _) = tools.turn("yes").run("ha_call_service", &retyped);
        assert!(!msg.starts_with("Not run"), "{msg}");
        assert!(tools.always_labels().is_empty());
    }

    #[test]
    fn only_risky_ha_changes_ask() {
        let tools = Tools::new(Arc::new(AtomicBool::new(false)), None, Some(test_ha()), Persona::Male, false)
            .with_prefs("risky");
        // Safe: runs straight away (then fails at the fake token, past the gate).
        for c in [
            json!({"domain": "light", "service": "turn_on", "entity_id": "light.kitchen"}),
            json!({"domain": "switch", "service": "toggle", "entity_id": "switch.fan"}),
            json!({"domain": "scene", "service": "turn_on", "entity_id": "scene.movie"}),
            json!({"domain": "climate", "service": "set_temperature", "entity_id": "climate.house"}),
        ] {
            let (msg, _) = tools.turn("do it").run("ha_call_service", &c);
            assert!(!msg.starts_with("Not run"), "{c}: {msg}");
        }
        // Risky: asks, including through homeassistant.* or a mixed entity list.
        for c in [
            json!({"domain": "lock", "service": "unlock", "entity_id": "lock.front"}),
            json!({"domain": "cover", "service": "open_cover", "entity_id": "cover.garage"}),
            json!({"domain": "alarm_control_panel", "service": "alarm_disarm", "entity_id": "alarm_control_panel.home"}),
            json!({"domain": "valve", "service": "open_valve", "entity_id": "valve.water"}),
            json!({"domain": "script", "service": "turn_on", "entity_id": "script.leave"}),
            json!({"domain": "automation", "service": "trigger", "entity_id": "automation.x"}),
            json!({"domain": "homeassistant", "service": "turn_on", "entity_id": "light.a, cover.garage"}),
        ] {
            let (msg, err) = tools.turn("do it").run("ha_call_service", &c);
            assert!(err && msg.starts_with("Not run"), "{c}: {msg}");
        }
    }

    #[test]
    fn index_lists_controls_first_and_skips_unavailable() {
        let v = json!([
            {"entity_id": "sensor.office_temp", "state": "21", "attributes": {"friendly_name": "Office Temp"}},
            {"entity_id": "light.kitchen", "state": "off", "attributes": {"friendly_name": "Kitchen Light"}},
            {"entity_id": "light.gone", "state": "unavailable", "attributes": {}},
            {"entity_id": "automation.x", "state": "on", "attributes": {}},
        ]);
        assert_eq!(format_index(&v), "light.kitchen (Kitchen Light); sensor.office_temp (Office Temp)");
        let many: Vec<Value> = (0..2000)
            .map(|i| json!({"entity_id": format!("sensor.s{i}"), "state": "1", "attributes": {}}))
            .chain([json!({"entity_id": "light.k", "state": "on", "attributes": {}})])
            .collect();
        let ix = format_index(&Value::Array(many));
        assert!(ix.len() <= INDEX_MAX && ix.starts_with("light.k; "));
    }

    #[test]
    fn always_words() {
        for y in ["always", "Yes, always.", "yes always", "always do that", "sure, always okay"] {
            assert!(is_always(y), "{y}");
        }
        for n in ["always ask first", "no, not always", "don't always do that", "yes", "", "always check with me"] {
            assert!(!is_always(n), "{n}");
        }
        assert!(!is_always("what do you always say when you start up in the morning"));
    }

    #[test]
    fn claude_code_yes_survives_a_rephrased_task() {
        let prev = vec![("ask_claude_code".to_string(), json!({"task": "what version is rex"}))];
        let new = json!({"task": "find the rex version"});
        assert_eq!(confirmed(&prev, "ask_claude_code", &new, "yes"), Some(new));
        assert!(confirmed(&prev, "ask_claude_code", &json!({"task": "x"}), "no").is_none());
    }

    #[test]
    fn always_is_remembered_per_service() {
        let tools = Tools::new(Arc::new(AtomicBool::new(false)), None, Some(test_ha()), Persona::Male, false)
            .with_prefs("ha-always");
        let call = json!({"domain": "cover", "service": "open_cover", "entity_id": "cover.x"});
        let (msg, err) = tools.turn("open the blinds").run("ha_call_service", &call);
        assert!(err && msg.starts_with("Not run"));
        // "always" lets it through (it then fails at the fake token, past the gate) and saves it.
        let (msg, _) = tools.turn("yes always").run("ha_call_service", &call);
        assert!(!msg.starts_with("Not run"), "{msg}");
        assert_eq!(tools.always_labels(), ["cover.open_cover in Home Assistant"]);
        // Next time no yes is needed, even for other blinds.
        let other = json!({"domain": "cover", "service": "open_cover", "entity_id": "cover.y"});
        let (msg, _) = tools.turn("open the other blinds").run("ha_call_service", &other);
        assert!(!msg.starts_with("Not run"), "{msg}");
        // A different service still asks.
        let lock = json!({"domain": "lock", "service": "unlock", "entity_id": "lock.front"});
        let (msg, err) = tools.turn("unlock the door").run("ha_call_service", &lock);
        assert!(err && msg.starts_with("Not run"));
        // Forget puts the gate back.
        let (_, err) = tools.turn("ask first again").run("forget_permissions", &json!({}));
        assert!(!err);
        assert!(tools.always_labels().is_empty());
        let (msg, err) = tools.turn("open the blinds").run("ha_call_service", &call);
        assert!(err && msg.starts_with("Not run"));
    }

    #[test]
    fn reading_other_peoples_text_holds_later_actions() {
        let tools = Tools::new(Arc::new(AtomicBool::new(false)), None, Some(test_ha()), Persona::Male, false)
            .with_prefs("taint");
        let light = json!({"domain": "light", "service": "turn_on", "entity_id": "light.kitchen"});
        // A safe change runs straight away in a clean turn...
        let (msg, _) = tools.turn("lights on").run("ha_call_service", &light);
        assert!(!msg.starts_with("Not run"), "{msg}");
        // ...but is held after the turn read the web, mail or the calendar.
        let t = tools.turn("read my mail and then turn the lights on");
        t.taint();
        let (msg, err) = t.run("ha_call_service", &light);
        assert!(err && msg.starts_with("Not run"), "{msg}");
        let (msg, err) = t.run("set_brief", &json!({"on": true}));
        assert!(err && msg.starts_with("Not run"), "{msg}");
        drop(t);
        // The user's own yes next turn lets it through.
        let (msg, _) = tools.turn("yes").run("ha_call_service", &light);
        assert!(!msg.starts_with("Not run"), "{msg}");
    }

    #[test]
    fn tainted_turn_ignores_always_grants() {
        let tools = Tools::new(Arc::new(AtomicBool::new(false)), None, Some(test_ha()), Persona::Male, false)
            .with_prefs("taint-always");
        let call = json!({"domain": "cover", "service": "open_cover", "entity_id": "cover.x"});
        let _ = tools.turn("open the blinds").run("ha_call_service", &call);
        let (msg, _) = tools.turn("yes always").run("ha_call_service", &call);
        assert!(!msg.starts_with("Not run"), "{msg}");
        assert_eq!(tools.always_labels().len(), 1);
        let t = tools.turn("check the web then open the blinds");
        t.taint();
        let (msg, err) = t.run("ha_call_service", &call);
        assert!(err && msg.starts_with("Not run"), "{msg}");
    }

    #[test]
    fn google_tools_only_when_on() {
        let names = |t: &Tools| -> Vec<String> {
            t.defs().iter().filter_map(|d| d["name"].as_str().map(String::from)).collect()
        };
        let off = Tools::new(Arc::new(AtomicBool::new(false)), None, None, Persona::Male, false).with_prefs("g-off");
        assert!(!names(&off).iter().any(|n| n.starts_with("gmail_") || n.starts_with("calendar_")));
        let (msg, err) = off.turn("x").run("gmail_search", &json!({"query": "from:bob"}));
        assert!(err && msg.contains("not available"), "{msg}");
        let on = Tools::new(Arc::new(AtomicBool::new(false)), None, None, Persona::Male, false)
            .with_google(true)
            .with_prefs("g-on");
        let n = names(&on);
        for t in [
            "gmail_search",
            "gmail_read",
            "gmail_draft",
            "calendar_events",
            "calendar_add",
            "calendar_update",
            "calendar_delete",
        ] {
            assert!(n.iter().any(|x| x == t), "{t} missing");
        }
        assert!(on.set().google);
    }

    #[test]
    fn calendar_changes_always_ask_and_never_remember() {
        let tools = Tools::new(Arc::new(AtomicBool::new(false)), None, None, Persona::Male, false)
            .with_google(true)
            .with_prefs("cal");
        let add = json!({"summary": "Dentist", "start": "2026-12-01T09:00:00-06:00", "end": "2026-12-01T10:00:00-06:00"});
        let (msg, err) = tools.turn("add the dentist").run("calendar_add", &add);
        assert!(err && msg.starts_with("Not run"), "{msg}");
        // "always" passes this once (then fails: no sign-in in tests) but isn't kept.
        let (msg, _) = tools.turn("yes always").run("calendar_add", &add);
        assert!(!msg.starts_with("Not run"), "{msg}");
        assert!(tools.always_labels().is_empty());
        let (msg, err) = tools.turn("add the dentist again").run("calendar_add", &add);
        assert!(err && msg.starts_with("Not run"), "{msg}");
        // A yes to deleting one event doesn't delete another.
        let del = json!({"event_id": "abc123"});
        let _ = tools.turn("delete it").run("calendar_delete", &del);
        let (msg, err) = tools.turn("yes").run("calendar_delete", &json!({"event_id": "zzz999"}));
        assert!(err && msg.starts_with("Not run"), "{msg}");
        // Reads and drafts don't ask.
        let (msg, _) = tools.turn("any mail from bob").run("gmail_search", &json!({"query": "from:bob"}));
        assert!(!msg.starts_with("Not run"), "{msg}");
        let draft = json!({"to": "bob@example.com", "subject": "Hi", "body": "Hello"});
        let (msg, _) = tools.turn("draft a note to bob").run("gmail_draft", &draft);
        assert!(!msg.starts_with("Not run"), "{msg}");
    }

    #[test]
    fn calendar_keys() {
        assert_eq!(call_key("calendar_update", &json!({"event_id": " ab1 "})).as_deref(), Some("calendar_update:ab1"));
        assert_ne!(
            call_key("calendar_update", &json!({"event_id": "a"})),
            call_key("calendar_delete", &json!({"event_id": "a"}))
        );
        assert!(call_key("calendar_delete", &json!({})).is_none());
        assert!(grant_key("calendar_add", &json!({})).is_none());
    }

    #[test]
    fn brief_by_voice_overrides_config() {
        let tools =
            Tools::new(Arc::new(AtomicBool::new(false)), None, None, Persona::Male, true).with_prefs("brief");
        assert!(tools.brief());
        let (_, err) = tools.turn("more detail please").run("set_brief", &json!({"on": false}));
        assert!(!err);
        assert!(!tools.brief());
        let (_, err) = tools.turn("x").run("set_brief", &json!({"on": "yes"}));
        assert!(err);
        let (_, err) = tools.turn("x").run("forget_permissions", &json!({}));
        assert!(err, "no gated tools, so nothing to forget");
    }

    #[test]
    fn we_by_voice_is_saved() {
        let tools = Tools::new(Arc::new(AtomicBool::new(false)), None, None, Persona::ModernFemale, false)
            .with_prefs("we");
        assert!(!tools.we());
        let (msg, err) = tools.turn("say we instead of I").run("set_we", &json!({"on": true}));
        assert!(!err && msg.contains("we, us and our"), "{msg}");
        assert!(tools.we());
        let (_, err) = tools.turn("go back to I").run("set_we", &json!({"on": false}));
        assert!(!err);
        assert!(!tools.we());
        let (_, err) = tools.turn("x").run("set_we", &json!({}));
        assert!(err);
    }

    #[test]
    fn spoken_times() {
        assert_eq!(
            spoken_span("2026-11-26T14:00:00-06:00", "2026-11-26T15:30:00-06:00").as_deref(),
            Some("Thursday, November 26, 2 PM to 3:30 PM")
        );
        assert_eq!(spoken_span("2026-11-26", "2026-11-27").as_deref(), Some("Thursday, November 26, all day"));
        assert_eq!(
            spoken_span("2026-12-31", "2027-01-02").as_deref(),
            Some("Thursday, December 31 to Friday, January 1, all day")
        );
        assert_eq!(
            spoken_span("2026-02-28T23:00:00Z", "2026-03-01T00:30:00Z").as_deref(),
            Some("Saturday, February 28 at 11 PM to Sunday, March 1 at 12:30 AM")
        );
        assert!(spoken_span("tomorrow", "2026-11-27").is_none());
        assert!(spoken_span("2026-11-26", "2026-11-26T10:00:00Z").is_none());
    }

    #[test]
    fn app_asks_and_runs_on_yes() {
        let tools = Tools::new(Arc::new(AtomicBool::new(false)), None, Some(test_ha()), Persona::Male, false);
        let call = json!({"domain": "lock", "service": "unlock", "entity_id": "lock.front"});
        let (_, err) = tools.turn("unlock the door").run("ha_call_service", &call);
        assert!(err);
        assert_eq!(tools.question().as_deref(), Some("Shall I unlock lock.front?"));
        // A plain no drops it, with no model round.
        assert!(matches!(tools.answer("no", &|_| {}), Answer::No));
        assert!(tools.question().is_none());
        assert!(matches!(tools.answer("yes", &|_| {}), Answer::Nothing));
        // Something else drops it and hands the turn to the model.
        tools.turn("unlock the door").run("ha_call_service", &call);
        assert!(matches!(tools.answer("no, the back door", &|_| {}), Answer::Other));
        assert!(tools.question().is_none());
        // A yes runs the parked call itself (test HA is unreachable, so it fails).
        tools.turn("unlock the door").run("ha_call_service", &call);
        let Answer::Ran(ran) = tools.answer("yes please", &|_| {}) else { panic!("not run") };
        assert_eq!(ran.len(), 1);
        assert!(ran[0].err && !ran[0].out.starts_with("Not run"), "{}", ran[0].out);
        assert!(Answer::reply(&ran).unwrap().starts_with("Sorry, that didn't work"));
        assert!(tools.question().is_none());
    }

    #[test]
    fn app_reply_and_messages() {
        let ok = |name: &str| Ran { name: name.into(), input: json!({}), out: "fine".into(), err: false };
        assert_eq!(Answer::reply(&[ok("calendar_add")]).as_deref(), Some("Done."));
        // Claude Code's findings go to the model.
        assert!(Answer::reply(&[ok("calendar_add"), ok("ask_claude_code")]).is_none());
        let m = Answer::messages(&[ok("ask_claude_code")]);
        assert_eq!(m[0]["content"][0]["id"], m[1]["content"][0]["tool_use_id"]);
        assert!(is_no("No.") && is_no("never mind") && !is_no("no, make it 3 PM") && !is_no("yes"));
    }

    #[test]
    fn describe_without_network() {
        let tools = Tools::new(Arc::new(AtomicBool::new(false)), None, None, Persona::Male, false);
        let add = json!({"summary": "Test\nevent", "start": "2026-11-26T14:00:00-06:00", "end": "2026-11-26T15:00:00-06:00"});
        assert_eq!(
            tools.describe("calendar_add", &add),
            "add \"Test event\", Thursday, November 26, 2 PM to 3 PM, to your calendar"
        );
        assert_eq!(
            tools.describe("ha_call_service", &json!({"domain": "lock", "service": "unlock", "entity_id": "lock.a, lock.b"})),
            "unlock lock.a and lock.b"
        );
        assert_eq!(
            tools.describe("ha_call_service", &json!({"domain": "lock", "service": "unlock", "entity_id": "lock.Front door"})),
            "do that in Home Assistant"
        );
        // Google is off here, so the event can't be looked up.
        assert_eq!(tools.describe("calendar_delete", &json!({"event_id": "abc"})), "delete the event from your calendar");
    }

    #[test]
    fn pending_lasts_one_turn() {
        let tools = Tools::new(Arc::new(AtomicBool::new(false)), None, Some(test_ha()), Persona::Male, false);
        let call = json!({"domain": "lock", "service": "unlock", "entity_id": "lock.x"});
        // Turn 1: not confirmed, gets parked. Never reaches the network.
        let (msg, err) = tools.turn("unlock the door").run("ha_call_service", &call);
        assert!(err && msg.starts_with("Not run"));
        // Turn 2: something else said; pending is dropped.
        drop(tools.turn("actually, what time is it"));
        // Turn 3: a yes now has nothing to confirm, so it parks again.
        let (msg, err) = tools.turn("yes").run("ha_call_service", &call);
        assert!(err && msg.starts_with("Not run"));
    }

    #[test]
    fn service_validation_blocks_before_network() {
        let tools = Tools::new(Arc::new(AtomicBool::new(false)), None, Some(test_ha()), Persona::Male, false);
        let t = Turn { tools: &tools, prev: Vec::new(), user_text: String::new(), tainted: AtomicBool::new(false) };
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
        assert!(format_states(&v, "office lamps").contains("light.office_lamp"));
        assert!(format_states(&v, "garage doors").contains("cover.garage_door"));
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
