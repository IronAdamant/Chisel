use super::*;
use crate::extract::{self};
use crate::git::{self};
use crate::hash::sha256_hex;
use crate::impact::{self, RiskInputs};
use crate::json::Value;
use crate::store::{self, Store};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;

fn cached_exists(cache: &mut HashMap<String, bool>, project: &std::path::Path, path: &str) -> bool {
    if let Some(hit) = cache.get(path) {
        return *hit;
    }
    let ok = project.join(path).exists();
    cache.insert(path.to_string(), ok);
    ok
}

fn cross_edge_key(edge: &Value) -> String {
    format!(
        "{}|{}|{}",
        edge.get("test_id").and_then(|v| v.as_str()).unwrap_or(""),
        edge.get("code_id").and_then(|v| v.as_str()).unwrap_or(""),
        edge.get("edge_type").and_then(|v| v.as_str()).unwrap_or("")
    )
}

impl Engine {
    pub fn has_data(&self) -> bool {
        self.store.array_len("code_units") > 0
    }

    pub(super) fn no_data() -> Value {
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

    pub(super) fn risk_map_with(&mut self, reweight: bool) -> Result<Value, String> {
        self.store.reload()?;
        self.overlay_applied = false;
        self.absorb_parent_overlay();
        if !self.has_data() {
            return Ok(Self::no_data());
        }
        let gtest_files: BTreeSet<String> = self.store.rows("test_units").iter().filter_map(|u| {
            let framework = u.get("framework").and_then(|v| v.as_str())?;
            if framework == "gtest" { u.get("file_path").and_then(|v| v.as_str()).map(|s| s.to_string()) } else { None }
        }).collect();
        let mut files: Vec<String> = self.store.rows("churn").iter().filter_map(|c| c.get("file_path").and_then(|v| v.as_str()).map(|s| s.to_string())).filter(|p| {
            if let Some(dir) = &self.directory {
                if !path_in_directory(p, dir) {
                    return false;
                }
            }
            if self.exclude_tests && (looks_like_test(p) || gtest_files.contains(p) || is_test_support_path(p)) {
                return false;
            }
            true
        }).collect();
        files.sort();
        files.dedup();
        let query_min: i64 = store::meta_get(&self.store.doc, "co_change_query_min").and_then(|s| s.parse().ok()).unwrap_or(1);
        let imports = self.store.rows("import_edges");

        let mut neighbor_map: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for e in imports {
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
        let mut branch_counts: BTreeMap<String, usize> = BTreeMap::new();
        for row in self.store.rows("branch_co_changes") {
            let n = row.get("co_commit_count").and_then(|v| v.as_i64()).unwrap_or(0);
            if n < 1 {
                continue;
            }
            let a = row.get("file_a").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let b = row.get("file_b").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if !a.is_empty() {
                *branch_counts.entry(a).or_default() += 1;
            }
            if !b.is_empty() {
                *branch_counts.entry(b).or_default() += 1;
            }
        }

        let recorded = self.result_rows();
        let failure_rates = failure_rate_map(&recorded);
        let duration_cvs = duration_cv_map(&recorded);
        let trend_now = now_secs();
        let mut exists_cache: HashMap<String, bool> = HashMap::new();
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
                if a != f && b != f {
                    continue;
                }
                let other = if a == f { b } else { a };
                if !cached_exists(&mut exists_cache, &self.project_dir, other) {
                    continue;
                }
                co_count += 1;
                if a == f {
                    partners.push((b.to_string(), n, 0));
                } else {
                    partners.push((a.to_string(), n, 1));
                }
            }
            partners.sort_by(|a, b| b.1.cmp(&a.1).then(a.2.cmp(&b.2)).then(a.0.cmp(&b.0)));
            let mut import_neighbors: Vec<String> = neighbor_map.get(f).map(|s| s.iter().filter(|n| cached_exists(&mut exists_cache, &self.project_dir, n)).cloned().collect()).unwrap_or_default();
            import_neighbors.sort();
            let file_units: Vec<&Value> = self.store.units_by_file(f).into_iter().filter(|u| !is_test_unit_row(u)).collect();
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
            let instability = test_instability(&covering, &failure_rates, &duration_cvs);
            let mut eval_patterns = 0i64;
            if self.working_tree && (f.ends_with(".js") || f.ends_with(".ts") || f.ends_with(".jsx") || f.ends_with(".tsx")) {
                self.source_reads += 1;
                if let Ok(text) = fs::read_to_string(self.project_dir.join(f)) {
                    eval_patterns = text.matches("new Function(").count() as i64;
                }
            }
            let mut row = impact::score_file(&RiskInputs {
                file_path: f.clone(),
                churn_raw,
                co_change_count: co_count,
                branch_co_count: branch_counts.get(f).copied().unwrap_or(0),
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
                instability,
                eval_patterns,
                exclude_new_file_boost: self.exclude_new_file_boost,
                coverage_mode: self.coverage_mode.clone(),
                proximity_hops: hops.get(f).copied(),
                proximity: self.proximity_adjustment,
            });
            if let Some(trend) = mean_flakiness(&covering, &recorded, trend_now) {
                row.insert("flakiness_trend", Value::Float(trend));
            }
            scored.push(row);
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
        let commits_n = self.store.array_len("commits") as i64;
        let test_edges_n = self.store.array_len("test_edges") as i64;
        let test_results_n = recorded.len() as i64;
        let import_n = imports.len() as i64;
        let co_n = self.store.rows("co_changes").iter().filter(|c| c.get("co_commit_count").and_then(|v| v.as_i64()).unwrap_or(0) >= query_min).count() as i64;
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
        let gap_mode = if self.proximity_adjustment { "proximity_adjusted" } else { self.coverage_mode.as_str() };
        meta.insert("coverage_gap_mode", Value::String(gap_mode.into()));
        let scored_paths: Vec<String> = scored.iter().filter_map(|row| row.get("file_path").and_then(|v| v.as_str()).map(|s| s.to_string())).collect();
        let scored_set: BTreeSet<String> = scored_paths.iter().cloned().collect();
        let mut directed: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for edge in imports {
            let conf = edge.get("confidence").and_then(|v| v.as_f64()).unwrap_or(1.0);
            if conf < 1.0 {
                continue;
            }
            let from = edge.get("importer_file").and_then(|v| v.as_str()).unwrap_or("");
            let to = edge.get("imported_file").and_then(|v| v.as_str()).unwrap_or("");
            if scored_set.contains(from) && scored_set.contains(to) && from != to {
                directed.entry(from.to_string()).or_default().insert(to.to_string());
            }
        }
        meta.insert("cycles", Value::Array(impact::import_cycles(&scored_paths, &directed)));
        let locks = self.active_locks();
        if !locks.is_empty() {
            let lock_map: BTreeMap<String, String> = locks.iter().filter_map(|lock| {
                Some((lock.get("file_path")?.as_str()?.to_string(), lock.get("agent_id")?.as_str()?.to_string()))
            }).collect();
            let mut claims = 0i64;
            for row in &mut scored {
                let path = row.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
                if let Some(agent) = lock_map.get(path) {
                    row.insert("locked_by", Value::String(agent.clone()));
                    claims += 1;
                }
            }
            meta.insert("active_locks", Value::Int(locks.len() as i64));
            meta.insert("lock_claims", Value::Int(claims));
        }
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
        // Each git spawn is a process start; on Windows that is ~100 ms. Overlap the independent ones.
        let (branch, untracked) = std::thread::scope(|s| {
            let untracked = s.spawn(|| git.untracked());
            (git.current_branch(), untracked.join().unwrap_or_else(|_| Ok(Vec::new())))
        });
        let branch = match branch {
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
        // The changed-file list and the changed functions come from two git calls run together.
        let (diff_files, all_functions) = std::thread::scope(|s| {
            let functions = s.spawn(|| git.changed_functions_all(&diff_ref));
            (git.changed_files(&diff_ref), functions.join().unwrap_or_else(|_| Ok(BTreeMap::new())))
        });
        let diff_files = match diff_files {
            Ok(v) => v,
            Err(e) => return Ok(git::git_tool_error(&self.project_dir.to_string_lossy(), &e.0)),
        };
        let all_functions = all_functions.unwrap_or_default();
        let untracked_code: BTreeSet<String> = untracked.unwrap_or_default().into_iter().filter(|p| extract::path_has_code_extension(p)).collect();
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
        let functions: BTreeMap<String, Vec<String>> = all_functions.into_iter().filter(|(f, names)| !names.is_empty() && changed.contains(f)).collect();
        let mut rows = self.impacted(&changed, &functions, &untracked_code);
        self.attach_working_tree_hits(&mut rows, &changed, &untracked_code);
        Ok(Value::Array(rows))
    }

    pub fn suggest_tests(&mut self, file: &str) -> Result<Value, String> {
        if !self.shards.is_empty() {
            let mut sub = self.open_shard(shard_owner(file, &self.shards))?;
            return sub.suggest_tests(file);
        }
        self.store.reload()?;
        self.overlay_applied = false;
        self.absorb_parent_overlay();
        if !self.has_data() {
            return Ok(Self::no_data());
        }
        if self.store.hash_for_file(file).is_empty() && !self.working_tree {
            return Ok(Value::object(vec![
                ("status".into(), Value::String("stale_db".into())),
                ("file_path".into(), Value::String(file.into())),
                ("message".into(), Value::String("File not found in analysis database.".into())),
                ("hint".into(), Value::String("Run 'chisel update' first to include new or changed files.".into())),
            ]));
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
        let recorded = self.result_rows();
        let failure_rates = failure_rate_map(&recorded);
        let trend_now = now_secs();
        let mut result = Vec::new();
        for (_tid, item) in by_id {
            let relevance = item.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let tid = item.get("test_id").and_then(|v| v.as_str()).unwrap_or("");
            let (relevance, rate_v, boost_v) = apply_failure(relevance, failure_rates.get(tid).copied().unwrap_or(0.0));
            let mut row = Value::object(vec![
                ("test_id".into(), item.get("test_id").cloned().unwrap_or(Value::Null)),
                ("file_path".into(), item.get("file_path").cloned().unwrap_or(Value::Null)),
                ("name".into(), item.get("name").cloned().unwrap_or(Value::Null)),
                ("relevance".into(), Value::Float(relevance)),
                ("reason".into(), item.get("reason").cloned().unwrap_or(Value::String(String::new()))),
                ("source".into(), item.get("source").cloned().unwrap_or(Value::String("direct".into()))),
                ("failure_rate".into(), Value::Float(rate_v)),
                ("failure_boost".into(), Value::Float(boost_v)),
            ]);
            if let Some(trend) = flakiness_trend(&recorded, tid, trend_now) {
                row.insert("flakiness_trend", Value::Float(trend));
            }
            result.push(row);
        }
        let skip_stem = result.is_empty() && self.fallback_to_all && self.working_tree && self.store.hash_for_file(file).is_empty();
        if result.is_empty() && !skip_stem {
            for (tid, file_path, name, relevance) in self.stem_matches(file) {
                let (relevance, rate_v, boost_v) = apply_failure(relevance, failure_rates.get(&tid).copied().unwrap_or(0.0));
                let mut row = Value::object(vec![
                    ("test_id".into(), Value::String(tid.clone())),
                    ("file_path".into(), Value::String(file_path)),
                    ("name".into(), Value::String(name)),
                    ("relevance".into(), Value::Float(relevance)),
                    ("reason".into(), Value::String("fallback: stem-matched test file".into())),
                    ("source".into(), Value::String("fallback".into())),
                    ("failure_rate".into(), Value::Float(rate_v)),
                    ("failure_boost".into(), Value::Float(boost_v)),
                ]);
                if let Some(trend) = flakiness_trend(&recorded, &tid, trend_now) {
                    row.insert("flakiness_trend", Value::Float(trend));
                }
                result.push(row);
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

    pub(super) fn static_hits(&mut self, file: &str) -> Vec<Value> {
        let tests = self.store.rows("test_units");
        let mut by_file: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for t in tests {
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
            for row in self.store.rows("static_test_imports") {
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

    pub(super) fn direct_brief(&self, file: &str) -> Vec<(String, f64, String, String, String)> {
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

    pub(super) fn remember_hit(best: &mut BTreeMap<String, (f64, Value)>, tid: &str, score: f64, row: Value) {
        match best.get(tid) {
            Some((prev, _)) if *prev >= score => {}
            _ => { best.insert(tid.to_string(), (score, row)); }
        }
    }

    pub(super) fn stem_matches(&self, file: &str) -> Vec<(String, String, String, f64)> {
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

    pub(super) fn attach_working_tree_hits(&mut self, rows: &mut Vec<Value>, changed: &[String], untracked: &BTreeSet<String>) {
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

    pub(super) fn impacted(&mut self, files: &[String], functions: &BTreeMap<String, Vec<String>>, untracked: &BTreeSet<String>) -> Vec<Value> {
        self.absorb_parent_overlay();
        let tests: BTreeMap<String, Value> = self.store.array("test_units").into_iter().filter_map(|t| {
            Some((t.get("id")?.as_str()?.to_string(), t))
        }).collect();
        let mut best: BTreeMap<String, (f64, Value)> = BTreeMap::new();
        for file in files {
            let units = self.store.units_by_file(file);
            let named = |unit: &Value, list: &[String]| {
                let uname = unit.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let bare = uname.rsplit('.').next().unwrap_or(uname);
                list.iter().any(|n| n == uname || n == bare)
            };
            let fns = functions.get(file).map(|v| v.as_slice());
            // Narrow to the changed functions only when one of them has a direct test. A changed
            // private helper is still exercised by the tests of its siblings in the same file.
            let narrowed = fns.filter(|list| {
                units.iter().any(|u| named(u, list) && !self.store.edges_for_code(u.get("id").and_then(|v| v.as_str()).unwrap_or("")).is_empty())
            });
            let widened = fns.is_some() && narrowed.is_none();
            for unit in &units {
                if narrowed.is_some_and(|list| !named(unit, list)) {
                    continue;
                }
                let uname = unit.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let id = unit.get("id").and_then(|v| v.as_str()).unwrap_or("");
                for e in self.store.edges_for_code(id) {
                    let tid = e.get("test_id").and_then(|v| v.as_str()).unwrap_or("");
                    let weight = e.get("weight").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let test = tests.get(tid);
                    let edge_type = e.get("edge_type").and_then(|v| v.as_str()).unwrap_or("import");
                    let reason = if widened {
                        format!("direct edge to {uname} ({edge_type}); no test calls the changed {} directly", fns.unwrap_or(&[]).join(", "))
                    } else {
                        format!("direct edge to {uname} ({edge_type})")
                    };
                    let row = Value::object(vec![
                        ("test_id".into(), Value::String(tid.into())),
                        ("file_path".into(), test.and_then(|t| t.get("file_path")).cloned().unwrap_or(Value::String(String::new()))),
                        ("name".into(), test.and_then(|t| t.get("name")).cloned().unwrap_or(Value::String(String::new()))),
                        ("reason".into(), Value::String(reason)),
                        ("score".into(), Value::Float(weight)),
                        ("source".into(), Value::String("direct".into())),
                    ]);
                    Self::remember_hit(&mut best, tid, weight, row);
                }
            }
        }
        let query_min: i64 = store::meta_get(&self.store.doc, "co_change_query_min").and_then(|s| s.parse().ok()).unwrap_or(1);
        for file in files {
            for cc in self.store.co_for_file(file) {
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
        let imports = self.store.rows("import_edges");
        let mut reverse: BTreeMap<String, Vec<(String, f64)>> = BTreeMap::new();
        for e in imports {
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
        self.append_working_tree_stale(&mut stale);
        Ok(Value::Array(stale))
    }

    fn append_working_tree_stale(&self, stale: &mut Vec<Value>) {
        if !self.working_tree {
            return;
        }
        let mut known: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for unit in self.store.array("code_units") {
            let path = unit.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
            let name = unit.get("name").and_then(|v| v.as_str()).unwrap_or("");
            if path.is_empty() || name.is_empty() {
                continue;
            }
            known.entry(path.to_string()).or_default().insert(name.to_string());
        }
        let mut live: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for path in known.keys() {
            let content = fs::read_to_string(self.project_dir.join(path)).unwrap_or_default();
            let names = extract::extract_code_units_in(path, &content, Some(&self.project_dir), self.allow_repo_extractor).into_iter().map(|u| u.name).collect();
            live.insert(path.clone(), names);
        }
        let mut test_files: BTreeSet<String> = self.store.array("test_units").into_iter().filter_map(|t| t.get("file_path").and_then(|v| v.as_str()).map(|s| s.to_string())).collect();
        if let Ok(untracked) = self.git.fork().untracked() {
            for path in untracked {
                if looks_like_test(&path) {
                    test_files.insert(path);
                }
            }
        }
        let mut seen: BTreeSet<String> = stale.iter().map(|row| {
            format!("{}|{}", row.get("test_id").and_then(|v| v.as_str()).unwrap_or(""), row.get("missing_code_id").and_then(|v| v.as_str()).unwrap_or(""))
        }).collect();
        for test_file in test_files {
            let content = fs::read_to_string(self.project_dir.join(&test_file)).unwrap_or_default();
            if content.is_empty() {
                continue;
            }
            for dep in extract::extract_deps(&test_file, &content) {
                if dep.dep_type != "import" && dep.dep_type != "tainted_import" {
                    continue;
                }
                let Some(module) = dep.module_path.as_deref() else { continue };
                let Some(code_file) = known.keys().find(|path| extract::matches_import_path(path, module)).cloned() else { continue };
                let Some(snapshot_names) = known.get(&code_file) else { continue };
                if !snapshot_names.contains(&dep.name) {
                    continue;
                }
                if live.get(&code_file).is_some_and(|names| names.contains(&dep.name)) {
                    continue;
                }
                let missing = format!("{code_file}:{}:function", dep.name);
                let mut owners: Vec<(String, String)> = self.store.array("test_units").into_iter().filter_map(|unit| {
                    if unit.get("file_path").and_then(|v| v.as_str()) != Some(test_file.as_str()) {
                        return None;
                    }
                    Some((
                        unit.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                        unit.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    ))
                }).filter(|(id, _)| !id.is_empty()).collect();
                if owners.is_empty() {
                    owners.push((test_file.clone(), test_file.clone()));
                }
                for (tid, tname) in owners {
                    let key = format!("{tid}|{missing}");
                    if !seen.insert(key) {
                        continue;
                    }
                    stale.push(Value::object(vec![
                        ("test_id".into(), Value::String(tid)),
                        ("test_name".into(), Value::String(tname)),
                        ("missing_code_id".into(), Value::String(missing.clone())),
                        ("edge_type".into(), Value::String("working_tree".into())),
                    ]));
                }
            }
        }
    }

    pub fn stats(&mut self) -> Result<Value, String> {
        self.stats_with(false)
    }

    pub(super) fn stats_with(&mut self, include_results: bool) -> Result<Value, String> {
        if !self.shards.is_empty() {
            return self.stats_shards(include_results);
        }
        self.store.reload()?;
        Ok(self.stats_doc(include_results))
    }

    pub(super) fn stats_doc(&self, include_results: bool) -> Value {
        let results = self.store.rows("test_results");
        let passed = results.iter().filter(|r| r.get("passed").and_then(|v| v.as_bool()) == Some(true)).count() as i64;
        let failed = results.iter().filter(|r| r.get("passed").and_then(|v| v.as_bool()) == Some(false)).count() as i64;
        let mut pairs = vec![
            ("code_units".into(), Value::Int(self.store.array_len("code_units") as i64)),
            ("test_units".into(), Value::Int(self.store.array_len("test_units") as i64)),
            ("test_edges".into(), Value::Int(self.store.array_len("test_edges") as i64)),
            ("commits".into(), Value::Int(self.store.array_len("commits") as i64)),
            ("co_changes".into(), Value::Int(self.store.array_len("co_changes") as i64)),
            ("import_edges".into(), Value::Int(self.store.array_len("import_edges") as i64)),
            ("test_results".into(), Value::Int(results.len() as i64)),
            ("passed".into(), Value::Int(passed)),
            ("failed".into(), Value::Int(failed)),
            ("commit_files".into(), Value::Int(self.store.array_len("commit_files") as i64)),
            ("blame_cache".into(), Value::Int(self.store.array_len("blame") as i64)),
            ("branch_co_changes".into(), Value::Int(self.store.array_len("branch_co_changes") as i64)),
            ("churn_stats".into(), Value::Int(self.store.array_len("churn") as i64)),
            ("file_hashes".into(), Value::Int(self.store.array_len("file_hashes") as i64)),
            ("coupling_threshold".into(), Value::Int(impact::coupling_threshold(self.store.array_len("commits") as i64))),
            ("co_change_query_min".into(), Value::Int(store::meta_get(&self.store.doc, "co_change_query_min").and_then(|s| s.parse().ok()).unwrap_or(3).max(1))),
        ];
        let edge_rows = self.store.rows("test_edges");
        if !edge_rows.is_empty() {
            let mut counts: BTreeMap<&str, i64> = BTreeMap::new();
            for edge in edge_rows {
                let kind = edge.get("edge_type").and_then(|v| v.as_str()).unwrap_or("");
                *counts.entry(kind).or_default() += 1;
            }
            let total = edge_rows.len() as f64;
            let dynamic = counts.get("dynamic_import").copied().unwrap_or(0) + counts.get("eval_import").copied().unwrap_or(0);
            pairs.push(("shadow_graph".into(), Value::object(vec![
                ("total_edges".into(), Value::Int(edge_rows.len() as i64)),
                ("call_edges".into(), Value::Int(counts.get("call").copied().unwrap_or(0))),
                ("import_edges".into(), Value::Int(counts.get("import").copied().unwrap_or(0))),
                ("dynamic_import_edges".into(), Value::Int(dynamic)),
                ("eval_import_edges".into(), Value::Int(counts.get("eval_import").copied().unwrap_or(0))),
                ("tainted_import_edges".into(), Value::Int(counts.get("tainted_import").copied().unwrap_or(0))),
                ("unknown_shadow_ratio".into(), Value::Float(crate::json::py_round(dynamic as f64 / total.max(1.0), 4))),
            ])));
        }
        if let Some(bc) = store::meta_get(&self.store.doc, "branch_coupling_commits") {
            if let Ok(n) = bc.parse::<i64>() {
                pairs.push(("branch_coupling_commits".into(), Value::Int(n)));
            }
        }
        if include_results {
            pairs.push(("recorded_results".into(), Value::Array(results.to_vec())));
        }
        Value::object(pairs)
    }

    pub(super) fn stats_shards(&self, include_results: bool) -> Result<Value, String> {
        let mut code_units = 0i64;
        let mut test_edges = 0i64;
        let mut test_results = 0i64;
        let mut commits = 0i64;
        for name in self.snapshot_names() {
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
            ("cross_shard_test_edges".into(), Value::Int(self.store.array_len("test_edges") as i64)),
        ];
        if include_results {
            pairs.push(("recorded_results".into(), Value::Array(vec![])));
        }
        Ok(Value::object(pairs))
    }

    fn result_rows(&self) -> Vec<Value> {
        if let Some(dir) = &self.result_log {
            if let Ok(store) = Store::open(dir) {
                return store.array("test_results");
            }
        }
        self.store.array("test_results")
    }

    fn absorb_parent_overlay(&mut self) {
        if self.overlay_applied {
            return;
        }
        let Some(dir) = self.result_log.clone() else {
            return;
        };
        let Some((shard, shards)) = self.shard_view.clone() else {
            return;
        };
        self.overlay_applied = true;
        let Ok(parent) = Store::open(&dir) else {
            return;
        };
        let in_shard = |file: &str| shard_owner(file, &shards) == shard.as_str();
        let mut edge_keys: BTreeSet<String> = self.store.array("test_edges").into_iter().map(|e| cross_edge_key(&e)).collect();
        let mut unit_ids: BTreeSet<String> = self.store.array("code_units").into_iter().filter_map(|u| u.get("id").and_then(|v| v.as_str()).map(|s| s.to_string())).collect();
        let mut test_ids: BTreeSet<String> = self.store.array("test_units").into_iter().filter_map(|u| u.get("id").and_then(|v| v.as_str()).map(|s| s.to_string())).collect();
        let mut static_keys: BTreeSet<String> = self.store.array("static_test_imports").into_iter().map(|r| {
            format!("{}|{}", r.get("test_file").and_then(|v| v.as_str()).unwrap_or(""), r.get("code_file").and_then(|v| v.as_str()).unwrap_or(""))
        }).collect();
        let mut import_keys: BTreeSet<String> = self.store.array("import_edges").into_iter().map(|r| {
            format!("{}|{}", r.get("importer_file").and_then(|v| v.as_str()).unwrap_or(""), r.get("imported_file").and_then(|v| v.as_str()).unwrap_or(""))
        }).collect();
        let mut importers = BTreeSet::new();
        for edge in parent.array("test_edges") {
            let code_file = edge.get("code_file").and_then(|v| v.as_str()).unwrap_or("");
            if !in_shard(code_file) {
                continue;
            }
            let key = cross_edge_key(&edge);
            if edge_keys.insert(key) {
                self.store.array_mut("test_edges").push(edge);
            }
        }
        for unit in parent.array("test_units") {
            let id = unit.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if id.is_empty() || !test_ids.insert(id) {
                continue;
            }
            self.store.array_mut("test_units").push(unit);
        }
        for row in parent.array("static_test_imports") {
            let code_file = row.get("code_file").and_then(|v| v.as_str()).unwrap_or("");
            if !in_shard(code_file) {
                continue;
            }
            let key = format!("{}|{code_file}", row.get("test_file").and_then(|v| v.as_str()).unwrap_or(""));
            if static_keys.insert(key) {
                self.store.array_mut("static_test_imports").push(row);
            }
        }
        for row in parent.array("import_edges") {
            let imported = row.get("imported_file").and_then(|v| v.as_str()).unwrap_or("");
            if !in_shard(imported) {
                continue;
            }
            if let Some(importer) = row.get("importer_file").and_then(|v| v.as_str()) {
                importers.insert(importer.to_string());
            }
            let key = format!("{}|{imported}", row.get("importer_file").and_then(|v| v.as_str()).unwrap_or(""));
            if import_keys.insert(key) {
                self.store.array_mut("import_edges").push(row);
            }
        }
        for unit in parent.array("bridge_units") {
            let file = unit.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
            if !importers.contains(file) {
                continue;
            }
            let id = unit.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if id.is_empty() || !unit_ids.insert(id) {
                continue;
            }
            self.store.array_mut("code_units").push(unit);
        }
        for edge in parent.array("bridge_edges") {
            let code_file = edge.get("code_file").and_then(|v| v.as_str()).unwrap_or("");
            if !importers.contains(code_file) {
                continue;
            }
            let key = cross_edge_key(&edge);
            if edge_keys.insert(key) {
                self.store.array_mut("test_edges").push(edge);
            }
        }
        self.store.reindex_memory();
    }

    fn snapshot_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.shards.iter().map(|(name, _)| name.clone()).collect();
        if self.store.dir().join("shards").join(UNASSIGNED_SHARD).join(store::STORE_NAME).exists() {
            names.push(UNASSIGNED_SHARD.to_string());
        }
        names
    }

    pub(super) fn open_shard(&self, name: &str) -> Result<Engine, String> {
        let dir = self.store.dir().join("shards").join(name);
        let mut sub = Engine::open(&self.project_dir, Some(&dir))?;
        sub.shards.clear();
        sub.result_log = Some(self.store.dir().to_path_buf());
        sub.shard_view = Some((name.to_string(), self.shards.clone()));
        sub.overlay_applied = false;
        sub.include_ignored = self.include_ignored;
        sub.working_tree = self.working_tree;
        sub.coverage_mode = self.coverage_mode.clone();
        sub.proximity_adjustment = self.proximity_adjustment;
        sub.exclude_new_file_boost = self.exclude_new_file_boost;
        sub.exclude_tests = self.exclude_tests;
        sub.fallback_to_all = self.fallback_to_all;
        sub.scope_file = self.scope_file.clone();
        sub.allow_repo_extractor = self.allow_repo_extractor;
        sub.diff_ref_override = self.diff_ref_override.clone();
        sub.directory = self.directory.clone();
        sub.git = self.git.fork();
        Ok(sub)
    }

    pub(super) fn aggregate_risk_map(&mut self) -> Result<Value, String> {
        let mut files = Vec::new();
        let mut any = false;
        for name in self.snapshot_names() {
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
                ("cross_shard_test_edges".into(), Value::Int(self.store.array_len("test_edges") as i64)),
            ])),
        ]))
    }

    pub(super) fn aggregate_stale(&self) -> Result<Value, String> {
        let mut stale = Vec::new();
        let mut any = false;
        let mut edges = false;
        for name in self.snapshot_names() {
            let mut sub = self.open_shard(&name)?;
            if !sub.has_data() {
                continue;
            }
            any = true;
            stale.extend(sub.stale_rows());
            if sub.store.array_len("test_edges") > 0 {
                edges = true;
            }
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
        Ok(Value::Array(stale))
    }

    pub(super) fn aggregate_impact(&self, files: &[String], functions: &BTreeMap<String, Vec<String>>) -> Result<Value, String> {
        let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for file in files {
            groups.entry(shard_owner(file, &self.shards).to_string()).or_default().push(file.clone());
        }
        let mut rows = Vec::new();
        let mut any = false;
        for (name, group) in groups {
            let mut sub = self.open_shard(&name)?;
            if !sub.has_data() {
                continue;
            }
            any = true;
            rows.extend(sub.impacted(&group, functions, &BTreeSet::new()));
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

    pub(super) fn aggregate_diff_impact(&mut self) -> Result<Value, String> {
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
            groups.entry(shard_owner(file, &self.shards).to_string()).or_default().push(file.clone());
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
        let all_functions = git.changed_functions_all(&diff_ref).unwrap_or_default();
        let mut rows = Vec::new();
        let mut any = false;
        for (name, group) in &groups {
            let mut sub = self.open_shard(name)?;
            if !sub.has_data() {
                continue;
            }
            any = true;
            let functions: BTreeMap<String, Vec<String>> = group
                .iter()
                .filter(|f| !untracked_code.contains(*f))
                .filter_map(|f| all_functions.get(f).filter(|names| !names.is_empty()).map(|names| (f.clone(), names.clone())))
                .collect();
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

    pub(super) fn changed_file_count(&self) -> usize {
        if !self.shards.is_empty() {
            let mut count = 0usize;
            for name in self.snapshot_names() {
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

    pub(super) fn auto_plan(&self, name: &str, args: &Value) -> AutoPlan {
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

    pub(super) fn finish_auto(&self, plan: AutoPlan, result: Value) -> Value {
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

    pub(super) fn apply_limit(&self, name: &str, args: &Value, result: Value) -> Value {
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

    pub(super) fn count_code_files(&self) -> usize {
        self.scan_code_files(&self.git.fork()).len()
    }

    pub(super) fn unknown_shard(&self, args: &Value) -> Option<Value> {
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

    pub fn dispatch(&mut self, name: &str, args: &Value) -> Result<Value, String> {
        for attempt in 0..3 {
            if attempt > 0 {
                self.store.reload().map_err(|e| e.to_string())?;
            }
            match self.dispatch_inner(name, args) {
                Err(err) if err == "stale generation" && attempt < 2 => continue,
                Err(err) if err == "stale generation" => return Ok(stale_status()),
                other => return other,
            }
        }
        Ok(stale_status())
    }

    pub(super) fn dispatch_inner(&mut self, name: &str, args: &Value) -> Result<Value, String> {
        if name == "suggest_tests" {
            let has_file = !argument_missing(args, "file_path", "string");
            let has_dir = args.get("directory").and_then(|v| v.as_str()).is_some_and(|s| !s.is_empty());
            if has_file && has_dir {
                return Ok(Value::object(vec![
                    ("status".into(), Value::String("error".into())),
                    ("message".into(), Value::String("Provide only one of file_path or directory, not both.".into())),
                ]));
            }
            if !has_file && !has_dir {
                return Ok(missing_arg("suggest_tests", "file_path"));
            }
            if has_file {
                if let Some(field) = first_missing_required(name, args) {
                    return Ok(missing_arg(name, &field));
                }
            }
        } else if let Some(field) = first_missing_required(name, args) {
            return Ok(missing_arg(name, &field));
        }
        if let Some(err) = self.unknown_shard(args) {
            if matches!(name, "analyze" | "update" | "start_job") {
                return Ok(err);
            }
        }
        self.working_tree = args.get("working_tree").and_then(|v| v.as_bool()).unwrap_or(false);
        if let Some(r) = args.get("ref").and_then(|v| v.as_str()) {
            if r.starts_with('-') {
                return Ok(Value::object(vec![
                    ("status".into(), Value::String("error".into())),
                    ("message".into(), Value::String("ref must not start with '-'".into())),
                ]));
            }
        }
        if let Some(mode) = args.get("coverage_mode").and_then(|v| v.as_str()) {
            if !mode.is_empty() {
                self.coverage_mode = mode.to_string();
            }
        }
        if let Some(proximity) = args.get("proximity_adjustment").and_then(|v| v.as_bool()) {
            self.proximity_adjustment = proximity;
        }
        if let Some(exclude) = args.get("exclude_new_file_boost").and_then(|v| v.as_bool()) {
            self.exclude_new_file_boost = exclude;
        }
        self.diff_ref_override = args.get("ref").and_then(|v| v.as_str()).map(|s| s.to_string());
        self.directory = args.get("directory").and_then(|v| v.as_str()).filter(|s| !s.is_empty() && *s != ".").map(|s| s.trim_end_matches('/').to_string());
        self.exclude_tests = args.get("exclude_tests").and_then(|v| v.as_bool()).unwrap_or(true);
        self.fallback_to_all = args.get("fallback_to_all").and_then(|v| v.as_bool()).unwrap_or(false);
        self.scope_file = if name == "test_gaps" {
            args.get("file_path").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(|s| s.to_string())
        } else {
            None
        };
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
                if file.is_empty() {
                    let dir = self.directory.clone().unwrap_or_default();
                    self.suggest_directory(&dir)
                } else {
                    self.suggest_tests(file)
                }
            }
            "stale_tests" => self.stale_tests(),
            "stats" => self.stats_with(args.get("include_results").and_then(|v| v.as_bool()).unwrap_or(false)),
            "record_result" => {
                let id = args.get("test_id").and_then(|v| v.as_str()).unwrap_or("");
                let passed = args.get("passed").and_then(|v| v.as_bool()).unwrap_or(false);
                let duration = args.get("duration_ms").and_then(|v| v.as_f64());
                self.record_result_with(id, passed, duration)
            }
            "impact" => {
                let files: Vec<String> = args.get("files").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
                let names: Vec<String> = args.get("functions").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).filter(|s| !s.is_empty()).collect()).unwrap_or_default();
                let mut functions: BTreeMap<String, Vec<String>> = BTreeMap::new();
                if !names.is_empty() {
                    for file in &files {
                        functions.insert(file.clone(), names.clone());
                    }
                }
                if !self.shards.is_empty() {
                    self.aggregate_impact(&files, &functions)
                } else {
                    self.store.reload()?;
                    if !self.has_data() { return Ok(Self::no_data()); }
                    Ok(Value::Array(self.impacted(&files, &functions, &BTreeSet::new())))
                }
            }
            "test_gaps" => self.test_gaps(),
            "triage" => {
                let cap = args.get("top_n").and_then(|v| v.as_i64()).or_else(|| args.get("limit").and_then(|v| v.as_i64())).unwrap_or(10);
                self.triage(cap)
            },
            "churn" | "ownership" | "coupling" | "history" | "who_reviews" => self.simple_file_tool(name, args),
            "optimize_storage" => {
                self.drop_expired_locks();
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
        Ok(self.finish_auto(auto, compact_risk_rows(name, limited)))
    }

    pub(super) fn test_gaps(&mut self) -> Result<Value, String> {
        if !self.shards.is_empty() {
            let mut gaps = Vec::new();
            let mut any = false;
            for name in self.snapshot_names() {
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

    pub(super) fn gap_rows(&self) -> Vec<Value> {
        let edges = self.store.rows("test_edges");
        let mut covered: BTreeSet<&str> = BTreeSet::new();
        for edge in edges {
            if let Some(id) = edge.get("code_id").and_then(|v| v.as_str()) {
                covered.insert(id);
            }
        }
        let mut churn_by: BTreeMap<(&str, &str), (f64, i64)> = BTreeMap::new();
        for row in self.store.rows("churn") {
            let path = row.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
            let unit = row.get("unit_name").and_then(|v| v.as_str()).unwrap_or("");
            let score = row.get("churn_score").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let commits = row.get("commit_count").and_then(|v| v.as_i64()).unwrap_or(0);
            churn_by.insert((path, unit), (score, commits));
        }
        let gtest_files: BTreeSet<&str> = self.store.rows("test_units").iter().filter_map(|u| {
            if u.get("framework").and_then(|v| v.as_str()) == Some("gtest") {
                u.get("file_path").and_then(|v| v.as_str())
            } else {
                None
            }
        }).collect();
        let mut gaps = Vec::new();
        for u in self.store.rows("code_units") {
            let path = u.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
            if let Some(file) = &self.scope_file {
                if path != file.as_str() { continue; }
            } else if let Some(dir) = &self.directory {
                if !path_in_directory(path, dir) { continue; }
            }
            if is_test_unit_row(u) && self.exclude_tests { continue; }
            if self.exclude_tests && (looks_like_test(path) || gtest_files.contains(path) || is_test_support_path(path)) { continue; }
            let id = u.get("id").and_then(|v| v.as_str()).unwrap_or("");
            if covered.contains(id) {
                continue;
            }
            let name = u.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let stat = churn_by.get(&(path, name));
            let churn_score = stat.map(|(score, _)| *score).unwrap_or(0.0);
            let commit_count = stat.map(|(_, commits)| *commits).unwrap_or(0);
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

    pub(super) fn stale_rows(&mut self) -> Vec<Value> {
        self.absorb_parent_overlay();
        let edges = self.store.array("test_edges");
        let ids: BTreeSet<String> = self.store.array("code_units").into_iter().filter_map(|u| u.get("id").and_then(|v| v.as_str()).map(|s| s.to_string())).collect();
        let tests = self.store.array("test_units");
        let shard_of = self.shard_view.clone();
        let mut stale = Vec::new();
        for e in &edges {
            let code_id = e.get("code_id").and_then(|v| v.as_str()).unwrap_or("");
            if ids.contains(code_id) { continue; }
            if let Some((shard, shards)) = &shard_of {
                if let Some(code_file) = e.get("code_file").and_then(|v| v.as_str()) {
                    if shard_owner(code_file, shards) != shard.as_str() {
                        continue;
                    }
                }
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
        self.append_working_tree_stale(&mut stale);
        stale
    }

    pub(super) fn triage(&mut self, cap: i64) -> Result<Value, String> {
        let cap = cap.clamp(1, 1000) as usize;
        if !self.shards.is_empty() {
            let risk = self.aggregate_risk_map()?;
            if risk.get("status").and_then(|v| v.as_str()) == Some("no_data") {
                return Ok(risk);
            }
            let mut top = risk.get("files").and_then(|v| v.as_array()).unwrap_or(&[]).to_vec();
            let mut truncated = false;
            if top.len() > cap {
                top.truncate(cap);
                truncated = true;
            }
            let top_paths: BTreeSet<String> = top.iter().filter_map(|f| f.get("file_path").and_then(|v| v.as_str()).map(|s| s.to_string())).collect();
            let mut gaps = Vec::new();
            let mut stale = Vec::new();
            let mut edges = 0i64;
            let mut results = 0i64;
            for name in self.snapshot_names() {
                let mut sub = self.open_shard(&name)?;
                gaps.extend(sub.gap_rows().into_iter().filter(|g| {
                    g.get("file_path").and_then(|v| v.as_str()).is_some_and(|p| top_paths.contains(p))
                }));
                stale.extend(sub.stale_rows());
                edges += sub.store.array_len("test_edges") as i64;
                results += sub.store.array_len("test_results") as i64;
            }
            let mut body = vec![
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
                    ("cross_shard_test_edges".into(), Value::Int(self.store.array_len("test_edges") as i64)),
                    ("active_locks".into(), Value::Int(0)),
                ])),
            ];
            if truncated {
                body.push(("truncated".into(), Value::Bool(true)));
            }
            return Ok(Value::object(body));
        }
        self.store.reload()?;
        if !self.has_data() { return Ok(Self::no_data()); }
        let risk = self.risk_map_with(false)?;
        let mut top = risk.get("files").and_then(|v| v.as_array()).unwrap_or(&[]).to_vec();
        let mut truncated = top.len() > cap;
        if top.len() > cap {
            top.truncate(cap);
        }
        let top_paths: BTreeSet<String> = top.iter().filter_map(|f| f.get("file_path").and_then(|v| v.as_str()).map(|s| s.to_string())).collect();
        let mut gaps: Vec<Value> = self.gap_rows().into_iter().filter(|g| {
            g.get("file_path").and_then(|v| v.as_str()).is_some_and(|p| top_paths.contains(p))
        }).collect();
        if gaps.len() > cap {
            gaps.truncate(cap);
            truncated = true;
        }
        let mut stale = self.stale_rows();
        if stale.len() > cap {
            stale.truncate(cap);
            truncated = true;
        }
        let commits_n = self.store.array_len("commits") as i64;
        let mut body = vec![
            ("top_risk_files".into(), Value::Array(top.clone())),
            ("test_gaps".into(), Value::Array(gaps.clone())),
            ("stale_tests".into(), Value::Array(stale.clone())),
            ("summary".into(), Value::object(vec![
                ("files_triaged".into(), Value::Int(top.len() as i64)),
                ("total_test_gaps".into(), Value::Int(gaps.len() as i64)),
                ("total_stale_tests".into(), Value::Int(stale.len() as i64)),
                ("test_edge_count".into(), Value::Int(self.store.array_len("test_edges") as i64)),
                ("test_result_count".into(), Value::Int(self.store.array_len("test_results") as i64)),
                ("coupling_threshold".into(), Value::Int(impact::coupling_threshold(commits_n))),
                ("active_locks".into(), Value::Int(self.active_locks().len() as i64)),
            ])),
        ];
        if truncated {
            body.push(("truncated".into(), Value::Bool(true)));
        }
        Ok(Value::object(body))
    }

    pub(super) fn simple_file_tool(&mut self, name: &str, args: &Value) -> Result<Value, String> {
        let file = args.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
        if !self.shards.is_empty() {
            let mut sub = self.open_shard(shard_owner(file, &self.shards))?;
            return sub.simple_file_tool(name, args);
        }
        self.store.reload()?;
        if !self.has_data() { return Ok(Self::no_data()); }
        match name {
            "churn" => {
                let unit = args.get("unit_name").and_then(|v| v.as_str()).filter(|s| !s.is_empty());
                let mut rows: Vec<Value> = self.store.churn_for_file(file).into_iter().cloned().collect();
                if let Some(unit) = unit {
                    rows.retain(|c| c.get("unit_name").and_then(|v| v.as_str()).unwrap_or("") == unit);
                } else {
                    let file_rows: Vec<Value> = rows.iter().filter(|c| c.get("unit_name").and_then(|v| v.as_str()).unwrap_or("").is_empty()).cloned().collect();
                    if !file_rows.is_empty() {
                        rows = file_rows;
                    }
                }
                Ok(Value::Array(rows))
            }
            "history" => Ok(Value::Array(self.history_rows(file))),
            "ownership" => Ok(Value::Array(self.ownership_rows(file))),
            "who_reviews" => Ok(Value::Array(self.reviewer_rows(file))),
            "coupling" => Ok(self.coupling_row(file, args.get("min_count").and_then(|v| v.as_i64()).unwrap_or(3))),
            _ => Err("unreachable".into()),
        }
    }

    pub(super) fn history_rows(&self, file: &str) -> Vec<Value> {
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

    pub(super) fn ownership_rows(&self, file: &str) -> Vec<Value> {
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

    pub(super) fn reviewer_rows(&self, file: &str) -> Vec<Value> {
        let history = self.history_rows(file);
        if history.is_empty() { return Vec::new(); }
        let now = impact::now_unix();
        struct ReviewerTotals {
            commits: i64,
            email: String,
            insertions: i64,
            deletions: i64,
            last_date: String,
            score: f64,
            last_ts: f64,
        }
        let mut stats: BTreeMap<String, ReviewerTotals> = BTreeMap::new();
        for c in &history {
            let author = c.get("author").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let email = c.get("author_email").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let date = c.get("date").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let ins = c.get("insertions").and_then(|v| v.as_i64()).unwrap_or(0);
            let del = c.get("deletions").and_then(|v| v.as_i64()).unwrap_or(0);
            let slot = stats.entry(author).or_insert(ReviewerTotals {
                commits: 0,
                email: String::new(),
                insertions: 0,
                deletions: 0,
                last_date: String::new(),
                score: 0.0,
                last_ts: 0.0,
            });
            slot.commits += 1;
            if !email.is_empty() { slot.email = email; }
            slot.insertions += ins;
            slot.deletions += del;
            if let Some(ts) = impact::parse_iso_unix(&date) {
                if date > slot.last_date {
                    slot.last_date = date.clone();
                    slot.last_ts = ts;
                }
                let days = ((now - ts) / 86400.0).max(0.0);
                slot.score += 1.0 / (1.0 + days);
            } else {
                slot.score += 0.01;
            }
        }
        let total: f64 = stats.values().map(|s| s.score).sum();
        let mut rows: Vec<Value> = stats.into_iter().map(|(author, totals)| {
            let days = if totals.last_ts > 0.0 { Some(crate::json::py_round((now - totals.last_ts) / 86400.0, 0) as i64) } else { None };
            let pct = if total > 0.0 { totals.score / total * 100.0 } else { 0.0 };
            Value::object(vec![
                ("author".into(), Value::String(author)),
                ("author_email".into(), Value::String(totals.email)),
                ("recent_commits".into(), Value::Int(totals.commits)),
                ("last_commit_date".into(), Value::String(totals.last_date)),
                ("days_since_last_commit".into(), days.map(Value::Int).unwrap_or(Value::Null)),
                ("insertions".into(), Value::Int(totals.insertions)),
                ("deletions".into(), Value::Int(totals.deletions)),
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

    pub(super) fn coupling_row(&self, file: &str, min_count: i64) -> Value {
        let query_min: i64 = store::meta_get(&self.store.doc, "co_change_query_min").and_then(|s| s.parse().ok()).unwrap_or(3);
        let effective = min_count.max(query_min);
        let mut partners = Vec::new();
        let mut exists_cache: HashMap<String, bool> = HashMap::new();
        for c in self.store.co_for_file(file) {
            let n = c.get("co_commit_count").and_then(|v| v.as_i64()).unwrap_or(0);
            if n < effective { continue; }
            let a = c.get("file_a").and_then(|v| v.as_str()).unwrap_or("");
            let b = c.get("file_b").and_then(|v| v.as_str()).unwrap_or("");
            let other = if a == file { b } else if b == file { a } else { continue };
            if !cached_exists(&mut exists_cache, &self.project_dir, other) {
                continue;
            }
            partners.push(c.clone());
        }
        partners.sort_by(|a, b| {
            let na = a.get("co_commit_count").and_then(|v| v.as_i64()).unwrap_or(0);
            let nb = b.get("co_commit_count").and_then(|v| v.as_i64()).unwrap_or(0);
            nb.cmp(&na)
        });
        let mut neighbors = BTreeSet::new();
        for e in self.store.rows("import_edges") {
            let a = e.get("importer_file").and_then(|v| v.as_str()).unwrap_or("");
            let b = e.get("imported_file").and_then(|v| v.as_str()).unwrap_or("");
            if a == file && b != file && cached_exists(&mut exists_cache, &self.project_dir, b) { neighbors.insert(b.to_string()); }
            if b == file && a != file && cached_exists(&mut exists_cache, &self.project_dir, a) { neighbors.insert(a.to_string()); }
        }
        let untracked = self.working_tree && self.store.hash_for_file(file).is_empty();
        if untracked && neighbors.is_empty() {
            if let Ok(text) = fs::read_to_string(self.project_dir.join(file)) {
                for dep in extract::extract_deps_in(file, &text, Some(&self.project_dir), self.allow_repo_extractor) {
                    if dep.dep_type == "import" {
                        if let Some(module) = dep.module_path {
                            if neighbors.len() < 20 {
                                neighbors.insert(module);
                            }
                        }
                    }
                }
            }
        }
        let co_len = partners.len() as f64;
        let imp_len = neighbors.len() as f64;
        let co_norm = (co_len / impact::COCHANGE_CAP).min(1.0);
        let import_norm = (imp_len / impact::IMPORT_CAP).min(1.0);
        let effective_coupling = co_norm.max(import_norm).max(0.5 * co_norm + 0.5 * import_norm);
        let mut body = vec![
            ("co_change_partners".into(), Value::Array(partners)),
            ("import_partners".into(), Value::Array(neighbors.into_iter().map(|f| Value::object(vec![("file".into(), Value::String(f))])).collect())),
            ("co_change_breadth".into(), Value::Int(co_len as i64)),
            ("import_breadth".into(), Value::Int(imp_len as i64)),
            ("cochange_coupling".into(), Value::Float(crate::json::py_round(co_norm, 4))),
            ("import_coupling".into(), Value::Float(crate::json::py_round(import_norm, 4))),
            ("effective_coupling".into(), Value::Float(crate::json::py_round(effective_coupling, 4))),
        ];
        if untracked {
            body.push(("note".into(), Value::String("working_tree: import_partners from on-disk scan (not yet in DB import graph)".into())));
        }
        Value::object(body)
    }

    pub(super) fn suggest_directory(&mut self, dir: &str) -> Result<Value, String> {
        if !self.shards.is_empty() {
            let mut merged: BTreeMap<String, Value> = BTreeMap::new();
            for name in self.snapshot_names() {
                let mut sub = self.open_shard(&name)?;
                let part = sub.suggest_directory(dir)?;
                if let Some(pairs) = part.as_object() {
                    for (k, v) in pairs {
                        merged.insert(k.to_string(), v.clone());
                    }
                }
            }
            return Ok(Value::object(merged.into_iter().map(|(k, v)| (crate::json::SharedStr::from(k), v)).collect()));
        }
        self.store.reload()?;
        if !self.has_data() {
            return Ok(Self::no_data());
        }
        let mut files: Vec<String> = self.store.array("file_hashes").into_iter().filter_map(|h| h.get("file_path").and_then(|v| v.as_str()).map(|s| s.to_string())).filter(|p| path_in_directory(p, dir) && !looks_like_test(p)).collect();
        files.sort();
        files.dedup();
        let mut pairs = Vec::new();
        for file in files {
            let suggested = self.suggest_tests(&file)?;
            if let Some(rows) = suggested.as_array() {
                if !rows.is_empty() {
                    pairs.push((crate::json::SharedStr::from(file), Value::Array(rows.to_vec())));
                }
            }
        }
        Ok(Value::object(pairs))
    }

}

/// `risk_map` and `triage` rows with their no-signal fields dropped (see `impact::compact_risk_row`).
fn compact_risk_rows(name: &str, mut result: Value) -> Value {
    let key = match name {
        "risk_map" => "files",
        "triage" => "top_risk_files",
        _ => return result,
    };
    if let Some(Value::Array(rows)) = result.get_mut(key) {
        for row in rows.iter_mut() {
            *row = impact::compact_risk_row(row);
        }
    }
    result
}
