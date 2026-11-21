//! Which project a process answers for, calls that never block for minutes, and the test
//! command diff_impact hands back for Python. From an A/B run where `serve-mcp` started in a
//! folder of ~50 repositories (not itself one), indexed all of it, and `analyze` hung for the
//! client's 30-minute timeout.

use chisel::json::{self, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn scratch(name: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let p = std::env::temp_dir().join(format!("chisel-root-{name}-{t}-{}", N.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(&p).unwrap();
    std::fs::canonicalize(&p).unwrap()
}

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(["-c", "user.email=t@t", "-c", "user.name=t", "-c", "core.autocrlf=false", "-c", "init.defaultBranch=main"])
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_DATE", "2024-03-01T00:00:00+00:00")
        .env("GIT_COMMITTER_DATE", "2024-03-01T00:00:00+00:00")
        .stdout(Stdio::null())
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

/// A git repository with one Python module and a unittest test of it, committed.
fn repo(dir: &Path) {
    std::fs::create_dir_all(dir.join("calc")).unwrap();
    std::fs::create_dir_all(dir.join("tests")).unwrap();
    std::fs::write(dir.join("calc/__init__.py"), "").unwrap();
    std::fs::write(dir.join("calc/ops.py"), "def add(a, b):\n    return a + b\n\n\ndef sub(a, b):\n    return a - b\n").unwrap();
    std::fs::write(dir.join("tests/__init__.py"), "").unwrap();
    std::fs::write(
        dir.join("tests/test_ops.py"),
        "import unittest\n\nfrom calc.ops import add, sub\n\n\nclass TestOps(unittest.TestCase):\n    def test_add(self):\n        self.assertEqual(add(1, 2), 3)\n\n    def test_sub(self):\n        self.assertEqual(sub(3, 2), 1)\n\n\nif __name__ == \"__main__\":\n    unittest.main()\n",
    )
    .unwrap();
    git(dir, &["init", "-q"]);
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "init"]);
}

/// A stdio MCP server started in `cwd` with no CHISEL_PROJECT_DIR.
struct Server {
    child: Child,
    stdin: ChildStdin,
    out: BufReader<std::process::ChildStdout>,
    next: i64,
}

impl Server {
    fn start(cwd: &Path, vars: &[(&str, &str)]) -> Server {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_chisel"));
        cmd.arg("serve-mcp").current_dir(cwd);
        for k in ["CHISEL_PROJECT_DIR", "CHISEL_STORAGE_DIR", "CHISEL_TOOLS", "CHISEL_DISABLE", "CHISEL_MCP_WAIT_SECS", "CHISEL_JOB_PACE_MS"] {
            cmd.env_remove(k);
        }
        for (k, v) in vars {
            cmd.env(k, v);
        }
        let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
        let stdin = child.stdin.take().unwrap();
        let out = BufReader::new(child.stdout.take().unwrap());
        let mut server = Server { child, stdin, out, next: 1 };
        server.request("initialize", r#"{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t","version":"0"}}"#);
        server
    }

    fn request(&mut self, method: &str, params: &str) -> Value {
        let id = self.next;
        self.next += 1;
        writeln!(self.stdin, r#"{{"jsonrpc":"2.0","id":{id},"method":"{method}","params":{params}}}"#).unwrap();
        self.stdin.flush().unwrap();
        let mut line = String::new();
        self.out.read_line(&mut line).unwrap();
        json::parse(line.trim_end()).unwrap_or_else(|_| panic!("not JSON: {line}"))
    }

    /// `(isError, the tool's reply)`: the `result` object for a success, the error body otherwise.
    fn call(&mut self, tool: &str, args: &str) -> (bool, Value) {
        let reply = self.request("tools/call", &format!(r#"{{"name":"{tool}","arguments":{args}}}"#));
        let result = reply.get("result").unwrap();
        let is_error = result.get("isError").and_then(|v| v.as_bool()).unwrap_or(false);
        let text = result.get("content").and_then(|c| c.as_array()).and_then(|a| a.first()).and_then(|c| c.get("text")).and_then(|t| t.as_str()).unwrap().to_string();
        let doc = json::parse(&text).unwrap();
        let body = if is_error { doc } else { doc.get("result").cloned().unwrap() };
        (is_error, body)
    }

    fn instructions(&mut self) -> String {
        let reply = self.request("initialize", "{}");
        reply.get("result").and_then(|r| r.get("instructions")).and_then(|v| v.as_str()).unwrap().to_string()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("")
}

fn arg_path(p: &Path) -> String {
    json::to_string(&Value::String(p.to_string_lossy().into_owned())).unwrap()
}

#[test]
fn outside_any_repository_the_server_refuses_at_once_and_scans_nothing() {
    // An umbrella folder (not a repository) holding repositories, like ~/coding_projects.
    let umbrella = scratch("umbrella");
    repo(&umbrella.join("a"));
    repo(&umbrella.join("b"));
    let mut server = Server::start(&umbrella, &[]);
    assert!(server.instructions().contains("not inside a git repository"));
    let started = Instant::now();
    let (is_error, body) = server.call("analyze", "{}");
    assert!(started.elapsed() < Duration::from_secs(5), "took {:?}", started.elapsed());
    assert!(is_error, "{body:?}");
    assert_eq!(s(&body, "status"), "error");
    let message = s(&body, "message");
    assert!(message.contains("CHISEL_PROJECT_DIR") && message.contains("project_dir") && message.contains("not inside a git repository"), "{message}");
    for tool in ["diff_impact", "stats", "update"] {
        let (is_error, body) = server.call(tool, "{}");
        assert!(is_error && s(&body, "message").contains("CHISEL_PROJECT_DIR"), "{tool}: {body:?}");
    }
    assert!(!umbrella.join(".chisel").exists(), "a store was written in the umbrella folder");
    assert!(!umbrella.join("a/.chisel").exists() && !umbrella.join("b/.chisel").exists());
}

#[test]
fn one_server_answers_for_each_repository_a_call_names() {
    let umbrella = scratch("multi");
    let (a, b) = (umbrella.join("a"), umbrella.join("b"));
    repo(&a);
    repo(&b);
    let mut server = Server::start(&umbrella, &[]);
    let (is_error, body) = server.call("analyze", &format!(r#"{{"project_dir":{}}}"#, arg_path(&a)));
    assert!(!is_error, "{body:?}");
    assert!(body.get("code_files_scanned").and_then(|v| v.as_i64()).unwrap_or(0) >= 2, "{body:?}");
    // A folder inside a repository answers for its git root.
    let (is_error, body) = server.call("analyze", &format!(r#"{{"project_dir":{}}}"#, arg_path(&b.join("calc"))));
    assert!(!is_error, "{body:?}");
    assert!(a.join(".chisel/store").is_file() && b.join(".chisel/store").is_file());
    assert!(!b.join("calc/.chisel").exists() && !umbrella.join(".chisel").exists());
    let (is_error, body) = server.call("stats", &format!(r#"{{"project_dir":{}}}"#, arg_path(&b)));
    assert!(!is_error && body.get("code_units").and_then(|v| v.as_i64()).unwrap_or(0) > 0, "{body:?}");
    let (is_error, body) = server.call("stats", r#"{"project_dir":"relative/path"}"#);
    assert!(is_error && s(&body, "message").contains("absolute"), "{body:?}");
    let (is_error, body) = server.call("stats", &format!(r#"{{"project_dir":{}}}"#, arg_path(&umbrella.join("missing"))));
    assert!(is_error && s(&body, "message").contains("not a directory"), "{body:?}");
}

#[test]
fn started_in_a_subfolder_the_project_is_the_git_root() {
    let dir = scratch("sub");
    repo(&dir);
    let mut server = Server::start(&dir.join("calc"), &[]);
    let (is_error, body) = server.call("analyze", "{}");
    assert!(!is_error, "{body:?}");
    assert!(dir.join(".chisel/store").is_file());
    assert!(!dir.join("calc/.chisel").exists());
    drop(server);
    // The CLI resolves the same way, and refuses outside a repository.
    let out = Command::new(env!("CARGO_BIN_EXE_chisel")).args(["stats", "--json"]).current_dir(dir.join("tests")).env_remove("CHISEL_PROJECT_DIR").env_remove("CHISEL_STORAGE_DIR").output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(!dir.join("tests/.chisel").exists());
    let outside = scratch("cli-outside");
    let out = Command::new(env!("CARGO_BIN_EXE_chisel")).args(["analyze", "--json"]).current_dir(&outside).env_remove("CHISEL_PROJECT_DIR").env_remove("CHISEL_STORAGE_DIR").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let doc = json::parse(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert!(s(&doc, "status") == "error" && s(&doc, "message").contains("--project-dir"), "{doc:?}");
    assert!(!outside.join(".chisel").exists());
    // An explicit --project-dir is used as given, repository or not.
    let out = Command::new(env!("CARGO_BIN_EXE_chisel")).args(["stats", "--json", "--project-dir"]).arg(&outside).current_dir(&outside).env_remove("CHISEL_PROJECT_DIR").env_remove("CHISEL_STORAGE_DIR").output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn a_long_analyze_answers_within_the_budget_and_job_status_waits_for_it() {
    let dir = scratch("budget");
    repo(&dir);
    // The job sleeps 2.5 s before indexing; each call may take 1 s.
    let mut server = Server::start(&dir, &[("CHISEL_MCP_WAIT_SECS", "1"), ("CHISEL_JOB_PACE_MS", "2500")]);
    let started = Instant::now();
    let (is_error, body) = server.call("analyze", "{}");
    assert!(started.elapsed() < Duration::from_secs(3), "took {:?}", started.elapsed());
    assert!(!is_error, "{body:?}");
    assert_eq!(s(&body, "status"), "auto_queued", "{body:?}");
    assert!(s(&body, "message").contains("Still indexing"), "{body:?}");
    let id = s(&body, "job_id").to_string();
    assert!(id.starts_with("job-"), "{body:?}");
    // A second analyze while it runs waits on the same job instead of starting another.
    let (_, again) = server.call("analyze", "{}");
    if s(&again, "status") == "auto_queued" {
        assert_eq!(s(&again, "job_id"), id, "{again:?}");
    }
    let mut done = None;
    for _ in 0..20 {
        let polled = Instant::now();
        let (_, status) = server.call("job_status", &format!(r#"{{"job_id":"{id}"}}"#));
        assert!(polled.elapsed() < Duration::from_secs(3), "job_status took {:?}", polled.elapsed());
        match s(&status, "status") {
            "queued" | "running" => continue,
            _ => {
                done = Some(status);
                break;
            }
        }
    }
    let done = done.expect("job never finished");
    assert_eq!(s(&done, "status"), "completed", "{done:?}");
    assert!(done.get("result").and_then(|r| r.get("code_files_scanned")).is_some(), "{done:?}");
    // The server's own reads see the job's snapshot.
    let (is_error, stats) = server.call("stats", "{}");
    assert!(!is_error && stats.get("code_units").and_then(|v| v.as_i64()).unwrap_or(0) > 0, "{stats:?}");
}

#[test]
fn a_quick_analyze_still_answers_in_one_call() {
    let dir = scratch("quick");
    repo(&dir);
    let mut server = Server::start(&dir, &[]);
    let (is_error, body) = server.call("analyze", "{}");
    assert!(!is_error, "{body:?}");
    assert!(body.get("code_files_scanned").is_some(), "{body:?}");
    let (is_error, body) = server.call("update", "{}");
    assert!(!is_error && body.get("files_updated").is_some(), "{body:?}");
}

fn chisel_json(dir: &Path, args: &[&str]) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_chisel"))
        .args(args)
        .arg("--json")
        .current_dir(dir)
        .env_remove("CHISEL_PROJECT_DIR")
        .env_remove("CHISEL_STORAGE_DIR")
        .env("CHISEL_PYTEST_PROBE", "0")
        .output()
        .unwrap();
    assert!(out.status.success(), "chisel {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    json::parse(&String::from_utf8_lossy(&out.stdout)).unwrap()
}

fn run_lines(v: &Value) -> Vec<String> {
    v.get("run").and_then(|r| r.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
}

#[test]
fn diff_impact_suggests_unittest_without_pytest_and_pytest_with_its_config() {
    let dir = scratch("runner");
    repo(&dir);
    chisel_json(&dir, &["analyze"]);
    std::fs::write(dir.join("calc/ops.py"), "def add(a, b):\n    return b + a\n\n\ndef sub(a, b):\n    return a - b\n").unwrap();
    let reply = chisel_json(&dir, &["diff-impact"]);
    let run = run_lines(&reply);
    let python = if cfg!(windows) { "python" } else { "python3" };
    assert_eq!(run.len(), 1, "{reply:?}");
    assert!(run[0].starts_with(&format!("{python} -m unittest ")) && run[0].contains(" tests.test_ops.TestOps.test_add"), "{run:?}");
    assert!(!run[0].contains("pytest") && !run[0].contains(".py"), "{run:?}");
    // pytest configured: node ids.
    std::fs::write(dir.join("pyproject.toml"), "[project]\nname = \"calc\"\n\n[tool.pytest.ini_options]\ntestpaths = [\"tests\"]\n").unwrap();
    let reply = chisel_json(&dir, &["diff-impact"]);
    let run = run_lines(&reply);
    assert!(run.len() == 1 && run[0].starts_with("pytest ") && run[0].contains("tests/test_ops.py::TestOps::test_add"), "{run:?}");
    std::fs::remove_file(dir.join("pyproject.toml")).unwrap();
    std::fs::write(dir.join("setup.cfg"), "[metadata]\nname = calc\n\n[tool:pytest]\naddopts = -q\n").unwrap();
    assert!(run_lines(&chisel_json(&dir, &["diff-impact"]))[0].starts_with("pytest "));
    std::fs::remove_file(dir.join("setup.cfg")).unwrap();
    std::fs::write(dir.join("tests/conftest.py"), "").unwrap();
    assert!(run_lines(&chisel_json(&dir, &["diff-impact"]))[0].starts_with("pytest "));
}

#[test]
fn diff_impact_indexes_a_new_file_first_instead_of_answering_stale() {
    let dir = scratch("refresh");
    repo(&dir);
    chisel_json(&dir, &["analyze"]);
    std::fs::write(dir.join("calc/mul.py"), "def mul(a, b):\n    return a * b\n").unwrap();
    std::fs::write(dir.join("tests/test_mul.py"), "import unittest\n\nfrom calc.mul import mul\n\n\nclass TestMul(unittest.TestCase):\n    def test_mul(self):\n        self.assertEqual(mul(2, 3), 6)\n").unwrap();
    let stale = chisel_json(&dir, &["diff-impact", "--no-auto-update"]);
    assert_eq!(s(&stale, "status"), "stale_db", "{stale:?}");
    let reply = chisel_json(&dir, &["diff-impact"]);
    assert_ne!(s(&reply, "status"), "stale_db", "{reply:?}");
    let updated = json::to_string(reply.get("auto_updated").unwrap()).unwrap();
    assert!(updated.contains("calc/mul.py") && updated.contains("tests/test_mul.py"), "{updated}");
    let text = json::to_string(&reply).unwrap();
    assert!(text.contains("TestMul.test_mul"), "{text}");
}
