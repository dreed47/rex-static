//! Long-term memory: every exchange goes into SQLite, plus rolling notes
//! about the user that Claude rewrites now and then. Each persona sees only
//! its own conversation history; the notes are shared.

use rusqlite::{Connection, params};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Root folder for the app's data. Tests always get a throwaway temp folder.
pub fn data_dir() -> PathBuf {
    if cfg!(test) {
        return std::env::temp_dir().join(format!("rex-static-test-{}", std::process::id()));
    }
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").expect("HOME is not set")).join(".local/share")
        });
    base.join("rex-static")
}

pub fn db_path() -> PathBuf {
    data_dir().join("memory.db")
}

#[derive(Clone, Debug, PartialEq)]
pub struct Turn {
    pub role: String, // "user" or "assistant"
    pub text: String,
}

pub struct Memory {
    conn: Connection,
    /// Persona tag stored with each message, and the one `recent` reads.
    tag: &'static str,
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl Memory {
    pub fn open(path: &Path, tag: &'static str) -> rusqlite::Result<Self> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS messages(
                 id   INTEGER PRIMARY KEY,
                 ts   INTEGER NOT NULL,
                 role TEXT NOT NULL,
                 text TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS notes(
                 id   INTEGER PRIMARY KEY CHECK (id = 1),
                 text TEXT NOT NULL,
                 upto INTEGER NOT NULL,
                 ts   INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS settings(
                 key   TEXT PRIMARY KEY,
                 value TEXT NOT NULL);",
        )?;
        // Older databases predate personas; their messages were all Rex's.
        let has_persona = conn
            .prepare("SELECT 1 FROM pragma_table_info('messages') WHERE name = 'persona'")?
            .exists([])?;
        if !has_persona {
            conn.execute_batch("ALTER TABLE messages ADD COLUMN persona TEXT NOT NULL DEFAULT 'male'")?;
        }
        Ok(Self { conn, tag })
    }

    pub fn add(&self, role: &str, text: &str) -> rusqlite::Result<i64> {
        self.conn.execute(
            "INSERT INTO messages(ts, role, text, persona) VALUES (?1, ?2, ?3, ?4)",
            params![now(), role, text, self.tag],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// This persona's last `n` turns, oldest first.
    pub fn recent(&self, n: usize) -> rusqlite::Result<Vec<Turn>> {
        let mut st = self
            .conn
            .prepare("SELECT role, text FROM messages WHERE persona = ?2 ORDER BY id DESC LIMIT ?1")?;
        let mut rows: Vec<Turn> = st
            .query_map(params![n as i64, self.tag], |r| {
                Ok(Turn { role: r.get(0)?, text: r.get(1)? })
            })?
            .collect::<Result<_, _>>()?;
        rows.reverse();
        Ok(rows)
    }

    /// Current notes and the last message id they cover.
    pub fn notes(&self) -> rusqlite::Result<(String, i64)> {
        let mut st = self.conn.prepare("SELECT text, upto FROM notes WHERE id = 1")?;
        let mut rows = st.query([])?;
        Ok(match rows.next()? {
            Some(r) => (r.get(0)?, r.get(1)?),
            None => (String::new(), 0),
        })
    }

    pub fn set_notes(&self, text: &str, upto: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO notes(id, text, upto, ts) VALUES (1, ?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET text = ?1, upto = ?2, ts = ?3",
            params![text, upto, now()],
        )?;
        Ok(())
    }

    /// Turns newer than `id`, with their ids.
    pub fn since(&self, id: i64) -> rusqlite::Result<Vec<(i64, Turn)>> {
        let mut st = self
            .conn
            .prepare("SELECT id, role, text FROM messages WHERE id > ?1 ORDER BY id")?;
        let rows = st
            .query_map(params![id], |r| {
                Ok((r.get(0)?, Turn { role: r.get(1)?, text: r.get(2)? }))
            })?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    pub fn setting(&self, key: &str) -> Option<String> {
        self.conn
            .query_row("SELECT value FROM settings WHERE key = ?1", params![key], |r| r.get(0))
            .ok()
    }

    pub fn set_setting(&self, key: &str, value: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO settings(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = ?2",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn count_since(&self, id: i64) -> rusqlite::Result<i64> {
        self.conn
            .query_row("SELECT COUNT(*) FROM messages WHERE id > ?1", params![id], |r| r.get(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_paths_are_under_temp() {
        let tmp = std::env::temp_dir();
        assert!(data_dir().starts_with(&tmp));
        assert!(db_path().starts_with(&tmp));
        assert!(!db_path().to_string_lossy().contains(".local/share"));
    }

    #[test]
    fn roundtrip() {
        let path = data_dir().join("roundtrip.db");
        assert!(path.starts_with(std::env::temp_dir()));
        let _ = std::fs::remove_file(&path);
        let m = Memory::open(&path, "male").unwrap();
        m.add("user", "hello rex").unwrap();
        let id = m.add("assistant", "w-w-welcome").unwrap();
        m.add("user", "again").unwrap();
        let r = m.recent(2).unwrap();
        assert_eq!(r[0].text, "w-w-welcome");
        assert_eq!(r[1].text, "again");
        assert_eq!(m.notes().unwrap(), (String::new(), 0));
        m.set_notes("- likes Rust", id).unwrap();
        assert_eq!(m.notes().unwrap(), ("- likes Rust".into(), id));
        assert_eq!(m.count_since(id).unwrap(), 1);
        assert_eq!(m.since(id).unwrap()[0].1.text, "again");
        assert_eq!(m.setting("name"), None);
        m.set_setting("name", "Vic").unwrap();
        m.set_setting("name", "Vic Vector").unwrap();
        assert_eq!(m.setting("name").as_deref(), Some("Vic Vector"));
        drop(m);
        // The other persona doesn't see Rex's history, but shares the notes.
        let f = Memory::open(&path, "female").unwrap();
        assert!(f.recent(10).unwrap().is_empty());
        f.add("user", "hi roxy").unwrap();
        assert_eq!(f.recent(10).unwrap().len(), 1);
        assert_eq!(f.notes().unwrap().0, "- likes Rust");
        assert_eq!(Memory::open(&path, "male").unwrap().recent(10).unwrap().len(), 3);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn old_database_gets_persona_column() {
        let path = data_dir().join("old.db");
        assert!(path.starts_with(std::env::temp_dir()));
        std::fs::create_dir_all(data_dir()).unwrap();
        let _ = std::fs::remove_file(&path);
        let c = Connection::open(&path).unwrap();
        c.execute_batch(
            "CREATE TABLE messages(id INTEGER PRIMARY KEY, ts INTEGER NOT NULL, role TEXT NOT NULL, text TEXT NOT NULL);
             INSERT INTO messages(ts, role, text) VALUES (1, 'user', 'old');",
        )
        .unwrap();
        drop(c);
        assert_eq!(Memory::open(&path, "male").unwrap().recent(5).unwrap()[0].text, "old");
        assert!(Memory::open(&path, "female").unwrap().recent(5).unwrap().is_empty());
        let _ = std::fs::remove_file(&path);
    }
}
