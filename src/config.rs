//! Settings from `~/.config/rex-static/config`: plain `key = value` lines,
//! `#` starts a comment. Every key is optional. A feature whose setting is
//! missing or invalid is turned off, and the problem goes to stderr.
//!
//! The file decides which programs run and where the Home Assistant token
//! is sent, so it's ignored unless it belongs to you and nobody else can
//! write to it.

use crate::persona::Persona;
use crate::mqtt::MqttConf;
use crate::tools::HaConf;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FaceChoice {
    /// The persona's head image (a `<data>/rex.png` or `roxy.png` file
    /// overrides the built-in one).
    Auto,
    /// The procedural mannequin, no image.
    Plastic,
    /// Your own `<data>/face.png`, laid out like the original photo.
    Photo,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub persona: Persona,
    pub user_name: Option<String>,
    /// Claude model id, for example `claude-haiku-5-5`.
    pub model: String,
    pub face: FaceChoice,
    pub piper: PathBuf,
    pub claude_code: Option<PathBuf>,
    pub ha: Option<HaConf>,
    /// Home Assistant announcements over MQTT.
    pub mqtt: Option<MqttConf>,
    /// Short replies by default. Saying "be brief" or "more detail" overrides it.
    pub brief: bool,
    /// Listen for "Hey <name>" all the time (needs vosk-api).
    pub wake: bool,
}

/// The config, read once on first use.
pub fn get() -> &'static Config {
    static CONFIG: OnceLock<Config> = OnceLock::new();
    CONFIG.get_or_init(|| {
        let home = home();
        let (cfg, problems) = load(&path(), &home);
        for p in problems {
            eprintln!("rex-static: config: {p}");
        }
        cfg
    })
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").expect("HOME is not set"))
}

pub fn path() -> PathBuf {
    if cfg!(test) {
        return crate::memory::data_dir().join("config");
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home().join(".config"));
    base.join("rex-static").join("config")
}

/// Set `persona` in the config file, keeping every other line. Writes a
/// temp file next to it and renames it over, so a crash can't leave half a
/// config. Refuses a file the app would ignore anyway.
pub fn save_persona(p: Persona) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let path = path();
    let old = match std::fs::metadata(&path) {
        Ok(meta) => {
            let me = std::fs::metadata("/proc/self").map(|m| m.uid()).ok();
            if Some(meta.uid()) != me || meta.mode() & 0o022 != 0 {
                return Err(format!("{} isn't safely yours; not changing it", path.display()));
            }
            std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?
        }
        Err(_) => String::new(),
    };
    let dir = path.parent().ok_or("config path has no folder")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let tmp = dir.join(".config.tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(|e| format!("write {}: {e}", tmp.display()))?;
    f.write_all(with_persona(&old, p).as_bytes())
        .and_then(|()| f.sync_all())
        .map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("rename to {}: {e}", path.display()))
}

/// Config text with the `persona` line set to `p`. Replaces the first
/// active `persona` line (keeping its comment), drops any later ones, or
/// appends one.
pub fn with_persona(text: &str, p: Persona) -> String {
    let mut out = String::new();
    let mut done = false;
    for raw in text.lines() {
        let (body, comment) = match raw.find('#') {
            Some(i) => (&raw[..i], &raw[i..]),
            None => (raw, ""),
        };
        let is_persona = body.split_once('=').is_some_and(|(k, _)| k.trim() == "persona");
        if !is_persona {
            out.push_str(raw);
        } else if done {
            continue;
        } else {
            done = true;
            out.push_str(&format!("persona = {}", p.tag()));
            if !comment.is_empty() {
                out.push_str("  ");
                out.push_str(comment);
            }
        }
        out.push('\n');
    }
    if !done {
        out.push_str(&format!("persona = {}\n", p.tag()));
    }
    out
}

/// Read and parse the file. A missing file means all defaults.
fn load(path: &Path, home: &Path) -> (Config, Vec<String>) {
    use std::os::unix::fs::MetadataExt;
    let Ok(meta) = std::fs::metadata(path) else {
        return parse("", home);
    };
    let me = std::fs::metadata("/proc/self").map(|m| m.uid()).ok();
    if Some(meta.uid()) != me || meta.mode() & 0o022 != 0 {
        let (c, mut p) = parse("", home);
        p.push(format!("ignoring {}: it must be yours and not writable by others", path.display()));
        return (c, p);
    }
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text, home),
        Err(e) => {
            let (c, mut p) = parse("", home);
            p.push(format!("can't read {}: {e}", path.display()));
            (c, p)
        }
    }
}

/// Parse config text. `home` expands a leading `~/` in paths.
pub fn parse(text: &str, home: &Path) -> (Config, Vec<String>) {
    let mut problems = Vec::new();
    let mut cfg = Config {
        persona: Persona::Male,
        user_name: None,
        model: crate::llm::DEFAULT_MODEL.to_string(),
        face: FaceChoice::Auto,
        piper: home.join(".local/bin/piper"),
        claude_code: None,
        ha: None,
        mqtt: None,
        brief: false,
        wake: false,
    };
    let (mut ha_url, mut ha_pin, mut ha_token) = (None, None, home.join(".config/ha-token"));
    let (mut mqtt_url, mut mqtt_user) = (None, None);
    for (n, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            problems.push(format!("line {}: expected key = value", n + 1));
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        let mut bad = |why: &str| problems.push(format!("line {}: {key}: {why}", n + 1));
        match key {
            "persona" => match Persona::from_tag(value) {
                Some(p) => cfg.persona = p,
                None => bad("use male, female, modern-male or modern-female"),
            },
            "user_name" => match clean_name(value) {
                Some(v) => cfg.user_name = Some(v),
                None => bad("use letters, spaces, - ' or . (32 at most)"),
            },
            "model" => {
                if is_model(value) {
                    cfg.model = value.to_string();
                } else {
                    bad("use a Claude model id, like claude-haiku-5-5");
                }
            }
            "face" => match value {
                "auto" => cfg.face = FaceChoice::Auto,
                "plastic" => cfg.face = FaceChoice::Plastic,
                "photo" => cfg.face = FaceChoice::Photo,
                _ => bad("use auto, plastic or photo"),
            },
            "piper" => match abs_path(value, home) {
                Some(p) => cfg.piper = p,
                None => bad("needs an absolute path"),
            },
            "brief" => match value {
                "yes" => cfg.brief = true,
                "no" => cfg.brief = false,
                _ => bad("use yes or no"),
            },
            "wake" => match value {
                "yes" => cfg.wake = true,
                "no" => cfg.wake = false,
                _ => bad("use yes or no"),
            },
            "claude_code" => match abs_path(value, home) {
                Some(p) => cfg.claude_code = Some(p),
                None => bad("needs an absolute path"),
            },
            "ha_url" => match https_origin(value) {
                Some(u) => ha_url = Some(u),
                None => bad("needs https://host or https://host:port, nothing after it"),
            },
            "ha_pin" => {
                if is_pin(value) {
                    ha_pin = Some(value.to_string());
                } else {
                    bad("needs sha256// and a base64 key hash");
                }
            }
            "ha_token_file" => match abs_path(value, home) {
                Some(p) => ha_token = p,
                None => bad("needs an absolute path"),
            },
            "mqtt_url" => match mqtt_origin(value) {
                Some(hp) => mqtt_url = Some(hp),
                None => bad("needs mqtt://host or mqtt://host:port, nothing after it"),
            },
            "mqtt_user" => {
                if is_mqtt_user(value) {
                    mqtt_user = Some(value.to_string());
                } else {
                    bad("use 64 visible characters at most, no spaces");
                }
            }
            _ => bad("unknown key"),
        }
    }
    cfg.ha = ha_url.map(|url| HaConf { url, pin: ha_pin, token_file: ha_token });
    cfg.mqtt = mqtt_url.map(|(host, port)| MqttConf { host, port, user: mqtt_user });
    (cfg, problems)
}

/// Same rules as a rename: letters, digits, space, `-`, `'`, `.`; 32 at most.
fn clean_name(v: &str) -> Option<String> {
    let ok = !v.is_empty()
        && v.chars().count() <= 32
        && v.chars().all(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '\'' | '.'));
    ok.then(|| v.split_whitespace().collect::<Vec<_>>().join(" "))
}

fn abs_path(v: &str, home: &Path) -> Option<PathBuf> {
    if v.chars().any(char::is_control) {
        return None;
    }
    let p = match v.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None => PathBuf::from(v),
    };
    (p.is_absolute() && !p.components().any(|c| c == std::path::Component::ParentDir)).then_some(p)
}

/// `host[:port]` after the scheme, with an optional trailing slash
/// dropped: the host and the port, if one is given.
fn host_port(rest: &str) -> Option<(&str, Option<u16>)> {
    let rest = rest.trim_end_matches('/');
    let (host, port) = match rest.split_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (rest, None),
    };
    let host_ok = !host.is_empty()
        && host.len() <= 253
        && host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        && !host.starts_with(['.', '-']);
    let port = match port {
        None => None,
        Some(p) if !p.is_empty() && p.len() <= 5 => Some(p.parse::<u16>().ok().filter(|&n| n > 0)?),
        Some(_) => return None,
    };
    host_ok.then_some((host, port))
}

/// `https://host[:port]`, with an optional trailing slash dropped.
fn https_origin(v: &str) -> Option<String> {
    let rest = v.strip_prefix("https://")?;
    host_port(rest)?;
    Some(format!("https://{}", rest.trim_end_matches('/')))
}

/// `mqtt://host[:port]`: the host and port (1883 when none is given).
fn mqtt_origin(v: &str) -> Option<(String, u16)> {
    let (host, port) = host_port(v.strip_prefix("mqtt://")?)?;
    Some((host.to_string(), port.unwrap_or(1883)))
}

/// A broker user name: 1 to 64 visible characters.
fn is_mqtt_user(v: &str) -> bool {
    !v.is_empty() && v.chars().count() <= 64 && v.chars().all(|c| !c.is_control() && !c.is_whitespace())
}

/// A Claude model id: `claude-` then lowercase letters, digits, `-` or `.`.
fn is_model(v: &str) -> bool {
    v.len() <= 64
        && v.strip_prefix("claude-").is_some_and(|r| {
            !r.is_empty() && r.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
        })
}

/// curl `--pinnedpubkey` form: `sha256//` plus a 44-character base64 hash.
fn is_pin(v: &str) -> bool {
    v.strip_prefix("sha256//").is_some_and(|h| {
        h.len() == 44
            && h.ends_with('=')
            && h[..43].chars().all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/')
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> PathBuf {
        PathBuf::from("/home/test")
    }

    #[test]
    fn persona_rewrite_keeps_other_lines() {
        let text = "# my config\nuser_name = Sam\npersona = male   # pick one\n# persona = female\nface = auto\n";
        let out = with_persona(text, Persona::Female);
        assert_eq!(
            out,
            "# my config\nuser_name = Sam\npersona = female  # pick one\n# persona = female\nface = auto\n"
        );
        assert_eq!(parse(&out, &home()).0.persona, Persona::Female);
        assert_eq!(with_persona("", Persona::Female), "persona = female\n");
        assert_eq!(with_persona("persona = male\npersona = male\n", Persona::Female), "persona = female\n");
    }

    #[test]
    fn test_config_path_is_temp() {
        assert!(path().starts_with(std::env::temp_dir()));
    }

    #[test]
    fn empty_is_all_defaults() {
        let (c, p) = parse("", &home());
        assert!(p.is_empty());
        assert_eq!(c.persona, Persona::Male);
        assert_eq!(c.user_name, None);
        assert_eq!(c.model, "claude-haiku-5-5");
        assert_eq!(c.face, FaceChoice::Auto);
        assert_eq!(c.piper, PathBuf::from("/home/test/.local/bin/piper"));
        assert!(c.claude_code.is_none() && c.ha.is_none());
    }

    #[test]
    fn full_file() {
        let text = "# comment\n\
persona = female\n\
user_name = Sam O'Neil\n\
model = claude-sonnet-5-5\n\
face = plastic   # trailing comment\n\
piper = ~/bin/piper\n\
claude_code = /opt/claude/claude\n\
ha_url = https://ha.local:8123/\n\
ha_pin = sha256//AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=\n\
ha_token_file = ~/.secret/ha\n\
mqtt_url = mqtt://broker.local\n\
mqtt_user = mqtt-user\n";
        let (c, p) = parse(text, &home());
        assert!(p.is_empty(), "{p:?}");
        assert_eq!(c.persona, Persona::Female);
        assert_eq!(c.user_name.as_deref(), Some("Sam O'Neil"));
        assert_eq!(c.model, "claude-sonnet-5-5");
        assert_eq!(c.face, FaceChoice::Plastic);
        assert_eq!(c.piper, PathBuf::from("/home/test/bin/piper"));
        assert_eq!(c.claude_code, Some(PathBuf::from("/opt/claude/claude")));
        let ha = c.ha.unwrap();
        assert_eq!(ha.url, "https://ha.local:8123");
        assert!(ha.pin.is_some());
        assert_eq!(ha.token_file, PathBuf::from("/home/test/.secret/ha"));
        let m = c.mqtt.unwrap();
        assert_eq!((m.host.as_str(), m.port, m.user.as_deref()), ("broker.local", 1883, Some("mqtt-user")));
    }

    #[test]
    fn bad_values_turn_features_off() {
        let text = "persona = robot\n\
user_name = <script>\n\
claude_code = claude\n\
ha_url = http://ha.local\n\
ha_pin = abc\n\
piper = ../piper\n\
nonsense\n\
colour = red\n\
model = gpt-5\n\
mqtt_url = tcp://broker\n\
mqtt_user = two words\n";
        let (c, p) = parse(text, &home());
        assert_eq!(p.len(), 11, "{p:?}");
        assert!(c.mqtt.is_none());
        assert_eq!(c.model, "claude-haiku-5-5");
        assert_eq!(c.persona, Persona::Male);
        assert!(c.user_name.is_none() && c.claude_code.is_none() && c.ha.is_none());
        assert_eq!(c.piper, PathBuf::from("/home/test/.local/bin/piper"));
    }

    #[test]
    fn urls() {
        for ok in ["https://homeassistant.local:8123", "https://10.0.0.5", "https://ha.example.com/"] {
            assert!(https_origin(ok).is_some(), "{ok}");
        }
        for bad in [
            "http://ha.local",
            "https://",
            "https://ha.local/api",
            "https://user@ha.local",
            "https://ha.local:0",
            "https://ha.local:99999",
            "https://ha.local?x=1",
            "https://ha .local",
            "ha.local",
        ] {
            assert!(https_origin(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn mqtt_urls() {
        assert_eq!(mqtt_origin("mqtt://192.168.86.120:1883"), Some(("192.168.86.120".into(), 1883)));
        assert_eq!(mqtt_origin("mqtt://broker/"), Some(("broker".into(), 1883)));
        assert_eq!(mqtt_origin("mqtt://broker:8883"), Some(("broker".into(), 8883)));
        for bad in ["mqtts://broker", "mqtt://", "mqtt://broker/x", "mqtt://u@broker", "mqtt://broker:0", "broker"] {
            assert!(mqtt_origin(bad).is_none(), "{bad}");
        }
        let (c, _) = parse("mqtt_user = someone", &home());
        assert!(c.mqtt.is_none(), "a user without a url is off");
    }

    #[test]
    fn pin_without_url_is_off() {
        let (c, _) = parse("ha_pin = sha256//AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=", &home());
        assert!(c.ha.is_none());
    }

    #[test]
    fn example_file_parses_cleanly() {
        let (c, p) = parse(include_str!("../config.example"), &home());
        assert!(p.is_empty(), "{p:?}");
        assert_eq!(c.persona, Persona::Male);
        assert!(c.claude_code.is_none() && c.ha.is_none() && c.mqtt.is_none());
    }

    #[test]
    fn model_ids() {
        for ok in ["claude-haiku-5-5", "claude-opus-5-5", "claude-haiku-4-5-20251001"] {
            assert!(is_model(ok), "{ok}");
        }
        for bad in ["claude-", "gpt-5", "Claude-Opus", "claude-opus 5", "claude-x\"y", &format!("claude-{}", "a".repeat(60))] {
            assert!(!is_model(bad), "{bad}");
        }
    }

    #[test]
    fn paths_reject_parent_dirs() {
        assert!(abs_path("/opt/../etc/x", &home()).is_none());
        assert!(abs_path("~/a/b", &home()).is_some());
        assert!(abs_path("relative/x", &home()).is_none());
    }
}
