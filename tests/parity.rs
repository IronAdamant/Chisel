//! Behaviour the Python 0.15.1 review called missing, partial, or different.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

fn scratch(name: &str) -> PathBuf {
    let n = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let p = std::env::temp_dir().join(format!("chisel-parity-{name}-{n}"));
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
    git(dir, &["config", "user.name", "Ada"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    git(dir, &["config", "core.autocrlf", "false"]);
}

fn commit_all(dir: &Path, msg: &str) {
    git(dir, &["add", "-A"]);
    let status = Command::new("git")
        .args(["commit", "-m", msg])
        .current_dir(dir)
        .env("GIT_AUTHOR_DATE", "2024-05-01T00:00:00+00:00")
        .env("GIT_COMMITTER_DATE", "2024-05-01T00:00:00+00:00")
        .status()
        .unwrap();
    assert!(status.success(), "{msg:?}");
}

fn write_sample(dir: &Path) {
    fs::create_dir_all(dir.join("py")).unwrap();
    fs::create_dir_all(dir.join("js")).unwrap();
    fs::create_dir_all(dir.join("java")).unwrap();
    fs::create_dir_all(dir.join("cs")).unwrap();
    fs::create_dir_all(dir.join("swift")).unwrap();
    fs::create_dir_all(dir.join("cpp")).unwrap();
    fs::create_dir_all(dir.join("rb")).unwrap();
    fs::create_dir_all(dir.join("tests")).unwrap();
    fs::write(dir.join("py/a.py"), "from py.b import pong\n\ndef ping():\n    return pong()\n").unwrap();
    fs::write(dir.join("py/b.py"), "from py.a import ping\n\ndef pong():\n    return ping()\n").unwrap();
    fs::write(dir.join("py/util.py"), "def util():\n    return 3\n").unwrap();
    fs::write(dir.join("tests/test_a.py"), "from py.a import ping\n\ndef test_ping():\n    assert ping() == 1\n").unwrap();
    fs::write(dir.join("js/math.js"), "function multiplies(a, b) {\n  return a * b;\n}\nmodule.exports = { multiplies };\n").unwrap();
    fs::write(dir.join("js/math.test.js"), "const math = require('./math');\ntest('multiplies', () => { math.multiplies(2, 3); });\n").unwrap();
    fs::write(dir.join("js/flow.spec.ts"), "import { test } from '@playwright/test';\ntest('loads', async () => {});\n").unwrap();
    fs::write(dir.join("java/CalcTest.java"), "import org.junit.Test;\npublic class CalcTest {\n  @Test\n  public void shouldAdd() {}\n}\n").unwrap();
    fs::write(dir.join("cs/CalcTests.cs"), "public class CalcTests {\n  [Fact]\n  public void AddsNumbers() {}\n}\n").unwrap();
    fs::write(dir.join("swift/CalcTests.swift"), "import Testing\n@Test func addsNumbers() {}\n").unwrap();
    fs::write(dir.join("cpp/calc_unittest.cpp"), "#include <gtest/gtest.h>\nTEST(Calc, Adds) {\n  EXPECT_EQ(1, 1);\n}\n").unwrap();
    fs::write(dir.join("rb/calc_spec.rb"), "describe 'calc' do\n  it 'adds' do\n  end\nend\n").unwrap();
    fs::write(dir.join("js/dynamic.js"), "function make() {\n  return eval('1');\n}\n").unwrap();
}

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_chisel")
}

#[test]
fn newline_mcp_answers_initialize() {
    let dir = scratch("mcp");
    let store = dir.join("store");
    let mut child = Command::new(bin())
        .args(["serve-mcp"])
        .env("CHISEL_PROJECT_DIR", &dir)
        .env("CHISEL_STORAGE_DIR", &store)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let init = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n";
    child.stdin.as_mut().unwrap().write_all(init).unwrap();
    drop(child.stdin.take());
    let mut out = String::new();
    child.stdout.take().unwrap().read_to_string(&mut out).unwrap();
    let _ = child.wait();
    assert!(out.contains("protocolVersion"), "{out:?}");
    assert!(out.contains(env!("CARGO_PKG_VERSION")), "{out:?}");
    assert!(!out.contains("Content-Length"), "newline clients must not get Content-Length framing: {out:?}");
    let tools = b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n";
    let framed = {
        let mut input = std::io::Cursor::new(tools.to_vec());
        chisel::mcp::read_message(&mut input).unwrap().unwrap()
    };
    assert!(framed.contains("tools/list"), "{framed:?}");
}

#[test]
fn content_length_round_trip_still_parses() {
    let body = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}";
    let mut raw = format!("Content-Length: {}\r\n\r\n{body}", body.len()).into_bytes();
    raw.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n");
    let mut input = std::io::Cursor::new(raw);
    assert_eq!(chisel::mcp::read_message(&mut input).unwrap().unwrap(), body);
    assert!(chisel::mcp::read_message(&mut input).unwrap().unwrap().contains("\"id\":2"));
}

#[test]
fn flags_reach_the_engine_on_the_sample() {
    let dir = scratch("flags");
    write_sample(&dir);
    init_repo(&dir);
    commit_all(&dir, "init");
    let calc = dir.join("py/a.py");
    let mut text = fs::read_to_string(&calc).unwrap();
    text.push_str("\n# again\n");
    fs::write(&calc, text).unwrap();
    git(&dir, &["add", "-A"]);
    let status = Command::new("git")
        .args(["commit", "-m", "edit ping"])
        .current_dir(&dir)
        .env("GIT_AUTHOR_DATE", "2024-06-01T00:00:00+00:00")
        .env("GIT_COMMITTER_DATE", "2024-06-01T00:00:00+00:00")
        .status()
        .unwrap();
    assert!(status.success());
    let store = dir.join("store");
    let analyze = Command::new(bin()).args(["--json", "analyze", "--project-dir"]).arg(&dir).arg("--storage-dir").arg(&store).output().unwrap();
    assert!(analyze.status.success(), "{}", String::from_utf8_lossy(&analyze.stderr));
    let snap = fs::read_to_string(store.join("store")).unwrap();
    assert!(snap.contains("shouldAdd"), "{snap:?}");
    assert!(snap.contains("AddsNumbers"), "{snap:?}");
    assert!(snap.contains("addsNumbers"), "{snap:?}");
    assert!(snap.contains("Calc.Adds"), "{snap:?}");
    assert!(snap.contains("\"framework\":\"junit\""), "{snap:?}");
    assert!(snap.contains("\"framework\":\"csharp_test\""), "{snap:?}");
    assert!(snap.contains("\"framework\":\"gtest\""), "{snap:?}");
    assert!(snap.contains("\"framework\":\"jest\""), "{snap:?}");
    assert!(snap.contains("\"framework\":\"playwright\""), "{snap:?}");
    assert!(snap.contains("\"framework\":\"rspec\""), "{snap:?}");
    assert!(!snap.contains("require:function"), "require() must not become a code unit: {snap:?}");

    let risk_all = Command::new(bin()).args(["--json", "risk-map", "--project-dir"]).arg(&dir).arg("--storage-dir").arg(&store).output().unwrap();
    let risk_text = String::from_utf8_lossy(&risk_all.stdout);
    let risk = chisel::json::parse(risk_text.trim()).unwrap();
    let files = risk.get("files").and_then(|v| v.as_array()).unwrap();
    let paths: Vec<&str> = files.iter().filter_map(|f| f.get("file_path").and_then(|v| v.as_str())).collect();
    assert!(paths.iter().any(|p| p.ends_with("a.py")) && paths.iter().any(|p| p.ends_with("b.py")), "{paths:?}");
    assert!(!paths.iter().any(|p| p.contains("calc_unittest")), "gtest file ranked as code: {paths:?}");
    let cycles = risk.get("_meta").and_then(|m| m.get("cycles")).and_then(|v| v.as_array()).unwrap();
    assert!(cycles.iter().any(|c| c.get("length").and_then(|v| v.as_i64()) == Some(2)), "{cycles:?}");

    let scoped = Command::new(bin()).args(["--json", "risk-map", "py", "--project-dir"]).arg(&dir).arg("--storage-dir").arg(&store).output().unwrap();
    let scoped_doc = chisel::json::parse(String::from_utf8_lossy(&scoped.stdout).trim()).unwrap();
    let scoped_files = scoped_doc.get("files").and_then(|v| v.as_array()).unwrap();
    assert!(scoped_files.iter().all(|f| f.get("file_path").and_then(|v| v.as_str()).unwrap_or("").starts_with("py/")), "{scoped_doc:?}");
    assert!(scoped_files.len() < files.len(), "directory scope did not shrink the map");

    let with_tests = Command::new(bin()).args(["--json", "risk-map", "--no-exclude-tests", "--project-dir"]).arg(&dir).arg("--storage-dir").arg(&store).output().unwrap();
    assert!(with_tests.status.success(), "{}", String::from_utf8_lossy(&with_tests.stderr));
    let with_doc = chisel::json::parse(String::from_utf8_lossy(&with_tests.stdout).trim()).unwrap();
    let with_paths: Vec<&str> = with_doc.get("files").and_then(|v| v.as_array()).unwrap().iter().filter_map(|f| f.get("file_path").and_then(|v| v.as_str())).collect();
    assert!(with_paths.iter().any(|p| p.contains("test_a.py")), "{with_paths:?}");

    let gaps = Command::new(bin()).args(["--json", "test-gaps", "py/util.py", "--project-dir"]).arg(&dir).arg("--storage-dir").arg(&store).output().unwrap();
    let gap_doc = chisel::json::parse(String::from_utf8_lossy(&gaps.stdout).trim()).unwrap();
    let gap_rows = gap_doc.as_array().unwrap();
    assert!(!gap_rows.is_empty(), "{gap_doc:?}");
    assert!(gap_rows.iter().all(|g| g.get("file_path").and_then(|v| v.as_str()) == Some("py/util.py")), "{gap_doc:?}");

    let triage = Command::new(bin()).args(["--json", "triage", "py", "--top-n", "1", "--project-dir"]).arg(&dir).arg("--storage-dir").arg(&store).output().unwrap();
    assert!(triage.status.success(), "{}", String::from_utf8_lossy(&triage.stderr));
    let triage_doc = chisel::json::parse(String::from_utf8_lossy(&triage.stdout).trim()).unwrap();
    assert_eq!(triage_doc.get("top_risk_files").and_then(|v| v.as_array()).unwrap().len(), 1);

    let suggest = Command::new(bin()).args(["--json", "suggest-tests", "--directory", "py", "--project-dir"]).arg(&dir).arg("--storage-dir").arg(&store).output().unwrap();
    assert!(suggest.status.success(), "{}", String::from_utf8_lossy(&suggest.stderr));
    let suggest_doc = chisel::json::parse(String::from_utf8_lossy(&suggest.stdout).trim()).unwrap();
    assert!(suggest_doc.as_object().is_some(), "directory suggest must be a map: {suggest_doc:?}");

    let fallback = Command::new(bin()).args(["--json", "suggest-tests", "py/util.py", "--fallback", "--project-dir"]).arg(&dir).arg("--storage-dir").arg(&store).output().unwrap();
    assert!(fallback.status.success(), "{}", String::from_utf8_lossy(&fallback.stderr));

    let impact = Command::new(bin()).args(["--json", "impact", "py/a.py", "--functions", "ping", "--project-dir"]).arg(&dir).arg("--storage-dir").arg(&store).output().unwrap();
    assert!(impact.status.success(), "{}", String::from_utf8_lossy(&impact.stderr));
    assert!(String::from_utf8_lossy(&impact.stdout).contains("ping") || String::from_utf8_lossy(&impact.stdout).contains("[]"), "{}", String::from_utf8_lossy(&impact.stdout));

    let churn = Command::new(bin()).args(["--json", "churn", "py/a.py", "--unit", "ping", "--project-dir"]).arg(&dir).arg("--storage-dir").arg(&store).output().unwrap();
    assert!(churn.status.success(), "{}", String::from_utf8_lossy(&churn.stderr));
    let churn_doc = chisel::json::parse(String::from_utf8_lossy(&churn.stdout).trim()).unwrap();
    let churn_rows = churn_doc.as_array().unwrap();
    assert_eq!(churn_rows.len(), 1, "{churn_doc:?}");
    assert_eq!(churn_rows[0].get("unit_name").and_then(|v| v.as_str()), Some("ping"));
    assert!(churn_rows[0].get("total_insertions").and_then(|v| v.as_i64()).unwrap_or(0) > 0, "{churn_doc:?}");
    assert!(churn_rows[0].get("last_changed").and_then(|v| v.as_str()).is_some(), "{churn_doc:?}");

    let file_churn = Command::new(bin()).args(["--json", "churn", "py/a.py", "--project-dir"]).arg(&dir).arg("--storage-dir").arg(&store).output().unwrap();
    let file_rows = chisel::json::parse(String::from_utf8_lossy(&file_churn.stdout).trim()).unwrap();
    assert_eq!(file_rows.as_array().unwrap().len(), 1, "file churn should be the file-level row: {file_rows:?}");

    let coupling = Command::new(bin()).args(["--json", "coupling", "py/a.py", "--min-count", "1", "--project-dir"]).arg(&dir).arg("--storage-dir").arg(&store).output().unwrap();
    assert!(coupling.status.success(), "{}", String::from_utf8_lossy(&coupling.stderr));

    fs::write(dir.join("py/new_mod.py"), "import a\n\ndef fresh():\n    return a.ping()\n").unwrap();
    let disk = Command::new(bin()).args(["--json", "coupling", "py/new_mod.py", "--working-tree", "--min-count", "1", "--project-dir"]).arg(&dir).arg("--storage-dir").arg(&store).output().unwrap();
    assert!(disk.status.success(), "{}", String::from_utf8_lossy(&disk.stderr));
    let disk_doc = chisel::json::parse(String::from_utf8_lossy(&disk.stdout).trim()).unwrap();
    assert!(disk_doc.get("note").and_then(|v| v.as_str()).unwrap_or("").contains("working_tree"), "{disk_doc:?}");
    assert!(!disk_doc.get("import_partners").and_then(|v| v.as_array()).unwrap().is_empty(), "{disk_doc:?}");

    let recorded = Command::new(bin()).args(["--json", "record-result", "tests/test_a.py:test_ping", "--passed", "--duration", "15", "--project-dir"]).arg(&dir).arg("--storage-dir").arg(&store).output().unwrap();
    assert!(recorded.status.success(), "{}", String::from_utf8_lossy(&recorded.stderr));
    let after = fs::read_to_string(store.join("store")).unwrap();
    assert!(after.contains("\"duration_ms\":15") || after.contains("\"duration_ms\": 15"), "{after:?}");

    let job = Command::new(bin()).args(["--json", "start-job", "update", "--project-dir"]).arg(&dir).arg("--storage-dir").arg(&store).output().unwrap();
    assert!(job.status.success(), "{}", String::from_utf8_lossy(&job.stderr));
    assert!(String::from_utf8_lossy(&job.stdout).contains("queued"), "{}", String::from_utf8_lossy(&job.stdout));

    let stats = Command::new(bin()).args(["--json", "stats", "--project-dir"]).arg(&dir).arg("--storage-dir").arg(&store).output().unwrap();
    let stats_doc = chisel::json::parse(String::from_utf8_lossy(&stats.stdout).trim()).unwrap();
    assert!(matches!(stats_doc.get("co_change_query_min"), Some(chisel::json::Value::Int(_))), "{stats_doc:?}");
    assert!(stats_doc.get("shadow_graph").is_some(), "{stats_doc:?}");
    assert!(stats_doc.get("commit_files").is_some(), "{stats_doc:?}");
    assert!(stats_doc.get("churn_stats").is_some(), "{stats_doc:?}");

    let update = Command::new(bin()).args(["--json", "update", "--project-dir"]).arg(&dir).arg("--storage-dir").arg(&store).output().unwrap();
    let update_doc = chisel::json::parse(String::from_utf8_lossy(&update.stdout).trim()).unwrap();
    if update_doc.get("files_updated").and_then(|v| v.as_i64()) == Some(0) {
        assert_eq!(update_doc.get("code_units_found").and_then(|v| v.as_i64()), Some(0), "{update_doc:?}");
    }
    assert!(update_doc.get("orphaned_results_cleaned").is_some(), "{update_doc:?}");
    assert!(update_doc.get("active_file_locks").is_some(), "{update_doc:?}");

    let human = Command::new(bin()).args(["risk-map", "--project-dir"]).arg(&dir).arg("--storage-dir").arg(&store).output().unwrap();
    let human_text = String::from_utf8_lossy(&human.stdout);
    assert!(human_text.contains("Diagnostics"), "{human_text:?}");
    assert!(human_text.contains("Risk map:"), "{human_text:?}");

    let help = Command::new(bin()).args(["churn", "--help"]).output().unwrap();
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("--unit"), "{}", String::from_utf8_lossy(&help.stdout));
}

#[test]
fn locks_show_on_the_risk_map_and_can_be_filtered() {
    let dir = scratch("locks");
    write_sample(&dir);
    init_repo(&dir);
    commit_all(&dir, "init");
    let store = dir.join("store");
    assert!(Command::new(bin()).args(["--json", "analyze", "--project-dir"]).arg(&dir).arg("--storage-dir").arg(&store).status().unwrap().success());
    assert!(Command::new(bin()).args(["--json", "acquire-lock", "py/a.py", "agentX", "--project-dir"]).arg(&dir).arg("--storage-dir").arg(&store).status().unwrap().success());
    let risk = Command::new(bin()).args(["--json", "risk-map", "--project-dir"]).arg(&dir).arg("--storage-dir").arg(&store).output().unwrap();
    let doc = chisel::json::parse(String::from_utf8_lossy(&risk.stdout).trim()).unwrap();
    let locked = doc.get("files").and_then(|v| v.as_array()).unwrap().iter().find(|f| f.get("file_path").and_then(|v| v.as_str()) == Some("py/a.py")).unwrap();
    assert_eq!(locked.get("locked_by").and_then(|v| v.as_str()), Some("agentX"));
    assert!(doc.get("_meta").and_then(|m| m.get("active_locks")).and_then(|v| v.as_i64()).unwrap_or(0) >= 1);
    let listed = Command::new(bin()).args(["--json", "list-locks", "--agent-id", "agentX", "--project-dir"]).arg(&dir).arg("--storage-dir").arg(&store).output().unwrap();
    assert!(listed.status.success(), "{}", String::from_utf8_lossy(&listed.stderr));
    assert!(String::from_utf8_lossy(&listed.stdout).contains("agentX"));
    let other = Command::new(bin()).args(["--json", "list-locks", "--agent-id", "nobody", "--project-dir"]).arg(&dir).arg("--storage-dir").arg(&store).output().unwrap();
    let other_doc = chisel::json::parse(String::from_utf8_lossy(&other.stdout).trim()).unwrap();
    assert_eq!(other_doc.get("total").and_then(|v| v.as_i64()), Some(0), "{other_doc:?}");
}

#[test]
fn pytest_collection_noise_is_not_a_result_and_jest_paths_are_relative() {
    assert!(chisel::run_cmd::parse_pytest_line("_____________________ ERROR collecting tests/x.py _____________________").is_none());
    let row = chisel::run_cmd::parse_pytest_line("tests/x.py::test_add PASSED [ 50%]").unwrap();
    assert_eq!(row.0, "tests/x.py::test_add");
    assert!(row.1);
    let jest = r#"{"testResults":[{"name":"/tmp/sample/js/math.test.js","assertionResults":[{"title":"multiplies","status":"passed"}]}]}"#;
    let rows = chisel::run_cmd::parse_jest_json(jest, Some(Path::new("/tmp/sample")));
    assert_eq!(rows[0].0, "js/math.test.js:multiplies");
}

#[test]
fn legacy_sqlite_results_migrate_into_the_store() {
    let dir = scratch("migrate");
    fs::write(dir.join("a.py"), "def a():\n    return 1\n").unwrap();
    init_repo(&dir);
    commit_all(&dir, "init");
    let store = dir.join("store");
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/legacy/chisel.sqlite");
    let out = Command::new(bin()).args(["--json", "migrate", "--project-dir"]).arg(&dir).arg("--storage-dir").arg(&store).arg(&fixture).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let doc = chisel::json::parse(String::from_utf8_lossy(&out.stdout).trim()).unwrap();
    assert_eq!(doc.get("imported").and_then(|v| v.as_i64()), Some(3), "{doc:?}");
    let snap = fs::read_to_string(store.join("store")).unwrap();
    assert!(snap.contains("test_add"), "{snap:?}");
    assert!(snap.contains("duration_ms"), "{snap:?}");
    let again = Command::new(bin()).args(["--json", "migrate", "--project-dir"]).arg(&dir).arg("--storage-dir").arg(&store).arg(&fixture).output().unwrap();
    let second = chisel::json::parse(String::from_utf8_lossy(&again.stdout).trim()).unwrap();
    assert_eq!(second.get("imported").and_then(|v| v.as_i64()), Some(0), "{second:?}");
    assert_eq!(second.get("skipped").and_then(|v| v.as_i64()), Some(3), "{second:?}");
}

#[cfg(unix)]
#[test]
fn dep_extractor_hook_replaces_register_dep_extractor() {
    let dir = scratch("dep");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/app.py"), "def app():\n    return 1\n").unwrap();
    fs::create_dir_all(dir.join("tests")).unwrap();
    fs::write(dir.join("tests/test_app.py"), "def test_app():\n    return 1\n").unwrap();
    let hook = dir.join("dep-hook");
    fs::write(&hook, "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '[{\"name\":\"app\",\"dep_type\":\"call\",\"confidence\":1}]'\n").unwrap();
    let _ = Command::new("chmod").arg("+x").arg(&hook).status();
    init_repo(&dir);
    commit_all(&dir, "init");
    let store = dir.join("store");
    let out = Command::new(bin())
        .args(["--json", "analyze", "--project-dir"]).arg(&dir).arg("--storage-dir").arg(&store)
        .env("CHISEL_DEP_EXTRACTOR", &hook)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let snap = fs::read_to_string(store.join("store")).unwrap();
    assert!(snap.contains("src/app.py:app:function"), "custom dep extractor did not link app: {snap:?}");
}

#[test]
fn http_suggest_warns_on_eval() {
    let dir = scratch("eval");
    fs::write(dir.join("dyn.js"), "function make(){ return eval('1'); }\n").unwrap();
    init_repo(&dir);
    commit_all(&dir, "init");
    let store = dir.join("store");
    assert!(Command::new(bin()).args(["--json", "analyze", "--project-dir"]).arg(&dir).arg("--storage-dir").arg(&store).status().unwrap().success());
    let mut engine = chisel::Engine::open(&dir, Some(&store)).unwrap();
    let result = engine.dispatch("suggest_tests", &chisel::json::Value::object(vec![
        ("file_path".into(), chisel::json::Value::String("dyn.js".into())),
    ])).unwrap();
    let warning = chisel::next_steps::eval_warning(&dir, Some("dyn.js")).unwrap();
    assert_eq!(warning.get("action").and_then(|v| v.as_str()), Some("warn_eval_used"));
    let _ = result;
}
