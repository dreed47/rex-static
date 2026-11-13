//! Settings from `~/.config/rex-static/config`: plain `key = value` lines,
//! `#` starts a comment. Every key is optional. A feature whose setting is
//! missing or invalid is turned off, and the problem goes to stderr.
//!
//! The file decides which programs run and where the Home Assistant token
//! is sent, so it's ignored unless it belongs to you and nobody else can
//! write to it.

use crate::persona::Persona;
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
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home().join(".config"));
    base.join("rex-static").join("config")
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
    };
    let (mut ha_url, mut ha_pin, mut ha_token) = (None, None, home.join(".config/ha-token"));
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
            "persona" => match value {
                "male" => cfg.persona = Persona::Male,
                "female" => cfg.persona = Persona::Female,
                _ => bad("use male or female"),
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
            _ => bad("unknown key"),
        }
    }
    cfg.ha = ha_url.map(|url| HaConf { url, pin: ha_pin, token_file: ha_token });
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

/// `https://host[:port]`, with an optional trailing slash dropped.
fn https_origin(v: &str) -> Option<String> {
    let rest = v.strip_prefix("https://")?.trim_end_matches('/');
    let (host, port) = match rest.split_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (rest, None),
    };
    let host_ok = !host.is_empty()
        && host.len() <= 253
        && host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        && !host.starts_with(['.', '-']);
    let port_ok = port.is_none_or(|p| !p.is_empty() && p.len() <= 5 && p.parse::<u16>().is_ok_and(|n| n > 0));
    (host_ok && port_ok).then(|| format!("https://{rest}"))
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
ha_token_file = ~/.secret/ha\n";
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
model = gpt-5\n";
        let (c, p) = parse(text, &home());
        assert_eq!(p.len(), 9, "{p:?}");
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
    fn pin_without_url_is_off() {
        let (c, _) = parse("ha_pin = sha256//AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=", &home());
        assert!(c.ha.is_none());
    }

    #[test]
    fn example_file_parses_cleanly() {
        let (c, p) = parse(include_str!("../config.example"), &home());
        assert!(p.is_empty(), "{p:?}");
        assert_eq!(c.persona, Persona::Male);
        assert!(c.claude_code.is_none() && c.ha.is_none());
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
