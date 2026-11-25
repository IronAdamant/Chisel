//! What a pass records about the code it ran against: `test_passes` keeps the commit (HEAD at
//! the time), the git tree of the working tree (`tree`, uncommitted and untracked work
//! included) and `dirty`. A test added since the last `analyze` is recorded under the id the
//! next `analyze` gives it.

use chisel::json::{self, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

fn scratch(name: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let p = std::env::temp_dir().join(format!("chisel-tree-{name}-{t}-{}", N.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git").args(args).current_dir(dir).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn write(dir: &Path, files: &[(&str, &str)]) {
    for (p, t) in files {
        let path = dir.join(p);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, t).unwrap();
    }
}

fn init(dir: &Path) {
    git_out(dir, &["init", "-q", "-b", "main"]);
    for (k, v) in [("user.email", "a@example.com"), ("user.name", "Ann"), ("commit.gpgsign", "false"), ("core.autocrlf", "false")] {
        git_out(dir, &["config", k, v]);
    }
}

fn commit(dir: &Path, message: &str) {
    git_out(dir, &["add", "-A", "--", ".", ":(exclude).chisel"]);
    git_out(dir, &["commit", "-q", "-m", message]);
}

fn chisel(dir: &Path, args: &[&str], stdin: &str) -> (i32, String, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_chisel"));
    cmd.args(args).arg("--project-dir").arg(dir).current_dir(dir).env_remove("CHISEL_PROJECT_DIR").env_remove("CHISEL_STORAGE_DIR");
    let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

fn store(dir: &Path) -> Value {
    json::parse(&std::fs::read_to_string(dir.join(".chisel/store")).unwrap()).unwrap()
}

fn pass(dir: &Path, id: &str) -> Value {
    store(dir).get("test_passes").and_then(|p| p.get(id)).cloned().unwrap_or_else(|| panic!("no test_passes entry for {id}"))
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(|x| x.as_str()).map(str::to_string)
}

const CARGO: &str = "[package]\nname = \"walk\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";
const LIB: &str = "pub fn depth(n: u32) -> u32 {\n    n + 1\n}\n\n#[cfg(test)]\nmod tests;\n";
const TESTS_MOD: &str = "mod recursive;\n";
const RECURSIVE: &str = "use crate::depth;\n\n#[test]\nfn old_walk() {\n    assert_eq!(depth(1), 2);\n}\n";
const NEW_TEST: &str = "\n#[test]\nfn new_walk() {\n    assert_eq!(depth(2), 3);\n}\n";

/// cargo test output as the hook sees it: stdout only, so no `Running` line names the crate root.
fn cargo_out(names: &[&str]) -> String {
    let mut out = format!("\nrunning {} tests\n", names.len());
    for n in names {
        out.push_str(&format!("test {n} ... ok\n"));
    }
    out.push_str(&format!("\ntest result: ok. {} passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n", names.len()));
    out
}

fn crate_repo() -> PathBuf {
    let dir = scratch("crate");
    write(&dir, &[("Cargo.toml", CARGO), ("src/lib.rs", LIB), ("src/tests/mod.rs", TESTS_MOD), ("src/tests/recursive.rs", RECURSIVE)]);
    init(&dir);
    commit(&dir, "init");
    let (code, _, err) = chisel(&dir, &["analyze"], "");
    assert_eq!(code, 0, "{err}");
    dir
}

#[test]
fn a_dirty_pass_records_the_tree_that_the_next_commit_has() {
    let dir = crate_repo();
    let before = git_out(&dir, &["rev-parse", "HEAD"]);
    std::fs::write(dir.join("src/lib.rs"), LIB.replace("n + 1", "1 + n")).unwrap();
    write(&dir, &[("notes/untracked.txt", "new file, not ignored\n")]);
    chisel(&dir, &["capture", "--command", "cargo test"], &cargo_out(&["tests::recursive::old_walk"]));
    let entry = pass(&dir, "src/tests/recursive.rs:old_walk");
    assert_eq!(text(&entry, "commit").as_deref(), Some(before.as_str()));
    assert_eq!(entry.get("dirty").and_then(|v| v.as_bool()), Some(true));
    let tree = text(&entry, "tree").expect("tree recorded");
    assert_ne!(tree, git_out(&dir, &["rev-parse", "HEAD^{tree}"]));
    // The user's index is untouched: nothing was staged by recording the tree.
    assert_eq!(git_out(&dir, &["diff", "--cached", "--name-only"]), "");
    commit(&dir, "the tested change");
    assert_eq!(git_out(&dir, &["rev-parse", "HEAD^{tree}"]), tree, "the tested tree is the committed tree");
}

#[test]
fn a_clean_pass_records_heads_tree_and_not_dirty() {
    let dir = crate_repo();
    let (code, out, err) = chisel(&dir, &["record-result", "src/tests/recursive.rs:old_walk", "passed", "--json"], "");
    assert_eq!(code, 0, "{out}{err}");
    let entry = pass(&dir, "src/tests/recursive.rs:old_walk");
    assert_eq!(text(&entry, "tree"), Some(git_out(&dir, &["rev-parse", "HEAD^{tree}"])));
    assert_eq!(entry.get("dirty").and_then(|v| v.as_bool()), Some(false));
    // The snapshot folder is not part of the tree, so writing it does not make the tree dirty.
    chisel(&dir, &["record-result", "src/tests/recursive.rs:old_walk", "passed"], "");
    assert_eq!(pass(&dir, "src/tests/recursive.rs:old_walk").get("dirty").and_then(|v| v.as_bool()), Some(false));
}

#[test]
fn no_tree_on_an_unborn_branch_or_outside_git() {
    let unborn = scratch("unborn");
    write(&unborn, &[("a.py", "def f():\n    return 1\n")]);
    init(&unborn);
    let (code, out, err) = chisel(&unborn, &["record-result", "tests/test_a.py:test_f", "passed"], "");
    assert_eq!(code, 0, "{out}{err}");
    let entries = store(&unborn).get("test_passes").cloned();
    if let Some(entry) = entries.as_ref().and_then(|p| p.get("tests/test_a.py:test_f")) {
        assert!(entry.get("tree").is_none(), "{}", json::to_string(entry).unwrap());
    }

    let plain = scratch("plain");
    write(&plain, &[("a.py", "def f():\n    return 1\n")]);
    let (code, out, err) = chisel(&plain, &["record-result", "tests/test_a.py:test_f", "passed"], "");
    assert_eq!(code, 0, "{out}{err}");
    let results = store(&plain).get("test_results").and_then(|v| v.as_array().map(|a| a.len())).unwrap_or(0);
    assert_eq!(results, 1, "the result is recorded even with no git");
    if let Some(entry) = store(&plain).get("test_passes").and_then(|p| p.get("tests/test_a.py:test_f")) {
        assert!(entry.get("tree").is_none());
    }
}

#[test]
fn a_test_added_since_analyze_is_recorded_under_the_id_analyze_gives_it() {
    let dir = crate_repo();
    std::fs::write(dir.join("src/tests/recursive.rs"), format!("{RECURSIVE}{NEW_TEST}")).unwrap();
    // As the hook sees it (no `Running` line) and as `chisel run` sees it (crate root named).
    chisel(&dir, &["capture", "--command", "cargo test"], &cargo_out(&["tests::recursive::new_walk"]));
    let named = format!("     Running unittests src/lib.rs (target/debug/deps/walk-1)\n{}", cargo_out(&["tests::recursive::new_walk", "tests::recursive::old_walk"]));
    chisel(&dir, &["capture", "--command", "cargo test"], &named);
    let (code, _, err) = chisel(&dir, &["update"], "");
    assert_eq!(code, 0, "{err}");
    let doc = store(&dir);
    let units: Vec<String> = doc.get("test_units").and_then(|v| v.as_array()).unwrap().iter().filter_map(|u| text(u, "id")).collect();
    assert!(units.iter().any(|u| u == "src/tests/recursive.rs:new_walk"), "{units:?}");
    let ids: Vec<String> = doc.get("test_results").and_then(|v| v.as_array()).unwrap().iter().filter_map(|r| text(r, "test_id")).collect();
    assert_eq!(ids.iter().filter(|i| *i == "src/tests/recursive.rs:new_walk").count(), 2, "{ids:?}");
    assert!(!ids.iter().any(|i| i.starts_with("tests:")), "{ids:?}");
    assert!(doc.get("test_passes").and_then(|p| p.get("src/tests/recursive.rs:new_walk")).is_some());
}
