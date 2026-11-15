//! `diff_impact` on real-sized diffs (0.24.0): names a test calls that are not project
//! functions, a hunk inside a test, ranking by breadth, always-run rules and source-reading
//! guards, the run command and the grouped summary, and what results remember. One small
//! Cargo package, driven through the binary.

use chisel::json::{self, Value};
use chisel::store::Store;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

fn scratch(name: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let p = std::env::temp_dir().join(format!("chisel-sel-{name}-{t}-{}", N.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(["-c", "user.email=t@t", "-c", "user.name=t", "-c", "core.autocrlf=false"])
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_DATE", "2024-03-01T00:00:00+00:00")
        .env("GIT_COMMITTER_DATE", "2024-03-01T00:00:00+00:00")
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

fn chisel(dir: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_chisel"))
        .args(args)
        .arg("--project-dir")
        .arg(dir)
        .current_dir(dir)
        .env_remove("CHISEL_PROJECT_DIR")
        .env_remove("CHISEL_STORAGE_DIR")
        .output()
        .unwrap();
    assert!(out.status.code().is_some_and(|c| c == 0), "chisel {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn diff_impact(dir: &Path, extra: &[&str]) -> Value {
    let mut args = vec!["diff-impact", "--json"];
    args.extend_from_slice(extra);
    json::parse(&chisel(dir, &args)).unwrap()
}

fn items(v: &Value) -> Vec<Value> {
    v.get("items").and_then(|i| i.as_array()).or_else(|| v.as_array()).map(|a| a.to_vec()).unwrap_or_else(|| panic!("no rows: {}", json::to_string(v).unwrap()))
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("")
}

fn f(v: &Value, k: &str) -> f64 {
    v.get(k).and_then(|x| x.as_f64()).unwrap_or(-1.0)
}

const ALPHA: &str = "pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

pub fn double(a: i32) -> i32 {
    add(a, a)
}

pub fn write(path: &str) -> usize {
    path.len()
}

pub fn twice(a: i32) -> i32 {
    double(a)
}
";

const BETA: &str = "use crate::alpha;

pub struct Rig {
    pub n: i32,
}

pub fn scale(a: i32, k: i32) -> i32 {
    a * k
}

pub fn quad(a: i32) -> i32 {
    alpha::double(alpha::double(a))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rig() -> Rig {
        Rig { n: 3 }
    }

    #[test]
    fn scale_by_two() {
        let r = rig();
        assert_eq!(scale(r.n, 2), 6);
    }

    #[test]
    fn scale_by_zero() {
        let r = rig();
        assert_eq!(scale(r.n, 0), 0);
    }

    #[test]
    fn quad_of_three() {
        let r = rig();
        assert_eq!(quad(r.n), 12);
    }

    #[test]
    fn saves_a_file() {
        let r = rig();
        std::fs::write(\"target/x.txt\", \"x\").ok();
        assert_eq!(r.n, 3);
    }

    #[test]
    fn pages_locally() {
        let page = || 2;
        assert_eq!(page(), 2);
    }
}
";

const REVIEW: &str = "fn page() -> u8 {
    1
}

pub fn shown() -> u8 {
    page()
}
";

const IT: &str = "use demo::alpha::{add, double};

fn fixture() -> i32 {
    2
}

#[test]
fn adds() {
    assert_eq!(add(fixture(), 2), 4);
}

#[test]
fn doubles() {
    assert_eq!(double(fixture()), 4);
}

#[test]
fn only_fixture() {
    assert_eq!(fixture(), 2);
}
";

const GUARD: &str = "use std::path::Path;

#[test]
fn no_prints_in_the_library() {
    let src = Path::new(env!(\"CARGO_MANIFEST_DIR\")).join(\"src\");
    for e in std::fs::read_dir(src).unwrap() {
        let text = std::fs::read_to_string(e.unwrap().path()).unwrap();
        assert!(!text.contains(\"println\"));
    }
}
";

fn project(agents: Option<&str>, guard: bool) -> PathBuf {
    let dir = scratch("p");
    let mut files: Vec<(&str, &str)> = vec![
        ("Cargo.toml", "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        ("src/lib.rs", "pub mod alpha;\npub mod beta;\npub mod review;\n"),
        ("src/alpha.rs", ALPHA),
        ("src/beta.rs", BETA),
        ("src/review.rs", REVIEW),
        ("tests/it.rs", IT),
    ];
    if guard {
        files.push(("tests/print_guard.rs", GUARD));
    }
    if let Some(a) = agents {
        files.push(("AGENTS.md", a));
    }
    for (p, t) in files {
        let path = dir.join(p);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, t).unwrap();
    }
    git(&dir, &["init", "-q", "-b", "main"]);
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "init"]);
    chisel(&dir, &["analyze", "--json"]);
    dir
}

fn edit(dir: &Path, file: &str, from: &str, to: &str) {
    let path = dir.join(file);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains(from), "{file} has no {from:?}");
    std::fs::write(path, text.replacen(from, to, 1)).unwrap();
}

fn restore(dir: &Path) {
    git(dir, &["checkout", "-q", "--", "."]);
}

fn edges_of(dir: &Path, test: &str) -> Vec<(String, f64)> {
    let store = Store::open(&dir.join(".chisel")).unwrap();
    store
        .rows("test_edges")
        .iter()
        .filter(|e| s(e, "test_id") == test)
        .map(|e| (s(e, "code_id").to_string(), f(e, "weight")))
        .collect()
}

#[test]
fn std_calls_and_local_closures_are_not_project_functions() {
    let dir = project(None, false);
    // `std::fs::write(..)` with no `use std::fs`: the project's only `write` is not called.
    assert!(!edges_of(&dir, "src/beta.rs:saves_a_file").iter().any(|(c, _)| c.contains(":write:")), "{:?}", edges_of(&dir, "src/beta.rs:saves_a_file"));
    // `let page = || 2; page()`: the closure, not the private `review.rs:page`.
    assert!(edges_of(&dir, "src/beta.rs:pages_locally").is_empty(), "{:?}", edges_of(&dir, "src/beta.rs:pages_locally"));
    edit(&dir, "src/alpha.rs", "path.len()", "path.len() + 1");
    let rows = items(&diff_impact(&dir, &[]));
    assert!(!rows.iter().any(|r| s(r, "name") == "saves_a_file"), "{rows:?}");
}

#[test]
fn a_changed_assertion_names_that_test_first_and_alone() {
    let dir = project(None, false);
    edit(&dir, "src/beta.rs", "assert_eq!(scale(r.n, 0), 0);", "assert_eq!(scale(r.n, 0), 0 * 1);");
    let rows = items(&diff_impact(&dir, &[]));
    let got: Vec<(&str, &str)> = rows.iter().map(|r| (s(r, "test_id"), s(r, "source"))).collect();
    assert_eq!(got, vec![("src/beta.rs:scale_by_zero", "changed_test")], "not the file's other tests through the shared helper");
    restore(&dir);

    // The shared helper itself changed: its tests, through their edges.
    edit(&dir, "src/beta.rs", "Rig { n: 3 }", "Rig { n: 3 + 0 }");
    let rows = items(&diff_impact(&dir, &[]));
    let mut names: Vec<&str> = rows.iter().map(|r| s(r, "name")).collect();
    names.sort();
    assert_eq!(names, vec!["quad_of_three", "saves_a_file", "scale_by_two", "scale_by_zero"], "{rows:?}");
    restore(&dir);

    // An integration test changed: it comes first, and the file's history adds nothing.
    edit(&dir, "tests/it.rs", "add(fixture(), 2), 4", "add(fixture(), 2), 2 + 2");
    let rows = items(&diff_impact(&dir, &[]));
    assert_eq!(s(&rows[0], "test_id"), "tests/it.rs:adds");
    assert_eq!((s(&rows[0], "source"), f(&rows[0], "score")), ("changed_test", 1.0));
    assert!(rows.iter().all(|r| s(r, "source") != "co_change"), "{rows:?}");
    restore(&dir);

    // A helper of an integration test: the tests of that file that call it.
    edit(&dir, "tests/it.rs", "    2\n}", "    1 + 1\n}");
    let rows = items(&diff_impact(&dir, &[]));
    let mut names: Vec<&str> = rows.iter().filter(|r| s(r, "file_path") == "tests/it.rs").map(|r| s(r, "name")).collect();
    names.sort();
    assert_eq!(names, vec!["adds", "doubles", "only_fixture"], "{rows:?}");
    assert!(rows.iter().filter(|r| s(r, "file_path") == "tests/it.rs").all(|r| s(r, "reason") == "calls fixture, a changed helper of its own file"), "{rows:?}");
}

#[test]
fn a_wide_diff_ranks_by_changed_units_reached_and_a_narrow_one_keeps_its_scores() {
    let dir = project(None, false);
    // One function: the direct test at its edge weight, the caller's test at half.
    edit(&dir, "src/beta.rs", "    a * k\n", "    a * k + 0\n");
    let rows = items(&diff_impact(&dir, &[]));
    let got: Vec<(&str, f64, &str)> = rows.iter().map(|r| (s(r, "name"), f(r, "score"), s(r, "source"))).collect();
    assert_eq!(got, vec![("scale_by_two", 1.0, "direct"), ("scale_by_zero", 1.0, "direct")]);
    restore(&dir);
    edit(&dir, "src/alpha.rs", "    a + b\n", "    a + b + 0\n");
    let rows = items(&diff_impact(&dir, &["--depth", "2"]));
    let got: Vec<(&str, f64, &str)> = rows.iter().filter(|r| s(r, "source") != "co_change").map(|r| (s(r, "name"), f(r, "score"), s(r, "source"))).collect();
    assert_eq!(got[0], ("adds", 0.4, "direct"), "{got:?}");
    assert!(got.iter().any(|g| g.0 == "doubles" && g.2 == "call_graph" && (g.1 - 0.2).abs() < 1e-9), "{got:?}");
    restore(&dir);

    // Three functions: quad_of_three reaches quad, double and (through it) add; scale's tests
    // reach scale only, which two tests share.
    edit(&dir, "src/beta.rs", "    a * k\n", "    a * k + 0\n");
    edit(&dir, "src/beta.rs", "alpha::double(alpha::double(a))", "alpha::double(alpha::double(a)) + 0");
    edit(&dir, "src/alpha.rs", "    add(a, a)\n", "    add(a, a) + 0\n");
    let rows = items(&diff_impact(&dir, &[]));
    let first = &rows[0];
    assert_eq!(s(first, "name"), "quad_of_three", "{rows:?}");
    assert_eq!((f(first, "score"), first.get("units").and_then(|v| v.as_i64())), (1.0, Some(2)), "{rows:?}");
    let scale = rows.iter().find(|r| s(r, "name") == "scale_by_two").unwrap();
    assert!(f(scale, "score") < 1.0, "a test of one shared unit ranks below: {rows:?}");
}

#[test]
fn rules_and_source_reading_guards_come_first_with_their_own_source() {
    let agents = "# Project\n\n```agent-tools\nchisel: disable locks\nchisel: always-run src/alpha.rs -> tests/it.rs:only_fixture\ntrammel: disable notes\n```\n";
    let dir = project(Some(agents), true);
    let store = Store::open(&dir.join(".chisel")).unwrap();
    let guard = store.rows("test_units").iter().find(|t| s(t, "id") == "tests/print_guard.rs:no_prints_in_the_library").cloned().unwrap();
    assert_eq!(guard.get("reads").and_then(|v| v.as_array()).map(|a| a.to_vec()), Some(vec![Value::String("src/**".into())]));
    edit(&dir, "src/alpha.rs", "path.len()", "path.len() + 1");
    let reply = diff_impact(&dir, &[]);
    let rows = items(&reply);
    let top: Vec<(&str, &str)> = rows.iter().take(2).map(|r| (s(r, "test_id"), s(r, "source"))).collect();
    assert_eq!(top, vec![("tests/it.rs:only_fixture", "always_run"), ("tests/print_guard.rs:no_prints_in_the_library", "reads_source")], "{rows:?}");
    assert_eq!(s(&rows[0], "reason"), "AGENTS.md always-run src/alpha.rs -> tests/it.rs:only_fixture (src/alpha.rs changed)");
    assert_eq!(s(&rows[1], "reason"), "reads src/** (src/alpha.rs changed)");
    // A change outside every rule and read path brings neither in.
    restore(&dir);
    edit(&dir, "tests/it.rs", "add(fixture(), 2), 4", "add(fixture(), 2), 2 + 2");
    let rows = items(&diff_impact(&dir, &[]));
    assert!(rows.iter().all(|r| !matches!(s(r, "source"), "always_run" | "reads_source")), "{rows:?}");
}

#[test]
fn the_reply_carries_the_command_and_summarises_ties_past_the_limit() {
    let dir = project(None, false);
    edit(&dir, "src/beta.rs", "Rig { n: 3 }", "Rig { n: 3 + 0 }");
    let reply = diff_impact(&dir, &[]);
    let run: Vec<&str> = reply.get("run").and_then(|v| v.as_array()).unwrap().iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(run, vec!["cargo test -p demo -- quad_of_three saves_a_file scale_by_two scale_by_zero"], "{}", json::to_string(&reply).unwrap());
    // Four tests tie at 1.0; a limit of 2 cannot list two of them fairly, so none are listed
    // and the group says how many and how to run them.
    let reply = diff_impact(&dir, &["--limit", "2"]);
    assert!(items(&reply).is_empty(), "{}", json::to_string(&reply).unwrap());
    assert_eq!((reply.get("total").and_then(|v| v.as_i64()), reply.get("tied").and_then(|v| v.as_i64())), (Some(4), Some(4)));
    let groups = reply.get("groups").and_then(|v| v.as_array()).unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!((s(&groups[0], "group"), groups[0].get("tests").and_then(|v| v.as_i64()), f(&groups[0], "top")), ("demo", Some(4), 1.0));
    assert_eq!(s(&groups[0], "run"), "cargo test -p demo -- quad_of_three saves_a_file scale_by_two scale_by_zero");
}

#[test]
fn results_keep_message_and_platform_and_say_what_has_not_run_where() {
    let dir = project(None, false);
    let id = "src/beta.rs:scale_by_two";
    for _ in 0..2 {
        chisel(&dir, &["record-result", id, "--passed", "--platform", "windows", "--json"]);
    }
    let failed = json::parse(&chisel(&dir, &["record-result", id, "--failed", "--message", "assertion failed: left 6, right 7", "--platform", "linux", "--json"])).unwrap();
    assert_eq!(failed.get("first_failure_after").and_then(|v| v.as_i64()), Some(2), "{failed:?}");
    assert_eq!(s(&failed, "platform"), "linux");
    let again = json::parse(&chisel(&dir, &["record-result", id, "--failed", "--json"])).unwrap();
    assert!(again.get("first_failure_after").is_none(), "only the first failure after passes: {again:?}");
    let store = Store::open(&dir.join(".chisel")).unwrap();
    let rows = store.rows("test_results");
    let kept = rows.iter().find(|r| s(r, "platform") == "linux").unwrap();
    assert_eq!(s(kept, "message"), "assertion failed: left 6, right 7");
    assert!(rows.iter().filter(|r| r.get("passed").and_then(|v| v.as_bool()) == Some(true)).all(|r| r.get("message").is_none()));

    let stats = json::parse(&chisel(&dir, &["stats", "--not-run-on", "linux", "--json"])).unwrap();
    let not_run = stats.get("not_run_on").unwrap();
    let total = store.rows("test_units").len() as i64;
    assert_eq!(not_run.get("total").and_then(|v| v.as_i64()), Some(total - 1), "{not_run:?}");
    assert!(!not_run.get("items").and_then(|v| v.as_array()).unwrap().iter().any(|v| v.as_str() == Some(id)));

    // A pass is remembered with its commit: it counts for that commit, not for a newer one.
    let stats = json::parse(&chisel(&dir, &["stats", "--not-passed-since", "HEAD", "--json"])).unwrap();
    let np = stats.get("not_passed_since").unwrap();
    assert_eq!((np.get("total").and_then(|v| v.as_i64()), np.get("passed_before").and_then(|v| v.as_i64())), (Some(total - 1), Some(0)), "{np:?}");
    std::fs::write(dir.join("NOTES.md"), "x\n").unwrap();
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "later"]);
    let stats = json::parse(&chisel(&dir, &["stats", "--not-passed-since", "HEAD", "--json"])).unwrap();
    let np = stats.get("not_passed_since").unwrap();
    assert_eq!(np.get("passed_before").and_then(|v| v.as_i64()), Some(1), "{np:?}");
    assert_eq!(np.get("items").and_then(|v| v.as_array()).unwrap()[0].as_str(), Some(id), "passed before the ref: listed first");
    let out = Command::new(env!("CARGO_BIN_EXE_chisel")).args(["stats", "--not-passed-since", "no-such-ref", "--json", "--project-dir"]).arg(&dir).output().unwrap();
    let bad = json::parse(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!((s(&bad, "status"), out.status.code()), ("error", Some(1)), "{bad:?}");
    assert!(s(&bad, "message").contains("no-such-ref"), "{bad:?}");
}
