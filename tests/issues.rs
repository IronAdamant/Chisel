//! Issue capture end to end: the `chisel` binary on a scripted git repo, fed test output the
//! way the PostToolUse hook feeds it.

use chisel::json::{self, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn scratch(name: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let p = std::env::temp_dir().join(format!("chisel-issues-{name}-{t}-{}", N.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git").args(args).current_dir(dir).env("GIT_AUTHOR_DATE", "2024-03-01T00:00:00+00:00").env("GIT_COMMITTER_DATE", "2024-03-01T00:00:00+00:00").status().unwrap().success();
    assert!(ok, "git {args:?}");
}

fn head(dir: &Path) -> String {
    String::from_utf8(Command::new("git").args(["rev-parse", "HEAD"]).current_dir(dir).output().unwrap().stdout).unwrap().trim().to_string()
}

fn repo(files: &[(&str, &str)]) -> PathBuf {
    let dir = scratch("repo");
    for (p, t) in files {
        let path = dir.join(p);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, t).unwrap();
    }
    git(&dir, &["init", "-q", "-b", "main"]);
    for (k, v) in [("user.email", "a@example.com"), ("user.name", "Ann"), ("commit.gpgsign", "false"), ("core.autocrlf", "false")] {
        git(&dir, &["config", k, v]);
    }
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-q", "-m", "init"]);
    chisel(&dir, &["analyze"], "");
    dir
}

/// Run `chisel ARGS` in `dir` with `stdin`; (exit code, stdout, stderr).
fn chisel_env(dir: &Path, args: &[&str], stdin: &str, env: &[(&str, &str)]) -> (i32, String, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_chisel"));
    cmd.args(args).arg("--project-dir").arg(dir).current_dir(dir).env_remove("CHISEL_PROJECT_DIR").env_remove("CHISEL_STORAGE_DIR");
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

fn chisel(dir: &Path, args: &[&str], stdin: &str) -> (i32, String, String) {
    chisel_env(dir, args, stdin, &[])
}

fn capture(dir: &Path, command: &str, output: &str) -> String {
    let (code, out, err) = chisel(dir, &["capture", "--command", command], output);
    assert_eq!(code, 0, "capture must always exit 0: {err}");
    out
}

fn issue(dir: &Path, id: &str) -> Value {
    json::parse(&chisel(dir, &["issue", id, "--json"], "").1).unwrap()
}

const CALC: &str = "from fmtx import fmt\n\n\ndef add(a, b):\n    return a + b\n\n\ndef sub(a, b):\n    return a - b\n";
const FMTX: &str = "def fmt(x):\n    return str(x)\n";
const TESTS: &str = "from calc import add, sub\nfrom fmtx import fmt\n\n\ndef test_add():\n    assert add(1, 2) == 3\n\n\ndef test_sub():\n    assert fmt(sub(5, 3)) == '2'\n";

const ALL_PASS_V: &str = "tests/test_calc.py::test_add PASSED                                    [ 50%]\ntests/test_calc.py::test_sub PASSED                                    [100%]\n\n============================== 2 passed in 0.02s ==============================\n";

fn failing(found: &str) -> String {
    format!(
        "tests/test_calc.py .F                                                   [100%]\n\n================================== FAILURES ===================================\n__________________________________ test_sub ___________________________________\n\n    def test_sub():\n>       assert fmt(sub(5, 3)) == '2'\nE       AssertionError: assert '{found}' == '2'\n\ntests/test_calc.py:10: AssertionError\n=========================== short test summary info ===========================\nFAILED tests/test_calc.py::test_sub - AssertionError: assert '{found}' == '2'\n========================= 1 failed, 1 passed in 0.31s =========================\n"
    )
}

#[test]
fn a_failure_becomes_an_issue_with_suspects_dedupes_closes_and_reopens() {
    let dir = repo(&[("calc.py", CALC), ("fmtx.py", FMTX), ("tests/test_calc.py", TESTS)]);
    let first = head(&dir);
    // A passing run records the commit each test last passed at, and prints nothing.
    assert_eq!(capture(&dir, "python -m pytest -v", ALL_PASS_V), "");

    // An uncommitted bug in sub: a new issue, one line, suspects ranked by the change.
    std::fs::write(dir.join("calc.py"), CALC.replace("return a - b", "return a + b")).unwrap();
    let line = capture(&dir, "python -m pytest", &failing("8"));
    assert_eq!(line.lines().count(), 1, "{line}");
    assert!(line.starts_with("chisel issues: new I1 tests/test_calc.py:test_sub (AssertionError: assert '8' == '2') suspect calc.py:sub."), "{line}");
    let i1 = issue(&dir, "I1");
    assert_eq!(i1.get("location").and_then(|v| v.as_str()), Some("tests/test_calc.py:10"));
    let suspects = i1.get("suspects").and_then(|v| v.as_array()).unwrap();
    let files: Vec<&str> = suspects.iter().map(|s| s.get("file").and_then(|v| v.as_str()).unwrap()).collect();
    assert_eq!(files, vec!["calc.py", "fmtx.py"], "{i1:?}");
    let reason = suspects[0].get("reason").and_then(|v| v.as_str()).unwrap();
    assert!(reason.contains(&format!("since last pass ({})", &first[..7])) && reason.contains("uncommitted") && reason.contains("changed sub, which the test calls"), "{reason}");
    assert!(suspects[1].get("reason").and_then(|v| v.as_str()).unwrap().starts_with("unchanged since last pass"));

    // The same failure with other numbers is the same issue, counted, and stays quiet.
    assert_eq!(capture(&dir, "python -m pytest", &failing("9")), "");
    assert_eq!(issue(&dir, "I1").get("count").and_then(|v| v.as_i64()), Some(2));

    // Fixed and committed; a default pytest run shows the file passing: closed, with the fix.
    std::fs::write(dir.join("calc.py"), CALC.replace("return a - b", "return (a - b)")).unwrap();
    git(&dir, &["commit", "-qam", "fix sub"]);
    let fixed_at = head(&dir);
    let line = capture(&dir, "python -m pytest", "tests/test_calc.py ..                                                   [100%]\n\n============================== 2 passed in 0.02s ==============================\n");
    assert_eq!(line.trim(), "chisel issues: fixed I1 tests/test_calc.py:test_sub. Details: chisel issue <id>.");
    let i1 = issue(&dir, "I1");
    assert_eq!(i1.get("status").and_then(|v| v.as_str()), Some("closed"));
    let fix = i1.get("fixed").unwrap();
    assert_eq!(fix.get("commit").and_then(|v| v.as_str()), Some(&fixed_at[..7]));
    assert_eq!(json::to_string(fix.get("files").unwrap()).unwrap(), r#"["calc.py +1 -1"]"#);

    // The same failure later reopens it.
    std::fs::write(dir.join("calc.py"), CALC.replace("return a - b", "return a + b")).unwrap();
    let line = capture(&dir, "python -m pytest", &failing("8"));
    assert!(line.contains("reopened I1 tests/test_calc.py:test_sub"), "{line}");
    assert_eq!(issue(&dir, "I1").get("reopened").and_then(|v| v.as_i64()), Some(1));

    // Compact listing and traceability.
    let list = json::parse(&chisel(&dir, &["issues", "--json"], "").1).unwrap();
    assert_eq!(list.get("open").and_then(|v| v.as_i64()), Some(1));
    let row = json::to_string(&list.get("items").and_then(|v| v.as_array()).unwrap()[0]).unwrap();
    assert!(row.len() <= 170, "{} bytes: {row}", row.len());
    assert!(row.contains(r#""suspect":"calc.py:sub""#), "{row}");
    let trace = json::parse(&chisel(&dir, &["trace", "calc.py", "--json"], "").1).unwrap();
    let rows = trace.get("issues").and_then(|v| v.as_array()).unwrap();
    assert!(rows[0].get("role").and_then(|v| v.as_str()).unwrap().contains("suspect"));
    assert!(rows[0].get("role").and_then(|v| v.as_str()).unwrap().contains("fixed_in"), "{rows:?}");
    let brief = chisel(&dir, &["issues", "--brief"], "").1;
    assert_eq!(brief.lines().count(), 2, "{brief}");
    assert!(brief.starts_with("chisel: 1 open test issue(s): I1 tests/test_calc.py:test_sub"), "{brief}");
}

#[test]
fn a_run_of_other_files_does_not_close_an_issue() {
    let dir = repo(&[("calc.py", CALC), ("fmtx.py", FMTX), ("tests/test_calc.py", TESTS), ("tests/test_other.py", "def test_other():\n    assert True\n")]);
    capture(&dir, "python -m pytest", &failing("8"));
    // Only another file ran and passed: no evidence about test_sub.
    let quiet = capture(&dir, "python -m pytest tests/test_other.py", "tests/test_other.py .                                                  [100%]\n\n============================== 1 passed in 0.01s ==============================\n");
    assert_eq!(quiet, "");
    assert_eq!(issue(&dir, "I1").get("status").and_then(|v| v.as_str()), Some("open"));
}

#[test]
fn cargo_failures_resolve_to_inline_rust_tests_and_their_code() {
    let dir = repo(&[
        ("Cargo.toml", "[package]\nname = \"sample_crate\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        ("src/lib.rs", "pub mod a;\n"),
        ("src/a.rs", "pub fn add_values(left: i32, right: i32) -> i32 {\n    left + right\n}\n\n#[cfg(test)]\nmod unit_checks {\n    use super::add_values;\n\n    #[test]\n    fn checks_add_values() {\n        assert_eq!(add_values(1, 1), 2);\n    }\n}\n"),
    ]);
    let out = "     Running unittests src\\lib.rs (target\\debug\\deps\\sample_crate-1.exe)\n\nrunning 1 test\ntest a::unit_checks::checks_add_values ... FAILED\n\nfailures:\n\n---- a::unit_checks::checks_add_values stdout ----\n\nthread 'a::unit_checks::checks_add_values' (4242) panicked at src\\a.rs:11:9:\nassertion `left == right` failed\n  left: 3\n right: 2\n\nfailures:\n    a::unit_checks::checks_add_values\n\ntest result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n";
    let line = capture(&dir, "cargo test", out);
    assert!(line.contains("new I1 src/a.rs:checks_add_values (assertion `left == right` failed) suspect src/a.rs:add_values"), "{line}");
}

#[test]
fn the_hook_is_silent_fast_and_always_exits_zero() {
    let dir = repo(&[("calc.py", CALC), ("fmtx.py", FMTX), ("tests/test_calc.py", TESTS)]);
    let payload = |cmd: &str, out: &str| {
        json::to_string(&Value::object(vec![
            ("tool_name".into(), Value::String("Bash".into())),
            ("cwd".into(), Value::String(dir.to_string_lossy().into_owned())),
            ("tool_input".into(), Value::object(vec![("command".into(), Value::String(cmd.into()))])),
            ("tool_response".into(), Value::object(vec![("stdout".into(), Value::String(out.into())), ("stderr".into(), Value::String(String::new()))])),
        ]))
        .unwrap()
    };
    // Not a test command, garbage, an empty payload: nothing printed, exit 0.
    for input in [payload("git status", "On branch main"), "{not json".to_string(), String::new()] {
        let (code, out, err) = chisel(&dir, &["capture", "--hook"], &input);
        assert_eq!((code, out.as_str(), err.as_str()), (0, "", ""), "{input}");
    }
    // A passing test run: silent.
    let (code, out, _) = chisel(&dir, &["capture", "--hook"], &payload("pytest -v", ALL_PASS_V));
    assert_eq!((code, out.as_str()), (0, ""));
    // A new failure: one JSON line the hook hands to Claude Code as additionalContext.
    let (code, out, _) = chisel(&dir, &["capture", "--hook"], &payload("pytest", &failing("8")));
    assert_eq!(code, 0);
    let doc = json::parse(out.trim()).unwrap();
    let ctx = doc.get("hookSpecificOutput").and_then(|h| h.get("additionalContext")).and_then(|v| v.as_str()).unwrap();
    assert!(ctx.starts_with("chisel issues: new I1 tests/test_calc.py:test_sub"), "{ctx}");
    assert_eq!(doc.get("hookSpecificOutput").and_then(|h| h.get("hookEventName")).and_then(|v| v.as_str()), Some("PostToolUse"));
    // Without a snapshot the hook does nothing at all.
    let bare = scratch("bare");
    let (code, out, _) = chisel(&bare, &["capture", "--hook"], &payload("pytest", &failing("8")));
    assert_eq!((code, out.as_str()), (0, ""));
    assert!(!bare.join(".chisel").exists(), "capture must not create a snapshot");

    // Speed: time spent inside capture, as reported by the binary itself.
    let timed = |cmd: &str, out: &str| -> Duration {
        let (_, _, err) = chisel_env(&dir, &["capture", "--hook"], &payload(cmd, out), &[("CHISEL_CAPTURE_TIMING", "1")]);
        let ms: u64 = err.trim().strip_prefix("chisel capture: ").and_then(|s| s.strip_suffix(" ms")).and_then(|s| s.parse().ok()).unwrap_or_else(|| panic!("{err}"));
        Duration::from_millis(ms)
    };
    let skip = timed("git status", "x");
    let pass = timed("pytest -v", ALL_PASS_V);
    let started = Instant::now();
    let _ = chisel(&dir, &["capture", "--hook"], &payload("ls", ""));
    let process = started.elapsed();
    eprintln!("capture timings: non-test {skip:?}, passing run {pass:?}, whole process {process:?}");
    let budget = if cfg!(debug_assertions) { 400 } else { 100 };
    assert!(skip < Duration::from_millis(20), "non-test command took {skip:?}");
    assert!(pass < Duration::from_millis(budget), "passing run took {pass:?}");
}

/// Child half of `chisel_run_writes_results_and_issues_in_one_flush`: a fake pytest.
#[test]
fn helper_fake_pytest() {
    if std::env::var_os("CHISEL_FAKE_PYTEST").is_none() {
        return;
    }
    println!("tests/test_calc.py::test_add PASSED");
    println!("tests/test_calc.py::test_sub FAILED");
    println!("FAILED tests/test_calc.py::test_sub - AssertionError: assert 8 == 2");
    std::process::exit(1);
}

fn generation(dir: &Path) -> i64 {
    let doc = json::parse(&std::fs::read_to_string(dir.join(".chisel/store")).unwrap()).unwrap();
    doc.get("generation").and_then(|v| v.as_i64()).unwrap()
}

#[test]
fn chisel_run_writes_results_and_issues_in_one_flush() {
    let dir = repo(&[("calc.py", CALC), ("fmtx.py", FMTX), ("tests/test_calc.py", TESTS)]);
    let before = generation(&dir);
    let me = std::env::current_exe().unwrap();
    let me = me.to_string_lossy().into_owned();
    let out = Command::new(env!("CARGO_BIN_EXE_chisel"))
        .arg("--project-dir")
        .arg(&dir)
        .args(["run", "--", &me, "helper_fake_pytest", "--exact", "--nocapture", "--test-threads=1"])
        .env("CHISEL_FAKE_PYTEST", "1")
        .env_remove("CHISEL_PROJECT_DIR")
        .env_remove("CHISEL_STORAGE_DIR")
        .output()
        .unwrap();
    let (code, out) = (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned());
    assert_eq!(code, 1, "the test command's exit code is forwarded: {out}");
    assert_eq!(generation(&dir), before + 1, "one snapshot flush for results and issues");
    assert!(out.contains("[chisel] Recorded 2 test result(s)") && out.contains("new I1 tests/test_calc.py:test_sub"), "{out}");
}
