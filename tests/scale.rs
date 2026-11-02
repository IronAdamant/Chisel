//! Scale, job, shard, and contract checks against the shipped engine and binary.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn scratch(name: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let p = std::env::temp_dir().join(format!("chisel-scale-{name}-{t}-{n}"));
    fs::create_dir_all(&p).unwrap();
    p
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git").args(args).current_dir(dir).status().unwrap();
    assert!(status.success(), "git {args:?}");
}

fn init_repo(dir: &Path) {
    git(dir, &["init", "-b", "main"]);
    git(dir, &["config", "user.email", "a@example.com"]);
    git(dir, &["config", "user.name", "Solo"]);
}

fn commit_all(dir: &Path, msg: &str) {
    git(dir, &["add", "-A"]);
    let status = Command::new("git")
        .args(["commit", "-m", msg])
        .env("GIT_AUTHOR_DATE", "2024-03-01T00:00:00+00:00")
        .env("GIT_COMMITTER_DATE", "2024-03-01T00:00:00+00:00")
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(status.success(), "{msg}");
}

fn overlap_runner(log: Arc<Mutex<Vec<String>>>, inflight: Arc<AtomicUsize>, max_seen: Arc<AtomicUsize>) -> chisel::git::GitRunner {
    Arc::new(move |repo, args| {
        log.lock().unwrap().push(args.join(" "));
        let tracked = args.iter().any(|arg| *arg == "blame" || arg.starts_with("-L:"));
        if tracked {
            let now = inflight.fetch_add(1, Ordering::SeqCst) + 1;
            max_seen.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(40));
        }
        let mut cmd = Command::new("git");
        cmd.args(args).current_dir(repo);
        let out = cmd.output();
        if tracked {
            inflight.fetch_sub(1, Ordering::SeqCst);
        }
        out
    })
}

fn churn_signature(snap: &str) -> Vec<String> {
    let doc = chisel::json::parse(snap).unwrap();
    doc.get("churn")
        .and_then(|v| v.as_array())
        .unwrap()
        .iter()
        .map(|row| {
            format!(
                "{}|{}|{}",
                row.get("file_path").and_then(|v| v.as_str()).unwrap_or(""),
                row.get("unit_name").and_then(|v| v.as_str()).unwrap_or(""),
                row.get("churn_score").and_then(|v| v.as_f64()).unwrap_or(0.0)
            )
        })
        .collect()
}

fn recording_runner(log: Arc<Mutex<Vec<String>>>) -> chisel::git::GitRunner {
    Arc::new(move |repo, args| {
        log.lock().unwrap().push(args.join(" "));
        let mut cmd = std::process::Command::new("git");
        cmd.args(args).current_dir(repo);
        cmd.output()
    })
}

#[test]
fn noop_update_skips_blame_and_function_log() {
    let dir = scratch("noop");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/a.py"), "def a():\n    return 1\n").unwrap();
    init_repo(&dir);
    commit_all(&dir, "init");
    let store = dir.join("store");
    let mut engine = chisel::Engine::open(&dir, Some(&store)).unwrap();
    let log = Arc::new(Mutex::new(Vec::new()));
    engine.set_git_runner(recording_runner(Arc::clone(&log)));
    engine.analyze().unwrap();
    log.lock().unwrap().clear();
    let update = engine.update().unwrap();
    let text = chisel::json::to_string(&update).unwrap();
    assert!(text.contains("\"edge_rebuild_skipped\":true") || text.contains("\"edge_rebuild_skipped\": true"), "{text}");
    let spawns = log.lock().unwrap().clone();
    assert!(spawns.iter().all(|s| !s.contains("-L:") && !s.contains("blame")), "{spawns:?}");
}

#[test]
fn one_changed_file_blames_only_that_file() {
    let dir = scratch("one");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/a.py"), "def a():\n    return 1\n").unwrap();
    fs::write(dir.join("src/b.py"), "def b():\n    return 2\n").unwrap();
    init_repo(&dir);
    commit_all(&dir, "init");
    let store = dir.join("store");
    let mut engine = chisel::Engine::open(&dir, Some(&store)).unwrap();
    engine.analyze().unwrap();
    fs::write(dir.join("src/a.py"), "def a():\n    return 3\n").unwrap();
    let log = Arc::new(Mutex::new(Vec::new()));
    engine.set_git_runner(recording_runner(Arc::clone(&log)));
    engine.source_reads = 0;
    let update = engine.update().unwrap();
    let text = chisel::json::to_string(&update).unwrap();
    assert!(text.contains("\"edge_rebuild_skipped\":true") || text.contains("\"edge_rebuild_skipped\": true"), "{text}");
    assert_eq!(engine.source_reads, 0, "source-only update reread tests");
    let spawns = log.lock().unwrap().clone();
    let blames: Vec<_> = spawns.iter().filter(|s| s.contains("blame")).cloned().collect();
    assert!(blames.iter().any(|s| s.contains("src/a.py")), "{blames:?}");
    assert!(blames.iter().all(|s| !s.contains("src/b.py")), "{blames:?}");
}

#[test]
fn unit_churn_skipped_above_2000_files() {
    let dir = scratch("cap");
    fs::create_dir_all(dir.join("src")).unwrap();
    for i in 0..2001 {
        fs::write(dir.join(format!("src/f{i}.py")), "def f():\n    return 1\n").unwrap();
    }
    init_repo(&dir);
    commit_all(&dir, "init");
    let store = dir.join("store");
    let mut engine = chisel::Engine::open(&dir, Some(&store)).unwrap();
    let log = Arc::new(Mutex::new(Vec::new()));
    engine.set_git_runner(recording_runner(Arc::clone(&log)));
    let summary = engine.analyze().unwrap();
    let text = chisel::json::to_string(&summary).unwrap();
    assert!(text.contains("\"unit_churn_skipped\":true") || text.contains("\"unit_churn_skipped\": true"), "{text}");
    let spawns = log.lock().unwrap().clone();
    assert!(spawns.iter().all(|s| !s.contains("-L:")), "function log ran: {spawns:?}");
    let snap = fs::read_to_string(store.join("store")).unwrap();
    assert!(snap.contains("\"file_path\":\"src/f0.py\"") && snap.contains("\"unit_name\":\"\""), "file-level churn row missing: {snap}");
}

#[test]
fn typescript_test_file_produces_an_edge() {
    let dir = scratch("ts");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/calc.ts"), "export function add(a, b) {\n  return a + b;\n}\n").unwrap();
    fs::write(dir.join("src/calc.test.ts"), "import { add } from './calc';\ntest('add', () => add(1, 1));\n").unwrap();
    fs::write(dir.join("src/calc.spec.ts"), "import { add } from './calc';\nit('add', () => add(1, 2));\n").unwrap();
    init_repo(&dir);
    commit_all(&dir, "init");
    let store = dir.join("store");
    let mut engine = chisel::Engine::open(&dir, Some(&store)).unwrap();
    engine.analyze().unwrap();
    let snap = fs::read_to_string(store.join("store")).unwrap();
    assert!(snap.contains("calc.test.ts"), "{snap}");
    assert!(snap.contains("calc.spec.ts"), "{snap}");
    assert!(snap.contains("src/calc.ts:add:function"), "no edge target for add: {snap}");
}

#[test]
fn batch_record_flushes_once_and_jest_parses() {
    let jest = r#"{"testResults":[{"name":"src/calc.test.ts","assertionResults":[{"title":"add","status":"passed"},{"title":"sub","status":"failed"}]}]}"#;
    let rows = chisel::cli::parse_jest_json(jest);
    assert_eq!(rows.len(), 2);
    assert!(rows[0].1);
    assert!(!rows[1].1);
    let dir = scratch("batch");
    fs::write(dir.join("a.py"), "def a():\n    return 1\n").unwrap();
    init_repo(&dir);
    commit_all(&dir, "init");
    let store = dir.join("store");
    let mut engine = chisel::Engine::open(&dir, Some(&store)).unwrap();
    engine.analyze().unwrap();
    let before = engine.store.doc.get("generation").and_then(|v| v.as_i64()).unwrap();
    let summary = engine.record_results_batch(&rows).unwrap();
    let text = chisel::json::to_string(&summary).unwrap();
    assert!(text.contains("\"generation_delta\":1") || text.contains("\"generation_delta\": 1"), "{text}");
    let after = engine.store.doc.get("generation").and_then(|v| v.as_i64()).unwrap();
    assert_eq!(after, before + 1);
}

#[test]
fn cli_run_records_jest_stdout() {
    let dir = scratch("jestcli");
    fs::write(dir.join("a.py"), "def a():\n    return 1\n").unwrap();
    init_repo(&dir);
    commit_all(&dir, "init");
    let store = dir.join("store");
    let jest = dir.join("jest");
    fs::write(&jest, "#!/bin/sh\nprintf '%s' '{\"testResults\":[{\"name\":\"a.test.ts\",\"assertionResults\":[{\"title\":\"ok\",\"status\":\"passed\"}]}]}'\n").unwrap();
    let _ = Command::new("chmod").arg("+x").arg(&jest).status();
    let bin = env!("CARGO_BIN_EXE_chisel");
    let _ = Command::new(bin).args(["--json", "analyze", "--project-dir"]).arg(&dir).arg("--storage-dir").arg(&store).status();
    let out = Command::new(bin)
        .args(["run", "--project-dir"]).arg(&dir).arg("--storage-dir").arg(&store).arg("--").arg(&jest)
        .output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let snap = fs::read_to_string(store.join("store")).unwrap();
    assert!(snap.contains("a.test.ts:ok"), "{snap}");
}

#[test]
fn missing_required_arg_is_an_error() {
    let dir = scratch("args");
    fs::write(dir.join("a.py"), "def a():\n    return 1\n").unwrap();
    let store = dir.join("store");
    let mut engine = chisel::Engine::open(&dir, Some(&store)).unwrap();
    let empty = chisel::json::Value::object(vec![]);
    let suggest = engine.dispatch("suggest_tests", &empty).unwrap();
    let text = chisel::json::to_string(&suggest).unwrap();
    assert!(text.contains("\"status\":\"error\"") || text.contains("\"status\": \"error\""), "{text}");
    assert!(!text.starts_with('['), "{text}");
    let impact = engine.dispatch("impact", &empty).unwrap();
    let text = chisel::json::to_string(&impact).unwrap();
    assert!(text.contains("impact requires files"), "{text}");
}

#[test]
fn cli_record_result_without_passed_exits_nonzero() {
    let dir = scratch("record-cli");
    fs::write(dir.join("a.py"), "def a():\n    return 1\n").unwrap();
    init_repo(&dir);
    commit_all(&dir, "init");
    let store = dir.join("store");
    let bin = env!("CARGO_BIN_EXE_chisel");
    let analyzed = Command::new(bin).args(["--json", "analyze", "--project-dir"]).arg(&dir).arg("--storage-dir").arg(&store).output().unwrap();
    assert!(analyzed.status.success(), "{}", String::from_utf8_lossy(&analyzed.stderr));
    let before = fs::read(store.join("store")).unwrap();
    let missing = Command::new(bin).args(["--json", "record-result", "t", "--project-dir"]).arg(&dir).arg("--storage-dir").arg(&store).output().unwrap();
    assert_ne!(missing.status.code(), Some(0), "{}", String::from_utf8_lossy(&missing.stderr));
    let text = String::from_utf8_lossy(&missing.stdout);
    assert!(text.contains("record_result requires passed"), "{text}");
    assert!(!text.contains("\"passed\":true") && !text.contains("\"passed\": true"), "{text}");
    let after = fs::read(store.join("store")).unwrap();
    assert_eq!(before, after, "record-result without --passed or --failed wrote the store");
    let plain = Command::new(bin).args(["record-result", "plain", "--project-dir"]).arg(&dir).arg("--storage-dir").arg(&store).output().unwrap();
    assert_ne!(plain.status.code(), Some(0), "{}", String::from_utf8_lossy(&plain.stderr));
    let plain_text = format!("{}{}", String::from_utf8_lossy(&plain.stdout), String::from_utf8_lossy(&plain.stderr));
    assert!(plain_text.contains("record_result requires passed"), "{plain_text}");
    assert_eq!(fs::read(store.join("store")).unwrap(), before, "plain record-result wrote the store");
    let failed = Command::new(bin).args(["--json", "record-result", "t", "--failed", "--project-dir"]).arg(&dir).arg("--storage-dir").arg(&store).output().unwrap();
    assert_eq!(failed.status.code(), Some(0), "{} {}", String::from_utf8_lossy(&failed.stderr), String::from_utf8_lossy(&failed.stdout));
    let snap = fs::read_to_string(store.join("store")).unwrap();
    assert!(snap.contains("\"passed\":false") || snap.contains("\"passed\": false"), "{snap}");
    let passed = Command::new(bin).args(["--json", "record-result", "ok", "--passed", "--project-dir"]).arg(&dir).arg("--storage-dir").arg(&store).output().unwrap();
    assert_eq!(passed.status.code(), Some(0), "{}", String::from_utf8_lossy(&passed.stderr));
    let snap = fs::read_to_string(store.join("store")).unwrap();
    assert!(snap.contains("\"test_id\":\"ok\"") || snap.contains("\"test_id\": \"ok\""), "{snap}");
    assert!(snap.contains("\"passed\":true") || snap.contains("\"passed\": true"), "{snap}");
}

#[test]
fn auto_update_skips_above_50_files() {
    let dir = scratch("auto");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/a.py"), "def a():\n    return 1\n").unwrap();
    init_repo(&dir);
    commit_all(&dir, "init");
    let store = dir.join("store");
    let mut engine = chisel::Engine::open(&dir, Some(&store)).unwrap();
    engine.analyze().unwrap();
    for i in 0..51 {
        fs::write(dir.join(format!("src/n{i}.py")), "def n():\n    return 1\n").unwrap();
    }
    let args = chisel::json::Value::object(vec![
        ("auto_update".into(), chisel::json::Value::Bool(true)),
        ("file_path".into(), chisel::json::Value::String("src/a.py".into())),
    ]);
    let result = engine.dispatch("suggest_tests", &args).unwrap();
    let text = chisel::json::to_string(&result).unwrap();
    assert!(text.contains("too_many_changed_files"), "{text}");
    assert!(text.contains("Auto-update skipped: more than 50 files changed"), "{text}");
}

#[test]
fn force_analyze_above_300_is_auto_queued_and_cancel_stops_it() {
    let dir = scratch("jobs");
    fs::create_dir_all(dir.join("src")).unwrap();
    for i in 0..301 {
        fs::write(dir.join(format!("src/f{i}.py")), "def f():\n    return 1\n").unwrap();
    }
    init_repo(&dir);
    commit_all(&dir, "init");
    let store = dir.join("store");
    std::env::set_var("CHISEL_JOB_PACE_MS", "400");
    let mut engine = chisel::Engine::open(&dir, Some(&store)).unwrap();
    let args = chisel::json::Value::object(vec![("force".into(), chisel::json::Value::Bool(true))]);
    let started = std::time::Instant::now();
    let result = engine.dispatch("analyze", &args).unwrap();
    assert!(started.elapsed().as_secs() < 15, "auto_queued waited on the scan");
    let text = chisel::json::to_string(&result).unwrap();
    assert!(text.contains("auto_queued"), "{text}");
    let id = result.get("job_id").and_then(|v| v.as_str()).unwrap().to_string();
    assert!(id.starts_with("job-") && id != "job-1", "{id}");
    let cancel_args = chisel::json::Value::object(vec![("job_id".into(), chisel::json::Value::String(id.clone()))]);
    engine.dispatch("cancel_job", &cancel_args).unwrap();
    let mut saw_cancel = false;
    for _ in 0..30 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        let status = engine.job_status_value(&id);
        let text = chisel::json::to_string(&status).unwrap();
        if text.contains("cancelled") {
            saw_cancel = true;
            break;
        }
    }
    std::env::remove_var("CHISEL_JOB_PACE_MS");
    assert!(saw_cancel, "cancel was not observed");
}

#[test]
fn two_shards_aggregate_and_unknown_shard_errors() {
    let dir = scratch("shards");
    fs::create_dir_all(dir.join("frontend")).unwrap();
    fs::create_dir_all(dir.join("backend")).unwrap();
    fs::write(dir.join("frontend/a.py"), "def a():\n    return 1\n").unwrap();
    fs::write(dir.join("backend/b.py"), "def b():\n    return 2\n").unwrap();
    init_repo(&dir);
    commit_all(&dir, "init");
    let store = dir.join("store");
    let mut engine = chisel::Engine::open(&dir, Some(&store)).unwrap();
    engine.set_shards(vec![("frontend".into(), "frontend".into()), ("backend".into(), "backend".into())]);
    engine.analyze().unwrap();
    assert!(store.join("shards/frontend/store").exists());
    assert!(store.join("shards/backend/store").exists());
    let stats = engine.stats().unwrap();
    let text = chisel::json::to_string(&stats).unwrap();
    assert!(text.contains("unsupported"), "{text}");
    let units = stats.get("code_units").and_then(|v| v.as_i64()).unwrap_or(0);
    assert!(units >= 2, "{text}");
    let bad = chisel::json::Value::object(vec![("shard".into(), chisel::json::Value::String("missing".into()))]);
    let err = engine.dispatch("analyze", &bad).unwrap();
    let text = chisel::json::to_string(&err).unwrap();
    assert!(text.contains("\"status\":\"error\"") || text.contains("unknown shard"), "{text}");
}

#[test]
fn bulk_commit_is_skipped_for_co_change_and_solo_author_halves_threshold() {
    let dir = scratch("bulk");
    fs::create_dir_all(dir.join("src")).unwrap();
    for i in 0..51 {
        fs::write(dir.join(format!("src/b{i}.py")), "def b():\n    return 1\n").unwrap();
    }
    init_repo(&dir);
    commit_all(&dir, "bulk");
    let store = dir.join("store");
    let mut engine = chisel::Engine::open(&dir, Some(&store)).unwrap();
    engine.analyze().unwrap();
    let snap = fs::read_to_string(store.join("store")).unwrap();
    assert!(snap.contains("\"co_changes\":[]") || snap.contains("\"co_changes\": []"), "bulk commit produced co-change pairs");
    let min = chisel::impact::coupling_threshold(1);
    let expected = (min / 2).max(1);
    assert!(snap.contains(&format!("\"co_change_query_min\":\"{expected}\"")), "{snap}");
}

#[test]
fn gitignored_tree_is_skipped_unless_env_set() {
    let dir = scratch("ignore");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join("vendor")).unwrap();
    fs::write(dir.join(".gitignore"), "vendor/\n").unwrap();
    fs::write(dir.join("src/a.py"), "def a():\n    return 1\n").unwrap();
    fs::write(dir.join("vendor/v.py"), "def v():\n    return 1\n").unwrap();
    init_repo(&dir);
    commit_all(&dir, "init");
    let store = dir.join("store");
    let mut engine = chisel::Engine::open(&dir, Some(&store)).unwrap();
    engine.analyze().unwrap();
    let snap = fs::read_to_string(store.join("store")).unwrap();
    assert!(snap.contains("src/a.py"), "{snap}");
    assert!(!snap.contains("vendor/v.py"), "ignored file was scanned");
    std::env::set_var("CHISEL_INCLUDE_IGNORED", "1");
    let store2 = dir.join("store2");
    let mut engine = chisel::Engine::open(&dir, Some(&store2)).unwrap();
    std::env::remove_var("CHISEL_INCLUDE_IGNORED");
    engine.analyze().unwrap();
    let snap2 = fs::read_to_string(store2.join("store")).unwrap();
    assert!(snap2.contains("vendor/v.py"), "include-ignored did not scan vendor");
}

#[test]
fn risk_map_sets_truncated_past_the_default_limit() {
    let dir = scratch("limit");
    fs::create_dir_all(dir.join("src")).unwrap();
    for i in 0..31 {
        fs::write(dir.join(format!("src/l{i}.py")), "def l():\n    return 1\n").unwrap();
    }
    init_repo(&dir);
    commit_all(&dir, "init");
    let store = dir.join("store");
    let mut engine = chisel::Engine::open(&dir, Some(&store)).unwrap();
    engine.analyze().unwrap();
    let result = engine.dispatch("risk_map", &chisel::json::Value::object(vec![])).unwrap();
    let text = chisel::json::to_string(&result).unwrap();
    assert!(text.contains("\"truncated\":true") || text.contains("\"truncated\": true"), "{text}");
}

#[test]
fn unimplemented_flag_exits_nonzero() {
    let bin = env!("CARGO_BIN_EXE_chisel");
    let dir = scratch("flag");
    fs::write(dir.join("a.py"), "def a():\n    return 1\n").unwrap();
    let out = Command::new(bin)
        .args(["--json", "analyze", "--not-a-chisel-flag", "--project-dir"]).arg(&dir)
        .output().unwrap();
    assert_ne!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stderr);
    assert!(text.contains("unimplemented") || text.contains("status=error"), "{text}");
}

#[test]
fn one_new_commit_limits_blame_and_function_log() {
    let dir = scratch("commit");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/a.py"), "def a():\n    return 1\n").unwrap();
    fs::write(dir.join("src/b.py"), "def b():\n    return 2\n").unwrap();
    init_repo(&dir);
    commit_all(&dir, "init");
    let store = dir.join("store");
    let mut engine = chisel::Engine::open(&dir, Some(&store)).unwrap();
    engine.analyze().unwrap();
    fs::write(dir.join("src/a.py"), "def a():\n    return 3\n").unwrap();
    git(&dir, &["add", "src/a.py"]);
    let status = Command::new("git").args(["commit", "-m", "touch-a"]).env("GIT_AUTHOR_DATE", "2024-04-01T00:00:00+00:00").env("GIT_COMMITTER_DATE", "2024-04-01T00:00:00+00:00").current_dir(&dir).status().unwrap();
    assert!(status.success());
    let log = Arc::new(Mutex::new(Vec::new()));
    engine.set_git_runner(recording_runner(Arc::clone(&log)));
    let update = engine.update().unwrap();
    let text = chisel::json::to_string(&update).unwrap();
    assert!(text.contains("\"new_commits\":1") || text.contains("\"new_commits\": 1"), "{text}");
    let spawns = log.lock().unwrap().clone();
    assert!(spawns.iter().any(|s| s.contains("--since=")), "{spawns:?}");
    assert!(spawns.iter().any(|s| s.contains("-L:") && s.contains("src/a.py")), "{spawns:?}");
    assert!(spawns.iter().filter(|s| s.contains("blame") || s.contains("-L:")).all(|s| !s.contains("src/b.py")), "{spawns:?}");
}

#[test]
fn statuses_and_small_auto_update_and_queued_job() {
    let dir = scratch("status");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/a.py"), "def a():\n    return 1\n").unwrap();
    init_repo(&dir);
    commit_all(&dir, "init");
    let store = dir.join("store");
    let mut engine = chisel::Engine::open(&dir, Some(&store)).unwrap();
    let empty = engine.dispatch("diff_impact", &chisel::json::Value::object(vec![])).unwrap();
    let text = chisel::json::to_string(&empty).unwrap();
    assert!(text.contains("no_data"), "{text}");
    engine.analyze().unwrap();
    let none = engine.dispatch("diff_impact", &chisel::json::Value::object(vec![])).unwrap();
    let text = chisel::json::to_string(&none).unwrap();
    assert!(text.contains("no_changes"), "{text}");
    let stale = engine.stale_tests().unwrap();
    let text = chisel::json::to_string(&stale).unwrap();
    assert!(text.contains("no_edges") || text.contains("stale_tests"), "{text}");
    fs::write(dir.join("src/new.py"), "def n():\n    return 1\n").unwrap();
    let stale_db = engine.dispatch("diff_impact", &chisel::json::Value::object(vec![])).unwrap();
    let text = chisel::json::to_string(&stale_db).unwrap();
    assert!(text.contains("stale_db"), "{text}");
    let suggested = engine.dispatch("suggest_tests", &chisel::json::Value::object(vec![("file_path".into(), chisel::json::Value::String("src/a.py".into()))])).unwrap();
    let text = chisel::json::to_string(&suggested).unwrap();
    assert!(text.contains("direct") || text.contains("static_require") || text.contains("no_data") || text.contains("[]"), "{text}");
    std::env::set_var("CHISEL_JOB_PACE_MS", "500");
    let job = engine.start_job("update").unwrap();
    let text = chisel::json::to_string(&job).unwrap();
    assert!(text.contains("queued"), "{text}");
    let blocked = engine.dispatch("suggest_tests", &chisel::json::Value::object(vec![
        ("file_path".into(), chisel::json::Value::String("src/a.py".into())),
        ("auto_update".into(), chisel::json::Value::Bool(true)),
    ])).unwrap();
    let text = chisel::json::to_string(&blocked).unwrap();
    assert!(text.contains("background_job_running"), "{text}");
    let id = job.get("job_id").and_then(|v| v.as_str()).unwrap();
    engine.cancel_job_value(id);
    std::env::remove_var("CHISEL_JOB_PACE_MS");
}

#[test]
fn project_dir_env_and_stdio_call_and_long_prefix() {
    let dir = scratch("envdir");
    fs::create_dir_all(dir.join("pkg/web")).unwrap();
    fs::create_dir_all(dir.join("pkg")).unwrap();
    fs::write(dir.join("pkg/a.py"), "def a():\n    return 1\n").unwrap();
    fs::write(dir.join("pkg/web/w.py"), "def w():\n    return 2\n").unwrap();
    init_repo(&dir);
    commit_all(&dir, "init");
    let store = dir.join("store");
    let bin = env!("CARGO_BIN_EXE_chisel");
    let out = Command::new(bin).args(["--json", "analyze"]).env("CHISEL_PROJECT_DIR", &dir).env("CHISEL_STORAGE_DIR", &store).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("code_files_scanned"), "{text}");
    let mut engine = chisel::Engine::open(&dir, Some(&store)).unwrap();
    engine.set_shards(vec![("pkg".into(), "pkg".into()), ("web".into(), "pkg/web".into())]);
    engine.analyze().unwrap();
    let web = fs::read_to_string(store.join("shards/web/store")).unwrap_or_default();
    let pkg = fs::read_to_string(store.join("shards/pkg/store")).unwrap_or_default();
    assert!(web.contains("\"file_path\":\"pkg/web/w.py\""), "{web}");
    assert!(!pkg.contains("\"file_path\":\"pkg/web/w.py\""), "longest prefix lost");
    let missing = Command::new(bin).args(["--json", "suggest-tests"]).env("CHISEL_PROJECT_DIR", &dir).env("CHISEL_STORAGE_DIR", &store).output().unwrap();
    assert_ne!(missing.status.code(), Some(0));
    let init = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}";
    let call = b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"stats\",\"arguments\":{}}}";
    fn frame(body: &[u8]) -> Vec<u8> {
        let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        out.extend_from_slice(body);
        out
    }
    let mut child = Command::new(bin).args(["serve-mcp"]).env("CHISEL_PROJECT_DIR", &dir).env("CHISEL_STORAGE_DIR", &store).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().unwrap();
    {
        use std::io::Write;
        let stdin = child.stdin.as_mut().unwrap();
        stdin.write_all(&frame(init)).unwrap();
        stdin.write_all(&frame(call)).unwrap();
    }
    let output = child.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("\"result\""), "{text}");
    assert!(text.contains("next_steps"), "{text}");
}

#[test]
fn chisel_run_pytest_flushes_once() {
    let dir = scratch("pyflush");
    fs::write(dir.join("a.py"), "def a():\n    return 1\n").unwrap();
    init_repo(&dir);
    commit_all(&dir, "init");
    let store = dir.join("store");
    let bin = env!("CARGO_BIN_EXE_chisel");
    let _ = Command::new(bin).args(["--json", "analyze", "--project-dir"]).arg(&dir).arg("--storage-dir").arg(&store).status();
    let before = fs::read_to_string(store.join("store")).unwrap();
    let gen_before = before.split("\"generation\":").nth(1).and_then(|s| s.split([',', '}']).next()).unwrap_or("0").trim().parse::<i64>().unwrap_or(0);
    let pytest = dir.join("pytest");
    fs::write(&pytest, "#!/bin/sh\necho 'tests/test_a.py::test_ok PASSED'\necho 'tests/test_a.py::test_bad FAILED'\nexit 1\n").unwrap();
    let _ = Command::new("chmod").arg("+x").arg(&pytest).status();
    let out = Command::new(bin).args(["run", "--project-dir"]).arg(&dir).arg("--storage-dir").arg(&store).arg("--").arg(&pytest).output().unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", String::from_utf8_lossy(&out.stderr));
    let after = fs::read_to_string(store.join("store")).unwrap();
    let gen_after = after.split("\"generation\":").nth(1).and_then(|s| s.split([',', '}']).next()).unwrap_or("0").trim().parse::<i64>().unwrap_or(0);
    assert_eq!(gen_after, gen_before + 1, "before {gen_before} after {gen_after}");
    assert!(after.contains("test_ok") && after.contains("test_bad"), "{after}");
}

#[test]
fn working_tree_reads_sources_only_when_set() {
    let dir = scratch("wt");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/a.ts"), "export function add(a, b) { return a + b; }\n").unwrap();
    init_repo(&dir);
    commit_all(&dir, "init");
    let store = dir.join("store");
    let mut engine = chisel::Engine::open(&dir, Some(&store)).unwrap();
    engine.analyze().unwrap();
    engine.source_reads = 0;
    engine.dispatch("risk_map", &chisel::json::Value::object(vec![])).unwrap();
    assert_eq!(engine.source_reads, 0);
    engine.dispatch("risk_map", &chisel::json::Value::object(vec![("working_tree".into(), chisel::json::Value::Bool(true))])).unwrap();
    assert!(engine.source_reads > 0);
}

#[test]
fn auto_update_under_50_refreshes() {
    let dir = scratch("autosmall");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/a.py"), "def a():\n    return 1\n").unwrap();
    init_repo(&dir);
    commit_all(&dir, "init");
    let store = dir.join("store");
    let mut engine = chisel::Engine::open(&dir, Some(&store)).unwrap();
    engine.analyze().unwrap();
    fs::write(dir.join("src/b.py"), "def b():\n    return 2\n").unwrap();
    let result = engine.dispatch("diff_impact", &chisel::json::Value::object(vec![
        ("auto_update".into(), chisel::json::Value::Bool(true)),
    ])).unwrap();
    let text = chisel::json::to_string(&result).unwrap();
    assert!(!text.contains("stale_db"), "auto_update returned the pre-update body: {text}");
    assert!(!text.contains("auto_update_skip_reason"), "{text}");
    let snap = fs::read_to_string(store.join("store")).unwrap();
    assert!(snap.contains("src/b.py"), "{snap}");
}

#[test]
fn source_only_update_keeps_untouched_test_edges() {
    let dir = scratch("edges");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join("tests")).unwrap();
    fs::write(dir.join("src/a.py"), "def a():\n    return 1\n").unwrap();
    fs::write(dir.join("src/b.py"), "def b():\n    return 2\n").unwrap();
    fs::write(dir.join("tests/test_b.py"), "from src.b import b\n\ndef test_b():\n    assert b() == 2\n").unwrap();
    init_repo(&dir);
    commit_all(&dir, "init");
    let store = dir.join("store");
    let mut engine = chisel::Engine::open(&dir, Some(&store)).unwrap();
    engine.analyze().unwrap();
    let before = fs::read_to_string(store.join("store")).unwrap();
    assert!(before.contains("tests/test_b.py:test_b"), "{before}");
    engine.source_reads = 0;
    fs::write(dir.join("src/a.py"), "def a():\n    return 3\n").unwrap();
    let update = engine.update().unwrap();
    let text = chisel::json::to_string(&update).unwrap();
    assert!(text.contains("\"edge_rebuild_skipped\":true") || text.contains("\"edge_rebuild_skipped\": true"), "{text}");
    assert_eq!(engine.source_reads, 0, "untouched test file was opened during update");
    let after = fs::read_to_string(store.join("store")).unwrap();
    assert!(after.contains("tests/test_b.py:test_b"), "test edge disappeared: {after}");
}

#[test]
fn commit_after_dirty_analyze_replaces_blame() {
    let dir = scratch("reblame");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/a.py"), "def a():\n    return 1\n").unwrap();
    init_repo(&dir);
    commit_all(&dir, "init");
    let dirty = "def a():\n    return 2\n";
    fs::write(dir.join("src/a.py"), dirty).unwrap();
    let line_len = dirty.lines().count() as i64;
    let store = dir.join("store");
    let mut engine = chisel::Engine::open(&dir, Some(&store)).unwrap();
    engine.analyze().unwrap();
    let before = fs::read_to_string(store.join("store")).unwrap();
    assert!(before.contains("Not Committed Yet"), "dirty analyze did not record uncommitted blame: {before}");
    commit_all(&dir, "commit dirty");
    let log = Arc::new(Mutex::new(Vec::new()));
    engine.set_git_runner(recording_runner(Arc::clone(&log)));
    engine.update().unwrap();
    let spawns = log.lock().unwrap().clone();
    let blames: Vec<_> = spawns.iter().filter(|s| s.contains("blame") && s.contains("src/a.py")).cloned().collect();
    assert_eq!(blames.len(), 1, "update blamed src/a.py more than once: {spawns:?}");
    let owned = engine.dispatch("ownership", &chisel::json::Value::object(vec![
        ("file_path".into(), chisel::json::Value::String("src/a.py".into())),
    ])).unwrap();
    let rows = owned.as_array().expect("ownership rows");
    let line_count: i64 = rows.iter().map(|row| row.get("line_count").and_then(|v| v.as_i64()).unwrap_or(0)).sum();
    assert_eq!(line_count, line_len, "{owned:?}");
    let text = chisel::json::to_string(&owned).unwrap();
    assert!(!text.contains("Not Committed Yet"), "{text}");
    let snap = fs::read_to_string(store.join("store")).unwrap();
    assert!(!snap.contains("Not Committed Yet"), "{snap}");
}

fn unit_churn_count(snap: &str, file: &str) -> usize {
    let doc = chisel::json::parse(snap).unwrap();
    doc.get("churn")
        .and_then(|v| v.as_array())
        .unwrap()
        .iter()
        .filter(|row| {
            row.get("file_path").and_then(|v| v.as_str()) == Some(file)
                && !row.get("unit_name").and_then(|v| v.as_str()).unwrap_or("").is_empty()
        })
        .count()
}

#[test]
fn renamed_source_drops_stale_edges_and_replaces_unit_churn() {
    let dir = scratch("rename");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join("tests")).unwrap();
    fs::write(dir.join("src/a.py"), "def a():\n    return 1\n").unwrap();
    fs::write(dir.join("src/b.py"), "def b():\n    return 2\n").unwrap();
    fs::write(dir.join("tests/test_a.py"), "from src.a import a\n\ndef test_a():\n    assert a() == 1\n").unwrap();
    fs::write(dir.join("tests/test_b.py"), "from src.b import b\n\ndef test_b():\n    assert b() == 2\n").unwrap();
    init_repo(&dir);
    commit_all(&dir, "init");
    let store = dir.join("store");
    let mut engine = chisel::Engine::open(&dir, Some(&store)).unwrap();
    engine.analyze().unwrap();
    let analyzed = fs::read_to_string(store.join("store")).unwrap();
    assert!(analyzed.contains("\"code_id\":\"src/a.py:a:function\""), "{analyzed}");
    let before_units = unit_churn_count(&analyzed, "src/a.py");
    assert!(before_units >= 1, "{analyzed}");
    fs::write(dir.join("src/a.py"), "def a():\n    return 3\n").unwrap();
    engine.update().unwrap();
    let same_name = fs::read_to_string(store.join("store")).unwrap();
    assert_eq!(unit_churn_count(&same_name, "src/a.py"), before_units, "unit churn rows stacked: {same_name}");
    fs::write(dir.join("src/a.py"), "def a():\n    return 4\n").unwrap();
    engine.update().unwrap();
    let second = fs::read_to_string(store.join("store")).unwrap();
    assert_eq!(unit_churn_count(&second, "src/a.py"), before_units, "second update stacked unit churn: {second}");
    engine.source_reads = 0;
    fs::write(dir.join("src/a.py"), "def renamed():\n    return 1\n").unwrap();
    let update = engine.update().unwrap();
    let text = chisel::json::to_string(&update).unwrap();
    assert!(!text.contains("\"edge_rebuild_skipped\":true") && !text.contains("\"edge_rebuild_skipped\": true"), "{text}");
    let after = fs::read_to_string(store.join("store")).unwrap();
    assert!(!after.contains("src/a.py:a:function"), "stale edge target survived: {after}");
    assert!(after.contains("src/a.py:renamed:function"), "{after}");
    assert!(after.contains("\"code_id\":\"src/b.py:b:function\""), "unrelated edge was dropped: {after}");
    assert!(engine.source_reads > 0, "covering test was not rematched");
    let covering_reads = engine.source_reads;
    engine.source_reads = 0;
    fs::write(dir.join("src/b.py"), "def b():\n    return 9\n").unwrap();
    engine.update().unwrap();
    let touched_b = fs::read_to_string(store.join("store")).unwrap();
    assert!(touched_b.contains("\"code_id\":\"src/b.py:b:function\""), "{touched_b}");
    assert!(engine.source_reads > 0 && engine.source_reads <= covering_reads, "untouched test_a was reread: {}", engine.source_reads);
}

#[test]
fn update_recomputes_co_changes_after_new_commit() {
    let dir = scratch("cocommit");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/a.py"), "def a():\n    return 1\n").unwrap();
    init_repo(&dir);
    commit_all(&dir, "init");
    let store = dir.join("store");
    let mut engine = chisel::Engine::open(&dir, Some(&store)).unwrap();
    engine.analyze().unwrap();
    fs::write(dir.join("src/b.py"), "def b():\n    return 2\n").unwrap();
    fs::write(dir.join("src/a.py"), "def a():\n    return 3\n").unwrap();
    commit_all(&dir, "pair");
    engine.update().unwrap();
    let snap = fs::read_to_string(store.join("store")).unwrap();
    assert!(snap.contains("src/a.py") && snap.contains("src/b.py"), "{snap}");
    assert!(snap.contains("\"co_commit_count\""), "new commit did not recompute co_changes: {snap}");
}

#[test]
fn impact_emits_co_change_and_import_graph() {
    let dir = scratch("sources");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join("tests")).unwrap();
    fs::write(dir.join("src/a.py"), "def a():\n    return 1\n").unwrap();
    fs::write(dir.join("src/b.py"), "def b():\n    return 2\n").unwrap();
    fs::write(dir.join("tests/test_b.py"), "from src.b import b\n\ndef test_b():\n    assert b() == 2\n").unwrap();
    init_repo(&dir);
    commit_all(&dir, "coupled");
    fs::write(dir.join("src/c.py"), "from src.a import a\n\ndef c():\n    return a()\n").unwrap();
    fs::write(dir.join("tests/test_c.py"), "from src.c import c\n\ndef test_c():\n    assert c() == 1\n").unwrap();
    commit_all(&dir, "importer");
    let store = dir.join("store");
    let mut engine = chisel::Engine::open(&dir, Some(&store)).unwrap();
    engine.analyze().unwrap();
    let args = chisel::json::Value::object(vec![
        ("files".into(), chisel::json::Value::Array(vec![chisel::json::Value::String("src/a.py".into())])),
    ]);
    let result = engine.dispatch("impact", &args).unwrap();
    let text = chisel::json::to_string(&result).unwrap();
    assert!(text.contains("\"source\":\"co_change\"") || text.contains("\"source\": \"co_change\""), "{text}");
    assert!(text.contains("test_b"), "{text}");
    assert!(text.contains("\"source\":\"import_graph\"") || text.contains("\"source\": \"import_graph\""), "{text}");
    assert!(text.contains("test_c"), "{text}");
}

#[test]
fn suggest_without_edges_uses_fallback_and_untracked_uses_working_tree() {
    let dir = scratch("fallback");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join("tests")).unwrap();
    fs::write(dir.join("src/lonely.py"), "def lonely():\n    return 1\n").unwrap();
    fs::write(dir.join("tests/test_widget.py"), "def test_widget():\n    assert True\n").unwrap();
    init_repo(&dir);
    commit_all(&dir, "init");
    let store = dir.join("store");
    let mut engine = chisel::Engine::open(&dir, Some(&store)).unwrap();
    engine.analyze().unwrap();
    let suggest = engine.dispatch("suggest_tests", &chisel::json::Value::object(vec![
        ("file_path".into(), chisel::json::Value::String("src/lonely.py".into())),
    ])).unwrap();
    let text = chisel::json::to_string(&suggest).unwrap();
    assert!(text.contains("\"source\":\"fallback\"") || text.contains("\"source\": \"fallback\""), "{text}");
    fs::write(dir.join("src/widget.py"), "def widget():\n    return 1\n").unwrap();
    engine.update().unwrap();
    let diff = engine.dispatch("diff_impact", &chisel::json::Value::object(vec![])).unwrap();
    let text = chisel::json::to_string(&diff).unwrap();
    assert!(text.contains("\"source\":\"working_tree\"") || text.contains("\"source\": \"working_tree\""), "{text}");
}

#[test]
fn suggest_tests_reads_disk_only_when_working_tree() {
    let dir = scratch("suggestdisk");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join("tests")).unwrap();
    fs::write(dir.join("src/a.py"), "def a():\n    return 1\n").unwrap();
    fs::write(dir.join("tests/test_a.py"), "from src.a import a\n\ndef test_a():\n    assert a() == 1\n").unwrap();
    init_repo(&dir);
    commit_all(&dir, "init");
    let store = dir.join("store");
    let mut engine = chisel::Engine::open(&dir, Some(&store)).unwrap();
    engine.analyze().unwrap();
    engine.source_reads = 0;
    engine.dispatch("suggest_tests", &chisel::json::Value::object(vec![
        ("file_path".into(), chisel::json::Value::String("src/a.py".into())),
    ])).unwrap();
    assert_eq!(engine.source_reads, 0, "suggest_tests read test sources without working_tree");
    engine.dispatch("suggest_tests", &chisel::json::Value::object(vec![
        ("file_path".into(), chisel::json::Value::String("src/a.py".into())),
        ("working_tree".into(), chisel::json::Value::Bool(true)),
    ])).unwrap();
    assert!(engine.source_reads > 0, "working_tree suggest_tests did not read test sources");
}

#[test]
fn shards_toml_expands_dir_glob_on_open() {
    let dir = scratch("shtoml");
    fs::create_dir_all(dir.join("packages/web")).unwrap();
    fs::create_dir_all(dir.join("packages/api")).unwrap();
    fs::create_dir_all(dir.join(".chisel")).unwrap();
    fs::write(dir.join(".chisel/shards.toml"), "shards = [\"packages/*\"]\n").unwrap();
    fs::write(dir.join("packages/web/a.py"), "def a():\n    return 1\n").unwrap();
    fs::write(dir.join("packages/api/b.py"), "def b():\n    return 2\n").unwrap();
    init_repo(&dir);
    commit_all(&dir, "init");
    let prev = std::env::var("CHISEL_SHARDS").ok();
    std::env::remove_var("CHISEL_SHARDS");
    let store = dir.join("store");
    let mut engine = chisel::Engine::open(&dir, Some(&store)).unwrap();
    if let Some(value) = prev {
        std::env::set_var("CHISEL_SHARDS", value);
    }
    let names: Vec<String> = engine.shards.iter().map(|(name, _)| name.clone()).collect();
    assert!(names.iter().any(|name| name == "packages/web"), "{names:?}");
    assert!(names.iter().any(|name| name == "packages/api"), "{names:?}");
    engine.analyze().unwrap();
    assert!(store.join("shards/packages/web/store").exists(), "web shard store missing");
    assert!(store.join("shards/packages/api/store").exists(), "api shard store missing");
}

#[test]
fn unscoped_reads_aggregate_and_update_uses_longest_prefix() {
    let dir = scratch("agg");
    fs::create_dir_all(dir.join("frontend")).unwrap();
    fs::create_dir_all(dir.join("frontend/web")).unwrap();
    fs::write(dir.join("frontend/a.py"), "def a():\n    return 1\n").unwrap();
    fs::write(dir.join("frontend/web/w.py"), "def w():\n    return 2\n").unwrap();
    init_repo(&dir);
    commit_all(&dir, "init");
    let store = dir.join("store");
    let mut engine = chisel::Engine::open(&dir, Some(&store)).unwrap();
    engine.set_shards(vec![
        ("frontend".into(), "frontend".into()),
        ("frontend/web".into(), "frontend/web".into()),
    ]);
    engine.analyze().unwrap();
    let risk = engine.risk_map().unwrap();
    let text = chisel::json::to_string(&risk).unwrap();
    assert!(text.contains("frontend/a.py"), "{text}");
    assert!(text.contains("frontend/web/w.py"), "{text}");
    assert!(text.contains("unsupported"), "{text}");
    let gaps = engine.dispatch("test_gaps", &chisel::json::Value::object(vec![])).unwrap();
    let gaps_text = chisel::json::to_string(&gaps).unwrap();
    assert!(gaps_text.contains("frontend/a.py") && gaps_text.contains("frontend/web/w.py"), "{gaps_text}");
    fs::write(dir.join("frontend/web/w.py"), "def w():\n    return 9\n").unwrap();
    let update = engine.update().unwrap();
    let update_text = chisel::json::to_string(&update).unwrap();
    assert!(update_text.contains("\"files_updated\":1") || update_text.contains("\"files_updated\": 1"), "{update_text}");
    let web = fs::read_to_string(store.join("shards/frontend/web/store")).unwrap();
    let front = fs::read_to_string(store.join("shards/frontend/store")).unwrap();
    let root = fs::read_to_string(store.join("store")).unwrap_or_default();
    let new_hash = chisel::hash::sha256_hex(b"def w():\n    return 9\n");
    assert!(web.contains(&format!("\"file_path\":\"frontend/web/w.py\"")) && web.contains(&new_hash), "longest prefix did not store the updated file: {web}");
    assert!(!front.contains("\"file_path\":\"frontend/web/w.py\""), "shorter prefix stored the longer-prefix file");
    assert!(!root.contains("\"file_path\":\"frontend/web/w.py\""), "root snapshot stored a shard file");
    let diff = engine.diff_impact().unwrap();
    let diff_text = chisel::json::to_string(&diff).unwrap();
    assert!(!diff_text.contains("stale_db"), "{diff_text}");
}

#[test]
fn git_pool_overlaps_at_most_eight_and_merges_in_path_order() {
    let dir = scratch("pool");
    fs::create_dir_all(dir.join("src")).unwrap();
    for i in 0..12 {
        fs::write(dir.join(format!("src/f{i:02}.py")), format!("def f{i}():\n    return {i}\n")).unwrap();
    }
    init_repo(&dir);
    commit_all(&dir, "init");
    let store = dir.join("store");
    let log = Arc::new(Mutex::new(Vec::new()));
    let inflight = Arc::new(AtomicUsize::new(0));
    let max_seen = Arc::new(AtomicUsize::new(0));
    let mut engine = chisel::Engine::open(&dir, Some(&store)).unwrap();
    engine.set_git_runner(overlap_runner(Arc::clone(&log), Arc::clone(&inflight), Arc::clone(&max_seen)));
    engine.analyze().unwrap();
    let peak = max_seen.load(Ordering::SeqCst);
    assert!(peak > 1 && peak <= 8, "in-flight git peak {peak}");
    let spawns = log.lock().unwrap().clone();
    let logged = spawns.iter().filter(|s| s.contains("-L:")).count();
    let blamed = spawns.iter().filter(|s| s.contains("blame")).count();
    assert_eq!(logged, 12, "{spawns:?}");
    assert_eq!(blamed, 12, "{spawns:?}");
    let first = churn_signature(&fs::read_to_string(store.join("store")).unwrap());
    let file_rows = first.iter().filter(|row| row.contains("||") || row.split('|').nth(1).unwrap_or("x").is_empty()).count();
    let unit_at = first.iter().position(|row| !row.split('|').nth(1).unwrap_or("").is_empty()).unwrap();
    assert!(file_rows >= 12, "{first:?}");
    assert!(first[..unit_at].iter().all(|row| row.split('|').nth(1).unwrap_or("x").is_empty()));
    max_seen.store(0, Ordering::SeqCst);
    engine.analyze().unwrap();
    let second = churn_signature(&fs::read_to_string(store.join("store")).unwrap());
    assert_eq!(first, second);
    let snap = fs::read_to_string(store.join("store")).unwrap();
    assert!(snap.contains("\"branch_co_changes\":[]") || snap.contains("\"branch_co_changes\": []"), "{snap}");
    assert!(snap.contains("branch_coupling_commits") && snap.contains("\"0\""), "{snap}");
}

fn commit_as(dir: &Path, msg: &str, author: &str, date: &str) {
    git(dir, &["add", "-A"]);
    let status = Command::new("git")
        .args(["commit", "-m", msg])
        .env("GIT_AUTHOR_NAME", author)
        .env("GIT_AUTHOR_EMAIL", format!("{author}@example.com"))
        .env("GIT_COMMITTER_NAME", author)
        .env("GIT_COMMITTER_EMAIL", format!("{author}@example.com"))
        .env("GIT_AUTHOR_DATE", date)
        .env("GIT_COMMITTER_DATE", date)
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(status.success(), "{msg}");
}

#[test]
fn feature_branch_co_change_feeds_risk_and_matches_python() {
    let dir = scratch("branch");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/c.py"), "def c():\n    return 1\n").unwrap();
    init_repo(&dir);
    for i in 0..8 {
        let author = if i % 2 == 0 { "Ada" } else { "Bea" };
        let text = fs::read_to_string(dir.join("src/c.py")).unwrap();
        fs::write(dir.join("src/c.py"), format!("{text}\n# {i}\n")).unwrap();
        commit_as(&dir, &format!("c{i}"), author, &format!("2024-03-{:02}T00:00:00+00:00", i + 1));
    }
    git(&dir, &["checkout", "-b", "feature"]);
    fs::write(dir.join("src/a.py"), "def a():\n    return 1\n").unwrap();
    fs::write(dir.join("src/b.py"), "def b():\n    return 2\n").unwrap();
    commit_as(&dir, "pair", "Ada", "2024-04-01T00:00:00+00:00");
    let store = dir.join("store");
    let now = include_str!("fixtures/oracle/clock.txt").trim();
    let bin = env!("CARGO_BIN_EXE_chisel");
    let analyze = Command::new(bin)
        .args(["analyze", ".", "--project-dir"])
        .arg(&dir)
        .arg("--storage-dir")
        .arg(&store)
        .arg("--json")
        .env("CHISEL_NOW", now)
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(analyze.status.success(), "{}", String::from_utf8_lossy(&analyze.stderr));
    let snap = fs::read_to_string(store.join("store")).unwrap();
    assert!(snap.contains("\"file_a\":\"src/a.py\"") && snap.contains("\"file_b\":\"src/b.py\""), "{snap}");
    let risk = Command::new(bin)
        .args(["risk-map", "--project-dir"])
        .arg(&dir)
        .arg("--storage-dir")
        .arg(&store)
        .arg("--json")
        .env("CHISEL_NOW", now)
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(risk.status.success(), "{}", String::from_utf8_lossy(&risk.stderr));
    let got = chisel::json::parse(&String::from_utf8_lossy(&risk.stdout)).unwrap();
    let want = chisel::json::parse(include_str!("fixtures/oracle/feature-risk-map.json")).unwrap();
    assert_eq!(got, want, "{}", String::from_utf8_lossy(&risk.stdout));
}

#[test]
fn jest_blocks_and_require_confidence_match_python() {
    let src = "\
describe.only('suite', () => {});\n\
it.skip('one', () => {});\n\
test.todo('two', () => {});\n\
const MODULE = './calc';\n\
require(MODULE);\n\
require(unknownName);\n\
require(`./${name}`);\n\
require('./' + name);\n\
require(flag ? './prod' : './dev');\n\
new Function('return require(\"x\")');\n\
function add() { return 1; }\n";
    let units = chisel::extract::extract_code_units("src/calc.test.js", src);
    let kinds: Vec<(String, String)> = units.iter().map(|u| (u.name.clone(), u.unit_type.clone())).collect();
    assert!(kinds.iter().any(|(n, t)| n == "suite" && t == "test_suite"), "{kinds:?}");
    assert!(kinds.iter().any(|(n, t)| n == "one" && t == "test_case"), "{kinds:?}");
    assert!(kinds.iter().any(|(n, t)| n == "two" && t == "test_case"), "{kinds:?}");
    let deps = chisel::extract::extract_deps("src/calc.test.js", src);
    assert!(deps.iter().any(|d| d.dep_type == "tainted_import" && (d.confidence - 1.0).abs() < 1e-9 && d.module_path.as_deref() == Some("./calc")), "{deps:?}");
    assert!(deps.iter().any(|d| d.name == "require(unknownName)" && (d.confidence - 0.3).abs() < 1e-9), "{deps:?}");
    assert!(deps.iter().any(|d| (d.confidence - 0.4).abs() < 1e-9 && d.module_path.as_deref() == Some("./${name}")), "{deps:?}");
    assert!(deps.iter().any(|d| (d.confidence - 0.2).abs() < 1e-9 && d.name.contains('+')), "{deps:?}");
    assert!(deps.iter().any(|d| (d.confidence - 0.3).abs() < 1e-9 && d.module_path.as_deref() == Some("./prod|./dev")), "{deps:?}");
    assert!(deps.iter().any(|d| d.dep_type == "eval_import" && d.confidence == 0.0), "{deps:?}");
    let frozen = chisel::json::parse(include_str!("fixtures/oracle/jest-deps.json")).unwrap();
    let rows = frozen.as_array().unwrap();
    for row in rows {
        let ty = row.get("dep_type").and_then(|v| v.as_str()).unwrap_or("");
        let conf = row.get("confidence").and_then(|v| v.as_f64()).unwrap_or(1.0);
        let module = row.get("module_path").and_then(|v| v.as_str()).unwrap_or("");
        assert!(
            deps.iter().any(|d| d.dep_type == ty && (d.confidence - conf).abs() < 1e-9 && d.module_path.clone().unwrap_or_default() == module),
            "missing frozen dep {ty} {conf} {module} in {deps:?}"
        );
    }
}

#[test]
fn record_result_changes_failure_rate_and_instability() {
    let dir = scratch("fail");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join("tests")).unwrap();
    fs::write(dir.join("src/calc.py"), "def add(a, b):\n    return a + b\n").unwrap();
    fs::write(dir.join("tests/test_calc.py"), "from src.calc import add\n\ndef test_add():\n    assert add(1, 1) == 2\n").unwrap();
    init_repo(&dir);
    commit_all(&dir, "init");
    let store = dir.join("store");
    let mut engine = chisel::Engine::open(&dir, Some(&store)).unwrap();
    engine.analyze().unwrap();
    let before = engine.suggest_tests("src/calc.py").unwrap();
    let row = before.as_array().and_then(|a| a.first()).expect("suggestion");
    let tid = row.get("test_id").and_then(|v| v.as_str()).unwrap().to_string();
    let before_rel = row.get("relevance").and_then(|v| v.as_f64()).unwrap();
    assert!(before_rel > 0.0, "{before_rel}");
    assert_eq!(row.get("failure_rate").and_then(|v| v.as_f64()), Some(0.0));
    engine.record_result(&tid, false).unwrap();
    let after = engine.suggest_tests("src/calc.py").unwrap();
    let again = after.as_array().unwrap().iter().find(|r| r.get("test_id").and_then(|v| v.as_str()) == Some(tid.as_str())).unwrap();
    let after_rel = again.get("relevance").and_then(|v| v.as_f64()).unwrap();
    assert!((again.get("failure_rate").and_then(|v| v.as_f64()).unwrap() - 1.0).abs() < 1e-9, "{again:?}");
    assert!(after_rel > before_rel, "{before_rel} -> {after_rel}");
    let risk = engine.risk_map().unwrap();
    let text = chisel::json::to_string(&risk).unwrap();
    assert!(text.contains("\"test_instability\":1") || text.contains("\"test_instability\": 1"), "{text}");
}

fn write_exec(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    let status = Command::new("chmod").args(["+x"]).arg(path).status().unwrap();
    assert!(status.success());
}

#[test]
fn extractor_failure_falls_back_and_success_replaces_units() {
    let dir = scratch("ext");
    fs::write(dir.join("a.py"), "def a():\n    return 1\n").unwrap();
    init_repo(&dir);
    commit_all(&dir, "init");
    let bad = dir.join("bad.sh");
    let nope = dir.join("nope.sh");
    let ok = dir.join("ok.sh");
    write_exec(&bad, "#!/bin/sh\ncat >/dev/null\nexit 1\n");
    write_exec(&nope, "#!/bin/sh\ncat >/dev/null\nprintf 'nope\\n'\n");
    write_exec(&ok, "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '[{\"name\":\"from_extractor\",\"unit_type\":\"function\",\"line_start\":1,\"line_end\":2}]'\n");
    let bin = env!("CARGO_BIN_EXE_chisel");
    let run = |script: &Path, store: &Path| {
        Command::new(bin)
            .env("CHISEL_EXTRACTOR", script)
            .args(["analyze", ".", "--project-dir"])
            .arg(&dir)
            .arg("--storage-dir")
            .arg(store)
            .arg("--json")
            .current_dir(&dir)
            .output()
            .unwrap()
    };
    let failed = run(&bad, &dir.join("bad-store"));
    let failed_out = String::from_utf8_lossy(&failed.stdout);
    assert!(failed.status.success(), "{}", String::from_utf8_lossy(&failed.stderr));
    assert!(failed_out.contains("extractor_warnings"), "{failed_out}");
    assert!(failed_out.contains("CHISEL_EXTRACTOR failed"), "{failed_out}");
    let snap = fs::read_to_string(dir.join("bad-store/store")).unwrap();
    assert!(snap.contains("\"name\":\"a\""), "builtin fallback missing: {snap}");
    let junk = run(&nope, &dir.join("nope-store"));
    let junk_out = String::from_utf8_lossy(&junk.stdout);
    assert!(junk.status.success() && junk_out.contains("bad JSON"), "{junk_out}");
    let snap = fs::read_to_string(dir.join("nope-store/store")).unwrap();
    assert!(snap.contains("\"name\":\"a\""), "{snap}");
    let custom = run(&ok, &dir.join("ok-store"));
    assert!(custom.status.success(), "{}", String::from_utf8_lossy(&custom.stderr));
    let custom_out = String::from_utf8_lossy(&custom.stdout);
    assert!(!custom_out.contains("extractor_warnings"), "{custom_out}");
    let snap = fs::read_to_string(dir.join("ok-store/store")).unwrap();
    assert!(snap.contains("from_extractor"), "{snap}");
    assert!(!snap.contains("\"name\":\"a\""), "builtin unit leaked: {snap}");
}

#[test]
fn stale_generation_returns_status_error_and_retry_keeps_other_writer() {
    let dir = scratch("stale");
    fs::write(dir.join("a.py"), "def a():\n    return 1\n").unwrap();
    init_repo(&dir);
    commit_all(&dir, "init");
    let store = dir.join("store");
    let mut engine = chisel::Engine::open(&dir, Some(&store)).unwrap();
    engine.analyze().unwrap();
    let mut other = chisel::store::Store::open(&store).unwrap();
    other.doc.insert("token", chisel::json::Value::String("winner".into()));
    other.save().unwrap();
    let recorded = engine.dispatch("record_result", &chisel::json::Value::object(vec![
        ("test_id".into(), chisel::json::Value::String("tests/test_a.py:test_a".into())),
        ("passed".into(), chisel::json::Value::Bool(false)),
    ])).unwrap();
    let text = chisel::json::to_string(&recorded).unwrap();
    assert!(text.contains("\"recorded\":true") || text.contains("\"recorded\": true"), "{text}");
    let snap = fs::read_to_string(store.join("store")).unwrap();
    assert!(snap.contains("winner"), "retry clobbered the other writer: {snap}");
    assert!(snap.contains("tests/test_a.py:test_a"), "{snap}");
    engine.store.fail_stale_remaining = 3;
    let err = engine.dispatch("record_result", &chisel::json::Value::object(vec![
        ("test_id".into(), chisel::json::Value::String("tests/test_a.py:test_a".into())),
        ("passed".into(), chisel::json::Value::Bool(false)),
    ])).unwrap();
    let err_text = chisel::json::to_string(&err).unwrap();
    assert!(err_text.contains("\"status\":\"error\"") || err_text.contains("\"status\": \"error\""), "{err_text}");
    assert!(err_text.contains("stale generation"), "{err_text}");
}

#[test]
fn triage_limit_and_schema_flags_change_results() {
    let dir = scratch("flags");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/calc.py"), "def add(a, b):\n    return a + b\n\ndef sub(a, b):\n    return a - b\n").unwrap();
    fs::write(dir.join("src/extra.py"), "from src.calc import add\n\ndef wrapper():\n    return add(1, 1)\n").unwrap();
    fs::write(dir.join("src/fresh.py"), "def fresh():\n    return 1\n").unwrap();
    init_repo(&dir);
    commit_all(&dir, "init");
    fs::write(dir.join("src/brand_new.py"), "def brand():\n    return 9\n").unwrap();
    let store = dir.join("store");
    let mut engine = chisel::Engine::open(&dir, Some(&store)).unwrap();
    engine.analyze().unwrap();
    assert_eq!(engine.bare_index_builds, 0, "no tests means no bare-name index");
    let summary = chisel::json::to_string(&engine.analyze().unwrap()).unwrap();
    assert!(summary.contains("test_*.py") && summary.contains("*.spec.ts"), "{summary}");
    fs::create_dir_all(dir.join("tests")).unwrap();
    fs::write(dir.join("tests/test_calc.py"), "from src.calc import add, sub\n\ndef test_add():\n    assert add(1, 1) == 2\n\ndef test_sub():\n    assert sub(1, 1) == 0\n").unwrap();
    engine.analyze().unwrap();
    assert_eq!(engine.bare_index_builds, 1, "one index per test file");
    let line = engine.dispatch("risk_map", &chisel::json::Value::object(vec![
        ("coverage_mode".into(), chisel::json::Value::String("line".into())),
        ("proximity_adjustment".into(), chisel::json::Value::Bool(true)),
    ])).unwrap();
    let unit = engine.dispatch("risk_map", &chisel::json::Value::object(vec![
        ("coverage_mode".into(), chisel::json::Value::String("unit".into())),
        ("proximity_adjustment".into(), chisel::json::Value::Bool(false)),
    ])).unwrap();
    let line_text = chisel::json::to_string(&line).unwrap();
    let unit_text = chisel::json::to_string(&unit).unwrap();
    assert_ne!(line_text, unit_text, "coverage mode and proximity did not change risk");
    assert!(unit_text.contains("\"coverage_gap_mode\":\"unit\"") || unit_text.contains("\"coverage_gap_mode\": \"unit\""), "{unit_text}");
    let boosted = engine.dispatch("risk_map", &chisel::json::Value::object(vec![])).unwrap();
    let plain = engine.dispatch("risk_map", &chisel::json::Value::object(vec![
        ("exclude_new_file_boost".into(), chisel::json::Value::Bool(true)),
    ])).unwrap();
    let boosted_text = chisel::json::to_string(&boosted).unwrap();
    let plain_text = chisel::json::to_string(&plain).unwrap();
    assert!(boosted_text.contains("brand_new.py"), "{boosted_text}");
    assert_ne!(boosted_text, plain_text, "exclude_new_file_boost did not change risk");
    let triage = engine.dispatch("triage", &chisel::json::Value::object(vec![
        ("limit".into(), chisel::json::Value::Int(1)),
    ])).unwrap();
    let triage_text = chisel::json::to_string(&triage).unwrap();
    assert!(triage_text.contains("\"truncated\":true") || triage_text.contains("\"truncated\": true"), "{triage_text}");
    let files = triage.get("top_risk_files").and_then(|v| v.as_array()).unwrap();
    assert_eq!(files.len(), 1, "{triage_text}");
}

#[test]
fn second_process_sees_job_progress_and_cancel() {
    let dir = scratch("jobproc");
    fs::create_dir_all(dir.join("src")).unwrap();
    for i in 0..8 {
        fs::write(dir.join(format!("src/f{i}.py")), "def f():\n    return 1\n").unwrap();
    }
    init_repo(&dir);
    commit_all(&dir, "init");
    let store = dir.join("store");
    std::env::set_var("CHISEL_JOB_PACE_MS", "250");
    let mut engine = chisel::Engine::open(&dir, Some(&store)).unwrap();
    let job = engine.start_job("analyze").unwrap();
    let id = job.get("job_id").and_then(|v| v.as_str()).unwrap().to_string();
    let bin = env!("CARGO_BIN_EXE_chisel");
    let mut saw_progress = false;
    for _ in 0..40 {
        std::thread::sleep(std::time::Duration::from_millis(40));
        let out = Command::new(bin)
            .args(["job-status", &id, "--project-dir"])
            .arg(&dir)
            .arg("--storage-dir")
            .arg(&store)
            .arg("--json")
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        if text.contains("progress_pct") && !text.contains("\"progress_pct\":0") && !text.contains("\"progress_pct\": 0") {
            saw_progress = true;
            break;
        }
    }
    assert!(saw_progress, "second process did not observe progress");
    let cancel = Command::new(bin)
        .args(["cancel-job", &id, "--project-dir"])
        .arg(&dir)
        .arg("--storage-dir")
        .arg(&store)
        .arg("--json")
        .output()
        .unwrap();
    assert!(cancel.status.success(), "{}", String::from_utf8_lossy(&cancel.stderr));
    let mut saw_cancel = false;
    let mut last_status = String::new();
    for _ in 0..40 {
        std::thread::sleep(std::time::Duration::from_millis(50));
        let out = Command::new(bin)
            .args(["job-status", &id, "--project-dir"])
            .arg(&dir)
            .arg("--storage-dir")
            .arg(&store)
            .arg("--json")
            .output()
            .unwrap();
        last_status = format!("stdout={} stderr={} code={:?}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr), out.status.code());
        if last_status.contains("cancelled") || last_status.contains("cancel_requested") {
            saw_cancel = true;
            break;
        }
    }
    std::env::remove_var("CHISEL_JOB_PACE_MS");
    assert!(saw_cancel, "second process did not observe cancel: {last_status}");
}
