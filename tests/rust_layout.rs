//! Cargo-layout recognition and a dogfood check on this repository.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

fn scratch(name: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let p = std::env::temp_dir().join(format!("chisel-rust-{name}-{t}-{n}"));
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
    git(dir, &["config", "core.autocrlf", "false"]);
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

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        let kind = entry.file_type().unwrap();
        if kind.is_dir() {
            copy_tree(&entry.path(), &dest);
        } else {
            fs::copy(entry.path(), dest).unwrap();
        }
    }
}

fn names_in(doc: &chisel::json::Value, key: &str) -> Vec<String> {
    doc.get(key)
        .and_then(|v| v.as_array())
        .unwrap_or(&[])
        .iter()
        .filter_map(|row| row.get("name").and_then(|v| v.as_str()).map(|s| s.to_string()))
        .collect()
}

fn paths_in(doc: &chisel::json::Value, key: &str) -> Vec<String> {
    doc.get(key)
        .and_then(|v| v.as_array())
        .unwrap_or(&[])
        .iter()
        .filter_map(|row| row.get("file_path").and_then(|v| v.as_str()).map(|s| s.to_string()))
        .collect()
}

#[test]
fn cargo_layout_links_tests_and_skips_them_as_code() {
    let dir = scratch("crate");
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rust_crate");
    copy_tree(&fixture, &dir);
    init_repo(&dir);
    commit_all(&dir, "init");
    let store = dir.join("store");
    let mut engine = chisel::Engine::open(&dir, Some(&store)).unwrap();
    let summary = engine.analyze().unwrap();
    let summary_text = chisel::json::to_string(&summary).unwrap();
    assert!(summary_text.contains("\"test_files_found\":2") || summary_text.contains("\"test_files_found\": 2"), "{summary_text}");
    let snap = chisel::json::parse(&fs::read_to_string(store.join("store")).unwrap()).unwrap();
    let test_names = names_in(&snap, "test_units");
    assert!(test_names.iter().any(|n| n == "checks_add_values"), "{test_names:?}");
    assert!(test_names.iter().any(|n| n == "checks_add_values_from_crate_tests"), "{test_names:?}");
    assert!(test_names.iter().any(|n| n == "checks_add_values_async"), "{test_names:?}");
    assert!(test_names.iter().any(|n| n == "checks_add_values_cases"), "{test_names:?}");
    assert!(!test_names.iter().any(|n| n == "helper_not_a_test"), "{test_names:?}");
    assert!(!test_names.iter().any(|n| n == "add_values"), "{test_names:?}");

    let suggested = engine.suggest_tests("src/a.rs").unwrap();
    let suggested_text = chisel::json::to_string(&suggested).unwrap();
    assert!(suggested_text.contains("tests/a.rs"), "{suggested_text}");
    assert!(suggested_text.contains("checks_add_values"), "{suggested_text}");

    fs::write(
        dir.join("src/a.rs"),
        fs::read_to_string(dir.join("src/a.rs")).unwrap() + "\n// touched\n",
    )
    .unwrap();
    let impact = engine.diff_impact().unwrap();
    let impact_text = chisel::json::to_string(&impact).unwrap();
    assert!(impact_text.contains("tests/a.rs"), "{impact_text}");

    let gaps = engine.dispatch("test_gaps", &chisel::json::Value::object(vec![])).unwrap();
    let gap_list = gaps.as_array().unwrap_or(&[]);
    assert!(gap_list.iter().all(|row| {
        let path = row.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
        let name = row.get("name").and_then(|v| v.as_str()).unwrap_or("");
        !path.starts_with("tests/") && name != "checks_add_values" && name != "helper_not_a_test"
    }), "{gap_list:?}");
    assert!(gap_list.iter().any(|row| row.get("name").and_then(|v| v.as_str()) == Some("unused_root")), "{gap_list:?}");

    let risk = engine.risk_map().unwrap();
    let risk_paths = paths_in(&risk, "files");
    assert!(risk_paths.iter().all(|path| !path.starts_with("tests/")), "{risk_paths:?}");
    assert!(risk_paths.iter().any(|path| path == "src/a.rs"), "{risk_paths:?}");
}

#[test]
fn empty_project_hint_names_the_rust_tests_directory() {
    let dir = scratch("hint");
    fs::write(dir.join("lib.rs"), "pub fn value() -> i32 { 1 }\n").unwrap();
    init_repo(&dir);
    commit_all(&dir, "init");
    let mut engine = chisel::Engine::open(&dir, Some(&dir.join("store"))).unwrap();
    let summary = chisel::json::to_string(&engine.analyze().unwrap()).unwrap();
    assert!(summary.contains("tests/*.rs"), "{summary}");
    assert!(summary.contains("#[test]"), "{summary}");
    assert!(summary.contains("test_*.py") && summary.contains("*.spec.ts"), "{summary}");
}

#[test]
fn this_repo_suggests_a_rust_test_for_analyze() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let store = scratch("dogfood");
    let mut engine = chisel::Engine::open(&root, Some(&store)).unwrap();
    engine.analyze().unwrap();
    let suggested = engine.suggest_tests("src/engine/analyze.rs").unwrap();
    let rows = suggested.as_array().expect("suggest-tests list");
    assert!(rows.iter().any(|row| {
        let path = row.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
        let rest = path.strip_prefix("tests/").unwrap_or("");
        rest.ends_with(".rs") && !rest.contains('/')
    }), "{suggested:?}");
    let risk = engine.risk_map().unwrap();
    for path in paths_in(&risk, "files") {
        assert!(!path.starts_with("tests/"), "risk-map listed a test path as code: {path}");
    }
    let gaps = engine.dispatch("test_gaps", &chisel::json::Value::object(vec![])).unwrap();
    for row in gaps.as_array().unwrap_or(&[]) {
        let path = row.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
        assert!(!path.starts_with("tests/"), "test-gaps listed a test path as code: {path}");
    }
}
