use super::*;
use crate::extract::{self, CodeUnit};
use crate::git::{self, GitAnalyzer};
use crate::hash::sha256_hex;
use crate::impact::{self};
use crate::json::Value;
use crate::store::{self};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

impl Engine {
    pub fn analyze(&mut self) -> Result<Value, String> {
        if !self.shards.is_empty() && self.only_files.is_none() {
            return self.analyze_all_shards();
        }
        extract::clear_extractor_warnings();
        if let Some(id) = self.active_job.clone() {
            if let Ok(ms) = std::env::var("CHISEL_JOB_PACE_MS") {
                if let Ok(n) = ms.parse::<u64>() {
                    thread::sleep(Duration::from_millis(n));
                }
            }
            if job_should_stop(&id, self.store.dir()) {
                self.note_job(&id, "cancelled", 0);
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
                self.note_job(&id, "running", pct);
                if let Ok(ms) = std::env::var("CHISEL_JOB_PACE_MS") {
                    if let Ok(n) = ms.parse::<u64>() {
                        thread::sleep(Duration::from_millis(n));
                    }
                }
                if job_should_stop(&id, self.store.dir()) {
                    self.note_job(&id, "cancelled", pct);
                    return Ok(Value::object(vec![
                        ("status".into(), Value::String("cancelled".into())),
                        ("progress_pct".into(), Value::Int(pct)),
                    ]));
                }
            }
            let abs = self.project_dir.join(rel);
            let content = fs::read(&abs).unwrap_or_default();
            let text = String::from_utf8_lossy(&content);
            let units = extract::extract_code_units_in(rel, &text, Some(&self.project_dir), self.allow_repo_extractor);
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
        }
        if files.len() <= 2000 {
            let tasks = unit_log_tasks(&files, &units_by_file);
            let logs = pool_map(&tasks, |task| git.function_log(&task.0, &task.2).ok());
            for (task, func_commits) in tasks.iter().zip(logs) {
                let Some(func_commits) = func_commits else { continue };
                if func_commits.is_empty() {
                    continue;
                }
                let func_dates: Vec<String> = func_commits.iter().map(|c| c.date.clone()).collect();
                let func_score = impact::churn_score(&func_dates, now);
                let func_count = func_dates.iter().filter(|d| impact::parse_iso_unix(d).is_some()).count() as i64;
                churn_rows.push(churn_row(&task.0, &task.1, func_count, 1, 0, 0, "", func_score));
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
        fill_branch_co_changes(&git, &mut self.store.doc);

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
        }
        let blame_blocks = pool_map(&files, |rel| git.blame(rel).unwrap_or_default());
        for (rel, blocks) in files.iter().zip(blame_blocks) {
            let hash = hash_rows.iter().find(|h| h.get("file_path").and_then(|v| v.as_str()) == Some(rel.as_str())).and_then(|h| h.get("hash").and_then(|v| v.as_str())).unwrap_or("");
            for b in blocks {
                blame_rows.push(Value::object(vec![
                    ("file_path".into(), Value::String(rel.clone())),
                    ("content_hash".into(), Value::String(hash.into())),
                    ("line_start".into(), Value::Int(b.line_start as i64)),
                    ("line_end".into(), Value::Int(b.line_end as i64)),
                    ("author".into(), Value::String(b.author)),
                    ("author_email".into(), Value::String(b.author_email)),
                    ("commit_hash".into(), Value::String(b.commit_hash)),
                ]));
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
            ("hint".into(), Value::String(analyze_hint(test_files))),
        ];
        if files.len() > 2000 {
            summary.push(("unit_churn_skipped".into(), Value::Bool(true)));
        }
        push_extractor_warnings(&mut summary);
        Ok(Value::object(summary))
    }

    pub fn update(&mut self) -> Result<Value, String> {
        if !self.shards.is_empty() && self.only_files.is_none() {
            return self.update_all_shards();
        }
        extract::clear_extractor_warnings();
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
        let removed: Vec<String> = before.keys().filter(|path| !files.iter().any(|f| f == *path)).cloned().collect();
        let since = store::meta_get(&self.store.doc, "last_commit_date");
        let known: BTreeSet<String> = self.store.array("commits").iter().filter_map(|c| c.get("hash").and_then(|v| v.as_str()).map(|s| s.to_string())).collect();
        let new_commits: Vec<git::Commit> = git.parse_log_since(since.as_deref()).unwrap_or_default().into_iter().filter(|c| !known.contains(&c.hash)).collect();
        let commit_touch: BTreeSet<String> = new_commits.iter().flat_map(|c| c.files.iter().map(|f| f.path.clone())).collect();
        if changed.is_empty() && new_commits.is_empty() && removed.is_empty() {
            return Ok(Value::object(vec![
                ("files_updated".into(), Value::Int(0)),
                ("code_units_found".into(), Value::Int(self.store.array("code_units").len() as i64)),
                ("new_commits".into(), Value::Int(0)),
                ("edge_rebuild_skipped".into(), Value::Bool(true)),
                ("hint".into(), Value::String("For large repos, use start_job with kind='update' to avoid MCP timeouts.".into())),
            ]));
        }
        // Changed files and new commits only. Untouched files keep their units, blame, and edges.
        let summary = self.analyze_subset(&git, &files, &changed, &new_commits, &commit_touch, &removed)?;
        Ok(summary)
    }

    pub(super) fn analyze_subset(
        &mut self,
        git: &GitAnalyzer,
        files: &[String],
        changed: &[String],
        new_commits: &[git::Commit],
        _commit_touch: &BTreeSet<String>,
        removed: &[String],
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
            let extracted = extract::extract_code_units_in(rel, &text, Some(&self.project_dir), self.allow_repo_extractor);
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
            !scope.iter().any(|s| s == file)
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
        }
        if !skip_unit {
            let stored_units = self.store.array("code_units");
            for rel in &scope {
                if units_by_file.contains_key(rel) {
                    continue;
                }
                let extracted = stored_units.iter().filter(|u| u.get("file_path").and_then(|v| v.as_str()) == Some(rel.as_str())).map(code_unit_from_row).collect();
                units_by_file.insert(rel.clone(), extracted);
            }
            let tasks = unit_log_tasks(&scope, &units_by_file);
            let logs = pool_map(&tasks, |task| git.function_log(&task.0, &task.2).ok());
            for (task, func_commits) in tasks.iter().zip(logs) {
                let Some(func_commits) = func_commits else { continue };
                if func_commits.is_empty() {
                    continue;
                }
                let func_dates: Vec<String> = func_commits.iter().map(|c| c.date.clone()).collect();
                let func_score = impact::churn_score(&func_dates, now);
                let func_count = func_dates.iter().filter(|d| impact::parse_iso_unix(d).is_some()).count() as i64;
                churn_rows.push(churn_row(&task.0, &task.1, func_count, 1, 0, 0, "", func_score));
            }
        }
        self.store.doc.insert("churn", Value::Array(churn_rows));

        let in_scope = |path: &str| scope.iter().any(|s| s == path);
        let mut blame_rows: Vec<Value> = self.store.array("blame").into_iter().filter(|row| {
            row.get("file_path").and_then(|v| v.as_str()).is_none_or(|p| !in_scope(p))
        }).collect();
        let mut hash_rows: Vec<Value> = self.store.array("file_hashes").into_iter().filter(|row| {
            row.get("file_path").and_then(|v| v.as_str()).is_none_or(|p| !in_scope(p))
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
        }
        let blame_blocks = pool_map(&scope, |rel| git.blame(rel).unwrap_or_default());
        for (rel, blocks) in scope.iter().zip(blame_blocks) {
            let hash = hash_rows.iter().rev().find(|h| h.get("file_path").and_then(|v| v.as_str()) == Some(rel.as_str())).and_then(|h| h.get("hash").and_then(|v| v.as_str())).unwrap_or("");
            for b in blocks {
                blame_rows.push(Value::object(vec![
                    ("file_path".into(), Value::String(rel.clone())),
                    ("content_hash".into(), Value::String(hash.into())),
                    ("line_start".into(), Value::Int(b.line_start as i64)),
                    ("line_end".into(), Value::Int(b.line_end as i64)),
                    ("author".into(), Value::String(b.author)),
                    ("author_email".into(), Value::String(b.author_email)),
                    ("commit_hash".into(), Value::String(b.commit_hash)),
                ]));
            }
        }
        self.store.doc.insert("blame", Value::Array(blame_rows));
        self.store.doc.insert("file_hashes", Value::Array(hash_rows));
        fill_branch_co_changes(git, &mut self.store.doc);

        let changed_tests: Vec<String> = changed.iter().filter(|p| looks_like_test(p)).cloned().collect();
        let changed_sources: Vec<String> = changed.iter().filter(|p| !looks_like_test(p)).cloned().collect();
        let mut tests_to_rebuild = changed_tests.clone();
        if !changed_sources.is_empty() || !removed.is_empty() {
            let known_tests = self.store.array("test_units");
            for edge in self.store.array("test_edges") {
                let code_id = edge.get("code_id").and_then(|v| v.as_str()).unwrap_or("");
                let code_file = edge.get("code_file").and_then(|v| v.as_str()).unwrap_or("");
                let targets_changed = changed_sources.iter().any(|path| code_id.starts_with(&format!("{path}:")));
                let targets_removed = removed.iter().any(|path| code_file == path.as_str() || code_id.starts_with(&format!("{path}:")));
                if !targets_changed && !targets_removed {
                    continue;
                }
                let tid = edge.get("test_id").and_then(|v| v.as_str()).unwrap_or("");
                let Some(test_file) = known_tests.iter().find(|unit| unit.get("id").and_then(|v| v.as_str()) == Some(tid)).and_then(|unit| unit.get("file_path").and_then(|v| v.as_str())) else {
                    continue;
                };
                if removed.iter().any(|path| path == test_file) {
                    continue;
                }
                if !tests_to_rebuild.iter().any(|path| path == test_file) {
                    tests_to_rebuild.push(test_file.to_string());
                }
            }
        }
        let edge_rebuild_skipped = tests_to_rebuild.is_empty();
        if edge_rebuild_skipped {
            // No test content changed and no edge still names a replaced code id.
        } else {
            for rel in &tests_to_rebuild {
                if !units_by_file.contains_key(rel) {
                    let content = fs::read_to_string(self.project_dir.join(rel)).unwrap_or_default();
                    self.source_reads += 1;
                    units_by_file.insert(rel.clone(), extract::extract_code_units_in(rel, &content, Some(&self.project_dir), self.allow_repo_extractor));
                }
            }
            let (fresh_units, fresh_edges, fresh_static) = self.build_tests_and_edges(&tests_to_rebuild, &units_by_file);
            let mut test_units: Vec<Value> = self.store.array("test_units").into_iter().filter(|u| {
                u.get("file_path").and_then(|v| v.as_str()).is_none_or(|p| !tests_to_rebuild.iter().any(|c| c == p))
            }).collect();
            test_units.extend(fresh_units);
            let mut edges: Vec<Value> = self.store.array("test_edges").into_iter().filter(|e| {
                let tid = e.get("test_id").and_then(|v| v.as_str()).unwrap_or("");
                let code_id = e.get("code_id").and_then(|v| v.as_str()).unwrap_or("");
                let rebuilt_test = tests_to_rebuild.iter().any(|c| tid.starts_with(&format!("{c}:")));
                let replaced_target = changed_sources.iter().any(|path| code_id.starts_with(&format!("{path}:")));
                !rebuilt_test && !replaced_target
            }).collect();
            edges.extend(fresh_edges);
            let mut static_rows: Vec<Value> = self.store.array("static_test_imports").into_iter().filter(|r| {
                r.get("test_file").and_then(|v| v.as_str()).is_none_or(|p| !tests_to_rebuild.iter().any(|c| c == p))
            }).collect();
            static_rows.extend(fresh_static);
            self.store.doc.insert("test_units", Value::Array(test_units));
            self.store.doc.insert("test_edges", Value::Array(edges));
            self.store.doc.insert("static_test_imports", Value::Array(static_rows));
        }
        let mut imports: Vec<Value> = self.store.array("import_edges").into_iter().filter(|e| {
            e.get("importer_file").and_then(|v| v.as_str()).is_none_or(|p| !changed_set.contains(p))
        }).collect();
        imports.extend(self.build_import_edges(&changed_sources));
        self.store.doc.insert("import_edges", Value::Array(imports));
        self.drop_removed_paths(removed);
        self.store.save()?;
        let mut summary = vec![
            ("files_updated".into(), Value::Int((changed.len() + removed.len()) as i64)),
            ("code_units_found".into(), Value::Int(self.store.array("code_units").len() as i64)),
            ("new_commits".into(), Value::Int(new_commits.len() as i64)),
            ("edge_rebuild_skipped".into(), Value::Bool(edge_rebuild_skipped)),
            ("hint".into(), Value::String("For large repos, use start_job with kind='update' to avoid MCP timeouts.".into())),
        ];
        if skip_unit {
            summary.push(("unit_churn_skipped".into(), Value::Bool(true)));
        }
        push_extractor_warnings(&mut summary);
        Ok(Value::object(summary))
    }

    fn drop_removed_paths(&mut self, removed: &[String]) {
        if removed.is_empty() {
            return;
        }
        let gone = |path: &str| removed.iter().any(|p| p == path);
        let id_gone = |id: &str| removed.iter().any(|path| id.starts_with(&format!("{path}:")));
        let keep_units = |rows: Vec<Value>| -> Vec<Value> {
            rows.into_iter().filter(|row| row.get("file_path").and_then(|v| v.as_str()).is_none_or(|p| !gone(p))).collect()
        };
        self.store.doc.insert("code_units", Value::Array(keep_units(self.store.array("code_units"))));
        self.store.doc.insert("test_units", Value::Array(keep_units(self.store.array("test_units"))));
        self.store.doc.insert("churn", Value::Array(keep_units(self.store.array("churn"))));
        self.store.doc.insert("blame", Value::Array(keep_units(self.store.array("blame"))));
        self.store.doc.insert("file_hashes", Value::Array(keep_units(self.store.array("file_hashes"))));
        let edges: Vec<Value> = self.store.array("test_edges").into_iter().filter(|edge| {
            let tid = edge.get("test_id").and_then(|v| v.as_str()).unwrap_or("");
            let code_id = edge.get("code_id").and_then(|v| v.as_str()).unwrap_or("");
            let code_file = edge.get("code_file").and_then(|v| v.as_str()).unwrap_or("");
            !id_gone(tid) && !id_gone(code_id) && !gone(code_file)
        }).collect();
        self.store.doc.insert("test_edges", Value::Array(edges));
        let static_rows: Vec<Value> = self.store.array("static_test_imports").into_iter().filter(|row| {
            row.get("test_file").and_then(|v| v.as_str()).is_none_or(|p| !gone(p))
                && row.get("code_file").and_then(|v| v.as_str()).is_none_or(|p| !gone(p))
        }).collect();
        self.store.doc.insert("static_test_imports", Value::Array(static_rows));
        let imports: Vec<Value> = self.store.array("import_edges").into_iter().filter(|row| {
            row.get("importer_file").and_then(|v| v.as_str()).is_none_or(|p| !gone(p))
                && row.get("imported_file").and_then(|v| v.as_str()).is_none_or(|p| !gone(p))
        }).collect();
        self.store.doc.insert("import_edges", Value::Array(imports));
    }

    pub(super) fn scan_code_files(&self, git: &GitAnalyzer) -> Vec<String> {
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

    pub(super) fn test_file_set(&self) -> BTreeSet<String> {
        self.store.array("test_units").into_iter().filter_map(|u| u.get("file_path").and_then(|v| v.as_str()).map(|s| s.to_string())).collect()
    }

    pub(super) fn build_tests_and_edges(&mut self, files: &[String], units_by_file: &BTreeMap<String, Vec<CodeUnit>>) -> (Vec<Value>, Vec<Value>, Vec<Value>) {
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
                if extract::is_test_name(&u.name, framework) || u.unit_type == "test_case" || u.unit_type == "test_suite" {
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
        let mut bare_indexes: BTreeMap<String, BTreeMap<String, Vec<(String, String)>>> = BTreeMap::new();
        for tu in &test_units {
            let file = tu.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
            let tid = tu.get("id").and_then(|v| v.as_str()).unwrap_or("");
            if !bare_indexes.contains_key(file) {
                self.bare_index_builds += 1;
                bare_indexes.insert(file.to_string(), bare_name_index(&code_units));
            }
            let index = bare_indexes.get(file).expect("bare-name index");
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
                let matched = match_dep(file, &dep, &code_units, &resolved, index);
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

    pub(super) fn build_import_edges(&self, files: &[String]) -> Vec<Value> {
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

    pub fn delete_code_unit(&mut self, id: &str) -> Result<(), String> {
        let kept: Vec<Value> = self.store.array("code_units").into_iter().filter(|u| u.get("id").and_then(|v| v.as_str()) != Some(id)).collect();
        self.store.doc.insert("code_units", Value::Array(kept));
        self.store.save()?;
        Ok(())
    }

    pub fn record_result(&mut self, test_id: &str, passed: bool) -> Result<Value, String> {
        let stored = normalize_test_id(test_id);
        let now = now_secs();
        self.store.edit_and_save(|doc| {
            prune_old_results(doc, now);
            append_test_result(doc, test_id, passed, now);
        })?;
        Ok(Value::object(vec![
            ("test_id".into(), Value::String(stored)),
            ("passed".into(), Value::Bool(passed)),
            ("recorded".into(), Value::Bool(true)),
        ]))
    }

    pub fn record_results_batch(&mut self, rows: &[(String, bool)]) -> Result<Value, String> {
        let before = self.store.doc.get("generation").and_then(|v| v.as_i64()).unwrap_or(0);
        let now = now_secs();
        prune_old_results(&mut self.store.doc, now);
        for (test_id, passed) in rows {
            append_test_result(&mut self.store.doc, test_id, *passed, now);
        }
        self.store.save()?;
        let after = self.store.doc.get("generation").and_then(|v| v.as_i64()).unwrap_or(0);
        Ok(Value::object(vec![
            ("recorded".into(), Value::Int(rows.len() as i64)),
            ("generation_delta".into(), Value::Int(after - before)),
        ]))
    }

    pub(super) fn rebuild_cross_shard(&mut self) -> Result<(), String> {
        let shards = self.shards.clone();
        if shards.is_empty() {
            return Ok(());
        }
        let mut names: Vec<String> = shards.iter().map(|(name, _)| name.clone()).collect();
        if self.store.dir().join("shards").join(UNASSIGNED_SHARD).join(store::STORE_NAME).exists() {
            names.push(UNASSIGNED_SHARD.to_string());
        }
        let mut all_units: Vec<Value> = Vec::new();
        let mut local_edges: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        let mut local_units: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        let mut test_files: Vec<(String, String, String)> = Vec::new();
        for name in &names {
            let sub = self.open_shard(name)?;
            let units = sub.store.array("code_units");
            all_units.extend(units.iter().cloned());
            local_units.insert(name.clone(), units);
            local_edges.insert(name.clone(), sub.store.array("test_edges"));
            for unit in sub.store.array("test_units") {
                let file = unit.get("file_path").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let id = unit.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let test_name = unit.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if file.is_empty() || id.is_empty() {
                    continue;
                }
                test_files.push((name.clone(), file, format!("{id}\t{test_name}")));
            }
        }
        let index = bare_name_index(&all_units);
        let mut edges = Vec::new();
        let mut slim_tests = Vec::new();
        let mut static_rows = Vec::new();
        let mut seen_edges = BTreeSet::new();
        let mut seen_static = BTreeSet::new();
        let mut seen_slim = BTreeSet::new();
        let mut content_cache: BTreeMap<String, String> = BTreeMap::new();
        let mut tests_by_file: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
        for (shard, file, packed) in &test_files {
            let Some((id, test_name)) = packed.split_once('\t') else { continue };
            if id.is_empty() {
                continue;
            }
            tests_by_file.entry(file.clone()).or_default().push((id.to_string(), test_name.to_string()));
            let _ = shard;
        }
        for (file, tests) in &tests_by_file {
            let content = content_cache.entry(file.clone()).or_insert_with(|| {
                fs::read_to_string(self.project_dir.join(file)).unwrap_or_default()
            });
            let deps = extract::extract_deps(file, content);
            let resolved = resolved_import_files(&deps, &all_units);
            let test_shard = shard_owner(file, &shards);
            for path in &resolved {
                let code_shard = shard_owner(path, &shards);
                if code_shard == test_shard {
                    continue;
                }
                let key = format!("{file}|{path}");
                if seen_static.insert(key) {
                    static_rows.push(Value::object(vec![
                        ("test_file".into(), Value::String(file.clone())),
                        ("code_file".into(), Value::String(path.clone())),
                    ]));
                }
            }
            for (id, test_name) in tests {
                for dep in &deps {
                    for cid in match_dep(file, dep, &all_units, &resolved, &index) {
                        let code_file = all_units.iter().find(|u| u.get("id").and_then(|v| v.as_str()) == Some(cid.as_str())).and_then(|u| u.get("file_path").and_then(|v| v.as_str())).unwrap_or("");
                        let code_shard = shard_owner(code_file, &shards);
                        if code_file.is_empty() || code_shard == test_shard {
                            continue;
                        }
                        let key = format!("{id}|{cid}|{}", dep.dep_type);
                        if !seen_edges.insert(key) {
                            continue;
                        }
                        let weight = extract::proximity_weight(file, code_file) * dep.confidence.sqrt();
                        edges.push(Value::object(vec![
                            ("test_id".into(), Value::String(id.clone())),
                            ("code_id".into(), Value::String(cid)),
                            ("code_file".into(), Value::String(code_file.into())),
                            ("edge_type".into(), Value::String(dep.dep_type.clone())),
                            ("weight".into(), Value::Float(weight)),
                        ]));
                        if seen_slim.insert(id.clone()) {
                            slim_tests.push(Value::object(vec![
                                ("id".into(), Value::String(id.clone())),
                                ("file_path".into(), Value::String(file.clone())),
                                ("name".into(), Value::String(test_name.clone())),
                            ]));
                        }
                    }
                }
            }
        }
        let mut import_edges = Vec::new();
        let mut seen_imports = BTreeSet::new();
        let mut source_files = BTreeSet::new();
        for units in local_units.values() {
            for unit in units {
                let path = unit.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
                if !path.is_empty() && !looks_like_test(path) {
                    source_files.insert(path.to_string());
                }
            }
        }
        for rel in &source_files {
            let content = content_cache.entry(rel.clone()).or_insert_with(|| {
                fs::read_to_string(self.project_dir.join(rel)).unwrap_or_default()
            });
            let from_shard = shard_owner(rel, &shards);
            for dep in extract::extract_deps(rel, content) {
                if dep.dep_type != "import" && dep.dep_type != "tainted_import" {
                    continue;
                }
                let Some(module) = dep.module_path.as_deref() else { continue };
                for cu in &all_units {
                    let path = cu.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
                    let name = cu.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    if name == dep.name && extract::matches_import_path(path, module) && path != rel {
                        let to_shard = shard_owner(path, &shards);
                        if to_shard == from_shard {
                            continue;
                        }
                        let key = format!("{rel}|{path}");
                        if seen_imports.insert(key) {
                            import_edges.push(Value::object(vec![
                                ("importer_file".into(), Value::String(rel.clone())),
                                ("imported_file".into(), Value::String(path.into())),
                                ("confidence".into(), Value::Float(1.0)),
                            ]));
                        }
                    }
                }
            }
        }
        let mut bridge_files = BTreeSet::new();
        for edge in &import_edges {
            if let Some(file) = edge.get("importer_file").and_then(|v| v.as_str()) {
                bridge_files.insert(file.to_string());
            }
        }
        let mut bridge_units = Vec::new();
        let mut bridge_edges = Vec::new();
        let mut seen_bridge_units = BTreeSet::new();
        let mut seen_bridge_edges = BTreeSet::new();
        for file in &bridge_files {
            let owner = shard_owner(file, &shards);
            let Some(units) = local_units.get(owner) else { continue };
            let mut ids = BTreeSet::new();
            for unit in units {
                if unit.get("file_path").and_then(|v| v.as_str()) != Some(file.as_str()) {
                    continue;
                }
                let id = unit.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if id.is_empty() || !seen_bridge_units.insert(id.clone()) {
                    continue;
                }
                ids.insert(id);
                bridge_units.push(unit.clone());
            }
            let Some(edges_here) = local_edges.get(owner) else { continue };
            for edge in edges_here {
                let cid = edge.get("code_id").and_then(|v| v.as_str()).unwrap_or("");
                if !ids.contains(cid) {
                    continue;
                }
                let key = format!("{}|{cid}|{}", edge.get("test_id").and_then(|v| v.as_str()).unwrap_or(""), edge.get("edge_type").and_then(|v| v.as_str()).unwrap_or(""));
                if !seen_bridge_edges.insert(key) {
                    continue;
                }
                let mut row = edge.clone();
                row.insert("code_file", Value::String(file.clone()));
                bridge_edges.push(row);
            }
        }
        self.store.edit_and_save(|doc| {
            doc.insert("test_edges", Value::Array(edges));
            doc.insert("test_units", Value::Array(slim_tests));
            doc.insert("static_test_imports", Value::Array(static_rows));
            doc.insert("import_edges", Value::Array(import_edges));
            doc.insert("bridge_units", Value::Array(bridge_units));
            doc.insert("bridge_edges", Value::Array(bridge_edges));
        })?;
        Ok(())
    }

    fn analyze_unassigned(&mut self, files: &[String], update: bool) -> Result<Option<Value>, String> {
        let rest: BTreeSet<String> = files.iter().filter(|f| shard_name_for(f, &self.shards).is_none()).cloned().collect();
        let snapshot = self.store.dir().join("shards").join(UNASSIGNED_SHARD).join(store::STORE_NAME);
        if rest.is_empty() {
            if update || !snapshot.exists() {
                return Ok(None);
            }
            let mut sub = self.open_shard(UNASSIGNED_SHARD)?;
            sub.only_files = Some(BTreeSet::new());
            return Ok(Some(sub.analyze()?));
        }
        let mut sub = self.open_shard(UNASSIGNED_SHARD)?;
        sub.only_files = Some(rest);
        if update {
            Ok(Some(sub.update()?))
        } else {
            Ok(Some(sub.analyze()?))
        }
    }

    pub(super) fn analyze_all_shards(&mut self) -> Result<Value, String> {
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
        if let Some(summary) = self.analyze_unassigned(&files, false)? {
            scanned += summary.get("code_files_scanned").and_then(|v| v.as_i64()).unwrap_or(0);
        }
        self.rebuild_cross_shard()?;
        Ok(Value::object(vec![
            ("code_files_scanned".into(), Value::Int(scanned)),
            ("shards".into(), Value::Int(shards.len() as i64)),
            ("hint".into(), Value::String("For large repos, use start_job with kind='analyze' to avoid MCP timeouts.".into())),
        ]))
    }

    pub(super) fn update_all_shards(&mut self) -> Result<Value, String> {
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
        if let Some(summary) = self.analyze_unassigned(&files, true)? {
            updated += summary.get("files_updated").and_then(|v| v.as_i64()).unwrap_or(0);
            commits += summary.get("new_commits").and_then(|v| v.as_i64()).unwrap_or(0);
            if summary.get("edge_rebuild_skipped").and_then(|v| v.as_bool()) != Some(true) {
                skipped = false;
            }
        }
        if !skipped {
            self.rebuild_cross_shard()?;
        }
        Ok(Value::object(vec![
            ("files_updated".into(), Value::Int(updated)),
            ("new_commits".into(), Value::Int(commits)),
            ("edge_rebuild_skipped".into(), Value::Bool(skipped)),
            ("hint".into(), Value::String("For large repos, use start_job with kind='update' to avoid MCP timeouts.".into())),
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
            ("progress_pct".into(), Value::Int(0)),
            ("cancel".into(), Value::Bool(false)),
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
            if status.as_deref() == Some("cancelled") || job_should_stop(&id_thread, engine.store.dir()) {
                engine.note_job(&id_thread, "cancelled", 0);
            } else {
                engine.note_job(&id_thread, "completed", 100);
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
        let mut known = false;
        if let Ok(mut jobs) = job_lock().lock() {
            if let Some(job) = jobs.iter_mut().find(|j| j.id == id) {
                job.cancel = true;
                known = true;
            }
        }
        let flag = job_cancel_path(self.store.dir(), id);
        if fs::write(&flag, b"1").is_ok() {
            known = true;
        }
        self.store.reload().ok();
        let mut rows = self.store.array("jobs");
        let mut found = false;
        for row in rows.iter_mut() {
            if row.get("job_id").and_then(|v| v.as_str()) == Some(id) {
                row.insert("cancel", Value::Bool(true));
                row.insert("status", Value::String("cancel_requested".into()));
                found = true;
            }
        }
        if found {
            self.store.doc.insert("jobs", Value::Array(rows));
            let _ = self.store.save();
            known = true;
        }
        if known {
            return Value::object(vec![
                ("job_id".into(), Value::String(id.into())),
                ("status".into(), Value::String("cancel_requested".into())),
            ]);
        }
        Value::object(vec![
            ("status".into(), Value::String("error".into())),
            ("message".into(), Value::String(format!("unknown job: {id}"))),
        ])
    }

    pub(super) fn note_job(&mut self, id: &str, status: &str, pct: i64) {
        let status = if job_cancel_path(self.store.dir(), id).exists() { "cancelled" } else { status };
        set_job(id, status, pct);
        for _ in 0..3 {
            let mut rows = self.store.array("jobs");
            let mut found = false;
            for row in rows.iter_mut() {
                if row.get("job_id").and_then(|v| v.as_str()) == Some(id) {
                    row.insert("status", Value::String(status.into()));
                    row.insert("progress_pct", Value::Int(pct));
                    if status == "cancelled" || status == "cancel_requested" {
                        row.insert("cancel", Value::Bool(true));
                    }
                    found = true;
                }
            }
            if !found {
                rows.push(Value::object(vec![
                    ("job_id".into(), Value::String(id.into())),
                    ("status".into(), Value::String(status.into())),
                    ("kind".into(), Value::String("analyze".into())),
                    ("progress_pct".into(), Value::Int(pct)),
                    ("cancel".into(), Value::Bool(status == "cancelled")),
                ]));
            }
            self.store.doc.insert("jobs", Value::Array(rows));
            if self.store.save().is_ok() {
                return;
            }
        }
    }

}
