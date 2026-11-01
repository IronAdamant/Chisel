//! Snapshot store. One published file, written via temp + fsync + atomic rename.
//! Readers and the writer do not overlap: shared flock for loads, exclusive for saves.
//! This module never opens `chisel.db`.

use crate::json::{self, Value};
use crate::lock::{LockGuard, ProcessLock};
use std::fs::{self, File, OpenOptions};
use std::io::{Write};
use std::path::{Path, PathBuf};

pub const STORE_NAME: &str = "store";
const TMP_NAME: &str = "store.tmp";

#[derive(Debug)]
pub struct StoreError(pub String);

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<StoreError> for String {
    fn from(value: StoreError) -> Self {
        value.0
    }
}

pub struct Store {
    dir: PathBuf,
    lock: ProcessLock,
    pub doc: Value,
}

impl Store {
    pub fn open(dir: &Path) -> Result<Self, StoreError> {
        reject_storage_dir(dir)?;
        fs::create_dir_all(dir).map_err(|e| StoreError(e.to_string()))?;
        let lock = ProcessLock::open(dir).map_err(|e| StoreError(e.to_string()))?;
        let mut store = Store {
            dir: dir.to_path_buf(),
            lock,
            doc: empty_doc(),
        };
        store.reload()?;
        Ok(store)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Published snapshot path. Never `chisel.db`.
    pub fn snapshot_path(&self) -> PathBuf {
        self.dir.join(STORE_NAME)
    }

    pub fn temp_path(&self) -> PathBuf {
        self.dir.join(TMP_NAME)
    }

    pub fn reload(&mut self) -> Result<(), StoreError> {
        let _guard = self.lock.shared().map_err(|e| StoreError(e.to_string()))?;
        self.doc = read_snapshot(&self.snapshot_path())?;
        Ok(())
    }

    pub fn save(&mut self) -> Result<(), StoreError> {
        let gen = self.doc.get("generation").and_then(|v| v.as_i64()).unwrap_or(0);
        self.doc.insert("generation", Value::Int(gen + 1));
        let guard = self.lock.exclusive().map_err(|e| StoreError(e.to_string()))?;
        write_snapshot_file(&self.temp_path(), &self.doc)?;
        self.publish_temp()?;
        drop(guard);
        Ok(())
    }

    /// Write `store.tmp` and fsync it, but do not rename. Used to prove a crash
    /// before publish leaves the previous snapshot loadable.
    pub fn write_temp_for_crash(&mut self) -> Result<(), StoreError> {
        let guard = self.lock.exclusive().map_err(|e| StoreError(e.to_string()))?;
        write_snapshot_file(&self.temp_path(), &self.doc)?;
        drop(guard);
        Ok(())
    }

    pub fn hold_exclusive_after_temp(&mut self) -> Result<LockGuard<'_>, StoreError> {
        let guard = self.lock.exclusive().map_err(|e| StoreError(e.to_string()))?;
        write_snapshot_file(&self.temp_path(), &self.doc)?;
        Ok(guard)
    }

    fn publish_temp(&self) -> Result<(), StoreError> {
        let tmp = self.temp_path();
        let dest = self.snapshot_path();
        fs::rename(&tmp, &dest).map_err(|e| StoreError(e.to_string()))?;
        if let Ok(dir) = File::open(&self.dir) {
            let _ = dir.sync_all();
        }
        Ok(())
    }

    pub fn shared_guard(&self) -> Result<LockGuard<'_>, StoreError> {
        self.lock.shared().map_err(|e| StoreError(e.to_string()))
    }

    pub fn array_mut(&mut self, key: &str) -> &mut Vec<Value> {
        if self.doc.get(key).and_then(|v| v.as_array()).is_none() {
            self.doc.insert(key, Value::Array(Vec::new()));
        }
        self.doc.get_mut(key).and_then(|v| v.as_array_mut()).unwrap()
    }

    pub fn array(&self, key: &str) -> Vec<Value> {
        self.doc.get(key).and_then(|v| v.as_array()).unwrap_or(&[]).to_vec()
    }
}

pub fn reject_storage_dir(dir: &Path) -> Result<(), StoreError> {
    let raw = dir.to_string_lossy();
    if raw == ":memory:" || raw.starts_with("file:") {
        return Err(StoreError(
            "storage directory must be a real directory, not a SQLite URI (:memory: or file:)".into(),
        ));
    }
    Ok(())
}

fn empty_doc() -> Value {
    Value::object(vec![
        ("magic".into(), Value::String("chisel-store".into())),
        ("version".into(), Value::Int(1)),
        ("generation".into(), Value::Int(0)),
        ("code_units".into(), Value::Array(vec![])),
        ("test_units".into(), Value::Array(vec![])),
        ("test_edges".into(), Value::Array(vec![])),
        ("commits".into(), Value::Array(vec![])),
        ("commit_files".into(), Value::Array(vec![])),
        ("blame".into(), Value::Array(vec![])),
        ("co_changes".into(), Value::Array(vec![])),
        ("branch_co_changes".into(), Value::Array(vec![])),
        ("churn".into(), Value::Array(vec![])),
        ("file_hashes".into(), Value::Array(vec![])),
        ("import_edges".into(), Value::Array(vec![])),
        ("test_results".into(), Value::Array(vec![])),
        ("meta".into(), Value::object(vec![])),
        ("file_locks".into(), Value::Array(vec![])),
        ("jobs".into(), Value::Array(vec![])),
        ("job_events".into(), Value::Array(vec![])),
        ("static_test_imports".into(), Value::Array(vec![])),
    ])
}

fn write_snapshot_file(path: &Path, doc: &Value) -> Result<(), StoreError> {
    let text = json::to_string(doc).map_err(|e| StoreError(e.message))?;
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .map_err(|e| StoreError(e.to_string()))?;
    file.write_all(text.as_bytes()).map_err(|e| StoreError(e.to_string()))?;
    file.sync_all().map_err(|e| StoreError(e.to_string()))?;
    Ok(())
}

fn read_snapshot(path: &Path) -> Result<Value, StoreError> {
    if !path.exists() {
        return Ok(empty_doc());
    }
    // Refuse to treat a sqlite file as our snapshot even if someone renames it.
    let bytes = fs::read(path).map_err(|e| StoreError(e.to_string()))?;
    if bytes.starts_with(b"SQLite format 3") {
        return Err(StoreError("refusing to read a SQLite database".into()));
    }
    let text = String::from_utf8_lossy(&bytes);
    let doc = json::parse(&text).map_err(|e| StoreError(e.message))?;
    if doc.get("magic").and_then(|v| v.as_str()) != Some("chisel-store") {
        return Err(StoreError("snapshot magic mismatch".into()));
    }
    Ok(doc)
}

pub fn meta_get(doc: &Value, key: &str) -> Option<String> {
    doc.get("meta")?.get(key)?.as_str().map(|s| s.to_string()).or_else(|| {
        doc.get("meta")?.get(key)?.as_i64().map(|i| i.to_string())
    })
}

pub fn meta_set(doc: &mut Value, key: &str, value: &str) {
    if doc.get("meta").is_none() {
        doc.insert("meta", Value::object(vec![]));
    }
    if let Some(meta) = doc.get_mut("meta") {
        meta.insert(key, Value::String(value.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::Value;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scratch() -> PathBuf {
        let n = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let p = std::env::temp_dir().join(format!("chisel-store-{n}"));
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn rejects_sqlite_uris_and_ignores_chisel_db() {
        assert!(reject_storage_dir(Path::new(":memory:")).is_err());
        assert!(reject_storage_dir(Path::new("file:mem")).is_err());
        let dir = scratch();
        fs::write(dir.join("chisel.db"), b"SQLite format 3\x00not our store").unwrap();
        let mut store = Store::open(&dir).unwrap();
        assert!(store.snapshot_path().ends_with("store"));
        assert!(!store.snapshot_path().to_string_lossy().ends_with("chisel.db"));
        store.doc.insert("token", Value::String("live".into()));
        store.save().unwrap();
        let again = Store::open(&dir).unwrap();
        assert_eq!(again.doc.get("token").and_then(|v| v.as_str()), Some("live"));
        let db = fs::read(dir.join("chisel.db")).unwrap();
        assert!(db.starts_with(b"SQLite format 3"));
    }

    #[test]
    fn temp_without_publish_keeps_previous_snapshot() {
        let dir = scratch();
        let mut store = Store::open(&dir).unwrap();
        store.doc.insert("token", Value::String("first".into()));
        store.save().unwrap();
        store.doc.insert("token", Value::String("second".into()));
        store.write_temp_for_crash().unwrap();
        assert!(dir.join("store.tmp").exists());
        let loaded = Store::open(&dir).unwrap();
        assert_eq!(loaded.doc.get("token").and_then(|v| v.as_str()), Some("first"));
        assert!(loaded.snapshot_path().ends_with("store"));
    }
}
