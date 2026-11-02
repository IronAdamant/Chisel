//! Drive the shipped `chisel` binary against the installed Python 0.15.1 oracle.

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

fn run(bin: &str, dir: &Path, storage: &Path, args: &[&str]) -> String {
    let out = Command::new(bin)
        .args(args)
        .arg("--project-dir")
        .arg(dir)
        .arg("--storage-dir")
        .arg(storage)
        .arg("--json")
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "{bin} {args:?} stderr {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn compare_locks(dir: &Path, py_store: &Path, rs_store: &Path) {
    let py_acq = run("chisel", dir, py_store, &["acquire-lock", "src/calc.py", "agent-a"]);
    let rs_acq = run(bin(), dir, rs_store, &["acquire-lock", "src/calc.py", "agent-a"]);
    assert!(py_acq.contains("\"acquired\": true"), "{py_acq}");
    assert!(rs_acq.contains("\"acquired\": true"), "{rs_acq}");
    assert!(py_acq.contains("\"holder\": null") && rs_acq.contains("\"holder\": null"));
    let py_rel = run("chisel", dir, py_store, &["release-lock", "src/calc.py", "agent-a"]);
    let rs_rel = run(bin(), dir, rs_store, &["release-lock", "src/calc.py", "agent-a"]);
    fs::write(dir.join("py-rel.json"), &py_rel).unwrap();
    fs::write(dir.join("rs-rel.json"), &rs_rel).unwrap();
    json_equal(&dir.join("py-rel.json").to_string_lossy(), &dir.join("rs-rel.json").to_string_lossy());
    let py_chk = run("chisel", dir, py_store, &["check-lock", "src/calc.py"]);
    let rs_chk = run(bin(), dir, rs_store, &["check-lock", "src/calc.py"]);
    fs::write(dir.join("py-chk.json"), &py_chk).unwrap();
    fs::write(dir.join("rs-chk.json"), &rs_chk).unwrap();
    json_equal(&dir.join("py-chk.json").to_string_lossy(), &dir.join("rs-chk.json").to_string_lossy());
}

fn json_equal(py: &str, rs: &str) {
    let status = Command::new("python3")
        .args(["-c", "import json,sys; a=json.load(open(sys.argv[1])); b=json.load(open(sys.argv[2])); assert a==b, (a,b)"])
        .arg(py)
        .arg(rs)
        .status()
        .unwrap();
    assert!(status.success(), "json mismatch {py} vs {rs}");
}

#[test]
fn read_tools_match_python_oracle() {
    let dir = scratch("oracle");
    write_fixture(&dir);
    let py_store = dir.join("py-store");
    let rs_store = dir.join("rs-store");
    let py_a = run("chisel", &dir, &py_store, &["analyze", "."]);
    let rs_a = run(bin(), &dir, &rs_store, &["analyze", "."]);
    fs::write(dir.join("py-a.json"), &py_a).unwrap();
    fs::write(dir.join("rs-a.json"), &rs_a).unwrap();
    json_equal(&dir.join("py-a.json").to_string_lossy(), &dir.join("rs-a.json").to_string_lossy());

    let py_r = run("chisel", &dir, &py_store, &["risk-map"]);
    let rs_r = run(bin(), &dir, &rs_store, &["risk-map"]);
    fs::write(dir.join("py-r.json"), &py_r).unwrap();
    fs::write(dir.join("rs-r.json"), &rs_r).unwrap();
    json_equal(&dir.join("py-r.json").to_string_lossy(), &dir.join("rs-r.json").to_string_lossy());

    let py_s = run("chisel", &dir, &py_store, &["suggest-tests", "src/calc.py"]);
    let rs_s = run(bin(), &dir, &rs_store, &["suggest-tests", "src/calc.py"]);
    fs::write(dir.join("py-s.json"), &py_s).unwrap();
    fs::write(dir.join("rs-s.json"), &rs_s).unwrap();
    json_equal(&dir.join("py-s.json").to_string_lossy(), &dir.join("rs-s.json").to_string_lossy());

    for (label, args) in [
        ("own", vec!["ownership", "src/calc.py"]),
        ("who", vec!["who-reviews", "src/calc.py"]),
        ("coup", vec!["coupling", "src/extra.py"]),
        ("hist", vec!["history", "src/calc.py"]),
        ("gaps", vec!["test-gaps"]),
        ("triage", vec!["triage"]),
    ] {
        let py_out = run("chisel", &dir, &py_store, &args);
        let rs_out = run(bin(), &dir, &rs_store, &args);
        let py_path = dir.join(format!("py-{label}.json"));
        let rs_path = dir.join(format!("rs-{label}.json"));
        fs::write(&py_path, &py_out).unwrap();
        fs::write(&rs_path, &rs_out).unwrap();
        json_equal(&py_path.to_string_lossy(), &rs_path.to_string_lossy());
    }
    compare_locks(&dir, &py_store, &rs_store);

    let calc = dir.join("src/calc.py");
    let mut text = fs::read_to_string(&calc).unwrap();
    text = text.replace("return a - b", "return a - b  # changed");
    fs::write(&calc, text).unwrap();
    let py_d = run("chisel", &dir, &py_store, &["diff-impact"]);
    let rs_d = run(bin(), &dir, &rs_store, &["diff-impact"]);
    fs::write(dir.join("py-d.json"), &py_d).unwrap();
    fs::write(dir.join("rs-d.json"), &rs_d).unwrap();
    json_equal(&dir.join("py-d.json").to_string_lossy(), &dir.join("rs-d.json").to_string_lossy());

    let second = run(bin(), &dir, &rs_store, &["analyze", "."]);
    assert_eq!(rs_a, second, "two analyze runs must agree");
    assert!(rs_a.contains("code_files_scanned"));
}

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
    let out = Command::new(bin())
        .args(["run", "--project-dir"])
        .arg(&dir)
        .arg("--storage-dir")
        .arg(&store)
        .arg("--")
        .arg("pytest")
        .arg("tests/test_outcomes.py")
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
    assert!(text.contains("stale_tests"), "{text}");
}
