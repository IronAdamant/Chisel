//! Drive the shipped `chisel` binary against JSON frozen from Python 0.15.1.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_chisel")
}

fn scratch(name: &str) -> std::path::PathBuf {
    let n = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let p = std::env::temp_dir().join(format!("chisel-{name}-{n}"));
    fs::create_dir_all(&p).unwrap();
    p
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git").args(args).current_dir(dir).status().unwrap();
    assert!(status.success(), "git {args:?}");
}

fn write_fixture(dir: &Path) {
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join("tests")).unwrap();
    fs::write(dir.join(".gitignore"), ".chisel/\n").unwrap();
    fs::write(
        dir.join("src/calc.py"),
        "def add(a, b):\n    return a + b\n\n\ndef sub(a, b):\n    return a - b\n",
    )
    .unwrap();
    fs::write(
        dir.join("src/extra.py"),
        "from src.calc import add\n\n\ndef wrapper():\n    return add(1, 1)\n",
    )
    .unwrap();
    fs::write(
        dir.join("tests/test_calc.py"),
        "from src.calc import add, sub\n\n\ndef test_add():\n    assert add(1, 1) == 2\n\n\ndef test_sub():\n    assert sub(2, 1) == 1\n",
    )
    .unwrap();
    git(dir, &["init", "-b", "main"]);
    git(dir, &["config", "user.email", "a@example.com"]);
    git(dir, &["config", "user.name", "Solo"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    git(dir, &["config", "core.autocrlf", "false"]);
    for (date, msg, paths) in [
        ("2024-01-15T12:00:00+00:00", "init", vec!["."]),
        ("2024-02-15T12:00:00+00:00", "touch", vec!["src/calc.py", "src/extra.py"]),
    ] {
        if msg == "touch" {
            let p = dir.join("src/calc.py");
            let mut t = fs::read_to_string(&p).unwrap();
            t.push('\n');
            fs::write(&p, t).unwrap();
            let p = dir.join("src/extra.py");
            let mut t = fs::read_to_string(&p).unwrap();
            t.push('\n');
            fs::write(&p, t).unwrap();
        }
        git(dir, &["add", "src/calc.py", "src/extra.py", "tests/test_calc.py", ".gitignore"]);
        let status = Command::new("git")
            .args(["commit", "-m", msg])
            .env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date)
            .current_dir(dir)
            .status()
            .unwrap();
        assert!(status.success());
        let _ = paths;
    }
}

fn oracle_now() -> &'static str {
    include_str!("fixtures/oracle/clock.txt").trim()
}

fn frozen(name: &str) -> chisel::json::Value {
    let text = match name {
        "analyze" => include_str!("fixtures/oracle/calc-analyze.json"),
        "risk-map" => include_str!("fixtures/oracle/calc-risk-map.json"),
        "suggest-tests" => include_str!("fixtures/oracle/calc-suggest-tests.json"),
        "ownership" => include_str!("fixtures/oracle/calc-ownership.json"),
        "who-reviews" => include_str!("fixtures/oracle/calc-who-reviews.json"),
        "coupling" => include_str!("fixtures/oracle/calc-coupling.json"),
        "history" => include_str!("fixtures/oracle/calc-history.json"),
        "test-gaps" => include_str!("fixtures/oracle/calc-test-gaps.json"),
        "triage" => include_str!("fixtures/oracle/calc-triage.json"),
        "release-lock" => include_str!("fixtures/oracle/calc-release-lock.json"),
        "check-lock" => include_str!("fixtures/oracle/calc-check-lock.json"),
        "diff-impact" => include_str!("fixtures/oracle/calc-diff-impact.json"),
        other => panic!("unknown fixture {other}"),
    };
    chisel::json::parse(text).unwrap()
}

fn run(bin: &str, dir: &Path, storage: &Path, args: &[&str]) -> String {
    let out = Command::new(bin)
        .args(args)
        .arg("--project-dir")
        .arg(dir)
        .arg("--storage-dir")
        .arg(storage)
        .arg("--json")
        .env("CHISEL_NOW", oracle_now())
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "{bin} {args:?} stderr {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn expect(actual: &str, fixture: &str) {
    let got = chisel::json::parse(actual.trim()).unwrap();
    let want = frozen(fixture);
    assert_eq!(got, want, "{fixture} mismatch\n{actual}");
}

#[test]
fn read_tools_match_python_oracle() {
    let dir = scratch("oracle");
    write_fixture(&dir);
    let store = dir.join("store");
    let analyze = run(bin(), &dir, &store, &["analyze", "."]);
    expect(&analyze, "analyze");
    expect(&run(bin(), &dir, &store, &["risk-map"]), "risk-map");
    expect(&run(bin(), &dir, &store, &["suggest-tests", "src/calc.py"]), "suggest-tests");
    expect(&run(bin(), &dir, &store, &["ownership", "src/calc.py"]), "ownership");
    expect(&run(bin(), &dir, &store, &["who-reviews", "src/calc.py"]), "who-reviews");
    expect(&run(bin(), &dir, &store, &["coupling", "src/extra.py"]), "coupling");
    expect(&run(bin(), &dir, &store, &["history", "src/calc.py"]), "history");
    expect(&run(bin(), &dir, &store, &["test-gaps"]), "test-gaps");
    expect(&run(bin(), &dir, &store, &["triage"]), "triage");
    let acquired = run(bin(), &dir, &store, &["acquire-lock", "src/calc.py", "agent-a"]);
    assert!(acquired.contains("\"acquired\": true"), "{acquired}");
    assert!(acquired.contains("\"holder\": null"), "{acquired}");
    expect(&run(bin(), &dir, &store, &["release-lock", "src/calc.py", "agent-a"]), "release-lock");
    expect(&run(bin(), &dir, &store, &["check-lock", "src/calc.py"]), "check-lock");
    let calc = dir.join("src/calc.py");
    let mut text = fs::read_to_string(&calc).unwrap();
    text = text.replace("return a - b", "return a - b  # changed");
    fs::write(&calc, text).unwrap();
    expect(&run(bin(), &dir, &store, &["diff-impact"]), "diff-impact");
    let second = run(bin(), &dir, &store, &["analyze", "."]);
    assert_eq!(analyze, second, "two analyze runs must agree");
    assert!(analyze.contains("code_files_scanned"));
}

#[cfg(unix)]
#[test]
fn run_records_pytest_and_forwards_exit_code() {
    let dir = scratch("run");
    write_fixture(&dir);
    let store = dir.join("store");
    let _ = run(bin(), &dir, &store, &["analyze", "."]);
    fs::write(
        dir.join("tests/test_outcomes.py"),
        "def test_ok():\n    assert 1 == 1\n\ndef test_bad():\n    assert 1 == 0\n",
    )
    .unwrap();
    let pytest = dir.join("pytest");
    fs::write(
        &pytest,
        "#!/bin/sh\necho 'tests/test_outcomes.py::test_ok PASSED'\necho 'tests/test_outcomes.py::test_bad FAILED'\nexit 1\n",
    )
    .unwrap();
    let _ = Command::new("chmod").arg("+x").arg(&pytest).status();
    let out = Command::new(bin())
        .args(["run", "--project-dir"])
        .arg(&dir)
        .arg("--storage-dir")
        .arg(&store)
        .arg("--")
        .arg(&pytest)
        .current_dir(&dir)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "stderr {}", String::from_utf8_lossy(&out.stderr));
    let stats = run(bin(), &dir, &store, &["stats"]);
    assert!(stats.contains("\"passed\": 1"), "{stats}");
    assert!(stats.contains("\"failed\": 1"), "{stats}");
    assert!(!stats.contains("recorded_results"), "{stats}");
    let snap = fs::read_to_string(store.join("store")).unwrap();
    assert!(snap.contains("test_ok"), "{snap}");
    assert!(snap.contains("test_bad"), "{snap}");
}

#[test]
fn git_error_exits_1() {
    let dir = scratch("notgit");
    fs::write(dir.join("a.py"), "def f():\n    return 1\n").unwrap();
    let out = Command::new(bin())
        .args(["diff-impact", "--json", "--project-dir"])
        .arg(&dir)
        .arg("--storage-dir")
        .arg(dir.join("store"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("git_error"), "{text}");
}

#[test]
fn deleting_a_code_unit_leaves_stale_edges() {
    let dir = scratch("stale");
    write_fixture(&dir);
    let store = dir.join("store");
    let mut engine = chisel::Engine::open(&dir, Some(&store)).unwrap();
    engine.analyze().unwrap();
    let id = "src/calc.py:add:function".to_string();
    engine.delete_code_unit(&id).unwrap();
    let stale = engine.stale_tests().unwrap();
    let text = chisel::json::to_string(&stale).unwrap();
    assert!(text.contains("src/calc.py:add:function"), "{text}");
    assert!(text.contains("test_add"), "{text}");
}
