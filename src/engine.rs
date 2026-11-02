//! Analysis engine. Same tool names and result shapes as the Python package.
//! The snapshot store is the only database. `chisel.db` is never opened.

use crate::extract::{self, CodeUnit, Dep};
use crate::git::{self, GitAnalyzer};
use crate::hash::sha256_hex;
use crate::impact::{self, RiskInputs};
use crate::json::Value;
use crate::store::{self, Store};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

struct LiveJob {
    id: String,
    status: String,
    progress: i64,
    cancel: bool,
    project: PathBuf,
    storage: PathBuf,
    kind: String,
}

fn job_lock() -> &'static Mutex<Vec<LiveJob>> {
    static JOBS: Mutex<Vec<LiveJob>> = Mutex::new(Vec::new());
    &JOBS
}

fn shard_name_for<'a>(file: &str, shards: &'a [(String, String)]) -> Option<&'a str> {
    let mut best: Option<&str> = None;
    let mut best_len = 0usize;
    for (name, prefix) in shards {
        if file == prefix.as_str() || file.starts_with(&format!("{prefix}/")) {
            if prefix.len() >= best_len {
                best = Some(name.as_str());
                best_len = prefix.len();
            }
        }
    }
    best
}

fn parse_shard_spec(spec: &str, project: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for part in spec.split(',') {
        let part = part.trim().trim_matches('/');
        if part.is_empty() {
            continue;
        }
        let (name, prefix) = if let Some((name, prefix)) = part.split_once(':') {
            (name.trim().trim_matches('/').to_string(), prefix.trim().trim_matches('/').to_string())
        } else {
            (part.to_string(), part.to_string())
        };
        if let Some(base) = prefix.strip_suffix("/*") {
            let base = base.trim_matches('/');
            let dir = if base.is_empty() { project.to_path_buf() } else { project.join(base) };
            let mut kids = Vec::new();
            if let Ok(rd) = fs::read_dir(&dir) {
                for ent in rd.flatten() {
                    if !ent.path().is_dir() {
                        continue;
                    }
                    let child = ent.file_name().to_string_lossy().to_string();
                    if child.starts_with('.') {
                        continue;
                    }
                    let rel = if base.is_empty() { child } else { format!("{base}/{child}") };
                    kids.push(rel);
                }
            }
            kids.sort();
            for rel in kids {
                out.push((rel.clone(), rel));
            }
            let _ = name;
        } else if !prefix.is_empty() {
            out.push((name, prefix));
        }
    }
    out
}

fn shards_from_toml(text: &str) -> Option<String> {
    let start = text.find('[')?;
    let end = text[start + 1..].find(']')? + start + 1;
    if !text[..start].contains("shards") {
        return None;
    }
    let mut parts = Vec::new();
    for raw in text[start + 1..end].split(',') {
        let part = raw.trim().trim_matches(|c| c == '"' || c == '\'' || c == ' ');
        if !part.is_empty() {
            parts.push(part.to_string());
        }
    }
    if parts.is_empty() { None } else { Some(parts.join(",")) }
}

fn load_shard_config(project: &Path) -> Vec<(String, String)> {
    if let Ok(spec) = std::env::var("CHISEL_SHARDS") {
        if !spec.trim().is_empty() {
            return parse_shard_spec(&spec, project);
        }
    }
    let path = project.join(".chisel").join("shards.toml");
    if let Ok(text) = fs::read_to_string(path) {
        if let Some(spec) = shards_from_toml(&text) {
            return parse_shard_spec(&spec, project);
        }
    }
    Vec::new()
}

fn job_should_stop(id: &str) -> bool {
    job_lock().lock().ok().and_then(|jobs| {
        jobs.iter().find(|j| j.id == id).map(|j| j.cancel || j.status == "cancelled")
    }).unwrap_or(false)
}

fn set_job(id: &str, status: &str, progress: i64) {
    if let Ok(mut jobs) = job_lock().lock() {
        if let Some(job) = jobs.iter_mut().find(|j| j.id == id) {
            job.status = status.to_string();
            job.progress = progress;
        }
    }
}

const NO_DATA_MSG: &str = "No analysis data. Run 'chisel analyze' on this project first.";

fn missing_arg(tool: &str, field: &str) -> Value {
    let mut value = Value::object(vec![
        ("status".into(), Value::String("error".into())),
        ("message".into(), Value::String(format!("{tool} requires {field}"))),
    ]);
    if tool == "suggest_tests" && field == "file_path" {
        value.insert("hint", Value::String("Pass the source file to rank tests for.".into()));
    }
    value
}

fn argument_missing(args: &Value, field: &str, ty: &str) -> bool {
    let Some(value) = args.get(field) else { return true };
    match value {
        Value::Null => true,
        Value::String(text) if text.is_empty() => true,
        Value::Array(items) if items.is_empty() => true,
        _ if ty == "boolean" => !matches!(value, Value::Bool(_)),
        _ => false,
    }
}

fn first_missing_required(name: &str, args: &Value) -> Option<String> {
    for field in crate::schemas::required_fields(name) {
        let ty = crate::schemas::field_type(name, &field).unwrap_or_default();
        if argument_missing(args, &field, &ty) {
            return Some(field);
        }
    }
    None
}

enum AutoPlan {
    Off,
    Run,
    Skip(&'static str),
}

pub struct Engine {
    pub project_dir: PathBuf,
    pub store: Store,
    git: GitAnalyzer,
    /// When set, `working_tree` reads are allowed. Default false.
    pub include_ignored: bool,
    pub working_tree: bool,
    pub shards: Vec<(String, String)>,
    only_files: Option<BTreeSet<String>>,
    active_job: Option<String>,
    diff_ref_override: Option<String>,
    directory: Option<String>,
    pub source_reads: u64,
}

impl Engine {
    pub fn open(project_dir: &Path, storage_dir: Option<&Path>) -> Result<Self, String> {
        let project_dir = fs::canonicalize(project_dir).unwrap_or_else(|_| project_dir.to_path_buf());
        let storage = match storage_dir {
            Some(p) => p.to_path_buf(),
            None => project_dir.join(".chisel"),
        };
        let store = Store::open(&storage)?;
        let git = GitAnalyzer::new(project_dir.to_string_lossy().to_string());
        let include_ignored = std::env::var("CHISEL_INCLUDE_IGNORED").ok().as_deref() == Some("1");
        let mut engine = Engine {
            project_dir,
            store,
            git,
            include_ignored,
            working_tree: false,
            shards: Vec::new(),
            only_files: None,
            active_job: None,
            diff_ref_override: None,
            directory: None,
            source_reads: 0,
        };
        engine.shards = load_shard_config(&engine.project_dir);
        Ok(engine)
    }

    pub fn set_shards(&mut self, shards: Vec<(String, String)>) {
        self.shards = shards;
    }

    pub fn set_git_runner(&mut self, runner: git::GitRunner) {
        self.git.set_runner(runner);
    }

    pub fn analyze(&mut self) -> Result<Value, String> {
        if !self.shards.is_empty() && self.only_files.is_none() {
            return self.analyze_all_shards();
        }
        if let Some(id) = self.active_job.clone() {
            if let Ok(ms) = std::env::var("CHISEL_JOB_PACE_MS") {
                if let Ok(n) = ms.parse::<u64>() {
                    thread::sleep(Duration::from_millis(n));
                }
            }
            if job_should_stop(&id) {
                set_job(&id, "cancelled", 0);
                return Ok(Value::object(vec![("status".into(), Value::String("cancelled".into()))]));
            }
        }
        let git = self.git.fork();
        let files = self.scan_code_files(&git);
        let commits = git.parse_log().unwrap_or_default();
        let mut units_by_file: BTreeMap<String, Vec<CodeUnit>> = BTreeMap::new();
        let mut all_units = Vec::new();
        for (index, rel) in files.iter().enumerate() {
            if let Some(id) = self.active_job.clone() {
                let pct = ((index as i64) * 100 / (files.len() as i64).max(1)).min(99);
                set_job(&id, "running", pct);
                if let Ok(ms) = std::env::var("CHISEL_JOB_PACE_MS") {
                    if let Ok(n) = ms.parse::<u64>() {
                        thread::sleep(Duration::from_millis(n));
                    }
                }
                if job_should_stop(&id) {
                    set_job(&id, "cancelled", pct);
                    return Ok(Value::object(vec![
                        ("status".into(), Value::String("cancelled".into())),
                        ("progress_pct".into(), Value::Int(pct)),
                    ]));
                }
            }
            let abs = self.project_dir.join(rel);
            let content = fs::read(&abs).unwrap_or_default();
            let text = String::from_utf8_lossy(&content);
            let units = extract::extract_code_units(rel, &text);
            let hash = sha256_hex(&content);
            for u in &units {
                all_units.push(unit_obj(rel, u, &hash));
            }
            units_by_file.insert(rel.clone(), units);
            let meta = fs::metadata(&abs).ok();
            let mtime = meta.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs_f64()).unwrap_or(0.0);
            let size = meta.map(|m| m.len() as i64).unwrap_or(0);
            // stored below
            let _ = (mtime, size, hash);
        }
        self.store.doc.insert("code_units", Value::Array(all_units));

        let mut commit_rows = Vec::new();
        let mut file_rows = Vec::new();
        for c in &commits {
            commit_rows.push(Value::object(vec![
                ("hash".into(), Value::String(c.hash.clone())),
                ("author".into(), Value::String(c.author.clone())),
                ("author_email".into(), Value::String(c.author_email.clone())),
                ("date".into(), Value::String(c.date.clone())),
                ("message".into(), Value::String(c.message.clone())),
            ]));
            for f in &c.files {
                file_rows.push(Value::object(vec![
                    ("hash".into(), Value::String(c.hash.clone())),
                    ("path".into(), Value::String(f.path.clone())),
                    ("insertions".into(), Value::Int(f.insertions)),
                    ("deletions".into(), Value::Int(f.deletions)),
                    ("date".into(), Value::String(c.date.clone())),
                    ("author".into(), Value::String(c.author.clone())),
                ]));
            }
        }
        self.store.doc.insert("commits", Value::Array(commit_rows));
        self.store.doc.insert("commit_files", Value::Array(file_rows));

        let now = impact::now_unix();
        let mut churn_rows = Vec::new();
        for rel in &files {
            let dates: Vec<String> = commits.iter().filter(|c| c.files.iter().any(|f| &f.path == rel)).map(|c| c.date.clone()).collect();
            let mut ins = 0i64;
            let mut del = 0i64;
            let mut authors = BTreeSet::new();
            let mut last = String::new();
            let mut count = 0i64;
            for c in &commits {
                if let Some(f) = c.files.iter().find(|f| &f.path == rel) {
                    if impact::parse_iso_unix(&c.date).is_some() {
                        count += 1;
                        authors.insert(c.author.clone());
                        ins += f.insertions;
                        del += f.deletions;
                        if last.is_empty() || c.date > last {
                            last = c.date.clone();
                        }
                    }
                }
            }
            let score = impact::churn_score(&dates, now);
            churn_rows.push(churn_row(rel, "", count, authors.len() as i64, ins, del, &last, score));
            if files.len() > 2000 {
                continue;
            }
            if let Some(units) = units_by_file.get(rel) {
                for unit in units {
                    if unit.unit_type != "function" && unit.unit_type != "async_function" {
                        continue;
                    }
                    let bare = unit.name.rsplit('.').next().unwrap_or(&unit.name);
                    let Ok(func_commits) = git.function_log(rel, bare) else { continue };
                    if func_commits.is_empty() {
                        continue;
                    }
                    let func_dates: Vec<String> = func_commits.iter().map(|c| c.date.clone()).collect();
                    let func_score = impact::churn_score(&func_dates, now);
                    let func_count = func_dates.iter().filter(|d| impact::parse_iso_unix(d).is_some()).count() as i64;
                    churn_rows.push(churn_row(rel, &unit.name, func_count, 1, 0, 0, "", func_score));
                }
            }
        }
        self.store.doc.insert("churn", Value::Array(churn_rows));

        let distinct_authors = commits.iter().map(|c| c.author.as_str()).collect::<BTreeSet<_>>().len();
        let mut adaptive = impact::coupling_threshold(commits.len() as i64);
        if distinct_authors <= 1 {
            adaptive = (adaptive / 2).max(1);
        }
        store::meta_set(&mut self.store.doc, "co_change_query_min", &adaptive.to_string());
        store::meta_set(&mut self.store.doc, "distinct_authors", &distinct_authors.to_string());
        self.store.doc.insert("co_changes", Value::Array(co_changes_of(&commits, adaptive)));
        self.store.doc.insert("branch_co_changes", Value::Array(vec![]));
        let branch = git.current_branch().unwrap_or_default();
        let branch_commits = if branch == "main" || branch == "master" { 0 } else { 0 };
        store::meta_set(&mut self.store.doc, "branch_coupling_commits", &branch_commits.to_string());

        let mut blame_rows = Vec::new();
        let mut hash_rows = Vec::new();
        for rel in &files {
            let abs = self.project_dir.join(rel);
            let content = fs::read(&abs).unwrap_or_default();
            let hash = sha256_hex(&content);
            let meta = fs::metadata(&abs).ok();
            let mtime = meta.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs_f64()).unwrap_or(0.0);
            let size = meta.map(|m| m.len() as i64).unwrap_or(0);
            hash_rows.push(Value::object(vec![
                ("file_path".into(), Value::String(rel.clone())),
                ("hash".into(), Value::String(hash.clone())),
                ("mtime".into(), Value::Float(mtime)),
                ("size".into(), Value::Int(size)),
            ]));
            if let Ok(blocks) = git.blame(rel) {
                for b in blocks {
                    blame_rows.push(Value::object(vec![
                        ("file_path".into(), Value::String(rel.clone())),
                        ("content_hash".into(), Value::String(hash.clone())),
                        ("line_start".into(), Value::Int(b.line_start as i64)),
                        ("line_end".into(), Value::Int(b.line_end as i64)),
                        ("author".into(), Value::String(b.author)),
                        ("author_email".into(), Value::String(b.author_email)),
                        ("commit_hash".into(), Value::String(b.commit_hash)),
                    ]));
                }
            }
        }
        self.store.doc.insert("blame", Value::Array(blame_rows));
        self.store.doc.insert("file_hashes", Value::Array(hash_rows));

        let (test_units, edges, static_rows) = self.build_tests_and_edges(&files, &units_by_file);
        self.store.doc.insert("test_units", Value::Array(test_units));
        self.store.doc.insert("test_edges", Value::Array(edges));
        self.store.doc.insert("static_test_imports", Value::Array(static_rows));
        let imports = self.build_import_edges(&files);
        self.store.doc.insert("import_edges", Value::Array(imports));
        store::meta_set(&mut self.store.doc, "project_fingerprint", &self.project_dir.to_string_lossy());

        let test_files = self.test_file_set().len() as i64;
        let test_units_n = self.store.array("test_units").len() as i64;
        let edge_n = self.store.array("test_edges").len() as i64;
        let unit_n = self.store.array("code_units").len() as i64;
        if let Some(latest) = commits.iter().map(|c| c.date.as_str()).max() {
            store::meta_set(&mut self.store.doc, "last_commit_date", latest);
        }
        self.store.save()?;
        let mut summary = vec![
            ("code_files_scanned".into(), Value::Int(files.len() as i64)),
            ("code_units_found".into(), Value::Int(unit_n)),
            ("test_files_found".into(), Value::Int(test_files)),
            ("test_units_found".into(), Value::Int(test_units_n)),
            ("test_edges_built".into(), Value::Int(edge_n)),
            ("commits_parsed".into(), Value::Int(commits.len() as i64)),
            ("orphaned_results_cleaned".into(), Value::Int(0)),
            ("active_file_locks".into(), Value::Int(self.store.array("file_locks").len() as i64)),
            ("hint".into(), Value::String("For large repos, use start_job with kind='analyze' to avoid MCP timeouts.".into())),
        ];
        if files.len() > 2000 {
            summary.push(("unit_churn_skipped".into(), Value::Bool(true)));
        }
        Ok(Value::object(summary))
    }

    pub fn update(&mut self) -> Result<Value, String> {
        if !self.shards.is_empty() && self.only_files.is_none() {
            return self.update_all_shards();
        }
        let git = self.git.fork();
        let files = self.scan_code_files(&git);
        let before: BTreeMap<String, String> = self.store.array("file_hashes").into_iter().filter_map(|row| {
            Some((row.get("file_path")?.as_str()?.to_string(), row.get("hash")?.as_str()?.to_string()))
        }).collect();
        let mut changed: Vec<String> = Vec::new();
        for rel in &files {
            let abs = self.project_dir.join(rel);
            let content = fs::read(&abs).unwrap_or_default();
            let hash = sha256_hex(&content);
            if before.get(rel).map(|h| h.as_str()) != Some(hash.as_str()) {
                changed.push(rel.clone());
            }
        }
        let since = store::meta_get(&self.store.doc, "last_commit_date");
        let known: BTreeSet<String> = self.store.array("commits").iter().filter_map(|c| c.get("hash").and_then(|v| v.as_str()).map(|s| s.to_string())).collect();
        let new_commits: Vec<git::Commit> = git.parse_log_since(since.as_deref()).unwrap_or_default().into_iter().filter(|c| !known.contains(&c.hash)).collect();
        let commit_touch: BTreeSet<String> = new_commits.iter().flat_map(|c| c.files.iter().map(|f| f.path.clone())).collect();
        if changed.is_empty() && new_commits.is_empty() {
            return Ok(Value::object(vec![
                ("files_updated".into(), Value::Int(0)),
                ("code_units_found".into(), Value::Int(self.store.array("code_units").len() as i64)),
                ("new_commits".into(), Value::Int(0)),
                ("edge_rebuild_skipped".into(), Value::Bool(true)),
                ("hint".into(), Value::String("For large repos, use start_job with kind='update' to avoid MCP timeouts.".into())),
            ]));
        }
        // Changed files and new commits only. Untouched files keep their units, blame, and edges.
        let summary = self.analyze_subset(&git, &files, &changed, &new_commits, &commit_touch)?;
        Ok(summary)
    }

    fn analyze_subset(
        &mut self,
        git: &GitAnalyzer,
        files: &[String],
        changed: &[String],
        new_commits: &[git::Commit],
        _commit_touch: &BTreeSet<String>,
    ) -> Result<Value, String> {
        let changed_set: BTreeSet<&str> = changed.iter().map(|s| s.as_str()).collect();
        let mut units: Vec<Value> = self.store.array("code_units").into_iter().filter(|u| {
            u.get("file_path").and_then(|v| v.as_str()).is_none_or(|p| !changed_set.contains(p))
        }).collect();
        let mut units_by_file: BTreeMap<String, Vec<CodeUnit>> = BTreeMap::new();
        for rel in changed {
            let abs = self.project_dir.join(rel);
            let content = fs::read(&abs).unwrap_or_default();
            let text = String::from_utf8_lossy(&content);
            let extracted = extract::extract_code_units(rel, &text);
            let hash = sha256_hex(&content);
            for u in &extracted {
                units.push(unit_obj(rel, u, &hash));
            }
            units_by_file.insert(rel.clone(), extracted);
        }
        self.store.doc.insert("code_units", Value::Array(units));

        if !new_commits.is_empty() {
            let mut commit_rows = self.store.array("commits");
            let mut file_rows = self.store.array("commit_files");
            for c in new_commits {
                if commit_rows.iter().any(|row| row.get("hash").and_then(|v| v.as_str()) == Some(c.hash.as_str())) {
                    continue;
                }
                commit_rows.push(Value::object(vec![
                    ("hash".into(), Value::String(c.hash.clone())),
                    ("author".into(), Value::String(c.author.clone())),
                    ("author_email".into(), Value::String(c.author_email.clone())),
                    ("date".into(), Value::String(c.date.clone())),
                    ("message".into(), Value::String(c.message.clone())),
                ]));
                for f in &c.files {
                    file_rows.push(Value::object(vec![
                        ("hash".into(), Value::String(c.hash.clone())),
                        ("path".into(), Value::String(f.path.clone())),
                        ("insertions".into(), Value::Int(f.insertions)),
                        ("deletions".into(), Value::Int(f.deletions)),
                        ("date".into(), Value::String(c.date.clone())),
                        ("author".into(), Value::String(c.author.clone())),
                    ]));
                }
            }
            self.store.doc.insert("commits", Value::Array(commit_rows));
            self.store.doc.insert("commit_files", Value::Array(file_rows));
            if let Some(latest) = new_commits.iter().map(|c| c.date.as_str()).max() {
                store::meta_set(&mut self.store.doc, "last_commit_date", latest);
            }
            let all_commits = commits_from_store(&self.store.doc);
            let authors = all_commits.iter().map(|c| c.author.as_str()).collect::<BTreeSet<_>>().len();
            let mut adaptive = impact::coupling_threshold(all_commits.len() as i64);
            if authors <= 1 {
                adaptive = (adaptive / 2).max(1);
            }
            store::meta_set(&mut self.store.doc, "co_change_query_min", &adaptive.to_string());
            store::meta_set(&mut self.store.doc, "distinct_authors", &authors.to_string());
            self.store.doc.insert("co_changes", Value::Array(co_changes_of(&all_commits, adaptive)));
        }

        let skip_unit = files.len() > 2000;
        let mut scope = changed.to_vec();
        for path in _commit_touch {
            if files.iter().any(|f| f == path) && !scope.iter().any(|f| f == path) {
                scope.push(path.clone());
            }
        }
        let mut churn_rows: Vec<Value> = self.store.array("churn").into_iter().filter(|row| {
            let file = row.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
            let unit = row.get("unit_name").and_then(|v| v.as_str()).unwrap_or("");
            !(scope.iter().any(|s| s == file) && unit.is_empty())
        }).collect();
        let now = impact::now_unix();
        let commit_files = self.store.array("commit_files");
        for rel in &scope {
            let mut dates = Vec::new();
            let mut ins = 0i64;
            let mut del = 0i64;
            let mut authors = BTreeSet::new();
            let mut last = String::new();
            let mut count = 0i64;
            for row in &commit_files {
                if row.get("path").and_then(|v| v.as_str()) != Some(rel.as_str()) {
                    continue;
                }
                let date = row.get("date").and_then(|v| v.as_str()).unwrap_or("");
                if impact::parse_iso_unix(date).is_some() {
                    count += 1;
                    dates.push(date.to_string());
                    if let Some(a) = row.get("author").and_then(|v| v.as_str()) {
                        authors.insert(a.to_string());
                    }
                    ins += row.get("insertions").and_then(|v| v.as_i64()).unwrap_or(0);
                    del += row.get("deletions").and_then(|v| v.as_i64()).unwrap_or(0);
                    if last.is_empty() || date > last.as_str() {
                        last = date.to_string();
                    }
                }
            }
            let score = impact::churn_score(&dates, now);
            churn_rows.push(churn_row(rel, "", count, authors.len() as i64, ins, del, &last, score));
            if skip_unit {
                continue;
            }
            if let Some(extracted) = units_by_file.get(rel) {
                for unit in extracted {
                    if unit.unit_type != "function" && unit.unit_type != "async_function" {
                        continue;
                    }
                    let bare = unit.name.rsplit('.').next().unwrap_or(&unit.name);
                    let Ok(func_commits) = git.function_log(rel, bare) else { continue };
                    if func_commits.is_empty() {
                        continue;
                    }
                    let func_dates: Vec<String> = func_commits.iter().map(|c| c.date.clone()).collect();
                    let func_score = impact::churn_score(&func_dates, now);
                    let func_count = func_dates.iter().filter(|d| impact::parse_iso_unix(d).is_some()).count() as i64;
                    churn_rows.push(churn_row(rel, &unit.name, func_count, 1, 0, 0, "", func_score));
                }
            }
        }
        self.store.doc.insert("churn", Value::Array(churn_rows));

        let mut blame_rows: Vec<Value> = self.store.array("blame").into_iter().filter(|row| {
            row.get("file_path").and_then(|v| v.as_str()).is_none_or(|p| !changed_set.contains(p))
        }).collect();
        let mut hash_rows: Vec<Value> = self.store.array("file_hashes").into_iter().filter(|row| {
            row.get("file_path").and_then(|v| v.as_str()).is_none_or(|p| !changed_set.contains(p))
        }).collect();
        for rel in &scope {
            let abs = self.project_dir.join(rel);
            let content = fs::read(&abs).unwrap_or_default();
            let hash = sha256_hex(&content);
            hash_rows.push(Value::object(vec![
                ("file_path".into(), Value::String(rel.clone())),
                ("hash".into(), Value::String(hash.clone())),
                ("mtime".into(), Value::Float(0.0)),
                ("size".into(), Value::Int(content.len() as i64)),
            ]));
            if let Ok(blocks) = git.blame(rel) {
                for b in blocks {
                    blame_rows.push(Value::object(vec![
                        ("file_path".into(), Value::String(rel.clone())),
                        ("content_hash".into(), Value::String(hash.clone())),
                        ("line_start".into(), Value::Int(b.line_start as i64)),
                        ("line_end".into(), Value::Int(b.line_end as i64)),
                        ("author".into(), Value::String(b.author)),
                        ("author_email".into(), Value::String(b.author_email)),
                        ("commit_hash".into(), Value::String(b.commit_hash)),
                    ]));
                }
            }
        }
        self.store.doc.insert("blame", Value::Array(blame_rows));
        self.store.doc.insert("file_hashes", Value::Array(hash_rows));

        let changed_tests: Vec<String> = changed.iter().filter(|p| looks_like_test(p)).cloned().collect();
        let edge_rebuild_skipped = changed_tests.is_empty();
        if edge_rebuild_skipped {
            // Untouched tests keep their edges. A source-only edit does not re-read them.
        } else {
            for rel in &changed_tests {
                if !units_by_file.contains_key(rel) {
                    let content = fs::read_to_string(self.project_dir.join(rel)).unwrap_or_default();
                    self.source_reads += 1;
                    units_by_file.insert(rel.clone(), extract::extract_code_units(rel, &content));
                }
            }
            let (fresh_units, fresh_edges, fresh_static) = self.build_tests_and_edges(&changed_tests, &units_by_file);
            let mut test_units: Vec<Value> = self.store.array("test_units").into_iter().filter(|u| {
                u.get("file_path").and_then(|v| v.as_str()).is_none_or(|p| !changed_tests.iter().any(|c| c == p))
            }).collect();
            test_units.extend(fresh_units);
            let mut edges: Vec<Value> = self.store.array("test_edges").into_iter().filter(|e| {
                let tid = e.get("test_id").and_then(|v| v.as_str()).unwrap_or("");
                !changed_tests.iter().any(|c| tid.starts_with(&format!("{c}:")))
            }).collect();
            edges.extend(fresh_edges);
            let mut static_rows: Vec<Value> = self.store.array("static_test_imports").into_iter().filter(|r| {
                r.get("test_file").and_then(|v| v.as_str()).is_none_or(|p| !changed_tests.iter().any(|c| c == p))
            }).collect();
            static_rows.extend(fresh_static);
            self.store.doc.insert("test_units", Value::Array(test_units));
            self.store.doc.insert("test_edges", Value::Array(edges));
            self.store.doc.insert("static_test_imports", Value::Array(static_rows));
        }
        let mut imports: Vec<Value> = self.store.array("import_edges").into_iter().filter(|e| {
            e.get("importer_file").and_then(|v| v.as_str()).is_none_or(|p| !changed_set.contains(p))
        }).collect();
        let changed_sources: Vec<String> = changed.iter().filter(|p| !looks_like_test(p)).cloned().collect();
        imports.extend(self.build_import_edges(&changed_sources));
        self.store.doc.insert("import_edges", Value::Array(imports));
        self.store.save()?;
        let mut summary = vec![
            ("files_updated".into(), Value::Int(changed.len() as i64)),
            ("code_units_found".into(), Value::Int(self.store.array("code_units").len() as i64)),
            ("new_commits".into(), Value::Int(new_commits.len() as i64)),
            ("edge_rebuild_skipped".into(), Value::Bool(edge_rebuild_skipped)),
            ("hint".into(), Value::String("For large repos, use start_job with kind='update' to avoid MCP timeouts.".into())),
        ];
        if skip_unit {
            summary.push(("unit_churn_skipped".into(), Value::Bool(true)));
        }
        Ok(Value::object(summary))
    }

    fn scan_code_files(&self, git: &GitAnalyzer) -> Vec<String> {
        let mut files = if self.include_ignored {
            walk_code(&self.project_dir)
        } else {
            git.ls_code_candidates().unwrap_or_else(|_| walk_code(&self.project_dir))
        };
        files.retain(|p| extract::path_has_code_extension(p) && !p.contains(".chisel/"));
        if let Some(only) = &self.only_files {
            files.retain(|p| only.contains(p));
        }
        if let Some(dir) = &self.directory {
            let prefix = format!("{dir}/");
            files.retain(|p| p == dir || p.starts_with(&prefix));
        }
        files.sort();
        files.dedup();
        files
    }

    fn test_file_set(&self) -> BTreeSet<String> {
        self.store.array("test_units").into_iter().filter_map(|u| u.get("file_path").and_then(|v| v.as_str()).map(|s| s.to_string())).collect()
    }

    fn build_tests_and_edges(&mut self, files: &[String], units_by_file: &BTreeMap<String, Vec<CodeUnit>>) -> (Vec<Value>, Vec<Value>, Vec<Value>) {
        let mut test_units = Vec::new();
        let mut test_files = Vec::new();
        for rel in files {
            if !looks_like_test(rel) {
                continue;
            }
            test_files.push(rel.clone());
            let abs = self.project_dir.join(rel);
            let content = fs::read_to_string(&abs).unwrap_or_default();
            self.source_reads += 1;
            let hash = sha256_hex(content.as_bytes());
            let framework = if rel.ends_with(".py") { "pytest" } else if rel.ends_with(".go") { "go" } else { "test" };
            let units = units_by_file.get(rel).cloned().unwrap_or_default();
            let before_n = test_units.len();
            for u in units {
                if extract::is_test_name(&u.name, framework) || u.unit_type == "test_case" {
                    test_units.push(Value::object(vec![
                        ("id".into(), Value::String(format!("{rel}:{}", u.name))),
                        ("file_path".into(), Value::String(rel.clone())),
                        ("name".into(), Value::String(u.name.clone())),
                        ("framework".into(), Value::String(framework.into())),
                        ("line_start".into(), Value::Int(u.line_start as i64)),
                        ("line_end".into(), Value::Int(u.line_end as i64)),
                        ("content_hash".into(), Value::String(hash.clone())),
                    ]));
                }
            }
            if test_units.len() == before_n {
                let stem = rel.rsplit(['/', '\\']).next().unwrap_or(rel);
                test_units.push(Value::object(vec![
                    ("id".into(), Value::String(format!("{rel}:{stem}"))),
                    ("file_path".into(), Value::String(rel.clone())),
                    ("name".into(), Value::String(stem.into())),
                    ("framework".into(), Value::String(framework.into())),
                    ("line_start".into(), Value::Int(1)),
                    ("line_end".into(), Value::Int(1)),
                    ("content_hash".into(), Value::String(hash)),
                ]));
            }
        }
        let code_units = self.store.array("code_units");
        let mut edges = Vec::new();
        let mut static_rows = Vec::new();
        let mut static_seen = BTreeSet::new();
        let mut seen = BTreeSet::new();
        let mut content_cache: BTreeMap<String, String> = BTreeMap::new();
        for tu in &test_units {
            let file = tu.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
            let tid = tu.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let content = content_cache.entry(file.to_string()).or_insert_with(|| {
                fs::read_to_string(self.project_dir.join(file)).unwrap_or_default()
            });
            let deps = extract::extract_deps(file, content);
            let resolved = resolved_import_files(&deps, &code_units);
            for path in &resolved {
                let key = format!("{file}|{path}");
                if static_seen.insert(key) {
                    static_rows.push(Value::object(vec![
                        ("test_file".into(), Value::String(file.into())),
                        ("code_file".into(), Value::String(path.clone())),
                    ]));
                }
            }
            for dep in deps {
                let matched = match_dep(file, &dep, &code_units, &resolved);
                for cid in matched {
                    let code_file = code_units.iter().find(|u| u.get("id").and_then(|v| v.as_str()) == Some(cid.as_str())).and_then(|u| u.get("file_path").and_then(|v| v.as_str())).unwrap_or("");
                    let key = format!("{tid}|{cid}|{}", dep.dep_type);
                    if !seen.insert(key) {
                        continue;
                    }
                    let weight = extract::proximity_weight(file, code_file) * dep.confidence.sqrt();
                    edges.push(Value::object(vec![
                        ("test_id".into(), Value::String(tid.into())),
                        ("code_id".into(), Value::String(cid)),
                        ("edge_type".into(), Value::String(dep.dep_type.clone())),
                        ("weight".into(), Value::Float(weight)),
                    ]));
                }
            }
        }
        (test_units, edges, static_rows)
    }

    fn build_import_edges(&self, files: &[String]) -> Vec<Value> {
        let tests = self.test_file_set();
        let code_units = self.store.array("code_units");
        let mut edges = Vec::new();
        let mut seen = BTreeSet::new();
        for rel in files {
            if tests.contains(rel) {
                continue;
            }
            let content = fs::read_to_string(self.project_dir.join(rel)).unwrap_or_default();
            for dep in extract::extract_deps(rel, &content) {
                if dep.dep_type != "import" && dep.dep_type != "tainted_import" {
                    continue;
                }
                let Some(module) = dep.module_path.as_deref() else { continue };
                for cu in &code_units {
                    let path = cu.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
                    let name = cu.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    if name == dep.name && extract::matches_import_path(path, module) && path != rel {
                        let key = format!("{rel}|{path}");
                        if seen.insert(key) {
                            edges.push(Value::object(vec![
                                ("importer_file".into(), Value::String(rel.clone())),
                                ("imported_file".into(), Value::String(path.into())),
                                ("confidence".into(), Value::Float(1.0)),
                            ]));
                        }
                    }
                }
            }
        }
        edges
    }

    pub fn has_data(&self) -> bool {
        !self.store.array("code_units").is_empty()
    }

    fn no_data() -> Value {
        Value::object(vec![
            ("status".into(), Value::String("no_data".into())),
            ("message".into(), Value::String(NO_DATA_MSG.into())),
            ("hint".into(), Value::String("chisel analyze".into())),
        ])
    }

    pub fn risk_map(&mut self) -> Result<Value, String> {
        if !self.shards.is_empty() {
            return self.aggregate_risk_map();
        }
        self.risk_map_with(true)
    }

    fn risk_map_with(&mut self, reweight: bool) -> Result<Value, String> {
        self.store.reload()?;
        if !self.has_data() {
            return Ok(Self::no_data());
        }
        let tests = self.test_file_set();
        let mut files: Vec<String> = self.store.array("churn").into_iter().filter_map(|c| c.get("file_path").and_then(|v| v.as_str()).map(|s| s.to_string())).filter(|p| !tests.contains(p)).collect();
        files.sort();
        files.dedup();
        let query_min: i64 = store::meta_get(&self.store.doc, "co_change_query_min").and_then(|s| s.parse().ok()).unwrap_or(1);
        let imports = self.store.array("import_edges");

        let mut neighbor_map: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for e in &imports {
            let a = e.get("importer_file").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let b = e.get("imported_file").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let conf = e.get("confidence").and_then(|v| v.as_f64()).unwrap_or(1.0);
            if conf >= 1.0 {
                neighbor_map.entry(a.clone()).or_default().insert(b.clone());
                neighbor_map.entry(b).or_default().insert(a);
            }
        }
        let mut tested_files = BTreeSet::new();
        for f in &files {
            for u in self.store.units_by_file(f) {
                let id = u.get("id").and_then(|v| v.as_str()).unwrap_or("");
                if !self.store.edges_for_code(id).is_empty() {
                    tested_files.insert(f.clone());
                    break;
                }
            }
        }
        let file_set: BTreeSet<String> = files.iter().cloned().collect();
        let hops = impact::hops_to_tested(&file_set, &tested_files, &neighbor_map);

        let mut scored = Vec::new();
        for f in &files {
            let churn_raw = self.store.churn_for_file(f).into_iter().find(|c| c.get("unit_name").and_then(|v| v.as_str()).unwrap_or("").is_empty()).and_then(|c| c.get("churn_score").and_then(|v| v.as_f64())).unwrap_or(0.0);
            let mut partners: Vec<(String, i64, u8)> = Vec::new();
            let mut co_count = 0usize;
            let co_rows = self.store.co_for_file(f);
            for c in &co_rows {
                let n = c.get("co_commit_count").and_then(|v| v.as_i64()).unwrap_or(0);
                if n < query_min {
                    continue;
                }
                let a = c.get("file_a").and_then(|v| v.as_str()).unwrap_or("");
                let b = c.get("file_b").and_then(|v| v.as_str()).unwrap_or("");
                if a == f || b == f {
                    co_count += 1;
                    // SQLite returns ORDER BY co_commit_count DESC, and for ties
                    // the rows where this file is file_a come before rows where it is file_b.
                    if a == f {
                        partners.push((b.to_string(), n, 0));
                    } else {
                        partners.push((a.to_string(), n, 1));
                    }
                }
            }
            partners.sort_by(|a, b| b.1.cmp(&a.1).then(a.2.cmp(&b.2)).then(a.0.cmp(&b.0)));
            let mut import_neighbors: Vec<String> = neighbor_map.get(f).map(|s| s.iter().cloned().collect()).unwrap_or_default();
            import_neighbors.sort();
            let file_units = self.store.units_by_file(f);
            let mut tested_lines = 0i64;
            let mut total_lines = 0i64;
            let mut tested_units = 0i64;
            let mut covering = BTreeSet::new();
            let mut call_edges = 0i64;
            let mut total_edges = 0i64;
            let mut dynamic_edges = 0i64;
            for u in &file_units {
                let start = u.get("line_start").and_then(|v| v.as_i64()).unwrap_or(1);
                let end = u.get("line_end").and_then(|v| v.as_i64()).unwrap_or(start);
                let lines = (end - start + 1).max(0);
                total_lines += lines;
                let id = u.get("id").and_then(|v| v.as_str()).unwrap_or("");
                let uedges = self.store.edges_for_code(id);
                if !uedges.is_empty() {
                    tested_units += 1;
                    tested_lines += lines;
                    for e in &uedges {
                        if let Some(tid) = e.get("test_id").and_then(|v| v.as_str()) {
                            covering.insert(tid.to_string());
                        }
                        total_edges += 1;
                        let kind = e.get("edge_type").and_then(|v| v.as_str()).unwrap_or("");
                        if kind == "call" {
                            call_edges += 1;
                        }
                        if kind == "dynamic_import" || kind == "eval_import" {
                            dynamic_edges += 1;
                        }
                    }
                }
            }
            let hash_owned = self.store.hash_for_file(f);
            let hash = hash_owned.as_str();
            let mut lines_by_author: BTreeMap<String, i64> = BTreeMap::new();
            let blame_rows = self.store.blame_for_file(f);
            for b in &blame_rows {
                if b.get("file_path").and_then(|v| v.as_str()) != Some(f.as_str()) {
                    continue;
                }
                if b.get("content_hash").and_then(|v| v.as_str()) != Some(hash) {
                    continue;
                }
                let a = b.get("author").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let ls = b.get("line_start").and_then(|v| v.as_i64()).unwrap_or(0);
                let le = b.get("line_end").and_then(|v| v.as_i64()).unwrap_or(ls);
                *lines_by_author.entry(a).or_default() += (le - ls + 1).max(0);
            }
            let conc = impact::author_concentration(&lines_by_author);
            let mut eval_patterns = 0i64;
            if self.working_tree && (f.ends_with(".js") || f.ends_with(".ts") || f.ends_with(".jsx") || f.ends_with(".tsx")) {
                self.source_reads += 1;
                if let Ok(text) = fs::read_to_string(self.project_dir.join(f)) {
                    eval_patterns = text.matches("new Function(").count() as i64;
                }
            }
            scored.push(impact::score_file(&RiskInputs {
                file_path: f.clone(),
                churn_raw,
                co_change_count: co_count,
                branch_co_count: 0,
                import_neighbors,
                coupling_partners: partners.iter().map(|(f, n, _)| (f.clone(), *n)).collect(),
                tested_lines,
                total_lines,
                tested_units,
                unit_count: file_units.len() as i64,
                covering_tests: covering.len(),
                call_edges,
                total_edges,
                dynamic_edges,
                author_conc: conc,
                instability: 0.0,
                eval_patterns,
                exclude_new_file_boost: false,
                coverage_mode: "line".into(),
                proximity_hops: hops.get(f).copied(),
                proximity: true,
            }));
        }
        scored.sort_by(|a, b| {
            let sa = a.get("risk_score").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let sb = b.get("risk_score").and_then(|v| v.as_f64()).unwrap_or(0.0);
            sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal).then_with(|| {
                let pa = a.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
                let pb = b.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
                pa.cmp(pb)
            })
        });
        let rw = if reweight {
            impact::apply_reweighting(&mut scored)
        } else {
            Value::object(vec![("reweighted".into(), Value::Bool(false))])
        };
        let commits_n = self.store.array("commits").len() as i64;
        let test_edges_n = self.store.array_len("test_edges") as i64;
        let test_results_n = self.store.array_len("test_results") as i64;
        let import_n = imports.len() as i64;
        let co_n = self.store.array("co_changes").iter().filter(|c| c.get("co_commit_count").and_then(|v| v.as_i64()).unwrap_or(0) >= query_min).count() as i64;
        let mut meta = impact::build_risk_meta(&scored, commits_n, test_edges_n, test_results_n, import_n, co_n);
        if let Some(obj) = meta.as_object_mut() {
            // merge reweighting keys
            if let Some(pairs) = rw.as_object() {
                for (k, v) in pairs {
                    if let Some(slot) = obj.iter_mut().find(|(kk, _)| kk == k) {
                        slot.1 = v.clone();
                    } else {
                        obj.push((k.clone(), v.clone()));
                    }
                }
            }
        }
        meta.insert("coverage_gap_mode", Value::String("proximity_adjusted".into()));
        meta.insert("cycles", Value::Array(vec![]));
        if let Some(bc) = store::meta_get(&self.store.doc, "branch_coupling_commits") {
            if let Ok(n) = bc.parse::<i64>() {
                meta.insert("branch_coupling_commits", Value::Int(n));
            }
        }
        let untracked = if self.working_tree {
            self.git.fork().untracked().unwrap_or_default().into_iter().filter(|p| extract::path_has_code_extension(p)).count()
        } else {
            0
        };
        if untracked > 0 {
            let warning = Value::String(format!("{untracked} untracked code files excluded from risk scoring due to lack of git history"));
            match meta.get_mut("warnings").and_then(|v| v.as_array_mut()) {
                Some(arr) => arr.push(warning),
                None => meta.insert("warnings", Value::Array(vec![warning])),
            }
        }
        Ok(Value::object(vec![
            ("files".into(), Value::Array(scored)),
            ("_meta".into(), meta),
        ]))
    }

    pub fn diff_impact(&mut self) -> Result<Value, String> {
        if !self.shards.is_empty() {
            return self.aggregate_diff_impact();
        }
        self.store.reload()?;
        let git = self.git.fork();
        let branch = match git.current_branch() {
            Ok(b) => b,
            Err(e) => return Ok(git::git_tool_error(&self.project_dir.to_string_lossy(), &e.0)),
        };
        if !self.has_data() {
            return Ok(Self::no_data());
        }
        let diff_ref = if let Some(r) = &self.diff_ref_override {
            r.clone()
        } else if branch == "main" || branch == "master" {
            "HEAD".to_string()
        } else if git.branch_exists("main") {
            "main".into()
        } else if git.branch_exists("master") {
            "master".into()
        } else {
            "HEAD".into()
        };
        let diff_files = match git.changed_files(&diff_ref) {
            Ok(v) => v,
            Err(e) => return Ok(git::git_tool_error(&self.project_dir.to_string_lossy(), &e.0)),
        };
        let untracked_code: BTreeSet<String> = git.untracked().unwrap_or_default().into_iter().filter(|p| extract::path_has_code_extension(p)).collect();
        let mut changed: Vec<String> = diff_files.into_iter().filter(|p| extract::path_has_code_extension(p)).chain(untracked_code.iter().cloned()).collect();
        changed.sort();
        changed.dedup();
        if changed.is_empty() {
            return Ok(Value::object(vec![
                ("status".into(), Value::String("no_changes".into())),
                ("ref".into(), Value::String(diff_ref.clone())),
                ("branch".into(), Value::String(branch)),
                ("message".into(), Value::String(format!("No files differ against '{diff_ref}' and no untracked code files"))),
            ]));
        }
        let known: BTreeSet<String> = self.store.array("file_hashes").into_iter().filter_map(|h| h.get("file_path").and_then(|v| v.as_str()).map(|s| s.to_string())).collect();
        let missing: Vec<String> = changed.iter().filter(|p| !known.contains(*p)).cloned().collect();
        if !missing.is_empty() {
            return Ok(Value::object(vec![
                ("status".into(), Value::String("stale_db".into())),
                ("changed_files".into(), Value::Array(changed.into_iter().map(Value::String).collect())),
                ("missing_from_db".into(), Value::Array(missing.into_iter().map(Value::String).collect())),
                ("message".into(), Value::String("Some changed files are missing from the analysis database.".into())),
                ("hint".into(), Value::String("Run 'chisel update' first to include new or changed files.".into())),
            ]));
        }
        let mut functions: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for f in &changed {
            if let Ok(names) = git.changed_functions(f, &diff_ref) {
                if !names.is_empty() {
                    functions.insert(f.clone(), names);
                }
            }
        }
        let mut rows = self.impacted(&changed, &functions, &untracked_code);
        self.attach_working_tree_hits(&mut rows, &changed, &untracked_code);
        Ok(Value::Array(rows))
    }

    pub fn suggest_tests(&mut self, file: &str) -> Result<Value, String> {
        if !self.shards.is_empty() {
            if let Some(name) = shard_name_for(file, &self.shards) {
                let mut sub = self.open_shard(name)?;
                return sub.suggest_tests(file);
            }
        }
        self.store.reload()?;
        if !self.has_data() {
            return Ok(Self::no_data());
        }
        let impacted = self.impacted(&[file.to_string()], &BTreeMap::new(), &BTreeSet::new());
        let static_hits = self.static_hits(file);
        let mut by_id: BTreeMap<String, Value> = BTreeMap::new();
        for item in impacted {
            let tid = item.get("test_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            by_id.insert(tid, item);
        }
        for item in static_hits {
            let tid = item.get("test_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if let Some(prev) = by_id.get(&tid) {
                let prev_score = prev.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let s = item.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let new_score = (prev_score + s * 0.28).min(1.0);
                let reason = format!(
                    "{}; {}",
                    prev.get("reason").and_then(|v| v.as_str()).unwrap_or(""),
                    item.get("reason").and_then(|v| v.as_str()).unwrap_or("")
                );
                by_id.insert(tid.clone(), Value::object(vec![
                    ("test_id".into(), Value::String(tid)),
                    ("file_path".into(), prev.get("file_path").cloned().unwrap_or(Value::String(String::new()))),
                    ("name".into(), prev.get("name").cloned().unwrap_or(Value::String(String::new()))),
                    ("score".into(), Value::Float(new_score)),
                    ("reason".into(), Value::String(reason)),
                    ("source".into(), Value::String("hybrid".into())),
                ]));
            } else {
                by_id.insert(tid, item);
            }
        }
        let mut result = Vec::new();
        for (_tid, item) in by_id {
            let relevance = item.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
            result.push(Value::object(vec![
                ("test_id".into(), item.get("test_id").cloned().unwrap_or(Value::Null)),
                ("file_path".into(), item.get("file_path").cloned().unwrap_or(Value::Null)),
                ("name".into(), item.get("name").cloned().unwrap_or(Value::Null)),
                ("relevance".into(), Value::Float(relevance)),
                ("reason".into(), item.get("reason").cloned().unwrap_or(Value::String(String::new()))),
                ("source".into(), item.get("source").cloned().unwrap_or(Value::String("direct".into()))),
                ("failure_rate".into(), Value::Float(0.0)),
                ("failure_boost".into(), Value::Float(1.0)),
            ]));
        }
        if result.is_empty() {
            for (tid, file_path, name, relevance) in self.stem_matches(file) {
                result.push(Value::object(vec![
                    ("test_id".into(), Value::String(tid)),
                    ("file_path".into(), Value::String(file_path)),
                    ("name".into(), Value::String(name)),
                    ("relevance".into(), Value::Float(relevance)),
                    ("reason".into(), Value::String("fallback: stem-matched test file".into())),
                    ("source".into(), Value::String("fallback".into())),
                    ("failure_rate".into(), Value::Float(0.0)),
                    ("failure_boost".into(), Value::Float(1.0)),
                ]));
            }
        }
        result.sort_by(|a, b| {
            let ra = a.get("relevance").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let rb = b.get("relevance").and_then(|v| v.as_f64()).unwrap_or(0.0);
            rb.partial_cmp(&ra).unwrap_or(std::cmp::Ordering::Equal).then_with(|| {
                a.get("test_id").and_then(|v| v.as_str()).unwrap_or("").cmp(b.get("test_id").and_then(|v| v.as_str()).unwrap_or(""))
            })
        });
        Ok(Value::Array(result))
    }

    fn static_hits(&mut self, file: &str) -> Vec<Value> {
        let tests = self.store.array("test_units");
        let mut by_file: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for t in &tests {
            let fp = t.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
            let name = t.get("name").and_then(|v| v.as_str()).unwrap_or("");
            by_file.entry(fp.to_string()).or_default().push(name.to_string());
        }
        let mut targets_by_test: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        if self.working_tree {
            for fp in by_file.keys() {
                self.source_reads += 1;
                let content = fs::read_to_string(self.project_dir.join(fp)).unwrap_or_default();
                let deps = extract::extract_deps(fp, &content);
                let targets = resolved_import_files(&deps, &self.store.array("code_units"));
                targets_by_test.insert(fp.clone(), targets);
            }
        } else {
            for row in self.store.array("static_test_imports") {
                let test_file = row.get("test_file").and_then(|v| v.as_str()).unwrap_or("");
                let code_file = row.get("code_file").and_then(|v| v.as_str()).unwrap_or("");
                if test_file.is_empty() || code_file.is_empty() { continue; }
                targets_by_test.entry(test_file.to_string()).or_default().insert(code_file.to_string());
            }
        }
        let mut hits = Vec::new();
        for (fp, names) in by_file {
            let Some(targets) = targets_by_test.get(&fp) else { continue };
            if !targets.iter().any(|t| t == file) { continue; }
            let score = extract::proximity_weight(&fp, file);
            for name in names {
                hits.push(Value::object(vec![
                    ("test_id".into(), Value::String(format!("{fp}:{name}"))),
                    ("file_path".into(), Value::String(fp.clone())),
                    ("name".into(), Value::String(name)),
                    ("score".into(), Value::Float(score)),
                    ("reason".into(), Value::String(format!("static import → {file}"))),
                    ("source".into(), Value::String("static_require".into())),
                ]));
            }
        }
        hits
    }

    fn direct_brief(&self, file: &str) -> Vec<(String, f64, String, String, String)> {
        let mut out = Vec::new();
        for unit in self.store.units_by_file(file) {
            let uname = unit.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let id = unit.get("id").and_then(|v| v.as_str()).unwrap_or("");
            for e in self.store.edges_for_code(id) {
                let tid = e.get("test_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let weight = e.get("weight").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let (test_file, test_name) = tid.rsplit_once(':').unwrap_or(("", ""));
                let test_file = test_file.to_string();
                let test_name = test_name.to_string();
                out.push((tid, weight, uname.clone(), test_file, test_name));
            }
        }
        out
    }

    fn remember_hit(best: &mut BTreeMap<String, (f64, Value)>, tid: &str, score: f64, row: Value) {
        match best.get(tid) {
            Some((prev, _)) if *prev >= score => {}
            _ => { best.insert(tid.to_string(), (score, row)); }
        }
    }

    fn stem_matches(&self, file: &str) -> Vec<(String, String, String, f64)> {
        let mut by_file: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for t in self.store.array("test_units") {
            let fp = t.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
            let name = t.get("name").and_then(|v| v.as_str()).unwrap_or("");
            if fp.is_empty() || name.is_empty() {
                continue;
            }
            by_file.entry(fp.to_string()).or_default().push(name.to_string());
        }
        let mut out = Vec::new();
        for (test_file, names) in by_file {
            let score = stem_score(file, &test_file);
            for name in names {
                out.push((format!("{test_file}:{name}"), test_file.clone(), name, score));
            }
        }
        out.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0)));
        out
    }

    fn attach_working_tree_hits(&mut self, rows: &mut Vec<Value>, changed: &[String], untracked: &BTreeSet<String>) {
        let mut covered: BTreeSet<String> = rows.iter().filter_map(|row| row.get("test_id").and_then(|v| v.as_str()).map(|s| s.to_string())).collect();
        if self.working_tree {
            for cf in changed {
                if untracked.contains(cf) {
                    continue;
                }
                for mut hit in self.static_hits(cf) {
                    let tid = hit.get("test_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    if tid.is_empty() || !covered.insert(tid) {
                        continue;
                    }
                    hit.insert("source", Value::String("working_tree".into()));
                    hit.insert("reason", Value::String(format!("static import of {cf}")));
                    rows.push(hit);
                }
            }
        }
        if untracked.is_empty() && !self.working_tree {
            return;
        }
        let targets: Vec<&String> = if self.working_tree { changed.iter().collect() } else { untracked.iter().collect() };
        for cf in targets {
            let mentioned = rows.iter().any(|row| row.get("reason").and_then(|v| v.as_str()).unwrap_or("").contains(cf.as_str()));
            if mentioned {
                continue;
            }
            for (tid, file_path, name, relevance) in self.stem_matches(cf) {
                if relevance < 0.5 || !covered.insert(tid.clone()) {
                    continue;
                }
                rows.push(Value::object(vec![
                    ("test_id".into(), Value::String(tid)),
                    ("file_path".into(), Value::String(file_path)),
                    ("name".into(), Value::String(name)),
                    ("reason".into(), Value::String(format!("stem-match for {cf}"))),
                    ("score".into(), Value::Float(relevance * 0.4)),
                    ("source".into(), Value::String("working_tree".into())),
                ]));
            }
        }
    }

    fn impacted(&self, files: &[String], functions: &BTreeMap<String, Vec<String>>, untracked: &BTreeSet<String>) -> Vec<Value> {
        let tests: BTreeMap<String, Value> = self.store.array("test_units").into_iter().filter_map(|t| {
            Some((t.get("id")?.as_str()?.to_string(), t))
        }).collect();
        let mut best: BTreeMap<String, (f64, Value)> = BTreeMap::new();
        for file in files {
            let fns = functions.get(file);
            for unit in self.store.units_by_file(file) {
                let uname = unit.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let bare = uname.rsplit('.').next().unwrap_or(uname);
                if let Some(list) = fns {
                    if !list.iter().any(|n| n == uname || n == bare) {
                        continue;
                    }
                }
                let id = unit.get("id").and_then(|v| v.as_str()).unwrap_or("");
                for e in self.store.edges_for_code(id) {
                    let tid = e.get("test_id").and_then(|v| v.as_str()).unwrap_or("");
                    let weight = e.get("weight").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let test = tests.get(tid);
                    let edge_type = e.get("edge_type").and_then(|v| v.as_str()).unwrap_or("import");
                    let row = Value::object(vec![
                        ("test_id".into(), Value::String(tid.into())),
                        ("file_path".into(), test.and_then(|t| t.get("file_path")).cloned().unwrap_or(Value::String(String::new()))),
                        ("name".into(), test.and_then(|t| t.get("name")).cloned().unwrap_or(Value::String(String::new()))),
                        ("reason".into(), Value::String(format!("direct edge to {uname} ({edge_type})"))),
                        ("score".into(), Value::Float(weight)),
                        ("source".into(), Value::String("direct".into())),
                    ]);
                    Self::remember_hit(&mut best, tid, weight, row);
                }
            }
        }
        let query_min: i64 = store::meta_get(&self.store.doc, "co_change_query_min").and_then(|s| s.parse().ok()).unwrap_or(1);
        let co = self.store.array("co_changes");
        for file in files {
            for cc in &co {
                let a = cc.get("file_a").and_then(|v| v.as_str()).unwrap_or("");
                let b = cc.get("file_b").and_then(|v| v.as_str()).unwrap_or("");
                let n = cc.get("co_commit_count").and_then(|v| v.as_i64()).unwrap_or(0);
                if n < query_min { continue; }
                let other = if a == file { b } else if b == file { a } else { continue };
                for (tid, weight, uname, test_file, test_name) in self.direct_brief(other) {
                    let score = weight * 0.5;
                    let row = Value::object(vec![
                        ("test_id".into(), Value::String(tid.clone())),
                        ("file_path".into(), Value::String(test_file)),
                        ("name".into(), Value::String(test_name)),
                        ("reason".into(), Value::String(format!("co-change coupling: {file} <-> {other} ({n} commits)"))),
                        ("score".into(), Value::Float(score)),
                        ("source".into(), Value::String("co_change".into())),
                    ]);
                    let _ = uname;
                    Self::remember_hit(&mut best, &tid, score, row);
                }
            }
        }
        let imports = self.store.array("import_edges");
        let mut reverse: BTreeMap<String, Vec<(String, f64)>> = BTreeMap::new();
        for e in &imports {
            let importer = e.get("importer_file").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let imported = e.get("imported_file").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let conf = e.get("confidence").and_then(|v| v.as_f64()).unwrap_or(1.0);
            if importer.is_empty() || imported.is_empty() { continue; }
            reverse.entry(imported).or_default().push((importer, conf));
        }
        for file in files {
            if untracked.contains(file) {
                continue;
            }
            let mut visited: BTreeMap<String, (i64, f64)> = BTreeMap::new();
            visited.insert(file.clone(), (0, 1.0));
            let mut queue = vec![file.clone()];
            let mut qi = 0;
            while qi < queue.len() {
                let u = queue[qi].clone();
                qi += 1;
                let (hops, decay) = visited[&u];
                if hops >= 32 || decay < 0.01 { continue; }
                for (v, conf) in reverse.get(&u).into_iter().flatten() {
                    let new_decay = decay * 0.88 * conf;
                    let replace = visited.get(v).map(|(_, d)| new_decay > *d).unwrap_or(true);
                    if replace {
                        visited.insert(v.clone(), (hops + 1, new_decay));
                        queue.push(v.clone());
                    }
                }
            }
            for (other, (hops, decay)) in &visited {
                if other == file || *hops == 0 { continue; }
                for (tid, weight, _uname, test_file, test_name) in self.direct_brief(other) {
                    let score = weight * 0.45 * decay;
                    let hop_word = if *hops == 1 { "hop" } else { "hops" };
                    let hard = 0.88_f64.powi(*hops as i32);
                    let qualifier = if *decay < hard - 1e-9 { " via dynamic require()" } else { "" };
                    let row = Value::object(vec![
                        ("test_id".into(), Value::String(tid.clone())),
                        ("file_path".into(), Value::String(test_file)),
                        ("name".into(), Value::String(test_name)),
                        ("reason".into(), Value::String(format!("import graph: tests cover {other} ({hops} {hop_word} from {file}{qualifier})"))),
                        ("score".into(), Value::Float(score)),
                        ("source".into(), Value::String("import_graph".into())),
                    ]);
                    Self::remember_hit(&mut best, &tid, score, row);
                }
            }
        }
        let mut out: Vec<Value> = best.into_values().map(|(_, v)| v).collect();
        out.sort_by(|a, b| {
            let sa = a.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let sb = b.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
            sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal).then_with(|| {
                a.get("test_id").and_then(|v| v.as_str()).unwrap_or("").cmp(b.get("test_id").and_then(|v| v.as_str()).unwrap_or(""))
            })
        });
        out
    }

    pub fn stale_tests(&mut self) -> Result<Value, String> {
        if !self.shards.is_empty() {
            return self.aggregate_stale();
        }
        self.store.reload()?;
        if !self.has_data() {
            return Ok(Self::no_data());
        }
        let edges = self.store.array("test_edges");
        if edges.is_empty() {
            return Ok(Value::object(vec![
                ("status".into(), Value::String("no_edges".into())),
                ("stale_tests".into(), Value::Array(vec![])),
                ("message".into(), Value::String("No test edges in the analysis database.".into())),
            ]));
        }
        let ids: BTreeSet<String> = self.store.array("code_units").into_iter().filter_map(|u| u.get("id").and_then(|v| v.as_str()).map(|s| s.to_string())).collect();
        let tests = self.store.array("test_units");
        let mut stale = Vec::new();
        for e in &edges {
            let code_id = e.get("code_id").and_then(|v| v.as_str()).unwrap_or("");
            if ids.contains(code_id) {
                continue;
            }
            let tid = e.get("test_id").and_then(|v| v.as_str()).unwrap_or("");
            let name = tests.iter().find(|t| t.get("id").and_then(|v| v.as_str()) == Some(tid)).and_then(|t| t.get("name")).and_then(|v| v.as_str()).unwrap_or("");
            stale.push(Value::object(vec![
                ("test_id".into(), Value::String(tid.into())),
                ("test_name".into(), Value::String(name.into())),
                ("missing_code_id".into(), Value::String(code_id.into())),
                ("edge_type".into(), e.get("edge_type").cloned().unwrap_or(Value::String("import".into()))),
            ]));
        }
        Ok(Value::object(vec![
            ("stale_tests".into(), Value::Array(stale)),
        ]))
    }

    pub fn delete_code_unit(&mut self, id: &str) -> Result<(), String> {
        let kept: Vec<Value> = self.store.array("code_units").into_iter().filter(|u| u.get("id").and_then(|v| v.as_str()) != Some(id)).collect();
        self.store.doc.insert("code_units", Value::Array(kept));
        self.store.save()?;
        Ok(())
    }

    pub fn record_result(&mut self, test_id: &str, passed: bool) -> Result<Value, String> {
        self.store.array_mut("test_results").push(Value::object(vec![
            ("test_id".into(), Value::String(test_id.into())),
            ("passed".into(), Value::Bool(passed)),
        ]));
        self.store.save()?;
        Ok(Value::object(vec![
            ("test_id".into(), Value::String(test_id.into())),
            ("passed".into(), Value::Bool(passed)),
            ("recorded".into(), Value::Bool(true)),
        ]))
    }

    pub fn record_results_batch(&mut self, rows: &[(String, bool)]) -> Result<Value, String> {
        let before = self.store.doc.get("generation").and_then(|v| v.as_i64()).unwrap_or(0);
        for (test_id, passed) in rows {
            self.store.array_mut("test_results").push(Value::object(vec![
                ("test_id".into(), Value::String(test_id.clone())),
                ("passed".into(), Value::Bool(*passed)),
            ]));
        }
        self.store.save()?;
        let after = self.store.doc.get("generation").and_then(|v| v.as_i64()).unwrap_or(0);
        Ok(Value::object(vec![
            ("recorded".into(), Value::Int(rows.len() as i64)),
            ("generation_delta".into(), Value::Int(after - before)),
        ]))
    }

    pub fn stats(&mut self) -> Result<Value, String> {
        self.stats_with(false)
    }

    fn stats_with(&mut self, include_results: bool) -> Result<Value, String> {
        if !self.shards.is_empty() {
            return self.stats_shards(include_results);
        }
        self.store.reload()?;
        Ok(self.stats_doc(include_results))
    }

    fn stats_doc(&self, include_results: bool) -> Value {
        let results = self.store.array("test_results");
        let passed = results.iter().filter(|r| r.get("passed").and_then(|v| v.as_bool()) == Some(true)).count() as i64;
        let failed = results.iter().filter(|r| r.get("passed").and_then(|v| v.as_bool()) == Some(false)).count() as i64;
        let mut pairs = vec![
            ("code_units".into(), Value::Int(self.store.array("code_units").len() as i64)),
            ("test_units".into(), Value::Int(self.store.array("test_units").len() as i64)),
            ("test_edges".into(), Value::Int(self.store.array("test_edges").len() as i64)),
            ("commits".into(), Value::Int(self.store.array("commits").len() as i64)),
            ("co_changes".into(), Value::Int(self.store.array("co_changes").len() as i64)),
            ("import_edges".into(), Value::Int(self.store.array("import_edges").len() as i64)),
            ("test_results".into(), Value::Int(results.len() as i64)),
            ("passed".into(), Value::Int(passed)),
            ("failed".into(), Value::Int(failed)),
            ("coupling_threshold".into(), Value::Int(impact::coupling_threshold(self.store.array("commits").len() as i64))),
            ("co_change_query_min".into(), Value::String(store::meta_get(&self.store.doc, "co_change_query_min").unwrap_or_else(|| "0".into()))),
        ];
        if include_results {
            pairs.push(("recorded_results".into(), Value::Array(results)));
        }
        Value::object(pairs)
    }

    fn stats_shards(&self, include_results: bool) -> Result<Value, String> {
        let mut code_units = 0i64;
        let mut test_edges = 0i64;
        let mut test_results = 0i64;
        let mut commits = 0i64;
        for (name, _) in &self.shards {
            let dir = self.store.dir().join("shards").join(name);
            let store = Store::open(&dir)?;
            code_units += store.array("code_units").len() as i64;
            test_edges += store.array("test_edges").len() as i64;
            test_results += store.array("test_results").len() as i64;
            commits += store.array("commits").len() as i64;
        }
        let mut pairs = vec![
            ("code_units".into(), Value::Int(code_units)),
            ("test_edges".into(), Value::Int(test_edges)),
            ("test_results".into(), Value::Int(test_results)),
            ("commits".into(), Value::Int(commits)),
            ("passed".into(), Value::Int(0)),
            ("failed".into(), Value::Int(0)),
            ("coupling_threshold".into(), Value::Int(impact::coupling_threshold(commits))),
            ("cross_shard_test_edges".into(), Value::String("unsupported".into())),
        ];
        if include_results {
            pairs.push(("recorded_results".into(), Value::Array(vec![])));
        }
        Ok(Value::object(pairs))
    }

    fn open_shard(&self, name: &str) -> Result<Engine, String> {
        let dir = self.store.dir().join("shards").join(name);
        let mut sub = Engine::open(&self.project_dir, Some(&dir))?;
        sub.shards.clear();
        sub.include_ignored = self.include_ignored;
        sub.working_tree = self.working_tree;
        sub.diff_ref_override = self.diff_ref_override.clone();
        sub.directory = self.directory.clone();
        sub.git = self.git.fork();
        Ok(sub)
    }

    fn analyze_all_shards(&mut self) -> Result<Value, String> {
        let git = self.git.fork();
        let files = self.scan_code_files(&git);
        let shards = self.shards.clone();
        let mut scanned = 0i64;
        for (name, _) in &shards {
            let only: BTreeSet<String> = files.iter().filter(|f| shard_name_for(f, &shards) == Some(name.as_str())).cloned().collect();
            if only.is_empty() {
                continue;
            }
            let mut sub = self.open_shard(name)?;
            sub.only_files = Some(only);
            let summary = sub.analyze()?;
            scanned += summary.get("code_files_scanned").and_then(|v| v.as_i64()).unwrap_or(0);
        }
        Ok(Value::object(vec![
            ("code_files_scanned".into(), Value::Int(scanned)),
            ("shards".into(), Value::Int(shards.len() as i64)),
            ("hint".into(), Value::String("For large repos, use start_job with kind='analyze' to avoid MCP timeouts.".into())),
        ]))
    }

    fn update_all_shards(&mut self) -> Result<Value, String> {
        let git = self.git.fork();
        let files = self.scan_code_files(&git);
        let shards = self.shards.clone();
        let mut updated = 0i64;
        let mut commits = 0i64;
        let mut skipped = true;
        for (name, _) in &shards {
            let only: BTreeSet<String> = files.iter().filter(|f| shard_name_for(f, &shards) == Some(name.as_str())).cloned().collect();
            if only.is_empty() {
                continue;
            }
            let mut sub = self.open_shard(name)?;
            sub.only_files = Some(only);
            let summary = sub.update()?;
            updated += summary.get("files_updated").and_then(|v| v.as_i64()).unwrap_or(0);
            commits += summary.get("new_commits").and_then(|v| v.as_i64()).unwrap_or(0);
            if summary.get("edge_rebuild_skipped").and_then(|v| v.as_bool()) != Some(true) {
                skipped = false;
            }
        }
        Ok(Value::object(vec![
            ("files_updated".into(), Value::Int(updated)),
            ("new_commits".into(), Value::Int(commits)),
            ("edge_rebuild_skipped".into(), Value::Bool(skipped)),
            ("hint".into(), Value::String("For large repos, use start_job with kind='update' to avoid MCP timeouts.".into())),
        ]))
    }

    fn aggregate_risk_map(&mut self) -> Result<Value, String> {
        let mut files = Vec::new();
        let mut any = false;
        for (name, _) in self.shards.clone() {
            let mut sub = self.open_shard(&name)?;
            let part = sub.risk_map_with(false)?;
            if part.get("status").and_then(|v| v.as_str()) == Some("no_data") {
                continue;
            }
            any = true;
            if let Some(arr) = part.get("files").and_then(|v| v.as_array()) {
                files.extend(arr.iter().cloned());
            }
        }
        if !any {
            return Ok(Self::no_data());
        }
        files.sort_by(|a, b| {
            let sa = a.get("risk_score").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let sb = b.get("risk_score").and_then(|v| v.as_f64()).unwrap_or(0.0);
            sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal).then_with(|| {
                a.get("file_path").and_then(|v| v.as_str()).unwrap_or("").cmp(b.get("file_path").and_then(|v| v.as_str()).unwrap_or(""))
            })
        });
        let _ = impact::apply_reweighting(&mut files);
        Ok(Value::object(vec![
            ("files".into(), Value::Array(files)),
            ("_meta".into(), Value::object(vec![
                ("cross_shard_test_edges".into(), Value::String("unsupported".into())),
            ])),
        ]))
    }

    fn aggregate_stale(&self) -> Result<Value, String> {
        let mut stale = Vec::new();
        let mut any = false;
        let mut edges = false;
        let names: Vec<String> = self.shards.iter().map(|(name, _)| name.clone()).collect();
        for name in names {
            let sub = self.open_shard(&name)?;
            if !sub.has_data() {
                continue;
            }
            any = true;
            if sub.store.array_len("test_edges") > 0 {
                edges = true;
            }
            stale.extend(sub.stale_rows());
        }
        if !any {
            return Ok(Self::no_data());
        }
        if !edges {
            return Ok(Value::object(vec![
                ("status".into(), Value::String("no_edges".into())),
                ("stale_tests".into(), Value::Array(vec![])),
                ("message".into(), Value::String("No test edges in the analysis database.".into())),
            ]));
        }
        Ok(Value::object(vec![("stale_tests".into(), Value::Array(stale))]))
    }

    fn aggregate_impact(&self, files: &[String]) -> Result<Value, String> {
        let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for file in files {
            if let Some(name) = shard_name_for(file, &self.shards) {
                groups.entry(name.to_string()).or_default().push(file.clone());
            }
        }
        let mut rows = Vec::new();
        let mut any = false;
        for (name, group) in groups {
            let sub = self.open_shard(&name)?;
            if !sub.has_data() {
                continue;
            }
            any = true;
            rows.extend(sub.impacted(&group, &BTreeMap::new(), &BTreeSet::new()));
        }
        if !any {
            return Ok(Self::no_data());
        }
        rows.sort_by(|a, b| {
            let sa = a.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let sb = b.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
            sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(Value::Array(rows))
    }

    fn aggregate_diff_impact(&mut self) -> Result<Value, String> {
        let git = self.git.fork();
        let branch = match git.current_branch() {
            Ok(b) => b,
            Err(e) => return Ok(git::git_tool_error(&self.project_dir.to_string_lossy(), &e.0)),
        };
        let diff_ref = if let Some(r) = &self.diff_ref_override {
            r.clone()
        } else if branch == "main" || branch == "master" {
            "HEAD".to_string()
        } else if git.branch_exists("main") {
            "main".into()
        } else if git.branch_exists("master") {
            "master".into()
        } else {
            "HEAD".into()
        };
        let diff_files = match git.changed_files(&diff_ref) {
            Ok(v) => v,
            Err(e) => return Ok(git::git_tool_error(&self.project_dir.to_string_lossy(), &e.0)),
        };
        let untracked_code: BTreeSet<String> = git.untracked().unwrap_or_default().into_iter().filter(|p| extract::path_has_code_extension(p)).collect();
        let mut changed: Vec<String> = diff_files.into_iter().filter(|p| extract::path_has_code_extension(p)).chain(untracked_code.iter().cloned()).collect();
        changed.sort();
        changed.dedup();
        if changed.is_empty() {
            return Ok(Value::object(vec![
                ("status".into(), Value::String("no_changes".into())),
                ("ref".into(), Value::String(diff_ref.clone())),
                ("branch".into(), Value::String(branch)),
                ("message".into(), Value::String(format!("No files differ against '{diff_ref}' and no untracked code files"))),
            ]));
        }
        let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut missing = Vec::new();
        for file in &changed {
            let Some(name) = shard_name_for(file, &self.shards) else {
                missing.push(file.clone());
                continue;
            };
            groups.entry(name.to_string()).or_default().push(file.clone());
        }
        for (name, group) in &groups {
            let sub = self.open_shard(name)?;
            let known: BTreeSet<String> = sub.store.array("file_hashes").into_iter().filter_map(|h| h.get("file_path").and_then(|v| v.as_str()).map(|s| s.to_string())).collect();
            for file in group {
                if !known.contains(file) {
                    missing.push(file.clone());
                }
            }
        }
        if !missing.is_empty() {
            return Ok(Value::object(vec![
                ("status".into(), Value::String("stale_db".into())),
                ("changed_files".into(), Value::Array(changed.into_iter().map(Value::String).collect())),
                ("missing_from_db".into(), Value::Array(missing.into_iter().map(Value::String).collect())),
                ("message".into(), Value::String("Some changed files are missing from the analysis database.".into())),
                ("hint".into(), Value::String("Run 'chisel update' first to include new or changed files.".into())),
            ]));
        }
        let mut rows = Vec::new();
        let mut any = false;
        for (name, group) in &groups {
            let mut sub = self.open_shard(name)?;
            if !sub.has_data() {
                continue;
            }
            any = true;
            let mut functions: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for file in group {
                if untracked_code.contains(file) {
                    continue;
                }
                if let Ok(names) = git.changed_functions(file, &diff_ref) {
                    if !names.is_empty() {
                        functions.insert(file.clone(), names);
                    }
                }
            }
            let owned: BTreeSet<String> = group.iter().filter(|f| untracked_code.contains(*f)).cloned().collect();
            let mut part = sub.impacted(group, &functions, &owned);
            sub.attach_working_tree_hits(&mut part, group, &owned);
            rows.extend(part);
        }
        if !any {
            return Ok(Self::no_data());
        }
        rows.sort_by(|a, b| {
            let sa = a.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let sb = b.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
            sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(Value::Array(rows))
    }

    fn changed_file_count(&self) -> usize {
        if !self.shards.is_empty() {
            let names: Vec<String> = self.shards.iter().map(|(name, _)| name.clone()).collect();
            let mut count = 0usize;
            for name in names {
                if let Ok(sub) = self.open_shard(&name) {
                    count += sub.changed_file_count();
                }
            }
            return count;
        }
        let git = self.git.fork();
        let files = self.scan_code_files(&git);
        let before: BTreeMap<String, String> = self.store.array("file_hashes").into_iter().filter_map(|row| {
            Some((row.get("file_path")?.as_str()?.to_string(), row.get("hash")?.as_str()?.to_string()))
        }).collect();
        files.iter().filter(|rel| {
            let content = fs::read(self.project_dir.join(rel)).unwrap_or_default();
            let hash = sha256_hex(&content);
            before.get(*rel).map(|h| h.as_str()) != Some(hash.as_str())
        }).count()
    }

    fn auto_plan(&self, name: &str, args: &Value) -> AutoPlan {
        if !matches!(name, "diff_impact" | "suggest_tests" | "risk_map" | "test_gaps" | "triage") {
            return AutoPlan::Off;
        }
        if args.get("auto_update").and_then(|v| v.as_bool()) != Some(true) {
            return AutoPlan::Off;
        }
        let running = job_lock().lock().ok().and_then(|jobs| {
            jobs.iter().any(|j| j.storage == self.store.dir() && (j.status == "queued" || j.status == "running")).then_some(())
        }).is_some();
        if running {
            return AutoPlan::Skip("background_job_running");
        }
        if self.changed_file_count() > 50 {
            return AutoPlan::Skip("too_many_changed_files");
        }
        AutoPlan::Run
    }

    fn finish_auto(&self, plan: AutoPlan, result: Value) -> Value {
        let AutoPlan::Skip(reason) = plan else { return result };
        let hint = match reason {
            "background_job_running" => "Auto-update skipped: a background analyze/update is in progress. Poll job_status and retry when it completes.",
            "too_many_changed_files" => "Auto-update skipped: more than 50 files changed. Run 'chisel update' or start_job(kind='update') to refresh the DB.",
            _ => "Run 'chisel update' first to include new or changed files.",
        };
        match result {
            Value::Object(obj) => {
                let mut value = Value::Object(obj);
                value.insert("auto_update_skip_reason", Value::String(reason.into()));
                value.insert("hint", Value::String(hint.into()));
                value
            }
            Value::Array(items) => Value::object(vec![
                ("items".into(), Value::Array(items)),
                ("auto_update_skip_reason".into(), Value::String(reason.into())),
                ("hint".into(), Value::String(hint.into())),
            ]),
            other => other,
        }
    }

    fn apply_limit(&self, name: &str, args: &Value, result: Value) -> Value {
        let default = match name {
            "risk_map" => 30,
            "test_gaps" => 50,
            "history" => 20,
            _ => 100,
        };
        let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(default).clamp(0, 1000);
        match result {
            Value::Array(items) => {
                if items.len() as i64 <= limit {
                    return Value::Array(items);
                }
                let total = items.len() as i64;
                Value::object(vec![
                    ("items".into(), Value::Array(items.into_iter().take(limit as usize).collect())),
                    ("truncated".into(), Value::Bool(true)),
                    ("total".into(), Value::Int(total)),
                ])
            }
            Value::Object(mut pairs) => {
                if let Some(pos) = pairs.iter().position(|(k, _)| k == "files") {
                    if let Value::Array(items) = &pairs[pos].1 {
                        if items.len() as i64 > limit {
                            let total = items.len() as i64;
                            let kept: Vec<Value> = items.iter().take(limit as usize).cloned().collect();
                            pairs[pos].1 = Value::Array(kept);
                            pairs.push(("truncated".into(), Value::Bool(true)));
                            pairs.push(("total".into(), Value::Int(total)));
                        }
                    }
                }
                Value::Object(pairs)
            }
            other => other,
        }
    }

    fn count_code_files(&self) -> usize {
        self.scan_code_files(&self.git.fork()).len()
    }

    fn unknown_shard(&self, args: &Value) -> Option<Value> {
        let name = args.get("shard").and_then(|v| v.as_str()).unwrap_or("");
        if name.is_empty() {
            return None;
        }
        if self.shards.iter().any(|(n, _)| n == name) {
            return None;
        }
        let known: Vec<Value> = self.shards.iter().map(|(n, _)| Value::String(n.clone())).collect();
        Some(Value::object(vec![
            ("status".into(), Value::String("error".into())),
            ("message".into(), Value::String(format!("unknown shard: {name}"))),
            ("known_shards".into(), Value::Array(known)),
        ]))
    }

    pub fn start_job(&mut self, kind: &str) -> Result<Value, String> {
        if kind != "analyze" && kind != "update" {
            return Ok(Value::object(vec![
                ("status".into(), Value::String("error".into())),
                ("message".into(), Value::String("kind must be analyze or update".into())),
            ]));
        }
        let id = {
            let mut jobs = job_lock().lock().map_err(|e| e.to_string())?;
            if let Some(existing) = jobs.iter().find(|j| j.storage == self.store.dir() && (j.status == "queued" || j.status == "running")) {
                return Ok(Value::object(vec![
                    ("status".into(), Value::String("error".into())),
                    ("message".into(), Value::String("a job is already running".into())),
                    ("job_id".into(), Value::String(existing.id.clone())),
                ]));
            }
            let id = format!("job-{}", SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
            jobs.push(LiveJob {
                id: id.clone(),
                status: "queued".into(),
                progress: 0,
                cancel: false,
                project: self.project_dir.clone(),
                storage: self.store.dir().to_path_buf(),
                kind: kind.to_string(),
            });
            id
        };
        self.store.array_mut("jobs").push(Value::object(vec![
            ("job_id".into(), Value::String(id.clone())),
            ("status".into(), Value::String("queued".into())),
            ("kind".into(), Value::String(kind.to_string())),
        ]));
        self.store.save()?;
        let id_thread = id.clone();
        thread::spawn(move || {
            let (project, storage, kind) = {
                let guard = job_lock().lock().ok();
                let Some(jobs) = guard else { return };
                let Some(job) = jobs.iter().find(|j| j.id == id_thread) else { return };
                (job.project.clone(), job.storage.clone(), job.kind.clone())
            };
            set_job(&id_thread, "running", 1);
            let Ok(mut engine) = Engine::open(&project, Some(&storage)) else {
                set_job(&id_thread, "error", 0);
                return;
            };
            engine.active_job = Some(id_thread.clone());
            let result = if kind == "update" { engine.update() } else { engine.analyze() };
            let status = result.ok().and_then(|v| v.get("status").and_then(|s| s.as_str()).map(|s| s.to_string()));
            if status.as_deref() == Some("cancelled") || job_should_stop(&id_thread) {
                set_job(&id_thread, "cancelled", 0);
            } else {
                set_job(&id_thread, "completed", 100);
            }
        });
        Ok(Value::object(vec![
            ("job_id".into(), Value::String(id)),
            ("status".into(), Value::String("queued".into())),
            ("kind".into(), Value::String(kind.to_string())),
        ]))
    }

    pub fn job_status_value(&mut self, id: &str) -> Value {
        if let Ok(jobs) = job_lock().lock() {
            if let Some(job) = jobs.iter().find(|j| j.id == id) {
                return Value::object(vec![
                    ("job_id".into(), Value::String(job.id.clone())),
                    ("status".into(), Value::String(job.status.clone())),
                    ("progress_pct".into(), Value::Int(job.progress)),
                ]);
            }
        }
        self.store.reload().ok();
        for row in self.store.array("jobs") {
            if row.get("job_id").and_then(|v| v.as_str()) == Some(id) {
                return row;
            }
        }
        Value::object(vec![
            ("status".into(), Value::String("error".into())),
            ("message".into(), Value::String(format!("unknown job: {id}"))),
            ("job_id".into(), Value::String(id.into())),
        ])
    }

    pub fn cancel_job_value(&mut self, id: &str) -> Value {
        if let Ok(mut jobs) = job_lock().lock() {
            if let Some(job) = jobs.iter_mut().find(|j| j.id == id) {
                job.cancel = true;
                return Value::object(vec![
                    ("job_id".into(), Value::String(id.into())),
                    ("status".into(), Value::String("cancel_requested".into())),
                ]);
            }
        }
        Value::object(vec![
            ("status".into(), Value::String("error".into())),
            ("message".into(), Value::String(format!("unknown job: {id}"))),
        ])
    }

    pub fn dispatch(&mut self, name: &str, args: &Value) -> Result<Value, String> {
        if let Some(field) = first_missing_required(name, args) {
            return Ok(missing_arg(name, &field));
        }
        if let Some(err) = self.unknown_shard(args) {
            if matches!(name, "analyze" | "update" | "start_job") {
                return Ok(err);
            }
        }
        self.working_tree = args.get("working_tree").and_then(|v| v.as_bool()).unwrap_or(false);
        self.diff_ref_override = args.get("ref").and_then(|v| v.as_str()).map(|s| s.to_string());
        self.directory = args.get("directory").and_then(|v| v.as_str()).filter(|s| !s.is_empty() && *s != ".").map(|s| s.trim_end_matches('/').to_string());
        let auto = self.auto_plan(name, args);
        if matches!(auto, AutoPlan::Run) {
            let _ = self.update();
        }
        let result = match name {
            "analyze" => {
                let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
                if force && self.count_code_files() > 300 {
                    let mut job = self.start_job("analyze")?;
                    job.insert("status", Value::String("auto_queued".into()));
                    return Ok(job);
                }
                self.analyze()
            }
            "update" => self.update(),
            "risk_map" => self.risk_map(),
            "diff_impact" => self.diff_impact(),
            "suggest_tests" => {
                let file = args.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
                self.suggest_tests(file)
            }
            "stale_tests" => self.stale_tests(),
            "stats" => self.stats_with(args.get("include_results").and_then(|v| v.as_bool()).unwrap_or(false)),
            "record_result" => {
                let id = args.get("test_id").and_then(|v| v.as_str()).unwrap_or("");
                let passed = args.get("passed").and_then(|v| v.as_bool()).unwrap_or(false);
                self.record_result(id, passed)
            }
            "impact" => {
                let files: Vec<String> = args.get("files").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
                if !self.shards.is_empty() {
                    self.aggregate_impact(&files)
                } else {
                    self.store.reload()?;
                    if !self.has_data() { return Ok(Self::no_data()); }
                    Ok(Value::Array(self.impacted(&files, &BTreeMap::new(), &BTreeSet::new())))
                }
            }
            "test_gaps" => self.test_gaps(),
            "triage" => self.triage(),
            "churn" | "ownership" | "coupling" | "history" | "who_reviews" => self.simple_file_tool(name, args),
            "optimize_storage" => {
                self.store.save()?;
                Ok(Value::object(vec![("optimized".into(), Value::Bool(true))]))
            }
            "start_job" => {
                let kind = args.get("kind").and_then(|v| v.as_str()).unwrap_or("");
                self.start_job(kind)
            }
            "job_status" => {
                let id = args.get("job_id").and_then(|v| v.as_str()).unwrap_or("");
                Ok(self.job_status_value(id))
            }
            "cancel_job" => {
                let id = args.get("job_id").and_then(|v| v.as_str()).unwrap_or("");
                Ok(self.cancel_job_value(id))
            }
            "acquire_file_lock" | "release_file_lock" | "refresh_file_lock" | "check_file_lock" | "check_locks" | "list_file_locks" => {
                self.file_lock_tool(name, args)
            }
            _ => Err(format!("unknown tool: {name}")),
        }?;
        let limited = self.apply_limit(name, args, result);
        Ok(self.finish_auto(auto, limited))
    }

    fn test_gaps(&mut self) -> Result<Value, String> {
        if !self.shards.is_empty() {
            let mut gaps = Vec::new();
            let mut any = false;
            for (name, _) in self.shards.clone() {
                let sub = self.open_shard(&name)?;
                if !sub.has_data() {
                    continue;
                }
                any = true;
                gaps.extend(sub.gap_rows());
            }
            if !any {
                return Ok(Self::no_data());
            }
            gaps.sort_by(|a, b| {
                let sa = a.get("churn_score").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let sb = b.get("churn_score").and_then(|v| v.as_f64()).unwrap_or(0.0);
                sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal).then_with(|| {
                    a.get("file_path").and_then(|v| v.as_str()).unwrap_or("").cmp(b.get("file_path").and_then(|v| v.as_str()).unwrap_or(""))
                })
            });
            return Ok(Value::Array(gaps));
        }
        self.store.reload()?;
        if !self.has_data() { return Ok(Self::no_data()); }
        Ok(Value::Array(self.gap_rows()))
    }

    fn gap_rows(&self) -> Vec<Value> {
        let tests = self.test_file_set();
        let edges = self.store.array("test_edges");
        let churn = self.store.array("churn");
        let mut gaps = Vec::new();
        for u in self.store.array("code_units") {
            let path = u.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
            if tests.contains(path) { continue; }
            let id = u.get("id").and_then(|v| v.as_str()).unwrap_or("");
            if edges.iter().any(|e| e.get("code_id").and_then(|v| v.as_str()) == Some(id)) {
                continue;
            }
            let name = u.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let stat = churn.iter().find(|c| {
                c.get("file_path").and_then(|v| v.as_str()) == Some(path)
                    && c.get("unit_name").and_then(|v| v.as_str()).unwrap_or("") == name
            });
            let churn_score = stat.and_then(|c| c.get("churn_score")).and_then(|v| v.as_f64()).unwrap_or(0.0);
            let commit_count = stat.and_then(|c| c.get("commit_count")).and_then(|v| v.as_i64()).unwrap_or(0);
            gaps.push(Value::object(vec![
                ("id".into(), Value::String(id.into())),
                ("file_path".into(), Value::String(path.into())),
                ("name".into(), Value::String(name.into())),
                ("unit_type".into(), u.get("unit_type").cloned().unwrap_or(Value::String("function".into()))),
                ("line_start".into(), u.get("line_start").cloned().unwrap_or(Value::Int(0))),
                ("line_end".into(), u.get("line_end").cloned().unwrap_or(Value::Int(0))),
                ("churn_score".into(), Value::Float(churn_score)),
                ("commit_count".into(), Value::Int(commit_count)),
            ]));
        }
        gaps.sort_by(|a, b| {
            let sa = a.get("churn_score").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let sb = b.get("churn_score").and_then(|v| v.as_f64()).unwrap_or(0.0);
            sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal).then_with(|| {
                let fa = a.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
                let fb = b.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
                fa.cmp(fb).then_with(|| {
                    let na = a.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    let nb = b.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    na.cmp(nb)
                })
            })
        });
        gaps
    }

    fn stale_rows(&self) -> Vec<Value> {
        let edges = self.store.array("test_edges");
        let ids: BTreeSet<String> = self.store.array("code_units").into_iter().filter_map(|u| u.get("id").and_then(|v| v.as_str()).map(|s| s.to_string())).collect();
        let tests = self.store.array("test_units");
        let mut stale = Vec::new();
        for e in &edges {
            let code_id = e.get("code_id").and_then(|v| v.as_str()).unwrap_or("");
            if ids.contains(code_id) { continue; }
            let tid = e.get("test_id").and_then(|v| v.as_str()).unwrap_or("");
            let name = tests.iter().find(|t| t.get("id").and_then(|v| v.as_str()) == Some(tid)).and_then(|t| t.get("name")).and_then(|v| v.as_str()).unwrap_or("");
            stale.push(Value::object(vec![
                ("test_id".into(), Value::String(tid.into())),
                ("test_name".into(), Value::String(name.into())),
                ("missing_code_id".into(), Value::String(code_id.into())),
                ("edge_type".into(), e.get("edge_type").cloned().unwrap_or(Value::String("import".into()))),
            ]));
        }
        stale
    }

    fn triage(&mut self) -> Result<Value, String> {
        if !self.shards.is_empty() {
            let risk = self.aggregate_risk_map()?;
            if risk.get("status").and_then(|v| v.as_str()) == Some("no_data") {
                return Ok(risk);
            }
            let mut top = risk.get("files").and_then(|v| v.as_array()).unwrap_or(&[]).to_vec();
            if top.len() > 10 {
                top.truncate(10);
            }
            let top_paths: BTreeSet<String> = top.iter().filter_map(|f| f.get("file_path").and_then(|v| v.as_str()).map(|s| s.to_string())).collect();
            let mut gaps = Vec::new();
            let mut stale = Vec::new();
            let mut edges = 0i64;
            let mut results = 0i64;
            for (name, _) in self.shards.clone() {
                let sub = self.open_shard(&name)?;
                gaps.extend(sub.gap_rows().into_iter().filter(|g| {
                    g.get("file_path").and_then(|v| v.as_str()).is_some_and(|p| top_paths.contains(p))
                }));
                stale.extend(sub.stale_rows());
                edges += sub.store.array_len("test_edges") as i64;
                results += sub.store.array_len("test_results") as i64;
            }
            return Ok(Value::object(vec![
                ("top_risk_files".into(), Value::Array(top.clone())),
                ("test_gaps".into(), Value::Array(gaps.clone())),
                ("stale_tests".into(), Value::Array(stale.clone())),
                ("summary".into(), Value::object(vec![
                    ("files_triaged".into(), Value::Int(top.len() as i64)),
                    ("total_test_gaps".into(), Value::Int(gaps.len() as i64)),
                    ("total_stale_tests".into(), Value::Int(stale.len() as i64)),
                    ("test_edge_count".into(), Value::Int(edges)),
                    ("test_result_count".into(), Value::Int(results)),
                    ("coupling_threshold".into(), Value::Int(impact::coupling_threshold(0))),
                    ("cross_shard_test_edges".into(), Value::String("unsupported".into())),
                    ("active_locks".into(), Value::Int(0)),
                ])),
            ]));
        }
        self.store.reload()?;
        if !self.has_data() { return Ok(Self::no_data()); }
        let risk = self.risk_map_with(false)?;
        let mut top = risk.get("files").and_then(|v| v.as_array()).unwrap_or(&[]).to_vec();
        if top.len() > 10 {
            top.truncate(10);
        }
        let top_paths: BTreeSet<String> = top.iter().filter_map(|f| f.get("file_path").and_then(|v| v.as_str()).map(|s| s.to_string())).collect();
        let gaps: Vec<Value> = self.gap_rows().into_iter().filter(|g| {
            g.get("file_path").and_then(|v| v.as_str()).is_some_and(|p| top_paths.contains(p))
        }).collect();
        let stale = self.stale_rows();
        let commits_n = self.store.array("commits").len() as i64;
        Ok(Value::object(vec![
            ("top_risk_files".into(), Value::Array(top.clone())),
            ("test_gaps".into(), Value::Array(gaps.clone())),
            ("stale_tests".into(), Value::Array(stale.clone())),
            ("summary".into(), Value::object(vec![
                ("files_triaged".into(), Value::Int(top.len() as i64)),
                ("total_test_gaps".into(), Value::Int(gaps.len() as i64)),
                ("total_stale_tests".into(), Value::Int(stale.len() as i64)),
                ("test_edge_count".into(), Value::Int(self.store.array("test_edges").len() as i64)),
                ("test_result_count".into(), Value::Int(self.store.array("test_results").len() as i64)),
                ("coupling_threshold".into(), Value::Int(impact::coupling_threshold(commits_n))),
                ("active_locks".into(), Value::Int(self.active_locks().len() as i64)),
            ])),
        ]))
    }

    fn simple_file_tool(&mut self, name: &str, args: &Value) -> Result<Value, String> {
        let file = args.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
        if !self.shards.is_empty() {
            if let Some(shard) = shard_name_for(file, &self.shards) {
                let mut sub = self.open_shard(shard)?;
                return sub.simple_file_tool(name, args);
            }
        }
        self.store.reload()?;
        if !self.has_data() { return Ok(Self::no_data()); }
        match name {
            "churn" => {
                let rows: Vec<Value> = self.store.array("churn").into_iter().filter(|c| c.get("file_path").and_then(|v| v.as_str()) == Some(file)).collect();
                Ok(Value::Array(rows))
            }
            "history" => Ok(Value::Array(self.history_rows(file))),
            "ownership" => Ok(Value::Array(self.ownership_rows(file))),
            "who_reviews" => Ok(Value::Array(self.reviewer_rows(file))),
            "coupling" => Ok(self.coupling_row(file, args.get("min_count").and_then(|v| v.as_i64()).unwrap_or(3))),
            _ => Err("unreachable".into()),
        }
    }

    fn history_rows(&self, file: &str) -> Vec<Value> {
        let commits = self.store.array("commits");
        let files = self.store.array("commit_files");
        let mut rows = Vec::new();
        for f in &files {
            if f.get("path").and_then(|v| v.as_str()) != Some(file) { continue; }
            let hash = f.get("hash").and_then(|v| v.as_str()).unwrap_or("");
            let Some(c) = commits.iter().find(|c| c.get("hash").and_then(|v| v.as_str()) == Some(hash)) else { continue };
            rows.push(Value::object(vec![
                ("hash".into(), c.get("hash").cloned().unwrap_or(Value::String(hash.into()))),
                ("author".into(), c.get("author").cloned().unwrap_or(Value::String(String::new()))),
                ("author_email".into(), c.get("author_email").cloned().unwrap_or(Value::String(String::new()))),
                ("date".into(), c.get("date").cloned().unwrap_or(Value::String(String::new()))),
                ("message".into(), c.get("message").cloned().unwrap_or(Value::String(String::new()))),
                ("insertions".into(), f.get("insertions").cloned().unwrap_or(Value::Int(0))),
                ("deletions".into(), f.get("deletions").cloned().unwrap_or(Value::Int(0))),
            ]));
        }
        rows.sort_by(|a, b| {
            let da = a.get("date").and_then(|v| v.as_str()).unwrap_or("");
            let db = b.get("date").and_then(|v| v.as_str()).unwrap_or("");
            db.cmp(da)
        });
        rows
    }

    fn ownership_rows(&self, file: &str) -> Vec<Value> {
        let hash = self.store.array("file_hashes").into_iter().find(|h| h.get("file_path").and_then(|v| v.as_str()) == Some(file)).and_then(|h| h.get("hash").and_then(|v| v.as_str()).map(|s| s.to_string())).unwrap_or_default();
        let mut lines: BTreeMap<String, (i64, String)> = BTreeMap::new();
        let mut total = 0i64;
        for b in self.store.array("blame") {
            if b.get("file_path").and_then(|v| v.as_str()) != Some(file) { continue; }
            if b.get("content_hash").and_then(|v| v.as_str()) != Some(hash.as_str()) { continue; }
            let author = b.get("author").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if author.is_empty() { continue; }
            let email = b.get("author_email").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let ls = b.get("line_start").and_then(|v| v.as_i64()).unwrap_or(0);
            let le = b.get("line_end").and_then(|v| v.as_i64()).unwrap_or(ls);
            let n = (le - ls + 1).max(0);
            let slot = lines.entry(author).or_insert((0, email.clone()));
            slot.0 += n;
            if !email.is_empty() { slot.1 = email; }
            total += n;
        }
        let mut rows: Vec<Value> = lines.into_iter().map(|(author, (count, email))| {
            let pct = if total > 0 { count as f64 / total as f64 * 100.0 } else { 0.0 };
            Value::object(vec![
                ("author".into(), Value::String(author)),
                ("author_email".into(), Value::String(email)),
                ("line_count".into(), Value::Int(count)),
                ("percentage".into(), Value::Float(crate::json::py_round(pct, 2))),
                ("role".into(), Value::String("original_author".into())),
            ])
        }).collect();
        rows.sort_by(|a, b| {
            let la = a.get("line_count").and_then(|v| v.as_i64()).unwrap_or(0);
            let lb = b.get("line_count").and_then(|v| v.as_i64()).unwrap_or(0);
            lb.cmp(&la)
        });
        rows
    }

    fn reviewer_rows(&self, file: &str) -> Vec<Value> {
        let history = self.history_rows(file);
        if history.is_empty() { return Vec::new(); }
        let now = impact::now_unix();
        let mut stats: BTreeMap<String, (i64, String, i64, i64, String, f64, f64)> = BTreeMap::new();
        for c in &history {
            let author = c.get("author").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let email = c.get("author_email").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let date = c.get("date").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let ins = c.get("insertions").and_then(|v| v.as_i64()).unwrap_or(0);
            let del = c.get("deletions").and_then(|v| v.as_i64()).unwrap_or(0);
            let slot = stats.entry(author).or_insert((0, String::new(), 0, 0, String::new(), 0.0, 0.0));
            slot.0 += 1;
            if !email.is_empty() { slot.1 = email; }
            slot.2 += ins;
            slot.3 += del;
            if let Some(ts) = impact::parse_iso_unix(&date) {
                if date > slot.4 {
                    slot.4 = date.clone();
                    slot.6 = ts;
                }
                let days = ((now - ts) / 86400.0).max(0.0);
                slot.5 += 1.0 / (1.0 + days);
            } else {
                slot.5 += 0.01;
            }
        }
        let total: f64 = stats.values().map(|s| s.5).sum();
        let mut rows: Vec<Value> = stats.into_iter().map(|(author, (commits, email, ins, del, last, score, last_ts))| {
            let days = if last_ts > 0.0 { Some(crate::json::py_round((now - last_ts) / 86400.0, 0) as i64) } else { None };
            let pct = if total > 0.0 { score / total * 100.0 } else { 0.0 };
            Value::object(vec![
                ("author".into(), Value::String(author)),
                ("author_email".into(), Value::String(email)),
                ("recent_commits".into(), Value::Int(commits)),
                ("last_commit_date".into(), Value::String(last)),
                ("days_since_last_commit".into(), days.map(Value::Int).unwrap_or(Value::Null)),
                ("insertions".into(), Value::Int(ins)),
                ("deletions".into(), Value::Int(del)),
                ("percentage".into(), Value::Float(crate::json::py_round(pct, 2))),
                ("role".into(), Value::String("suggested_reviewer".into())),
            ])
        }).collect();
        rows.sort_by(|a, b| {
            let pa = a.get("percentage").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let pb = b.get("percentage").and_then(|v| v.as_f64()).unwrap_or(0.0);
            pb.partial_cmp(&pa).unwrap_or(std::cmp::Ordering::Equal)
        });
        rows
    }

    fn coupling_row(&self, file: &str, min_count: i64) -> Value {
        let query_min: i64 = store::meta_get(&self.store.doc, "co_change_query_min").and_then(|s| s.parse().ok()).unwrap_or(3);
        let effective = min_count.max(query_min);
        let mut partners = Vec::new();
        for c in self.store.array("co_changes") {
            let n = c.get("co_commit_count").and_then(|v| v.as_i64()).unwrap_or(0);
            if n < effective { continue; }
            let a = c.get("file_a").and_then(|v| v.as_str()).unwrap_or("");
            let b = c.get("file_b").and_then(|v| v.as_str()).unwrap_or("");
            if a == file || b == file {
                partners.push(c);
            }
        }
        partners.sort_by(|a, b| {
            let na = a.get("co_commit_count").and_then(|v| v.as_i64()).unwrap_or(0);
            let nb = b.get("co_commit_count").and_then(|v| v.as_i64()).unwrap_or(0);
            nb.cmp(&na)
        });
        let mut neighbors = BTreeSet::new();
        for e in self.store.array("import_edges") {
            let a = e.get("importer_file").and_then(|v| v.as_str()).unwrap_or("");
            let b = e.get("imported_file").and_then(|v| v.as_str()).unwrap_or("");
            if a == file && b != file { neighbors.insert(b.to_string()); }
            if b == file && a != file { neighbors.insert(a.to_string()); }
        }
        let co_len = partners.len() as f64;
        let imp_len = neighbors.len() as f64;
        let co_norm = (co_len / impact::COCHANGE_CAP).min(1.0);
        let import_norm = (imp_len / impact::IMPORT_CAP).min(1.0);
        let effective_coupling = co_norm.max(import_norm).max(0.5 * co_norm + 0.5 * import_norm);
        Value::object(vec![
            ("co_change_partners".into(), Value::Array(partners)),
            ("import_partners".into(), Value::Array(neighbors.into_iter().map(|f| Value::object(vec![("file".into(), Value::String(f))])).collect())),
            ("co_change_breadth".into(), Value::Int(co_len as i64)),
            ("import_breadth".into(), Value::Int(imp_len as i64)),
            ("cochange_coupling".into(), Value::Float(crate::json::py_round(co_norm, 4))),
            ("import_coupling".into(), Value::Float(crate::json::py_round(import_norm, 4))),
            ("effective_coupling".into(), Value::Float(crate::json::py_round(effective_coupling, 4))),
        ])
    }

    fn active_locks(&self) -> Vec<Value> {
        let now = now_secs();
        self.store.array("file_locks").into_iter().filter(|l| l.get("expires_at").and_then(|v| v.as_f64()).unwrap_or(0.0) >= now).collect()
    }

    fn file_lock_tool(&mut self, name: &str, args: &Value) -> Result<Value, String> {
        let file = args.get("file_path").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let agent = args.get("agent_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let ttl = args.get("ttl").and_then(|v| v.as_i64()).unwrap_or(300);
        let purpose = args.get("purpose").and_then(|v| v.as_str()).map(|s| s.to_string());
        self.drop_expired_locks();
        match name {
            "acquire_file_lock" => {
                let existing = self.active_locks().into_iter().find(|l| l.get("file_path").and_then(|v| v.as_str()) == Some(file.as_str()));
                if let Some(lock) = existing {
                    let holder = lock.get("agent_id").and_then(|v| v.as_str()).unwrap_or("");
                    if holder != agent {
                        return Ok(Value::object(vec![
                            ("acquired".into(), Value::Bool(false)),
                            ("holder".into(), Value::String(holder.into())),
                            ("expires_at".into(), lock.get("expires_at").cloned().unwrap_or(Value::Null)),
                        ]));
                    }
                }
                let now = now_secs();
                let expires = now + ttl as f64;
                let kept: Vec<Value> = self.store.array("file_locks").into_iter().filter(|l| l.get("file_path").and_then(|v| v.as_str()) != Some(file.as_str())).collect();
                self.store.doc.insert("file_locks", Value::Array(kept));
                self.store.array_mut("file_locks").push(Value::object(vec![
                    ("file_path".into(), Value::String(file.clone())),
                    ("agent_id".into(), Value::String(agent)),
                    ("acquired_at".into(), Value::Float(now)),
                    ("expires_at".into(), Value::Float(expires)),
                    ("purpose".into(), purpose.map(Value::String).unwrap_or(Value::Null)),
                ]));
                self.store.save()?;
                Ok(Value::object(vec![
                    ("acquired".into(), Value::Bool(true)),
                    ("holder".into(), Value::Null),
                    ("expires_at".into(), Value::Float(expires)),
                ]))
            }
            "release_file_lock" => {
                let before = self.store.array("file_locks");
                let held = before.iter().any(|l| l.get("file_path").and_then(|v| v.as_str()) == Some(file.as_str()) && l.get("agent_id").and_then(|v| v.as_str()) == Some(agent.as_str()));
                let kept: Vec<Value> = before.into_iter().filter(|l| {
                    !(l.get("file_path").and_then(|v| v.as_str()) == Some(file.as_str()) && l.get("agent_id").and_then(|v| v.as_str()) == Some(agent.as_str()))
                }).collect();
                self.store.doc.insert("file_locks", Value::Array(kept));
                self.store.save()?;
                Ok(Value::object(vec![("released".into(), Value::Bool(held))]))
            }
            "refresh_file_lock" => {
                let now = now_secs();
                let expires = now + ttl as f64;
                let mut ok = false;
                for lock in self.store.array_mut("file_locks") {
                    if lock.get("file_path").and_then(|v| v.as_str()) == Some(file.as_str()) && lock.get("agent_id").and_then(|v| v.as_str()) == Some(agent.as_str()) {
                        lock.insert("expires_at", Value::Float(expires));
                        ok = true;
                    }
                }
                self.store.save()?;
                Ok(Value::object(vec![
                    ("refreshed".into(), Value::Bool(ok)),
                    ("expires_at".into(), if ok { Value::Float(expires) } else { Value::Null }),
                ]))
            }
            "check_file_lock" => Ok(self.lock_status(&file)),
            "check_locks" => {
                let paths = args.get("file_paths").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect()).unwrap_or_else(|| if file.is_empty() { Vec::new() } else { vec![file] });
                let mut conflicts = Vec::new();
                for fp in &paths {
                    let status = self.lock_status(fp);
                    if status.get("locked").and_then(|v| v.as_bool()) == Some(true) {
                        conflicts.push(Value::object(vec![
                            ("file_path".into(), Value::String(fp.clone())),
                            ("holder".into(), status.get("holder").cloned().unwrap_or(Value::Null)),
                            ("expires_at".into(), status.get("expires_at").cloned().unwrap_or(Value::Null)),
                            ("ttl_remaining".into(), status.get("ttl_remaining").cloned().unwrap_or(Value::Null)),
                            ("stale".into(), status.get("stale").cloned().unwrap_or(Value::Bool(false))),
                        ]));
                    }
                }
                Ok(Value::object(vec![
                    ("conflicts".into(), Value::Array(conflicts)),
                    ("checked".into(), Value::Int(paths.len() as i64)),
                ]))
            }
            "list_file_locks" => {
                let agent_filter = args.get("agent_id").and_then(|v| v.as_str()).map(|s| s.to_string());
                let now = now_secs();
                let mut locks = Vec::new();
                for lock in self.active_locks() {
                    if let Some(agent) = agent_filter.as_deref() {
                        if lock.get("agent_id").and_then(|v| v.as_str()) != Some(agent) { continue; }
                    }
                    let expires = lock.get("expires_at").and_then(|v| v.as_f64()).unwrap_or(now);
                    let mut row = lock;
                    row.insert("ttl_remaining", Value::Float(crate::json::py_round((expires - now).max(0.0), 1)));
                    locks.push(row);
                }
                let total = locks.len() as i64;
                Ok(Value::object(vec![
                    ("locks".into(), Value::Array(locks)),
                    ("total".into(), Value::Int(total)),
                ]))
            }
            _ => Err("unreachable".into()),
        }
    }

    fn drop_expired_locks(&mut self) {
        let now = now_secs();
        let kept: Vec<Value> = self.store.array("file_locks").into_iter().filter(|l| l.get("expires_at").and_then(|v| v.as_f64()).unwrap_or(0.0) >= now).collect();
        self.store.doc.insert("file_locks", Value::Array(kept));
    }

    fn lock_status(&self, file: &str) -> Value {
        let now = now_secs();
        let lock = self.active_locks().into_iter().find(|l| l.get("file_path").and_then(|v| v.as_str()) == Some(file));
        let Some(lock) = lock else {
            return Value::object(vec![
                ("locked".into(), Value::Bool(false)),
                ("holder".into(), Value::Null),
                ("expires_at".into(), Value::Null),
                ("ttl_remaining".into(), Value::Null),
            ]);
        };
        let expires = lock.get("expires_at").and_then(|v| v.as_f64()).unwrap_or(now);
        let ttl = crate::json::py_round((expires - now).max(0.0), 1);
        Value::object(vec![
            ("locked".into(), Value::Bool(true)),
            ("holder".into(), lock.get("agent_id").cloned().unwrap_or(Value::Null)),
            ("acquired_at".into(), lock.get("acquired_at").cloned().unwrap_or(Value::Null)),
            ("expires_at".into(), Value::Float(expires)),
            ("ttl_remaining".into(), Value::Float(ttl)),
            ("stale".into(), Value::Bool(ttl < 60.0)),
            ("purpose".into(), lock.get("purpose").cloned().unwrap_or(Value::Null)),
        ])
    }
}

fn now_secs() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

fn churn_row(rel: &str, unit: &str, count: i64, authors: i64, ins: i64, del: i64, last: &str, score: f64) -> Value {
    Value::object(vec![
        ("file_path".into(), Value::String(rel.into())),
        ("unit_name".into(), Value::String(unit.into())),
        ("commit_count".into(), Value::Int(count)),
        ("distinct_authors".into(), Value::Int(authors)),
        ("total_insertions".into(), Value::Int(ins)),
        ("total_deletions".into(), Value::Int(del)),
        ("last_changed".into(), if last.is_empty() { Value::Null } else { Value::String(last.into()) }),
        ("churn_score".into(), Value::Float(score)),
    ])
}

fn unit_obj(rel: &str, u: &CodeUnit, hash: &str) -> Value {
    Value::object(vec![
        ("id".into(), Value::String(extract::unit_id(rel, u))),
        ("file_path".into(), Value::String(rel.into())),
        ("name".into(), Value::String(u.name.clone())),
        ("unit_type".into(), Value::String(u.unit_type.clone())),
        ("line_start".into(), Value::Int(u.line_start as i64)),
        ("line_end".into(), Value::Int(u.line_end as i64)),
        ("content_hash".into(), Value::String(hash.into())),
    ])
}

fn looks_like_test(path: &str) -> bool {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let ext_ok = |suffix: &str| name.ends_with(suffix);
    (name.starts_with("test_") && name.ends_with(".py"))
        || name.ends_with("_test.py")
        || name.ends_with("_test.go")
        || name.ends_with("_test.rs")
        || name.ends_with("_test.dart")
        || name.ends_with("_spec.rb")
        || (name.starts_with("test_") && name.ends_with(".rb"))
        || ext_ok(".test.js")
        || ext_ok(".test.jsx")
        || ext_ok(".test.ts")
        || ext_ok(".test.tsx")
        || ext_ok(".spec.js")
        || ext_ok(".spec.jsx")
        || ext_ok(".spec.ts")
        || ext_ok(".spec.tsx")
        || name.ends_with("Test.cs")
        || name.ends_with("Tests.cs")
        || name.ends_with("Test.java")
        || (name.starts_with("Test") && name.ends_with(".java"))
        || name.ends_with("Test.kt")
        || name.ends_with("Test.swift")
        || name.ends_with("Tests.swift")
        || name.ends_with("Test.php")
}

fn resolved_import_files(deps: &[Dep], units: &[Value]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for dep in deps {
        let Some(module) = dep.module_path.as_deref() else { continue };
        for u in units {
            let path = u.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
            if extract::matches_import_path(path, module) {
                out.insert(path.to_string());
            }
        }
    }
    out
}

fn match_dep(from_file: &str, dep: &Dep, units: &[Value], resolved: &BTreeSet<String>) -> Vec<String> {
    let mut matched = Vec::new();
    if let Some(module) = dep.module_path.as_deref() {
        for u in units {
            let path = u.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
            let name = u.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let bare = name.rsplit('.').next().unwrap_or(name);
            let path_ok = extract::matches_import_path(path, module) || extract::matches_relative_import(from_file, path, module);
            if path_ok && (name == dep.name || bare == dep.name) {
                if let Some(id) = u.get("id").and_then(|v| v.as_str()) {
                    matched.push(id.to_string());
                }
            }
        }
    }
    if matched.is_empty() && dep.module_path.is_none() {
        for u in units {
            let path = u.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
            if !resolved.is_empty() && !resolved.contains(path) {
                continue;
            }
            let name = u.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let bare = name.rsplit('.').next().unwrap_or(name);
            if name == dep.name || bare == dep.name {
                if let Some(id) = u.get("id").and_then(|v| v.as_str()) {
                    matched.push(id.to_string());
                }
            }
        }
    }
    matched
}

fn commits_from_store(doc: &Value) -> Vec<crate::git::Commit> {
    let commits = doc.get("commits").and_then(|v| v.as_array()).map(|a| a.to_vec()).unwrap_or_default();
    let files = doc.get("commit_files").and_then(|v| v.as_array()).map(|a| a.to_vec()).unwrap_or_default();
    let mut by_hash: BTreeMap<String, Vec<crate::git::GitFile>> = BTreeMap::new();
    for row in files {
        let hash = row.get("hash").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if hash.is_empty() {
            continue;
        }
        by_hash.entry(hash).or_default().push(crate::git::GitFile {
            path: row.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            insertions: row.get("insertions").and_then(|v| v.as_i64()).unwrap_or(0),
            deletions: row.get("deletions").and_then(|v| v.as_i64()).unwrap_or(0),
        });
    }
    commits.into_iter().filter_map(|row| {
        let hash = row.get("hash").and_then(|v| v.as_str())?.to_string();
        Some(crate::git::Commit {
            hash: hash.clone(),
            author: row.get("author").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            author_email: row.get("author_email").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            date: row.get("date").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            message: row.get("message").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            files: by_hash.remove(&hash).unwrap_or_default(),
        })
    }).collect()
}

fn path_stem(path: &str) -> &str {
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    base.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(base)
}

fn stem_score(source: &str, test_file: &str) -> f64 {
    let source_stem = path_stem(source);
    let test_stem = path_stem(test_file);
    let mut score = if test_stem == source_stem {
        1.0
    } else if !source_stem.is_empty() && (test_stem.contains(source_stem) || source_stem.contains(test_stem)) {
        0.5
    } else {
        0.1
    };
    let source_dir = source.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("");
    let test_dir = test_file.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("");
    if !source_dir.is_empty() && !test_dir.is_empty() {
        let source_last = source_dir.rsplit('/').next().unwrap_or(source_dir);
        let test_last = test_dir.rsplit('/').next().unwrap_or(test_dir);
        if source_last == test_last {
            score = (score + 0.3_f64).min(1.0);
        }
    }
    score
}

fn co_changes_of(commits: &[crate::git::Commit], min_count: i64) -> Vec<Value> {
    let mut counts: BTreeMap<(String, String), (i64, String)> = BTreeMap::new();
    for c in commits {
        let mut paths: Vec<String> = c.files.iter().map(|f| f.path.clone()).collect();
        paths.sort();
        paths.dedup();
        if paths.len() < 2 || paths.len() > 50 {
            continue;
        }
        for i in 0..paths.len() {
            for j in (i + 1)..paths.len() {
                let key = (paths[i].clone(), paths[j].clone());
                let slot = counts.entry(key).or_insert((0, String::new()));
                slot.0 += 1;
                if c.date > slot.1 {
                    slot.1 = c.date.clone();
                }
            }
        }
    }
    let mut rows = Vec::new();
    for ((a, b), (n, date)) in counts {
        if n >= min_count {
            rows.push(Value::object(vec![
                ("file_a".into(), Value::String(a)),
                ("file_b".into(), Value::String(b)),
                ("co_commit_count".into(), Value::Int(n)),
                ("last_co_commit".into(), Value::String(date)),
            ]));
        }
    }
    rows.sort_by(|a, b| {
        let na = a.get("co_commit_count").and_then(|v| v.as_i64()).unwrap_or(0);
        let nb = b.get("co_commit_count").and_then(|v| v.as_i64()).unwrap_or(0);
        nb.cmp(&na)
    });
    rows
}

fn walk_code(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for ent in rd.flatten() {
            let path = ent.path();
            let name = ent.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || name == "target" || name == "node_modules" || name == "__pycache__" {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(rel) = path.strip_prefix(root) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    out
}
