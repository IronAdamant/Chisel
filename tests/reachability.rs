//! A Rust test only reaches its own crate and the crates it depends on; a changed function
//! reaches the tests of its callers. Both on a scripted Cargo workspace, through the binary.

use chisel::json::{self, Value};
use chisel::store::Store;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

fn scratch(name: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let p = std::env::temp_dir().join(format!("chisel-reach-{name}-{t}-{}", N.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git").args(args).current_dir(dir).env("GIT_AUTHOR_DATE", "2024-03-01T00:00:00+00:00").env("GIT_COMMITTER_DATE", "2024-03-01T00:00:00+00:00").status().unwrap().success();
    assert!(ok, "git {args:?}");
}

fn chisel(dir: &Path, args: &[&str], stdin: &str) -> (i32, String, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_chisel"))
        .args(args)
        .arg("--project-dir")
        .arg(dir)
        .current_dir(dir)
        .env_remove("CHISEL_PROJECT_DIR")
        .env_remove("CHISEL_STORAGE_DIR")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

const GEOM: &str = "use base::math::Vec3;
use std::cell::Cell;

pub fn yaw_toward(x: f32) -> f32 {
    x * 2.0
}

pub fn aligned(x: f32) -> bool {
    yaw_toward(x) > 1.0
}

pub fn generator(x: f32) -> f32 {
    if aligned(x) { 1.0 } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faces_away() {
        let c = Cell::new(1u8);
        let v = Vec3::new(c.get() as f32);
        assert!(generator(v.x) > 0.5, \"front (+Z) away from the wall: {}\", v.x);
    }

    #[test]
    fn yaw_is_doubled() {
        assert_eq!(yaw_toward(1.0), 2.0);
    }
}
";

/// `locus` depends on `base` only. `far` (which `locus` cannot see) and `base::cell` define the
/// names the test calls: `new` and `front` (in a string), and a `Cell` the test imports from std.
fn workspace() -> PathBuf {
    let dir = scratch("ws");
    let files: &[(&str, &str)] = &[
        ("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\nresolver = \"2\"\n\n[workspace.dependencies]\nbase = { path = \"crates/base\" }\n"),
        ("crates/base/Cargo.toml", "[package]\nname = \"base\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        ("crates/base/src/lib.rs", "pub mod cell;\npub mod math;\n"),
        ("crates/base/src/math.rs", "#[derive(Clone, Copy)]\npub struct Vec3 {\n    pub x: f32,\n}\n\nimpl Vec3 {\n    pub fn new(x: f32) -> Vec3 {\n        Vec3 { x }\n    }\n}\n"),
        ("crates/base/src/cell.rs", "pub struct Cell(u8);\n\nimpl Cell {\n    pub fn new(n: u8) -> Cell {\n        Cell(n)\n    }\n}\n"),
        ("crates/far/Cargo.toml", "[package]\nname = \"far\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nbase = { workspace = true }\nlocus = { path = \"../locus\" }\n"),
        ("crates/far/src/lib.rs", "pub mod stream;\n"),
        ("crates/far/src/stream.rs", "use base::math::Vec3;\n\npub struct Cell {\n    n: u8,\n}\n\nimpl Cell {\n    pub fn new(n: u8) -> Cell {\n        Cell { n }\n    }\n}\n\npub fn front(v: Vec3) -> f32 {\n    v.x\n}\n\npub trait Twice {\n    fn twice(&self) -> f32;\n}\n\nimpl Twice for Vec3 {\n    fn twice(&self) -> f32 {\n        self.x * 2.0\n    }\n}\n"),
        ("crates/locus/Cargo.toml", "[package]\nname = \"locus\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nbase.workspace = true\n"),
        ("crates/locus/src/lib.rs", "pub mod geom;\n"),
        ("crates/locus/src/geom.rs", GEOM),
    ];
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
    let (code, _, err) = chisel(&dir, &["analyze"], "");
    assert_eq!(code, 0, "{err}");
    dir
}

fn edges_of(dir: &Path, test: &str) -> Vec<String> {
    let store = Store::open(&dir.join(".chisel")).unwrap();
    let file_of = |id: &str| store.unit_by_id(id).and_then(|u| u.get("file_path")).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let mut out: Vec<String> = store
        .rows("test_edges")
        .iter()
        .filter(|e| e.get("test_id").and_then(|v| v.as_str()) == Some(test))
        .map(|e| {
            let id = e.get("code_id").and_then(|v| v.as_str()).unwrap_or("");
            format!("{}:{}", file_of(id), id.split(':').nth(1).unwrap_or(""))
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

const RUN_OK: &str = "     Running unittests src\\lib.rs (target\\debug\\deps\\locus-1.exe)\n\nrunning 2 tests\ntest geom::tests::faces_away ... ok\ntest geom::tests::yaw_is_doubled ... ok\n\ntest result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n";
const RUN_FAIL: &str = "     Running unittests src\\lib.rs (target\\debug\\deps\\locus-1.exe)\n\nrunning 2 tests\ntest geom::tests::faces_away ... FAILED\ntest geom::tests::yaw_is_doubled ... ok\n\nfailures:\n\n---- geom::tests::faces_away stdout ----\n\nthread 'geom::tests::faces_away' (7) panicked at crates\\locus\\src\\geom.rs:23:9:\nfront (+Z) away from the wall: 1\n\nfailures:\n    geom::tests::faces_away\n\ntest result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n";

#[test]
fn a_rust_test_links_only_to_crates_it_can_reach() {
    let dir = workspace();
    let edges = edges_of(&dir, "crates/locus/src/geom.rs:faces_away");
    // Same-file helpers it calls, and Vec3::new in the crate it depends on.
    assert!(edges.contains(&"crates/locus/src/geom.rs:generator".to_string()), "{edges:?}");
    assert!(edges.contains(&"crates/base/src/math.rs:new".to_string()), "{edges:?}");
    // Not `far` (no dependency path), not base's Cell (the test's Cell is std's), not `front`
    // (only in a string literal).
    for e in &edges {
        assert!(!e.starts_with("crates/far/"), "far is not a dependency of locus: {edges:?}");
        assert!(!e.starts_with("crates/base/src/cell.rs"), "Cell is imported from std: {edges:?}");
    }
}

#[test]
fn suspects_come_only_from_reachable_crates() {
    let dir = workspace();
    let (code, out, err) = chisel(&dir, &["capture", "--command", "cargo test"], RUN_OK);
    assert_eq!((code, out.as_str()), (0, ""), "{err}");
    std::fs::write(dir.join("crates/locus/src/geom.rs"), GEOM.replace("x * 2.0", "x * 2.0 + 1.0")).unwrap();
    // Recent edits in an unreachable crate must not rank as suspects either.
    std::fs::write(dir.join("crates/far/src/stream.rs"), "pub fn front() -> u8 {\n    1\n}\n").unwrap();
    let (_, line, _) = chisel(&dir, &["capture", "--command", "cargo test"], RUN_FAIL);
    assert!(line.contains("new I1 crates/locus/src/geom.rs:faces_away"), "{line}");
    let issue: Value = json::parse(&chisel(&dir, &["issue", "I1", "--json"], "").1).unwrap();
    let suspects = issue.get("suspects").and_then(|v| v.as_array()).unwrap();
    let files: Vec<&str> = suspects.iter().filter_map(|s| s.get("file").and_then(|v| v.as_str())).collect();
    assert_eq!(suspects[0].get("unit").and_then(|v| v.as_str()), Some("yaw_toward"), "{suspects:?}");
    assert!(files.iter().all(|f| !f.starts_with("crates/far/") && *f != "crates/base/src/cell.rs"), "{files:?}");
}

fn impact(dir: &Path, depth: Option<&str>) -> Vec<(String, String)> {
    let mut args = vec!["diff-impact", "--json"];
    if let Some(d) = depth {
        args.extend(["--depth", d]);
    }
    let (code, out, err) = chisel(dir, &args, "");
    assert_eq!(code, 0, "{err}");
    let v = json::parse(&out).unwrap();
    v.as_array()
        .unwrap_or_else(|| panic!("{out}"))
        .iter()
        .map(|r| (r.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(), r.get("source").and_then(|v| v.as_str()).unwrap_or("").to_string()))
        // Every file shares the one commit, so co-change rows appear too; they are not under test.
        .filter(|(_, source)| source != "co_change")
        .collect()
}

#[test]
fn diff_impact_depth_adds_callers_tests_after_direct_hits() {
    let dir = workspace();
    std::fs::write(dir.join("crates/locus/src/geom.rs"), GEOM.replace("x * 2.0", "x * 2.0 + 1.0")).unwrap();
    // Depth 1: only the test that calls yaw_toward itself.
    assert_eq!(impact(&dir, Some("1")), vec![("yaw_is_doubled".to_string(), "direct".to_string())]);
    // Depth 2: faces_away calls aligned (through generator), which calls yaw_toward. Direct first.
    let two = impact(&dir, Some("2"));
    assert_eq!(two, vec![("yaw_is_doubled".to_string(), "direct".to_string()), ("faces_away".to_string(), "call_graph".to_string())]);
    let (_, out, _) = chisel(&dir, &["diff-impact", "--json", "--depth", "2"], "");
    assert!(out.contains("calls aligned, which reaches the changed yaw_toward (1 call away)"), "{out}");
    // The default matches the documented one.
    let default = impact(&dir, None);
    assert_eq!(default.len(), if chisel::engine::DEFAULT_IMPACT_DEPTH >= 2 { 2 } else { 1 }, "{default:?}");
}
