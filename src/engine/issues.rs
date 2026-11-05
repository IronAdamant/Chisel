//! Issues: failures captured from test output, deduplicated, linked to suspect code through
//! test edges and git history, and closed with the fixing commit when the test passes.
//!
//! Stored in the snapshot as `issues` (newest last) and `test_passes` (test id -> the commit
//! and time it last passed). Test results go to `test_results`, as `record_result` writes them.

use super::*;
use crate::git;
use crate::json::SharedStr;
use crate::test_output::{command_targets, Outcome, Parsed};
use std::collections::{BTreeSet, HashMap};

const MAX_ISSUES: usize = 300;
const ERROR_CHARS: usize = 200;
const SUSPECTS: usize = 3;

/// What one capture changed, for the one-line report.
#[derive(Debug, Default)]
pub struct CaptureSummary {
    pub recorded: usize,
    pub new: Vec<String>,
    pub reopened: Vec<String>,
    pub fixed: Vec<String>,
}

impl CaptureSummary {
    /// One line when something is new, reopened or fixed; `None` when nothing changed.
    pub fn line(&self) -> Option<String> {
        let mut parts = Vec::new();
        if !self.new.is_empty() {
            parts.push(format!("new {}", self.new.join("; ")));
        }
        if !self.reopened.is_empty() {
            parts.push(format!("reopened {}", self.reopened.join("; ")));
        }
        if !self.fixed.is_empty() {
            parts.push(format!("fixed {}", self.fixed.join(", ")));
        }
        (!parts.is_empty()).then(|| format!("chisel issues: {}. Details: chisel issue <id>.", parts.join(" | ")))
    }
}

/// The error with what varies between runs removed: numbers, hex addresses, paths, times.
pub fn normalize_error(message: &str) -> String {
    let mut out = String::new();
    for token in message.split_whitespace() {
        let t = token.trim_matches(|c: char| c == '\'' || c == '"' || c == '(' || c == ')' || c == ',');
        let word = if (t.contains('/') || t.contains('\\')) && t.len() > 2 {
            "<path>".to_string()
        } else {
            let mut w = String::new();
            let mut chars = t.chars().peekable();
            while let Some(c) = chars.next() {
                if c == '0' && matches!(chars.peek(), Some('x') | Some('X')) {
                    chars.next();
                    while chars.peek().is_some_and(|d| d.is_ascii_hexdigit()) {
                        chars.next();
                    }
                    w.push_str("0x#");
                } else if c.is_ascii_digit() {
                    while chars.peek().is_some_and(|d| d.is_ascii_digit() || *d == '.' || *d == ':') {
                        chars.next();
                    }
                    w.push('#');
                } else {
                    w.push(c);
                }
            }
            w
        };
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&word);
    }
    out
}

pub fn signature(test_id: &str, message: &str) -> String {
    crate::hash::sha256_hex(format!("{test_id}\n{}", normalize_error(message)).as_bytes())[..12].to_string()
}

fn short(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn short_sha(sha: &str) -> &str {
    if sha.len() > 7 && sha.chars().all(|c| c.is_ascii_hexdigit()) {
        &sha[..7]
    } else {
        sha
    }
}

pub fn age(now: f64, then: f64) -> String {
    let s = (now - then).max(0.0) as i64;
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m", s / 60),
        3600..=86399 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86400),
    }
}

fn iso(t: f64) -> String {
    let secs = t as i64;
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    // civil_from_days (Howard Hinnant)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}Z", rem / 3600, rem % 3600 / 60)
}

/// The test file of a stored test id (`path:name`), when it has one.
fn test_file(id: &str) -> Option<&str> {
    id.split_once(':').map(|(f, _)| f).filter(|f| f.contains('/') || f.contains('.'))
}

fn s(v: &Value, key: &str) -> String {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

fn f(v: &Value, key: &str) -> f64 {
    v.get(key).and_then(|x| x.as_f64()).unwrap_or(0.0)
}

/// Parse `since`: `30m`, `12h`, `7d`, a unix time, or `YYYY-MM-DD`.
fn since_cutoff(since: &str, now: f64) -> Option<f64> {
    let t = since.trim();
    if let Some(n) = t.strip_suffix('m').and_then(|n| n.parse::<f64>().ok()) {
        return Some(now - n * 60.0);
    }
    if let Some(n) = t.strip_suffix('h').and_then(|n| n.parse::<f64>().ok()) {
        return Some(now - n * 3600.0);
    }
    if let Some(n) = t.strip_suffix('d').and_then(|n| n.parse::<f64>().ok()) {
        return Some(now - n * 86400.0);
    }
    if let Ok(n) = t.parse::<f64>() {
        return Some(n);
    }
    impact::parse_iso_unix(&format!("{t}T00:00:00Z"))
}

impl Engine {
    /// Map a runner's test name onto the snapshot's test ids, so issues meet test edges.
    fn resolve_test(&self, o: &Outcome, units: &HashMap<String, Vec<(String, String)>>, ids: &BTreeSet<String>) -> String {
        let base = o.name.rsplit("::").next().unwrap_or(&o.name);
        let base = base.split('[').next().unwrap_or(base).trim();
        if let Some(file) = &o.file {
            let direct = format!("{file}:{base}");
            if ids.contains(&direct) {
                return direct;
            }
            if let Some((id, _)) = units.get(base).and_then(|c| c.iter().find(|(_, f)| f == file)) {
                return id.clone();
            }
            // cargo names the crate root (`Running unittests src/lib.rs`) for every unit test in
            // the crate; the module path then says which file holds it.
            if !o.name.contains("::") {
                return format!("{file}:{}", o.name.split('[').next().unwrap_or(base));
            }
        }
        let candidates = units.get(base).cloned().unwrap_or_default();
        if candidates.len() == 1 {
            return candidates[0].0.clone();
        }
        // Hints: cargo module path segments, or the go package's last path segments.
        let hints: Vec<&str> = match &o.package {
            Some(pkg) => pkg.rsplit('/').take(2).collect(),
            None => o.name.split("::").filter(|seg| *seg != base && *seg != "tests").collect(),
        };
        let matches_hint = |file: &str| {
            let segments: Vec<&str> = file.trim_end_matches(".rs").trim_end_matches(".go").split('/').collect();
            hints.iter().any(|h| segments.contains(h))
        };
        if let Some((id, _)) = candidates.iter().find(|(_, f)| matches_hint(f)) {
            return id.clone();
        }
        match (&o.package, &o.file) {
            (Some(pkg), _) => format!("{pkg}:{}", o.name),
            (None, Some(file)) => format!("{file}:{}", o.name.replace("::", ".")),
            (None, None) => o.name.clone(),
        }
    }

    /// Code the failing test covers that changed since it last passed, best first.
    fn suspects(&self, test_id: &str, base: Option<&str>, now: f64) -> Vec<Value> {
        let units: HashMap<&str, (&str, &str)> = self
            .store
            .rows("code_units")
            .iter()
            .filter_map(|u| Some((u.get("id")?.as_str()?, (u.get("file_path")?.as_str()?, u.get("name")?.as_str()?))))
            .collect();
        let mut files: BTreeMap<String, (f64, BTreeSet<String>)> = BTreeMap::new();
        for e in self.store.rows("test_edges") {
            if e.get("test_id").and_then(|v| v.as_str()) != Some(test_id) {
                continue;
            }
            let Some(&(file, name)) = e.get("code_id").and_then(|v| v.as_str()).and_then(|c| units.get(c)) else { continue };
            let entry = files.entry(file.to_string()).or_default();
            entry.0 += e.get("weight").and_then(|v| v.as_f64()).unwrap_or(0.5);
            entry.1.insert(name.to_string());
        }
        if files.is_empty() {
            if let Some(tf) = test_file(test_id) {
                for row in self.store.rows("static_test_imports") {
                    if row.get("test_file").and_then(|v| v.as_str()) == Some(tf) {
                        if let Some(cf) = row.get("code_file").and_then(|v| v.as_str()) {
                            files.entry(cf.to_string()).or_default().0 += 0.3;
                        }
                    }
                }
            }
        }
        if files.is_empty() {
            return Vec::new();
        }
        let list: Vec<String> = files.keys().cloned().collect();
        let base_ref = base.unwrap_or("HEAD").to_string();
        let mut diff_args: Vec<String> = ["-c", "core.quotePath=false", "diff", "-U0", "--no-color", "--no-ext-diff", "--src-prefix=a/", "--dst-prefix=b/", base_ref.as_str(), "--"].iter().map(|s| s.to_string()).collect();
        diff_args.extend(list.iter().cloned());
        let mut dirty_args: Vec<String> = ["-c", "core.quotePath=false", "diff", "--name-only", "HEAD", "--"].iter().map(|s| s.to_string()).collect();
        dirty_args.extend(list.iter().cloned());
        let (diff, dirty) = std::thread::scope(|sc| {
            let d = sc.spawn(|| self.git.run_args(&dirty_args).unwrap_or_default());
            (self.git.run_args(&diff_args).unwrap_or_default(), d.join().unwrap_or_default())
        });
        let dirty: BTreeSet<&str> = dirty.lines().map(str::trim).collect();
        let sections: HashMap<String, String> = git::split_diff_by_file(&diff).into_iter().collect();
        let churn: HashMap<&str, (f64, String)> = self
            .store
            .rows("churn")
            .iter()
            .filter(|r| r.get("unit_name").and_then(|v| v.as_str()) == Some(""))
            .filter_map(|r| Some((r.get("file_path")?.as_str()?, (r.get("churn_score").and_then(|v| v.as_f64()).unwrap_or(0.0), r.get("last_changed").and_then(|v| v.as_str()).unwrap_or("").to_string()))))
            .collect();
        let since = match base {
            Some(b) => format!("since last pass ({})", short_sha(b)),
            None => "vs HEAD (no recorded pass)".to_string(),
        };
        let mut rows: Vec<(f64, Value)> = Vec::new();
        for (file, (weight, called)) in &files {
            let (churn_score, last) = churn.get(file.as_str()).cloned().unwrap_or((0.0, String::new()));
            let days = impact::parse_iso_unix(&last).map(|t| ((now - t) / 86400.0).max(0.0)).unwrap_or(30.0);
            let recency = 1.0 / (1.0 + days / 7.0);
            let called_list: Vec<&str> = called.iter().map(|s| s.as_str()).take(3).collect();
            let (score, unit, reason) = match sections.get(file) {
                Some(section) => {
                    let (mut add, mut del) = (0i64, 0i64);
                    let mut in_hunk = false;
                    for l in section.lines() {
                        if l.starts_with("@@") {
                            in_hunk = true;
                        } else if l.starts_with("diff --git") {
                            in_hunk = false;
                        } else if in_hunk && l.starts_with('+') {
                            add += 1;
                        } else if in_hunk && l.starts_with('-') {
                            del += 1;
                        }
                    }
                    let changed = git::parse_diff_functions(section);
                    // Functions the test names (test_sub -> sub) come first: file-level imports put
                    // every imported function on a test's edges.
                    let test_name = test_id.rsplit(':').next().unwrap_or(test_id).to_ascii_lowercase();
                    let mut hit: Vec<&String> = changed.iter().filter(|n| called.contains(*n)).collect();
                    hit.sort_by_key(|n| !test_name.contains(&n.to_ascii_lowercase()));
                    let uncommitted = dirty.contains(file.as_str());
                    let score = weight * (1.0 + ((add + del) as f64).ln_1p()) * if uncommitted { 1.5 } else { 1.0 } * (1.0 + 0.5 * churn_score.min(2.0)) * (1.0 + hit.len() as f64);
                    let what = if hit.is_empty() {
                        format!("the test calls {}", called_list.join(", "))
                    } else {
                        format!("changed {}, which the test calls", hit.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "))
                    };
                    let unit = hit.first().map(|s| s.to_string()).or_else(|| called_list.first().map(|s| s.to_string()));
                    (score, unit, format!("+{add} -{del} {since}{}; {what}", if uncommitted { ", uncommitted" } else { "" }))
                }
                None => {
                    let score = 0.2 * weight * (1.0 + churn_score.min(2.0)) * (0.5 + recency);
                    (score, called_list.first().map(|s| s.to_string()), format!("unchanged {since}; the test calls {}", called_list.join(", ")))
                }
            };
            let mut row = vec![("file".into(), Value::String(file.clone()))];
            if let Some(u) = unit {
                row.push(("unit".into(), Value::String(u)));
            }
            row.push(("score".into(), Value::Float((score * 100.0).round() / 100.0)));
            row.push(("reason".into(), Value::String(short(&reason, 160))));
            rows.push((score, Value::object(row)));
        }
        rows.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        rows.into_iter().take(SUSPECTS).map(|(_, v)| v).collect()
    }

    /// Files changed between the failing commit and the working tree, for a closed issue.
    fn fix_summary(&self, failing_commit: &str, head: &str) -> (String, Vec<Value>) {
        if failing_commit.is_empty() {
            return (if head.is_empty() { "unknown".into() } else { short_sha(head).to_string() }, Vec::new());
        }
        let out = self.git.run_args(&["-c".into(), "core.quotePath=false".into(), "diff".into(), "--numstat".into(), failing_commit.to_string(), "--".into()]).unwrap_or_default();
        let mut files: Vec<(i64, Value)> = out
            .lines()
            .filter_map(|l| {
                let mut p = l.split('\t');
                let (a, d, path) = (p.next()?, p.next()?, p.next()?);
                if path.starts_with(".chisel/") {
                    return None;
                }
                let (a, d) = (a.parse::<i64>().unwrap_or(0), d.parse::<i64>().unwrap_or(0));
                Some((a + d, Value::String(format!("{path} +{a} -{d}"))))
            })
            .collect();
        files.sort_by(|x, y| y.0.cmp(&x.0));
        let commit = if !head.is_empty() && head != failing_commit {
            short_sha(head).to_string()
        } else if files.is_empty() {
            "none (passed with no code change; flaky?)".to_string()
        } else {
            "uncommitted".to_string()
        };
        (commit, files.into_iter().take(8).map(|(_, v)| v).collect())
    }

    /// Record a parsed test run: results, new or recurring issues, and fixes.
    /// `record` is false when the caller already stored the results (`chisel run`).
    pub fn capture(&mut self, parsed: &Parsed, command: &str, record: bool) -> Result<CaptureSummary, String> {
        let mut summary = CaptureSummary::default();
        if parsed.outcomes.is_empty() && parsed.passed_files.is_empty() && parsed.passed_packages.is_empty() {
            return Ok(summary);
        }
        self.store.reload().map_err(|e| e.to_string())?;
        let now = now_secs();
        let mut units: HashMap<String, Vec<(String, String)>> = HashMap::new();
        let mut ids = BTreeSet::new();
        for u in self.store.rows("test_units") {
            let (Some(id), Some(name), Some(file)) = (u.get("id").and_then(|v| v.as_str()), u.get("name").and_then(|v| v.as_str()), u.get("file_path").and_then(|v| v.as_str())) else { continue };
            ids.insert(id.to_string());
            units.entry(name.to_string()).or_default().push((id.to_string(), file.to_string()));
        }
        let resolved: Vec<(String, &Outcome)> = parsed.outcomes.iter().map(|o| (self.resolve_test(o, &units, &ids), o)).collect();
        let mut failing: BTreeMap<String, &Outcome> = BTreeMap::new();
        let mut passing: BTreeSet<String> = BTreeSet::new();
        for (id, o) in &resolved {
            if o.passed {
                passing.insert(id.clone());
            } else {
                failing.entry(id.clone()).or_insert(o);
            }
        }
        passing.retain(|id| !failing.contains_key(id));
        let head = git::head_commit_fast(&self.project_dir).unwrap_or_default();
        let mut issues = self.store.array("issues");
        let runner = parsed.runners.first().copied().unwrap_or("");

        // A test not listed as passing still passed when its file or package passed, when the
        // command named its file, or when its runner ran the whole suite to the end.
        let targets = command_targets(command);
        let filtered = targets.iter().any(|t| t.starts_with("filter:"));
        for issue in &issues {
            if s(issue, "status") != "open" {
                continue;
            }
            let test = s(issue, "test");
            if failing.contains_key(&test) || passing.contains(&test) || !parsed.complete {
                continue;
            }
            let file = test_file(&test).unwrap_or("");
            let pkg = s(issue, "package");
            let evidence = (!file.is_empty() && parsed.passed_files.iter().any(|p| p == file))
                || (!pkg.is_empty() && parsed.passed_packages.contains(&pkg))
                || (!filtered && !file.is_empty() && targets.iter().any(|t| file.starts_with(t.trim_end_matches('/')) || t.ends_with(file)))
                || (targets.is_empty() && !runner.is_empty() && s(issue, "runner") == runner);
            if evidence {
                passing.insert(test);
            }
        }

        // Git work first, then one edit and one save.
        let passes: HashMap<String, String> = self
            .store
            .doc
            .get("test_passes")
            .and_then(|v| v.as_object())
            .map(|o| o.iter().map(|(k, v)| (k.to_string(), s(v, "commit"))).collect())
            .unwrap_or_default();
        let mut fresh: Vec<(String, &Outcome, String, Vec<Value>)> = Vec::new();
        for (id, o) in &failing {
            let sig = signature(id, &o.message);
            if issues.iter().any(|i| s(i, "sig") == sig) {
                continue;
            }
            let base = passes.get(id).filter(|c| !c.is_empty()).map(|c| c.as_str());
            fresh.push((id.clone(), o, sig, self.suspects(id, base, now)));
        }
        let closing: Vec<(usize, String, Vec<Value>)> = issues
            .iter()
            .enumerate()
            .filter(|(_, i)| s(i, "status") == "open" && passing.contains(&s(i, "test")))
            .map(|(k, i)| {
                let (commit, files) = self.fix_summary(&s(i, "last_commit"), &head);
                (k, commit, files)
            })
            .collect();

        let mut seq: i64 = store::meta_get(&self.store.doc, "issue_seq").and_then(|v| v.parse().ok()).unwrap_or(0);
        for (k, commit, files) in closing {
            let issue = &mut issues[k];
            issue.insert("status", Value::String("closed".into()));
            issue.insert("fixed", Value::object(vec![("at".into(), Value::Float(now)), ("commit".into(), Value::String(commit)), ("files".into(), Value::Array(files))]));
            summary.fixed.push(format!("{} {}", s(issue, "id"), s(issue, "test")));
        }
        for (id, o) in &failing {
            let sig = signature(id, &o.message);
            if let Some(issue) = issues.iter_mut().find(|i| s(i, "sig") == sig) {
                let reopened = s(issue, "status") == "closed";
                issue.insert("count", Value::Int(issue.get("count").and_then(|v| v.as_i64()).unwrap_or(1) + 1));
                issue.insert("last_at", Value::Float(now));
                issue.insert("last_commit", Value::String(head.clone()));
                if reopened {
                    issue.insert("status", Value::String("open".into()));
                    issue.insert("reopened", Value::Int(issue.get("reopened").and_then(|v| v.as_i64()).unwrap_or(0) + 1));
                    summary.reopened.push(format!("{} {}", s(issue, "id"), id));
                }
            }
        }
        for (id, o, sig, suspects) in fresh {
            seq += 1;
            let iid = format!("I{seq}");
            let suspect = suspects.first().map(|v| match v.get("unit").and_then(|u| u.as_str()) {
                Some(u) => format!(" suspect {}:{u}", s(v, "file")),
                None => format!(" suspect {}", s(v, "file")),
            });
            summary.new.push(format!("{iid} {id} ({}){}", short(&o.message, 80), suspect.unwrap_or_default()));
            let mut row = vec![
                ("id".into(), Value::String(iid)),
                ("status".into(), Value::String("open".into())),
                ("test".into(), Value::String(id.clone())),
                ("sig".into(), Value::String(sig)),
                ("error".into(), Value::String(short(&o.message, ERROR_CHARS))),
                ("location".into(), Value::String(o.location.clone())),
                ("runner".into(), Value::String(runner.to_string())),
            ];
            if let Some(pkg) = &o.package {
                row.push(("package".into(), Value::String(pkg.clone())));
            }
            row.extend([
                ("count".into(), Value::Int(1)),
                ("first_at".into(), Value::Float(now)),
                ("first_commit".into(), Value::String(head.clone())),
                ("last_at".into(), Value::Float(now)),
                ("last_commit".into(), Value::String(head.clone())),
                ("suspects".into(), Value::Array(suspects)),
                ("excerpt".into(), Value::String(o.excerpt.clone())),
            ]);
            issues.push(Value::object(row));
        }
        // Keep every open issue and the newest closed ones.
        if issues.len() > MAX_ISSUES {
            let mut drop = issues.len() - MAX_ISSUES;
            issues.retain(|i| {
                if drop > 0 && s(i, "status") == "closed" {
                    drop -= 1;
                    false
                } else {
                    true
                }
            });
        }
        let doc = &mut self.store.doc;
        if record {
            prune_old_results(doc, now);
            for (id, o) in &resolved {
                append_test_result(doc, id, o.passed, now);
                summary.recorded += 1;
            }
        }
        let mut pass_map: Vec<(SharedStr, Value)> = doc.get("test_passes").and_then(|v| v.as_object()).map(|o| o.to_vec()).unwrap_or_default();
        for id in &passing {
            let entry = Value::object(vec![("commit".into(), Value::String(head.clone())), ("at".into(), Value::Float(now))]);
            match pass_map.iter_mut().find(|(k, _)| k == id.as_str()) {
                Some(slot) => slot.1 = entry,
                None => pass_map.push((id.as_str().into(), entry)),
            }
        }
        doc.insert("test_passes", Value::Object(pass_map));
        doc.insert("issues", Value::Array(issues));
        store::meta_set(doc, "issue_seq", &seq.to_string());
        self.store.save().map_err(|e| e.to_string())?;
        Ok(summary)
    }

    /// `issues`: open issues, compact. `all` adds closed ones; `since` and `limit` narrow.
    pub fn issues_tool(&mut self, args: &Value) -> Result<Value, String> {
        self.store.reload().map_err(|e| e.to_string())?;
        let now = now_secs();
        let all = args.get("all").and_then(|v| v.as_bool()).unwrap_or(false);
        let cutoff = args.get("since").and_then(|v| v.as_str()).and_then(|t| since_cutoff(t, now));
        let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(20).clamp(1, 500) as usize;
        let issues = self.store.rows("issues");
        let open = issues.iter().filter(|i| s(i, "status") == "open").count();
        let mut rows: Vec<&Value> = issues.iter().filter(|i| (all || s(i, "status") == "open") && cutoff.is_none_or(|c| f(i, "last_at") >= c)).collect();
        rows.sort_by(|a, b| f(b, "last_at").partial_cmp(&f(a, "last_at")).unwrap_or(std::cmp::Ordering::Equal));
        let total = rows.len();
        let items: Vec<Value> = rows
            .into_iter()
            .take(limit)
            .map(|i| {
                let mut row = vec![("id".into(), Value::String(s(i, "id")))];
                if all {
                    row.push(("status".into(), Value::String(s(i, "status"))));
                }
                row.push(("test".into(), Value::String(s(i, "test"))));
                row.push(("error".into(), Value::String(short(&s(i, "error"), 60))));
                row.push(("n".into(), Value::Int(i.get("count").and_then(|v| v.as_i64()).unwrap_or(1))));
                row.push(("age".into(), Value::String(age(now, f(i, "first_at")))));
                if let Some(top) = i.get("suspects").and_then(|v| v.as_array()).and_then(|a| a.first()) {
                    let unit = top.get("unit").and_then(|u| u.as_str()).map(|u| format!(":{u}")).unwrap_or_default();
                    row.push(("suspect".into(), Value::String(format!("{}{unit}", s(top, "file")))));
                }
                Value::object(row)
            })
            .collect();
        let mut out = vec![("open".into(), Value::Int(open as i64)), ("items".into(), Value::Array(items))];
        if total > limit {
            out.push(("truncated".into(), Value::Bool(true)));
            out.push(("total".into(), Value::Int(total as i64)));
        }
        Ok(Value::object(out))
    }

    /// `issue`: one issue in full: suspects, occurrences, excerpt, fix.
    pub fn issue_tool(&mut self, id: &str) -> Result<Value, String> {
        self.store.reload().map_err(|e| e.to_string())?;
        let id = id.trim();
        let Some(i) = self.store.rows("issues").iter().find(|i| s(i, "id").eq_ignore_ascii_case(id)) else {
            return Ok(Value::object(vec![
                ("status".into(), Value::String("error".into())),
                ("message".into(), Value::String(format!("no issue {id}; the issues tool lists them"))),
            ]));
        };
        let mut out = vec![
            ("id".into(), Value::String(s(i, "id"))),
            ("status".into(), Value::String(s(i, "status"))),
            ("test".into(), Value::String(s(i, "test"))),
            ("error".into(), Value::String(s(i, "error"))),
        ];
        if !s(i, "location").is_empty() {
            out.push(("location".into(), Value::String(s(i, "location"))));
        }
        out.push(("count".into(), Value::Int(i.get("count").and_then(|v| v.as_i64()).unwrap_or(1))));
        if let Some(n) = i.get("reopened").and_then(|v| v.as_i64()) {
            out.push(("reopened".into(), Value::Int(n)));
        }
        out.push(("first".into(), Value::String(format!("{} {}", iso(f(i, "first_at")), short_sha(&s(i, "first_commit"))).trim().to_string())));
        out.push(("last".into(), Value::String(format!("{} {}", iso(f(i, "last_at")), short_sha(&s(i, "last_commit"))).trim().to_string())));
        out.push(("suspects".into(), i.get("suspects").cloned().unwrap_or(Value::Array(Vec::new()))));
        if let Some(fix) = i.get("fixed") {
            out.push(("fixed".into(), Value::object(vec![
                ("at".into(), Value::String(iso(f(fix, "at")))),
                ("commit".into(), Value::String(s(fix, "commit"))),
                ("files".into(), fix.get("files").cloned().unwrap_or(Value::Array(Vec::new()))),
            ])));
        }
        if !s(i, "excerpt").is_empty() {
            out.push(("excerpt".into(), Value::String(s(i, "excerpt"))));
        }
        Ok(Value::object(out))
    }

    /// `trace`: what broke in a file (or one function) and what fixed it.
    pub fn trace_tool(&mut self, file: &str, unit: Option<&str>) -> Result<Value, String> {
        self.store.reload().map_err(|e| e.to_string())?;
        let file = file.trim().replace('\\', "/");
        let mut rows = Vec::new();
        for i in self.store.rows("issues").iter().rev() {
            let mut roles: Vec<&str> = Vec::new();
            let suspects = i.get("suspects").and_then(|v| v.as_array()).unwrap_or(&[]);
            if suspects.iter().any(|sp| s(sp, "file") == file && unit.is_none_or(|u| sp.get("unit").and_then(|x| x.as_str()) == Some(u))) {
                roles.push("suspect");
            }
            let fixed_here = i.get("fixed").and_then(|fx| fx.get("files")).and_then(|v| v.as_array()).is_some_and(|fs| fs.iter().any(|x| x.as_str().is_some_and(|p| p.split(' ').next() == Some(file.as_str()))));
            if fixed_here && unit.is_none() {
                roles.push("fixed_in");
            }
            if test_file(&s(i, "test")) == Some(file.as_str()) && unit.is_none_or(|u| s(i, "test").ends_with(&format!(":{u}"))) {
                roles.push("test");
            }
            if s(i, "location").rsplit_once(':').map(|(p, _)| p) == Some(file.as_str()) && unit.is_none() {
                roles.push("failed_at");
            }
            if roles.is_empty() {
                continue;
            }
            let mut row = vec![
                ("id".into(), Value::String(s(i, "id"))),
                ("status".into(), Value::String(s(i, "status"))),
                ("test".into(), Value::String(s(i, "test"))),
                ("error".into(), Value::String(short(&s(i, "error"), 60))),
                ("role".into(), Value::String(roles.join(","))),
                ("when".into(), Value::String(iso(f(i, "first_at"))[..10].to_string())),
            ];
            if let Some(fix) = i.get("fixed") {
                row.push(("fix".into(), Value::String(s(fix, "commit"))));
            }
            rows.push(Value::object(row));
        }
        let open = rows.iter().filter(|r| r.get("status").and_then(|v| v.as_str()) == Some("open")).count();
        Ok(Value::object(vec![
            ("file".into(), Value::String(file)),
            ("open".into(), Value::Int(open as i64)),
            ("issues".into(), Value::Array(rows)),
        ]))
    }

    /// At most two lines for a SessionStart hook; empty when nothing is open.
    pub fn issues_brief(&mut self) -> Vec<String> {
        if self.store.reload().is_err() {
            return Vec::new();
        }
        let now = now_secs();
        let open: Vec<&Value> = self.store.rows("issues").iter().filter(|i| s(i, "status") == "open").collect();
        if open.is_empty() {
            return Vec::new();
        }
        let list: Vec<String> = open.iter().rev().take(3).map(|i| format!("{} {} ({}, {})", s(i, "id"), s(i, "test"), short(&s(i, "error"), 40), age(now, f(i, "first_at")))).collect();
        let more = if open.len() > 3 { format!(" and {} more", open.len() - 3) } else { String::new() };
        vec![
            format!("chisel: {} open test issue(s): {}{more}.", open.len(), list.join("; ")),
            "Suspects and history: the `issue` tool (or `chisel issue <id>`).".to_string(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_ignore_numbers_paths_addresses_and_times() {
        let a = normalize_error("thread 'x' (21920) panicked at C:\\work\\a.rs:10:22 left 8 at 0x7ffd1234 2026-11-05T10:20:01Z");
        let b = normalize_error("thread 'x' (5511) panicked at /home/b/a.rs:99:1 left 9 at 0x1 2026-11-06T11:00:00Z");
        assert_eq!(a, b);
        assert_ne!(signature("t:a", "KeyError: 'missing'"), signature("t:a", "TypeError: x"));
        assert_ne!(signature("t:a", "boom"), signature("t:b", "boom"));
    }

    #[test]
    fn ages_and_dates() {
        assert_eq!(age(1000.0, 1000.0 - 7200.0), "2h");
        assert_eq!(age(1000.0, 1000.0 - 3.0 * 86400.0), "3d");
        assert_eq!(iso(1_700_000_000.0), "2023-11-14T22:13Z");
        assert_eq!(since_cutoff("2d", 1_000_000.0), Some(1_000_000.0 - 172_800.0));
    }
}
