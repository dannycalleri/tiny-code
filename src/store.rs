use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexEntry {
    pub session_id: String,
    pub title: String,
    pub started_at: u64,
    pub updated_at: u64,
    pub cost_usd: Option<f64>,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn open(data_dir: &Path, cwd: &Path) -> Store {
        Store {
            dir: data_dir.join("sessions").join(dir_key(cwd)),
        }
    }

    pub fn default_data_dir() -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", "turbo-harness").map(|d| d.data_dir().to_path_buf())
    }

    pub fn log_path(&self, session_id: &str) -> PathBuf {
        self.dir.join(format!("{session_id}.jsonl"))
    }

    pub fn history(&self) -> Vec<IndexEntry> {
        let mut entries = self.index();
        entries.sort_by_key(|e| std::cmp::Reverse(e.updated_at));
        entries
    }

    pub fn index(&self) -> Vec<IndexEntry> {
        fs::read(self.dir.join("index.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save_entry(&self, entry: IndexEntry) -> io::Result<()> {
        let mut entries = self.index();
        upsert(&mut entries, entry);
        fs::create_dir_all(&self.dir)?;
        let tmp = self.dir.join("index.json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(&entries)?)?;
        fs::rename(tmp, self.dir.join("index.json"))
    }

    pub fn read_log(&self, session_id: &str) -> Vec<String> {
        let Ok(text) = fs::read_to_string(self.log_path(session_id)) else {
            return Vec::new();
        };
        text.lines()
            .filter(|line| serde_json::from_str::<serde_json::Value>(line).is_ok())
            .map(String::from)
            .collect()
    }

    pub fn log_writer(&self, session_id: Option<&str>) -> LogWriter {
        let mut writer = LogWriter {
            store: self.clone(),
            pending: Vec::new(),
            file: None,
        };
        if let Some(id) = session_id {
            writer.set_session(id);
        }
        writer
    }
}

pub fn dir_key(cwd: &Path) -> String {
    Sha256::digest(cwd.as_os_str().as_bytes())
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn prompt_line(prompt: &str, at: u64) -> String {
    serde_json::json!({"type": "turbo_prompt", "prompt": prompt, "at": at}).to_string()
}

pub fn exit_line(code: Option<i32>, stderr: &[String], stopped: bool) -> String {
    serde_json::json!({"type": "turbo_exit", "code": code, "stderr": stderr, "stopped": stopped})
        .to_string()
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn upsert(entries: &mut Vec<IndexEntry>, entry: IndexEntry) {
    match entries
        .iter_mut()
        .find(|e| e.session_id == entry.session_id)
    {
        Some(existing) => {
            *existing = IndexEntry {
                title: std::mem::take(&mut existing.title),
                started_at: existing.started_at,
                ..entry
            }
        }
        None => entries.push(entry),
    }
}

#[derive(Debug)]
pub struct LogWriter {
    store: Store,
    pending: Vec<String>,
    file: Option<File>,
}

impl LogWriter {
    pub fn has_session(&self) -> bool {
        self.file.is_some()
    }

    pub fn set_session(&mut self, session_id: &str) {
        let file = fs::create_dir_all(&self.store.dir).and_then(|_| {
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.store.log_path(session_id))
        });
        self.file = file.ok();
        for line in std::mem::take(&mut self.pending) {
            self.write(&line);
        }
    }

    pub fn write(&mut self, line: &str) {
        match &mut self.file {
            Some(file) => {
                let _ = writeln!(file, "{line}");
            }
            None => self.pending.push(line.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(name: &str) -> Store {
        let root =
            std::env::temp_dir().join(format!("turbo-harness-store-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        Store::open(&root, Path::new("/home/user/demo"))
    }

    fn entry(id: &str, title: &str, at: u64, cost: f64) -> IndexEntry {
        IndexEntry {
            session_id: id.into(),
            title: title.into(),
            started_at: at,
            updated_at: at,
            cost_usd: Some(cost),
            status: "done".into(),
        }
    }

    #[test]
    fn the_same_directory_gives_the_same_key() {
        let key = dir_key(Path::new("/home/user/demo"));
        assert_eq!(key, dir_key(Path::new("/home/user/demo")));
        assert_eq!(key.len(), 16);
        assert!(key.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn two_directories_give_two_keys() {
        assert_ne!(dir_key(Path::new("/a")), dir_key(Path::new("/b")));
    }

    #[test]
    fn an_index_update_adds_a_new_session() {
        let mut entries = vec![entry("a", "first", 1, 0.1)];
        upsert(&mut entries, entry("b", "second", 2, 0.2));
        assert_eq!(
            entries,
            [entry("a", "first", 1, 0.1), entry("b", "second", 2, 0.2)]
        );
    }

    #[test]
    fn an_index_update_changes_one_session_and_keeps_its_title_and_start() {
        let mut entries = vec![entry("a", "first", 1, 0.1), entry("b", "second", 2, 0.2)];
        upsert(&mut entries, entry("a", "follow-up", 9, 0.5));
        let mut changed = entry("a", "first", 1, 0.5);
        changed.updated_at = 9;
        assert_eq!(entries, [changed, entry("b", "second", 2, 0.2)]);
    }

    #[test]
    fn save_entry_writes_an_index_that_reads_back() {
        let store = temp_store("index");
        assert!(store.index().is_empty());
        store.save_entry(entry("a", "first", 1, 0.1)).unwrap();
        store.save_entry(entry("b", "second", 2, 0.2)).unwrap();
        assert_eq!(store.index().len(), 2);
        assert!(!store.dir.join("index.json.tmp").exists());
    }

    #[test]
    fn history_lists_the_sessions_newest_first() {
        let store = temp_store("history");
        for (id, at) in [("old", 1), ("new", 3), ("mid", 2)] {
            store.save_entry(entry(id, id, at, 0.1)).unwrap();
        }
        let ids: Vec<String> = store.history().into_iter().map(|e| e.session_id).collect();
        assert_eq!(ids, ["new", "mid", "old"]);
    }

    #[test]
    fn a_damaged_log_line_is_skipped_on_load() {
        let store = temp_store("damaged");
        fs::create_dir_all(&store.dir).unwrap();
        fs::write(
            store.log_path("s"),
            "{\"type\":\"a\"}\n{\"type\":\"b\n{\"type\":\"c\"}\n",
        )
        .unwrap();
        assert_eq!(
            store.read_log("s"),
            ["{\"type\":\"a\"}", "{\"type\":\"c\"}"]
        );
    }

    #[test]
    fn lines_wait_in_memory_until_the_session_id_is_known() {
        let store = temp_store("pending");
        let mut log = store.log_writer(None);
        log.write("{\"n\":1}");
        assert!(!log.has_session());
        assert!(!store.dir.exists());
        log.set_session("s");
        log.write("{\"n\":2}");
        assert_eq!(store.read_log("s"), ["{\"n\":1}", "{\"n\":2}"]);
    }

    #[test]
    fn a_known_session_appends_to_its_log() {
        let store = temp_store("append");
        store.log_writer(Some("s")).write("{\"n\":1}");
        store.log_writer(Some("s")).write("{\"n\":2}");
        assert_eq!(store.read_log("s"), ["{\"n\":1}", "{\"n\":2}"]);
    }
}
