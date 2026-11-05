//! Snapshot store. One published file, written via temp + fsync + atomic rename.
//! Readers and the writer do not overlap: shared flock for loads, exclusive for saves.
//! This module never opens `chisel.db`.

use crate::json::{self, Value};
use crate::lock::{LockGuard, ProcessLock};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
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
    loaded_generation: i64,
    /// Test hook: the next N saves fail as a stale generation and do not publish.
    pub fail_stale_remaining: i64,
    /// `(len, mtime_nanos)` of the snapshot last parsed. A matching stamp skips a re-parse.
    snapshot_stamp: Option<(u64, u128)>,
    units_by_id: HashMap<String, usize>,
    units_by_file: HashMap<String, Vec<usize>>,
    edges_by_code: HashMap<String, Vec<usize>>,
    churn_by_file: HashMap<String, Vec<usize>>,
    hash_by_file: HashMap<String, String>,
    blame_by_file: HashMap<String, Vec<usize>>,
    co_by_file: HashMap<String, Vec<usize>>,
}

impl Store {
    pub fn open(dir: &Path) -> Result<Self, StoreError> {
        reject_storage_dir(dir)?;
        fs::create_dir_all(dir).map_err(|e| StoreError(e.to_string()))?;
        let lock = ProcessLock::open(dir).map_err(|e| StoreError(e.to_string()))?;
        {
            // Only a crashed writer leaves store.tmp. A live writer holds this
            // exclusive lock across write and rename, so deleting here cannot
            // remove a temp file that is about to be published.
            let _guard = lock.exclusive().map_err(|e| StoreError(e.to_string()))?;
            let tmp = dir.join(TMP_NAME);
            if tmp.exists() {
                let _ = fs::remove_file(&tmp);
            }
        }
        let mut store = Store {
            dir: dir.to_path_buf(),
            lock,
            doc: empty_doc(),
            loaded_generation: 0,
            fail_stale_remaining: 0,
            snapshot_stamp: None,
            units_by_id: HashMap::new(),
            units_by_file: HashMap::new(),
            edges_by_code: HashMap::new(),
            churn_by_file: HashMap::new(),
            hash_by_file: HashMap::new(),
            blame_by_file: HashMap::new(),
            co_by_file: HashMap::new(),
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
        {
            let _guard = self.lock.shared().map_err(|e| StoreError(e.to_string()))?;
            let path = self.snapshot_path();
            let stamp = file_stamp(&path);
            if stamp.is_some() && self.snapshot_stamp == stamp {
                return Ok(());
            }
            self.doc = read_snapshot(&path)?;
            self.loaded_generation = self.doc.get("generation").and_then(|v| v.as_i64()).unwrap_or(0);
            self.snapshot_stamp = file_stamp(&path);
        }
        self.reindex();
        Ok(())
    }

    pub fn save(&mut self) -> Result<(), StoreError> {
        self.edit_and_save(|_| {})
    }

    /// Hold the exclusive lock from the in-memory edit through the rename.
    pub fn edit_and_save<F>(&mut self, edit: F) -> Result<(), StoreError>
    where
        F: FnOnce(&mut Value),
    {
        if self.fail_stale_remaining > 0 {
            self.fail_stale_remaining -= 1;
            return Err(StoreError("stale generation".into()));
        }
        {
            let _guard = self.lock.exclusive().map_err(|e| StoreError(e.to_string()))?;
            // The file this process loaded is still the one on disk (same mtime and size):
            // its generation is ours, with no need to parse it again.
            let path = self.snapshot_path();
            let disk_gen = if self.snapshot_stamp.is_some() && file_stamp(&path) == self.snapshot_stamp {
                self.loaded_generation
            } else {
                read_snapshot(&path)?.get("generation").and_then(|v| v.as_i64()).unwrap_or(0)
            };
            if disk_gen != self.loaded_generation {
                return Err(StoreError("stale generation".into()));
            }
            edit(&mut self.doc);
            let gen = disk_gen + 1;
            self.doc.insert("generation", Value::Int(gen));
            write_snapshot_file(&self.temp_path(), &self.doc)?;
            self.publish_temp()?;
            self.loaded_generation = gen;
            self.snapshot_stamp = file_stamp(&self.snapshot_path());
        }
        self.reindex();
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
        self.rows(key).to_vec()
    }

    pub fn rows(&self, key: &str) -> &[Value] {
        self.doc.get(key).and_then(|v| v.as_array()).unwrap_or(&[])
    }

    pub fn array_len(&self, key: &str) -> usize {
        self.rows(key).len()
    }

    pub fn units_by_file<'a>(&'a self, file: &str) -> Vec<&'a Value> {
        self.indexed("code_units", self.units_by_file.get(file))
    }

    pub fn unit_by_id<'a>(&'a self, id: &str) -> Option<&'a Value> {
        let idx = *self.units_by_id.get(id)?;
        self.rows("code_units").get(idx)
    }

    pub fn edges_for_code<'a>(&'a self, code_id: &str) -> Vec<&'a Value> {
        self.indexed("test_edges", self.edges_by_code.get(code_id))
    }

    pub fn churn_for_file<'a>(&'a self, file: &str) -> Vec<&'a Value> {
        self.indexed("churn", self.churn_by_file.get(file))
    }

    pub fn hash_for_file(&self, file: &str) -> String {
        self.hash_by_file.get(file).cloned().unwrap_or_default()
    }

    pub fn blame_for_file<'a>(&'a self, file: &str) -> Vec<&'a Value> {
        self.indexed("blame", self.blame_by_file.get(file))
    }

    pub fn co_for_file<'a>(&'a self, file: &str) -> Vec<&'a Value> {
        self.indexed("co_changes", self.co_by_file.get(file))
    }

    fn indexed<'a>(&'a self, key: &str, idxs: Option<&Vec<usize>>) -> Vec<&'a Value> {
        let Some(idxs) = idxs else { return Vec::new() };
        let rows = self.rows(key);
        idxs.iter().filter_map(|i| rows.get(*i)).collect()
    }

    pub fn reindex_memory(&mut self) {
        self.reindex();
    }

    fn reindex(&mut self) {
        self.units_by_id.clear();
        self.units_by_file.clear();
        self.edges_by_code.clear();
        self.churn_by_file.clear();
        self.hash_by_file.clear();
        self.blame_by_file.clear();
        self.co_by_file.clear();
        index_by(self.doc.get("code_units"), "id", "file_path", &mut self.units_by_id, &mut self.units_by_file);
        index_one(self.doc.get("test_edges"), "code_id", &mut self.edges_by_code);
        index_one(self.doc.get("churn"), "file_path", &mut self.churn_by_file);
        index_hashes(self.doc.get("file_hashes"), &mut self.hash_by_file);
        index_one(self.doc.get("blame"), "file_path", &mut self.blame_by_file);
        index_co(self.doc.get("co_changes"), &mut self.co_by_file);
    }
}

fn index_by(value: Option<&Value>, id_key: &str, file_key: &str, by_id: &mut HashMap<String, usize>, by_file: &mut HashMap<String, Vec<usize>>) {
    let Some(rows) = value.and_then(|v| v.as_array()) else { return };
    for (i, row) in rows.iter().enumerate() {
        if let Some(id) = row.get(id_key).and_then(|v| v.as_str()) {
            by_id.insert(id.to_string(), i);
        }
        if let Some(file) = row.get(file_key).and_then(|v| v.as_str()) {
            by_file.entry(file.to_string()).or_default().push(i);
        }
    }
}

fn index_one(value: Option<&Value>, key: &str, into: &mut HashMap<String, Vec<usize>>) {
    let Some(rows) = value.and_then(|v| v.as_array()) else { return };
    for (i, row) in rows.iter().enumerate() {
        if let Some(id) = row.get(key).and_then(|v| v.as_str()) {
            into.entry(id.to_string()).or_default().push(i);
        }
    }
}

fn index_hashes(value: Option<&Value>, into: &mut HashMap<String, String>) {
    let Some(rows) = value.and_then(|v| v.as_array()) else { return };
    for row in rows {
        if let (Some(file), Some(hash)) = (row.get("file_path").and_then(|v| v.as_str()), row.get("hash").and_then(|v| v.as_str())) {
            into.insert(file.to_string(), hash.to_string());
        }
    }
}

fn index_co(value: Option<&Value>, into: &mut HashMap<String, Vec<usize>>) {
    let Some(rows) = value.and_then(|v| v.as_array()) else { return };
    for (i, row) in rows.iter().enumerate() {
        let a = row.get("file_a").and_then(|v| v.as_str()).unwrap_or("");
        let b = row.get("file_b").and_then(|v| v.as_str()).unwrap_or("");
        if !a.is_empty() {
            into.entry(a.to_string()).or_default().push(i);
        }
        if !b.is_empty() && b != a {
            into.entry(b.to_string()).or_default().push(i);
        }
    }
}

fn file_stamp(path: &Path) -> Option<(u64, u128)> {
    let meta = fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    let nanos = modified.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
    Some((meta.len(), nanos))
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

/// Arrays written as compact tables: they are most of a large snapshot, and every row repeats
/// the same keys and long ids. `read_snapshot` expands them, so readers see plain rows.
const COMPACT_TABLES: [&str; 2] = ["test_edges", "blame"];

/// `{"table":1,"cols":[..],"dict":[..],"strings":[..],"rows":[[..],..]}`: string columns hold
/// indexes into `strings`. `None` when the rows do not all have the same keys in the same order.
fn compact_table(rows: &[Value]) -> Option<Value> {
    let first = rows.first()?.as_object()?;
    let cols: Vec<&str> = first.iter().map(|(k, _)| k.as_ref()).collect();
    let mut is_text = vec![true; cols.len()];
    for row in rows {
        let pairs = row.as_object()?;
        if pairs.len() != cols.len() || pairs.iter().zip(&cols).any(|((k, _), c)| *k != **c) {
            return None;
        }
        for (i, (_, v)) in pairs.iter().enumerate() {
            if v.as_str().is_none() {
                is_text[i] = false;
            }
        }
    }
    let mut strings: Vec<Value> = Vec::new();
    let mut index: std::collections::HashMap<&str, i64> = std::collections::HashMap::new();
    let mut out_rows = Vec::with_capacity(rows.len());
    for row in rows {
        let pairs = row.as_object()?;
        let cells = pairs
            .iter()
            .zip(&is_text)
            .map(|((_, v), text)| match (text, v.as_str()) {
                (true, Some(s)) => {
                    let next = index.len() as i64;
                    let id = *index.entry(s).or_insert_with(|| {
                        strings.push(Value::String(s.to_string()));
                        next
                    });
                    Value::Int(id)
                }
                _ => v.clone(),
            })
            .collect();
        out_rows.push(Value::Array(cells));
    }
    Some(Value::object(vec![
        ("table".into(), Value::Int(1)),
        ("cols".into(), Value::Array(cols.iter().map(|c| Value::String(c.to_string())).collect())),
        ("dict".into(), Value::Array(is_text.iter().map(|t| Value::Bool(*t)).collect())),
        ("strings".into(), Value::Array(strings)),
        ("rows".into(), Value::Array(out_rows)),
    ]))
}

fn expand_table(table: &Value) -> Option<Vec<Value>> {
    if table.get("table").and_then(|v| v.as_i64()) != Some(1) {
        return None;
    }
    let cols: Vec<json::SharedStr> = table.get("cols")?.as_array()?.iter().map(|c| c.as_str().map(json::SharedStr::from)).collect::<Option<_>>()?;
    let dict: Vec<bool> = table.get("dict")?.as_array()?.iter().map(|d| d.as_bool().unwrap_or(false)).collect();
    let strings = table.get("strings")?.as_array()?;
    let rows = table.get("rows")?.as_array()?;
    rows.iter()
        .map(|row| {
            let cells = row.as_array()?;
            let pairs = cols
                .iter()
                .zip(cells)
                .zip(&dict)
                .map(|((c, v), text)| {
                    let value = if *text { strings.get(v.as_i64()? as usize)?.clone() } else { v.clone() };
                    Some((c.clone(), value))
                })
                .collect::<Option<Vec<_>>>()?;
            Some(Value::Object(pairs))
        })
        .collect()
}

/// The snapshot text: compact tables for `COMPACT_TABLES`, plain JSON for the rest.
fn snapshot_text(doc: &Value) -> Result<String, StoreError> {
    let Some(pairs) = doc.as_object() else { return json::to_string(doc).map_err(|e| StoreError(e.message)) };
    let mut text = String::from("{");
    for (i, (k, v)) in pairs.iter().enumerate() {
        if i > 0 {
            text.push(',');
        }
        text.push_str(&json::to_string(&Value::String(k.to_string())).map_err(|e| StoreError(e.message))?);
        text.push(':');
        let compact = if COMPACT_TABLES.contains(&k.as_ref()) { v.as_array().and_then(compact_table) } else { None };
        text.push_str(&json::to_string(compact.as_ref().unwrap_or(v)).map_err(|e| StoreError(e.message))?);
    }
    text.push('}');
    if text.len() > json::MAX_INPUT {
        return Err(StoreError(format!("snapshot would be {} MiB, over the {} MiB limit Chisel can read back; analyze a directory or configure shards", text.len() >> 20, json::MAX_INPUT >> 20)));
    }
    Ok(text)
}

fn write_snapshot_file(path: &Path, doc: &Value) -> Result<(), StoreError> {
    let text = snapshot_text(doc)?;
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
    let mut doc = json::parse(&text).map_err(|e| StoreError(e.message))?;
    if doc.get("magic").and_then(|v| v.as_str()) != Some("chisel-store") {
        return Err(StoreError("snapshot magic mismatch".into()));
    }
    for key in COMPACT_TABLES {
        if let Some(rows) = doc.get(key).and_then(expand_table) {
            doc.insert(key, Value::Array(rows));
        }
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
    fn edges_and_blame_are_compact_on_disk_and_plain_in_memory() {
        let dir = scratch().join("compact");
        let mut store = Store::open(&dir).unwrap();
        let edge = |t: &str, c: &str, w: f64| Value::object(vec![
            ("test_id".into(), Value::String(t.into())),
            ("code_id".into(), Value::String(c.into())),
            ("edge_type".into(), Value::String("call".into())),
            ("weight".into(), Value::Float(w)),
        ]);
        let edges = vec![edge("tests/a.rs:t1", "src/a.rs:f:function", 1.0), edge("tests/a.rs:t2", "src/a.rs:f:function", 0.5), edge("tests/a.rs:t1", "src/b.rs:g:function", 0.8)];
        store.doc.insert("test_edges", Value::Array(edges.clone()));
        let blame = vec![Value::object(vec![
            ("file_path".into(), Value::String("src/a.rs".into())),
            ("line_start".into(), Value::Int(1)),
            ("author".into(), Value::String("Ann".into())),
        ])];
        store.doc.insert("blame", Value::Array(blame.clone()));
        store.save().unwrap();
        let raw = fs::read_to_string(store.snapshot_path()).unwrap();
        assert!(raw.contains(r#""test_edges":{"table":1,"cols":["test_id","code_id","edge_type","weight"]"#), "{raw}");
        assert_eq!(raw.matches("src/a.rs:f:function").count(), 1, "strings are stored once: {raw}");
        let again = Store::open(&dir).unwrap();
        assert_eq!(again.rows("test_edges"), edges.as_slice());
        assert_eq!(again.rows("blame"), blame.as_slice());
    }

    #[test]
    fn a_snapshot_over_the_old_32_mib_limit_reads_back() {
        let dir = scratch().join("big");
        let mut store = Store::open(&dir).unwrap();
        store.doc.insert("padding", Value::String("x".repeat(40 << 20)));
        store.save().unwrap();
        let again = Store::open(&dir).unwrap();
        assert_eq!(again.doc.get("padding").and_then(|v| v.as_str()).map(str::len), Some(40 << 20));
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
