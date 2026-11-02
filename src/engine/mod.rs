//! Analysis engine. Tool names and result shapes follow `AGENTS.md`.
//! The snapshot is `.chisel/store`. `chisel.db` is never opened.

mod analyze;
mod query;
mod locks;

use crate::extract::{self, CodeUnit, Dep};
use crate::git::{self, GitAnalyzer};
use crate::impact;
use crate::json::Value;
use crate::store::{self, Store};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::thread;

pub(crate) struct LiveJob {
    id: String,
    status: String,
    progress: i64,
    cancel: bool,
    project: PathBuf,
    storage: PathBuf,
    kind: String,
}

pub(crate) fn job_lock() -> &'static Mutex<Vec<LiveJob>> {
    static JOBS: Mutex<Vec<LiveJob>> = Mutex::new(Vec::new());
    &JOBS
}

pub(crate) const UNASSIGNED_SHARD: &str = "_unassigned";

pub(crate) fn shard_name_for<'a>(file: &str, shards: &'a [(String, String)]) -> Option<&'a str> {
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

pub(crate) fn shard_owner<'a>(file: &str, shards: &'a [(String, String)]) -> &'a str {
    shard_name_for(file, shards).unwrap_or(UNASSIGNED_SHARD)
}

pub(crate) fn parse_shard_spec(spec: &str, project: &Path) -> Vec<(String, String)> {
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

pub(crate) fn shards_from_toml(text: &str) -> Option<String> {
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

pub(crate) fn load_shard_config(project: &Path) -> Vec<(String, String)> {
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

pub(crate) fn job_cancel_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("job-cancel-{id}"))
}

pub(crate) fn job_should_stop(id: &str, dir: &Path) -> bool {
    if job_cancel_path(dir, id).exists() {
        return true;
    }
    job_lock().lock().ok().and_then(|jobs| {
        jobs.iter().find(|j| j.id == id).map(|j| j.cancel || j.status == "cancelled")
    }).unwrap_or(false)
}

pub(crate) fn set_job(id: &str, status: &str, progress: i64) {
    if let Ok(mut jobs) = job_lock().lock() {
        if let Some(job) = jobs.iter_mut().find(|j| j.id == id) {
            job.status = status.to_string();
            job.progress = progress;
        }
    }
}

const NO_DATA_MSG: &str = "No analysis data. Run 'chisel analyze' on this project first.";

pub(crate) fn missing_arg(tool: &str, field: &str) -> Value {
    let mut value = Value::object(vec![
        ("status".into(), Value::String("error".into())),
        ("message".into(), Value::String(format!("{tool} requires {field}"))),
    ]);
    if tool == "suggest_tests" && field == "file_path" {
        value.insert("hint", Value::String("Pass the source file to rank tests for.".into()));
    }
    value
}

pub(crate) fn argument_missing(args: &Value, field: &str, ty: &str) -> bool {
    let Some(value) = args.get(field) else { return true };
    match value {
        Value::Null => true,
        Value::String(text) if text.is_empty() => true,
        Value::Array(items) if items.is_empty() => true,
        _ if ty == "boolean" => !matches!(value, Value::Bool(_)),
        _ => false,
    }
}

pub(crate) fn first_missing_required(name: &str, args: &Value) -> Option<String> {
    for field in crate::schemas::required_fields(name) {
        let ty = crate::schemas::field_type(name, &field).unwrap_or_default();
        if argument_missing(args, &field, &ty) {
            return Some(field);
        }
    }
    None
}

pub(crate) enum AutoPlan {
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
    pub bare_index_builds: u64,
    coverage_mode: String,
    proximity_adjustment: bool,
    exclude_new_file_boost: bool,
    /// Parent storage dir when this engine is a shard view. Result rows live there.
    result_log: Option<PathBuf>,
    /// `(shard name, full shard config)` for overlaying cross edges. `shards` stays empty on a view.
    shard_view: Option<(String, Vec<(String, String)>)>,
    overlay_applied: bool,
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
            bare_index_builds: 0,
            coverage_mode: "line".into(),
            proximity_adjustment: true,
            exclude_new_file_boost: false,
            result_log: None,
            shard_view: None,
            overlay_applied: false,
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

}

pub(crate) fn analyze_hint(test_files: i64) -> String {
    if test_files == 0 {
        "No test files matched test_*.py, *_test.py, *_test.go, *_test.rs, *.test.ts, *.test.js, *.spec.ts, or *.spec.js. For large repos, use start_job with kind='analyze' to avoid MCP timeouts.".into()
    } else {
        "For large repos, use start_job with kind='analyze' to avoid MCP timeouts.".into()
    }
}

pub(crate) fn stale_status() -> Value {
    Value::object(vec![
        ("status".into(), Value::String("error".into())),
        ("message".into(), Value::String("stale generation".into())),
    ])
}

pub(crate) fn push_extractor_warnings(summary: &mut Vec<(String, Value)>) {
    let warnings = extract::take_extractor_warnings();
    if warnings.is_empty() {
        return;
    }
    summary.push((
        "extractor_warnings".into(),
        Value::Array(warnings.into_iter().map(Value::String).collect()),
    ));
}

pub(crate) fn normalize_test_id(id: &str) -> String {
    let id = id.trim();
    let Some((path, rest)) = id.split_once("::") else {
        return id.to_string();
    };
    if path.contains('/') || path.contains('.') {
        return format!("{path}:{}", rest.replace("::", "."));
    }
    id.replace("::", ":")
}

pub(crate) fn prune_old_results(doc: &mut Value, now: f64) {
    let cutoff = now - 90.0 * 86400.0;
    if doc.get("test_results").and_then(|v| v.as_array()).is_none() {
        doc.insert("test_results", Value::Array(Vec::new()));
    }
    if let Some(results) = doc.get_mut("test_results").and_then(|v| v.as_array_mut()) {
        results.retain(|row| match row.get("recorded_at").and_then(|v| v.as_f64()) {
            Some(when) => when >= cutoff,
            None => true,
        });
    }
}

pub(crate) fn append_test_result(doc: &mut Value, test_id: &str, passed: bool, now: f64) {
    if doc.get("test_results").and_then(|v| v.as_array()).is_none() {
        doc.insert("test_results", Value::Array(Vec::new()));
    }
    if let Some(results) = doc.get_mut("test_results").and_then(|v| v.as_array_mut()) {
        results.push(Value::object(vec![
            ("test_id".into(), Value::String(normalize_test_id(test_id))),
            ("passed".into(), Value::Bool(passed)),
            ("recorded_at".into(), Value::Float(now)),
        ]));
    }
}

/// Newest week fail-rate minus the oldest week fail-rate over 28 days.
/// `None` unless the current week and at least one older week both have a row.
pub(crate) fn flakiness_trend(results: &[Value], test_id: &str, now: f64) -> Option<f64> {
    const WEEK: f64 = 7.0 * 86400.0;
    const WINDOW: f64 = 28.0 * 86400.0;
    let mut total = [0i64; 4];
    let mut failed = [0i64; 4];
    for row in results {
        if row.get("test_id").and_then(|v| v.as_str()) != Some(test_id) {
            continue;
        }
        let Some(when) = row.get("recorded_at").and_then(|v| v.as_f64()) else {
            continue;
        };
        let age = now - when;
        if age < 0.0 || age >= WINDOW {
            continue;
        }
        let bucket = (age / WEEK) as usize;
        if bucket >= 4 {
            continue;
        }
        total[bucket] += 1;
        if row.get("passed").and_then(|v| v.as_bool()) == Some(false) {
            failed[bucket] += 1;
        }
    }
    if total[0] == 0 {
        return None;
    }
    let oldest = (1..4).rev().find(|i| total[*i] > 0)?;
    let newest_rate = failed[0] as f64 / total[0] as f64;
    let oldest_rate = failed[oldest] as f64 / total[oldest] as f64;
    Some(crate::json::py_round((newest_rate - oldest_rate).clamp(-1.0, 1.0), 4))
}

pub(crate) fn mean_flakiness(ids: &BTreeSet<String>, results: &[Value], now: f64) -> Option<f64> {
    let vals: Vec<f64> = ids.iter().filter_map(|id| flakiness_trend(results, id, now)).collect();
    if vals.is_empty() {
        return None;
    }
    Some(crate::json::py_round(vals.iter().sum::<f64>() / vals.len() as f64, 4))
}

pub(crate) fn apply_failure(relevance: f64, rate: f64) -> (f64, f64, f64) {
    if rate == 0.0 {
        return (relevance, 0.0, 1.0);
    }
    let boost = 1.0 + 0.5 * rate;
    (relevance * boost, crate::json::py_round(rate, 4), crate::json::py_round(boost, 4))
}

pub(crate) fn failure_rate_map(results: &[Value]) -> BTreeMap<String, f64> {
    let mut totals: BTreeMap<String, (i64, i64)> = BTreeMap::new();
    for row in results {
        let Some(id) = row.get("test_id").and_then(|v| v.as_str()) else { continue };
        if id.is_empty() {
            continue;
        }
        let slot = totals.entry(id.to_string()).or_insert((0, 0));
        slot.0 += 1;
        if row.get("passed").and_then(|v| v.as_bool()) == Some(false) {
            slot.1 += 1;
        }
    }
    totals.into_iter().filter(|(_, (total, _))| *total > 0).map(|(id, (total, failed))| (id, failed as f64 / total as f64)).collect()
}

pub(crate) fn duration_cv_map(results: &[Value]) -> BTreeMap<String, f64> {
    let mut samples: BTreeMap<String, Vec<(f64, f64)>> = BTreeMap::new();
    for row in results {
        let Some(id) = row.get("test_id").and_then(|v| v.as_str()) else { continue };
        let Some(duration) = row.get("duration_ms").and_then(|v| v.as_f64()) else { continue };
        let when = row.get("recorded_at").and_then(|v| v.as_f64()).unwrap_or(0.0);
        samples.entry(id.to_string()).or_default().push((when, duration));
    }
    let mut out = BTreeMap::new();
    for (id, mut pairs) in samples {
        pairs.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let durs: Vec<f64> = pairs.into_iter().take(20).map(|(_, d)| d).collect();
        if durs.len() < 3 {
            continue;
        }
        let mean = durs.iter().sum::<f64>() / durs.len() as f64;
        if mean <= 0.0 {
            continue;
        }
        let var = durs.iter().map(|d| (d - mean) * (d - mean)).sum::<f64>() / durs.len() as f64;
        out.insert(id, (var.sqrt() / mean).min(1.0));
    }
    out
}

pub(crate) fn test_instability(ids: &BTreeSet<String>, rates: &BTreeMap<String, f64>, cvs: &BTreeMap<String, f64>) -> f64 {
    if ids.is_empty() {
        return 0.0;
    }
    let rate_vals: Vec<f64> = ids.iter().filter_map(|id| rates.get(id).copied()).collect();
    let cv_vals: Vec<f64> = ids.iter().filter_map(|id| cvs.get(id).copied()).collect();
    let fail_component = if rate_vals.is_empty() { 0.0 } else { rate_vals.iter().sum::<f64>() / rate_vals.len() as f64 };
    let cv_component = if cv_vals.is_empty() { 0.0 } else { cv_vals.iter().sum::<f64>() / cv_vals.len() as f64 };
    if rate_vals.is_empty() && cv_vals.is_empty() {
        0.0
    } else if rate_vals.is_empty() {
        cv_component.min(1.0)
    } else if cv_vals.is_empty() {
        fail_component
    } else {
        (0.65 * fail_component + 0.35 * cv_component).min(1.0)
    }
}

pub(crate) fn now_secs() -> f64 {
    impact::now_unix()
}

pub(crate) fn churn_row(rel: &str, unit: &str, count: i64, authors: i64, ins: i64, del: i64, last: &str, score: f64) -> Value {
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

pub(crate) fn unit_obj(rel: &str, u: &CodeUnit, hash: &str) -> Value {
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

pub(crate) fn looks_like_test(path: &str) -> bool {
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

pub(crate) fn resolved_import_files(deps: &[Dep], units: &[Value]) -> BTreeSet<String> {
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

pub(crate) fn bare_name_index(units: &[Value]) -> BTreeMap<String, Vec<(String, String)>> {
    let mut index: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for u in units {
        let id = u.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let name = u.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let path = u.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
        if id.is_empty() {
            continue;
        }
        let bare = name.rsplit('.').next().unwrap_or(name);
        index.entry(name.to_string()).or_default().push((id.to_string(), path.to_string()));
        if bare != name {
            index.entry(bare.to_string()).or_default().push((id.to_string(), path.to_string()));
        }
    }
    index
}

pub(crate) fn match_dep(from_file: &str, dep: &Dep, units: &[Value], resolved: &BTreeSet<String>, index: &BTreeMap<String, Vec<(String, String)>>) -> Vec<String> {
    let mut matched = Vec::new();
    if matches!(dep.dep_type.as_str(), "tainted_import" | "dynamic_import") {
        if let Some(module) = dep.module_path.as_deref() {
            for u in units {
                let path = u.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
                let path_ok = extract::matches_import_path(path, module) || extract::matches_relative_import(from_file, path, module);
                if path_ok {
                    if let Some(id) = u.get("id").and_then(|v| v.as_str()) {
                        matched.push(id.to_string());
                    }
                }
            }
        }
        return matched;
    }
    if let Some(module) = dep.module_path.as_deref() {
        if let Some(cands) = index.get(&dep.name) {
            for (id, path) in cands {
                let path_ok = extract::matches_import_path(path, module) || extract::matches_relative_import(from_file, path, module);
                if path_ok {
                    matched.push(id.clone());
                }
            }
        }
    }
    if matched.is_empty() && dep.module_path.is_none() {
        if let Some(cands) = index.get(&dep.name) {
            for (id, path) in cands {
                if !resolved.is_empty() && !resolved.contains(path) {
                    continue;
                }
                matched.push(id.clone());
            }
        }
    }
    matched
}

pub(crate) fn commits_from_store(doc: &Value) -> Vec<crate::git::Commit> {
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

pub(crate) fn path_stem(path: &str) -> &str {
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    base.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(base)
}

pub(crate) fn stem_score(source: &str, test_file: &str) -> f64 {
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

const GIT_POOL: usize = 8;

/// Run `work` on a pool of at most 8 threads. Results stay in input order.
pub(crate) fn pool_map<T, R, F>(items: &[T], work: F) -> Vec<R>
where
    T: Sync,
    R: Send,
    F: Fn(&T) -> R + Sync,
{
    let n = items.len();
    if n == 0 {
        return Vec::new();
    }
    let workers = n.min(GIT_POOL);
    let next = AtomicUsize::new(0);
    let slots: Mutex<Vec<Option<R>>> = Mutex::new((0..n).map(|_| None).collect());
    thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= n {
                        break;
                    }
                    let value = work(&items[i]);
                    let mut guard = slots.lock().expect("git pool");
                    guard[i] = Some(value);
                }
            });
        }
    });
    slots.into_inner().expect("git pool").into_iter().map(|slot| slot.expect("pool slot filled")).collect()
}

pub(crate) fn unit_log_tasks(files: &[String], units_by_file: &BTreeMap<String, Vec<CodeUnit>>) -> Vec<(String, String, String)> {
    let mut tasks = Vec::new();
    for rel in files {
        let Some(units) = units_by_file.get(rel) else { continue };
        for unit in units {
            if unit.unit_type != "function" && unit.unit_type != "async_function" {
                continue;
            }
            let bare = unit.name.rsplit('.').next().unwrap_or(&unit.name).to_string();
            tasks.push((rel.clone(), unit.name.clone(), bare));
        }
    }
    tasks
}

pub(crate) fn code_unit_from_row(row: &Value) -> CodeUnit {
    CodeUnit {
        file_path: row.get("file_path").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        name: row.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        unit_type: row.get("unit_type").and_then(|v| v.as_str()).unwrap_or("function").to_string(),
        line_start: row.get("line_start").and_then(|v| v.as_i64()).unwrap_or(1) as usize,
        line_end: row.get("line_end").and_then(|v| v.as_i64()).unwrap_or(1) as usize,
    }
}

/// Feature-branch pairs from `merge-base(main|master)..HEAD`. `main` and `master` stay empty.
pub(crate) fn fill_branch_co_changes(git: &GitAnalyzer, doc: &mut Value) {
    doc.insert("branch_co_changes", Value::Array(Vec::new()));
    let branch = match git.current_branch() {
        Ok(name) => name,
        Err(_) => {
            store::meta_set(doc, "branch_coupling_commits", "0");
            return;
        }
    };
    if branch == "main" || branch == "master" {
        store::meta_set(doc, "branch_coupling_commits", "0");
        return;
    }
    let Some(base) = ["main", "master"].into_iter().find(|name| git.branch_exists(name)) else {
        store::meta_set(doc, "branch_coupling_commits", "0");
        return;
    };
    let filled = (|| {
        let mb = git.merge_base(base)?;
        let commits = git.parse_log_range(&format!("{mb}..HEAD"))?;
        Ok::<_, crate::git::GitError>(commits)
    })();
    match filled {
        Ok(commits) => {
            store::meta_set(doc, "branch_coupling_commits", &commits.len().to_string());
            doc.insert("branch_co_changes", Value::Array(co_changes_of(&commits, 1)));
        }
        Err(_) => store::meta_set(doc, "branch_coupling_commits", "0"),
    }
}

pub(crate) fn co_changes_of(commits: &[crate::git::Commit], min_count: i64) -> Vec<Value> {
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

pub(crate) fn walk_code(root: &Path) -> Vec<String> {
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
