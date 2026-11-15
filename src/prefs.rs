//! Choices made by voice that outlast a restart: the actions the user said
//! "always" to, and brief mode. Kept in `<data>/prefs`, one entry per line:
//! `always <key>` or `brief yes|no`.
//!
//! The file decides which actions run without asking, so it's ignored
//! unless it belongs to you and nobody else can write to it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub fn path() -> PathBuf {
    crate::memory::data_dir().join("prefs")
}

#[derive(Debug, Default, PartialEq)]
pub struct Prefs {
    path: PathBuf,
    always: BTreeSet<String>,
    /// None until the user picks a mode by voice; the config decides then.
    brief: Option<bool>,
}

/// A key worth storing: a tool name or `ha:domain.service`.
fn good_key(k: &str) -> bool {
    !k.is_empty()
        && k.len() <= 140
        && k.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '.' | ':'))
}

impl Prefs {
    /// Read the file. A missing, unsafe or unreadable file means no prefs.
    pub fn load(path: &Path) -> Self {
        use std::os::unix::fs::MetadataExt;
        let mut p = Prefs { path: path.to_path_buf(), ..Default::default() };
        let Ok(meta) = std::fs::metadata(path) else { return p };
        let me = std::fs::metadata("/proc/self").map(|m| m.uid()).ok();
        if Some(meta.uid()) != me || meta.mode() & 0o022 != 0 {
            eprintln!("rex-static: ignoring {}: it must be yours and not writable by others", path.display());
            return p;
        }
        let Ok(text) = std::fs::read_to_string(path) else { return p };
        for line in text.lines() {
            match line.trim().split_once(' ') {
                Some(("always", k)) if good_key(k) => {
                    p.always.insert(k.to_string());
                }
                Some(("brief", "yes")) => p.brief = Some(true),
                Some(("brief", "no")) => p.brief = Some(false),
                _ => {}
            }
        }
        p
    }

    pub fn has(&self, key: &str) -> bool {
        self.always.contains(key)
    }

    pub fn always(&self) -> impl Iterator<Item = &str> {
        self.always.iter().map(String::as_str)
    }

    pub fn grant(&mut self, key: &str) -> Result<(), String> {
        if !good_key(key) {
            return Err(format!("bad permission key {key}"));
        }
        self.always.insert(key.to_string());
        self.save()
    }

    pub fn forget_all(&mut self) -> Result<(), String> {
        self.always.clear();
        self.save()
    }

    pub fn brief(&self) -> Option<bool> {
        self.brief
    }

    pub fn set_brief(&mut self, on: bool) -> Result<(), String> {
        self.brief = Some(on);
        self.save()
    }

    fn text(&self) -> String {
        let mut s: String = self.always.iter().map(|k| format!("always {k}\n")).collect();
        if let Some(b) = self.brief {
            s.push_str(if b { "brief yes\n" } else { "brief no\n" });
        }
        s
    }

    /// Write a temp file next to it and rename it over, so a crash can't
    /// leave half a file.
    fn save(&self) -> Result<(), String> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let dir = self.path.parent().ok_or("prefs path has no folder")?;
        std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
        let name = self.path.file_name().and_then(|n| n.to_str()).unwrap_or("prefs");
        let tmp = dir.join(format!(".{name}.tmp"));
        let _ = std::fs::remove_file(&tmp);
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|e| format!("write {}: {e}", tmp.display()))?;
        f.write_all(self.text().as_bytes())
            .and_then(|()| f.sync_all())
            .map_err(|e| format!("write {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| format!("rename to {}: {e}", self.path.display()))
    }
}

#[cfg(test)]
pub fn test_path(name: &str) -> PathBuf {
    crate::memory::data_dir().join(format!("prefs-{name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefs_file_is_in_temp_folder() {
        assert!(path().starts_with(std::env::temp_dir()));
        assert!(test_path("x").starts_with(std::env::temp_dir()));
    }

    #[test]
    fn prefs_round_trip() {
        let p = test_path("roundtrip");
        let _ = std::fs::remove_file(&p);
        let mut a = Prefs::load(&p);
        assert!(!a.has("ask_claude_code") && a.brief().is_none());
        a.grant("ask_claude_code").unwrap();
        a.grant("ha:light.turn_on").unwrap();
        a.set_brief(true).unwrap();
        assert!(a.grant("bad key\nalways x").is_err());
        let b = Prefs::load(&p);
        assert_eq!(a, b);
        assert!(b.has("ha:light.turn_on") && b.brief() == Some(true));
        let mut b = b;
        b.forget_all().unwrap();
        let c = Prefs::load(&p);
        assert!(!c.has("ask_claude_code") && c.brief() == Some(true));
    }

    #[test]
    fn junk_and_unsafe_files_are_ignored() {
        use std::os::unix::fs::PermissionsExt;
        let p = test_path("junk");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "always Bad Key\nalways\nbrief maybe\nalways ask_claude_code\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
        let a = Prefs::load(&p);
        assert_eq!(a.always().collect::<Vec<_>>(), ["ask_claude_code"]);
        assert!(a.brief().is_none());
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o666)).unwrap();
        assert!(!Prefs::load(&p).has("ask_claude_code"));
    }
}
