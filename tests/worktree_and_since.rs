//! Fixes from the Specula M5 lanes, through the binary: `diff-impact --since`, the merge-base
//! default on a branch, deleted files never "missing", worktree overlays (`--worktree`,
//! `worktree_root`, and a process started inside a linked worktree), Rust calls resolved by type
//! rather than name, help for every command, and exit codes.

use chisel::json::{self, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

fn scratch(name: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let p = std::env::temp_dir().join(format!("chisel-m5-{name}-{t}-{}", N.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir_all(&p).unwrap();
    p
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git").args(args).current_dir(dir).env("GIT_AUTHOR_DATE", "2024-03-01T00:00:00+00:00").env("GIT_COMMITTER_DATE", "2024-03-01T00:00:00+00:00").output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

/// `chisel` with `--project-dir dir` unless `cwd_only`, which runs it in `dir` with no
/// directory given (as an MCP client would start it).
fn run(dir: &Path, args: &[&str], cwd_only: bool) -> (i32, String, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_chisel"));
    cmd.args(args).current_dir(dir).env_remove("CHISEL_PROJECT_DIR").env_remove("CHISEL_STORAGE_DIR");
    if !cwd_only {
        cmd.arg("--project-dir").arg(dir);
    }
    let out = cmd.output().unwrap();
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

fn json_of(text: &str) -> Value {
    json::parse(text.trim()).unwrap_or_else(|e| panic!("{e:?}: {text}"))
}

fn items(doc: &Value) -> Vec<Value> {
    match doc {
        Value::Array(a) => a.clone(),
        other => other.get("items").and_then(|v| v.as_array()).map(|a| a.to_vec()).unwrap_or_default(),
    }
}

fn test_ids(doc: &Value) -> Vec<String> {
    items(doc).iter().filter_map(|r| r.get("test_id").and_then(|v| v.as_str()).map(|s| s.to_string())).collect()
}

const CARGO: &str = "[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";
const LIB: &str = "pub mod atlas;\npub mod geom;\npub mod old;\n";
const ATLAS: &str = "pub struct GlyphKey(u8);\n\nimpl GlyphKey {\n    pub fn new(n: u8) -> GlyphKey {\n        GlyphKey(n)\n    }\n}\n\npub struct GlyphAtlas {\n    pages: Vec<u8>,\n}\n\nimpl GlyphAtlas {\n    pub fn new() -> GlyphAtlas {\n        GlyphAtlas { pages: Vec::new() }\n    }\n\n    pub fn clear(&mut self) {\n        self.pages.clear();\n    }\n}\n\nimpl Default for GlyphAtlas {\n    fn default() -> Self {\n        GlyphAtlas::new()\n    }\n}\n";
const GEOM: &str = "pub fn yaw(x: f32) -> f32 {\n    x * 2.0\n}\n\npub fn aligned(x: f32) -> bool {\n    yaw(x) > 1.0\n}\n";
const OLD: &str = "pub fn legacy() -> u8 {\n    1\n}\n";
const TESTS: &str = "use sample::atlas::{GlyphAtlas, GlyphKey};\nuse sample::geom::{aligned, yaw};\n\n#[test]\nfn atlas_starts_empty() {\n    let mut a = GlyphAtlas::new();\n    a.clear();\n}\n\n#[test]\nfn keys_are_small() {\n    let k = GlyphKey::new(3);\n    let _ = k;\n}\n\n#[test]\nfn defaults_are_built_by_std() {\n    let s: String = Default::default();\n    assert!(s.is_empty());\n}\n\n#[test]\nfn yaw_doubles() {\n    assert_eq!(yaw(1.0), 2.0);\n}\n\n#[test]\nfn aligned_past_one() {\n    assert!(aligned(1.0));\n}\n";

/// A one-crate repo on `main`, analyzed.
fn repo() -> PathBuf {
    let dir = scratch("repo");
    for (p, t) in [("Cargo.toml", CARGO), ("src/lib.rs", LIB), ("src/atlas.rs", ATLAS), ("src/geom.rs", GEOM), ("src/old.rs", OLD), ("tests/t.rs", TESTS), (".gitignore", ".chisel/\n")] {
        let path = dir.join(p);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, t).unwrap();
    }
    git(&dir, &["init", "-q", "-b", "main"]);
    for (k, v) in [("user.email", "a@example.com"), ("user.name", "Ann"), ("commit.gpgsign", "false"), ("core.autocrlf", "false")] {
        git(&dir, &["config", k, v]);
    }
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-q", "-m", "init"]);
    let (code, out, err) = run(&dir, &["analyze"], false);
    assert_eq!(code, 0, "analyze exits 0 on success: {out}{err}");
    dir
}

fn edit(path: &Path, from: &str, to: &str) {
    let text = fs::read_to_string(path).unwrap();
    assert!(text.contains(from), "{from} not in {}", path.display());
    fs::write(path, text.replacen(from, to, 1)).unwrap();
}

#[test]
fn rust_calls_resolve_by_type_not_by_name() {
    let dir = repo();
    // Change GlyphAtlas::new (GlyphKey::new shares the name and the file).
    edit(&dir.join("src/atlas.rs"), "GlyphAtlas { pages: Vec::new() }", "GlyphAtlas { pages: Vec::with_capacity(4) }");
    let (code, out, err) = run(&dir, &["--json", "diff-impact"], false);
    assert_eq!(code, 0, "{err}");
    let ids = test_ids(&json_of(&out));
    assert!(ids.contains(&"tests/t.rs:atlas_starts_empty".to_string()), "{ids:?}");
    assert!(!ids.contains(&"tests/t.rs:keys_are_small".to_string()), "GlyphKey::new is another type's new: {ids:?}");
    // Default::default() names std's Default, not GlyphAtlas::default; `s.is_empty()` is String's.
    let store = chisel::store::Store::open(&dir.join(".chisel")).unwrap();
    let targets: Vec<&str> = store.rows("test_edges").iter().filter(|e| e.get("test_id").and_then(|v| v.as_str()) == Some("tests/t.rs:defaults_are_built_by_std")).filter_map(|e| e.get("code_id").and_then(|v| v.as_str())).collect();
    assert!(targets.is_empty(), "{targets:?}");
}

#[test]
fn since_scores_committed_work_from_the_merge_base() {
    let dir = repo();
    git(&dir, &["checkout", "-q", "-b", "feature"]);
    edit(&dir.join("src/geom.rs"), "x * 2.0", "x + x");
    git(&dir, &["commit", "-qam", "yaw"]);
    // main moves on with a file the branch does not have.
    git(&dir, &["checkout", "-q", "main"]);
    fs::write(dir.join("src/later.rs"), "pub fn later() -> u8 {\n    2\n}\n").unwrap();
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "later on main"]);
    git(&dir, &["checkout", "-q", "feature"]);
    let (code, _, err) = run(&dir, &["update"], false);
    assert_eq!(code, 0, "{err}");

    // Default on a branch: the merge-base with main, so main's later.rs is not "missing".
    let (code, out, err) = run(&dir, &["--json", "diff-impact"], false);
    assert_eq!(code, 0, "{err}");
    let doc = json_of(&out);
    assert_ne!(doc.get("status").and_then(|v| v.as_str()), Some("stale_db"), "{out}");
    let ids = test_ids(&doc);
    assert!(ids.contains(&"tests/t.rs:yaw_doubles".to_string()), "{out}");
    assert!(ids.contains(&"tests/t.rs:aligned_past_one".to_string()), "aligned calls yaw: {out}");

    // --since main: the same committed work, with the base in the reply.
    let (code, out, _) = run(&dir, &["--json", "diff-impact", "--since", "main"], false);
    assert_eq!(code, 0);
    let doc = json_of(&out);
    assert_eq!(doc.get("since").and_then(|v| v.as_str()), Some("main"), "{out}");
    assert_eq!(doc.get("base").and_then(|v| v.as_str()).map(|s| s.len()), Some(40), "{out}");
    assert!(test_ids(&doc).contains(&"tests/t.rs:yaw_doubles".to_string()), "{out}");

    // Without since, a plain diff against HEAD has nothing: the work is committed.
    let (_, out, _) = run(&dir, &["--json", "diff-impact", "--ref", "HEAD"], false);
    assert_eq!(json_of(&out).get("status").and_then(|v| v.as_str()), Some("no_changes"), "{out}");

    // Both, or an unknown ref: one actionable line and exit 1.
    let (code, out, _) = run(&dir, &["--json", "diff-impact", "--since", "main", "--ref", "HEAD"], false);
    assert_eq!(code, 1);
    assert!(out.contains("not both"), "{out}");
    let (code, out, _) = run(&dir, &["--json", "diff-impact", "--since", "no-such-branch"], false);
    assert_eq!(code, 1);
    assert_eq!(json_of(&out).get("status").and_then(|v| v.as_str()), Some("git_error"), "{out}");
    assert!(out.contains("since=main"), "{out}");
}

#[test]
fn a_deleted_file_is_never_missing_from_the_snapshot() {
    let dir = repo();
    edit(&dir.join("src/lib.rs"), "pub mod old;\n", "");
    fs::remove_file(dir.join("src/old.rs")).unwrap();
    edit(&dir.join("src/geom.rs"), "x * 2.0", "x + x");
    let (code, _, _) = run(&dir, &["update"], false);
    assert_eq!(code, 0);
    let (code, out, err) = run(&dir, &["--json", "diff-impact"], false);
    assert_eq!(code, 0, "{err}");
    let doc = json_of(&out);
    assert_ne!(doc.get("status").and_then(|v| v.as_str()), Some("stale_db"), "update dropped old.rs; it is deleted, not missing: {out}");
    assert!(test_ids(&doc).contains(&"tests/t.rs:yaw_doubles".to_string()), "{out}");
}

#[test]
fn a_worktree_overlays_the_main_snapshot() {
    let dir = repo();
    let base_store = dir.join(".chisel").join("store");
    let before = fs::read(&base_store).unwrap();
    let wt = scratch("wt").join("tree");
    git(&dir, &["worktree", "add", "-q", "--detach", wt.to_str().unwrap(), "main"]);
    // One file edited and committed, one added, one deleted.
    edit(&wt.join("src/geom.rs"), "x * 2.0", "x + x");
    git(&wt, &["commit", "-qam", "yaw in the worktree"]);
    fs::write(wt.join("src/fresh.rs"), "pub fn fresh() -> u8 {\n    7\n}\n").unwrap();
    edit(&wt.join("src/lib.rs"), "pub mod old;\n", "pub mod fresh;\n");
    fs::write(wt.join("tests/fresh_test.rs"), "use sample::fresh::fresh;\n\n#[test]\nfn fresh_is_seven() {\n    assert_eq!(fresh(), 7);\n}\n").unwrap();
    fs::remove_file(wt.join("src/old.rs")).unwrap();

    let wt_arg = wt.to_string_lossy().to_string();
    let (code, out, err) = run(&dir, &["--json", "diff-impact", "--since", "main", "--worktree", &wt_arg], false);
    assert_eq!(code, 0, "{err}");
    let doc = json_of(&out);
    assert_ne!(doc.get("status").and_then(|v| v.as_str()), Some("stale_db"), "{out}");
    let ids = test_ids(&doc);
    assert!(ids.contains(&"tests/t.rs:yaw_doubles".to_string()), "committed edit in the worktree: {out}");
    assert!(ids.contains(&"tests/fresh_test.rs:fresh_is_seven".to_string()), "new test, indexed in the overlay: {out}");
    // atlas.rs is untouched: GlyphKey and GlyphAtlas tests are not this change's.
    // (A misread worktree index makes every file look changed and every test a direct hit.)
    for row in items(&doc) {
        let id = row.get("test_id").and_then(|v| v.as_str()).unwrap_or("");
        let source = row.get("source").and_then(|v| v.as_str()).unwrap_or("");
        if matches!(id, "tests/t.rs:keys_are_small" | "tests/t.rs:atlas_starts_empty") {
            assert!(!matches!(source, "direct" | "call_graph"), "{id} is not reached by this change: {out}");
        }
    }
    assert!(doc.get("worktree").and_then(|v| v.as_str()).is_some_and(|w| w.ends_with("tree")), "{out}");
    // geom.rs, lib.rs, fresh.rs, fresh_test.rs and old.rs differ from main's snapshot.
    assert_eq!(doc.get("overlay_files").and_then(|v| v.as_i64()), Some(5), "{out}");
    assert!(items(&doc).iter().all(|r| !r.get("file_path").and_then(|v| v.as_str()).unwrap_or("").contains(':')), "paths are relative: {out}");
    assert_eq!(fs::read(&base_store).unwrap(), before, "main's snapshot is never written");

    // A fresh checkout can hold main's file with other line endings: not a change.
    let crlf = fs::read_to_string(wt.join("src/atlas.rs")).unwrap().replace('\n', "\r\n");
    fs::write(wt.join("src/atlas.rs"), crlf).unwrap();
    let (_, out, _) = run(&dir, &["--json", "diff-impact", "--since", "main", "--worktree", &wt_arg], false);
    assert_eq!(json_of(&out).get("overlay_files").and_then(|v| v.as_i64()), Some(5), "a line-ending-only copy of main's file is no overlay file: {out}");

    // Other read tools answer for the worktree too.
    let (_, out, _) = run(&dir, &["--json", "suggest-tests", "src/fresh.rs", "--worktree", &wt_arg], false);
    assert!(out.contains("fresh_is_seven"), "{out}");
    let (_, out, _) = run(&dir, &["--json", "suggest-tests", "src/fresh.rs"], false);
    assert!(out.contains("stale_db"), "without the worktree, main has no fresh.rs: {out}");

    // Started inside the worktree with no directory given: the same overlay, automatically.
    let (code, out, err) = run(&wt, &["--json", "diff-impact", "--since", "main"], true);
    assert_eq!(code, 0, "{err}");
    let doc = json_of(&out);
    assert!(doc.get("worktree").is_some() && test_ids(&doc).contains(&"tests/fresh_test.rs:fresh_is_seven".to_string()), "{out}");
    assert!(!wt.join(".chisel").exists(), "nothing is written into the worktree");

    // A later edit is seen on the next call, by mtime and size.
    fs::write(wt.join("tests/fresh_test.rs"), "use sample::fresh::fresh;\n\n#[test]\nfn fresh_is_still_seven() {\n    assert_eq!(fresh(), 7);\n}\n").unwrap();
    let (_, out, _) = run(&dir, &["--json", "diff-impact", "--since", "main", "--worktree", &wt_arg], false);
    assert!(out.contains("fresh_is_still_seven") && !out.contains("fresh_is_seven\""), "{out}");

    // Bad worktree paths: one actionable line, exit 1.
    let missing = dir.join("no-such-tree").to_string_lossy().to_string();
    let (code, out, _) = run(&dir, &["--json", "diff-impact", "--worktree", &missing], false);
    assert_eq!(code, 1);
    assert!(out.contains("does not exist"), "{out}");
    let other = scratch("other");
    git(&other, &["init", "-q"]);
    let (code, out, _) = run(&dir, &["--json", "stats", "--worktree", other.to_str().unwrap()], false);
    assert_eq!(code, 1);
    assert!(out.contains("not a checkout"), "{out}");

    // Shards keep their own snapshots: no overlay, a clear answer.
    let sharded = Command::new(env!("CARGO_BIN_EXE_chisel")).args(["--json", "stats", "--worktree", &wt_arg, "--project-dir"]).arg(&dir).env("CHISEL_SHARDS", "src").output().unwrap();
    assert!(String::from_utf8_lossy(&sharded.stdout).contains("unsupported"), "{}", String::from_utf8_lossy(&sharded.stdout));

    // An overlay whose worktree is gone is removed on the next overlay call.
    let overlays = dir.join(".chisel").join("overlays");
    assert_eq!(fs::read_dir(&overlays).unwrap().count(), 1);
    git(&dir, &["worktree", "remove", "--force", &wt_arg]);
    let wt2 = scratch("wt2").join("tree");
    git(&dir, &["worktree", "add", "-q", "--detach", wt2.to_str().unwrap(), "main"]);
    let (code, _, err) = run(&dir, &["--json", "stats", "--worktree", wt2.to_str().unwrap()], false);
    assert_eq!(code, 0, "{err}");
    let left: Vec<String> = fs::read_dir(&overlays).unwrap().flatten().map(|e| fs::read_to_string(e.path().join("worktree.txt")).unwrap_or_default()).collect();
    assert_eq!(left.len(), 1, "{left:?}");
    assert!(left[0].ends_with("tree") && !left[0].contains("chisel-m5-wt-"), "{left:?}");
}

#[test]
fn analyze_rebuilds_a_snapshot_from_another_chisel_version() {
    let dir = repo();
    let read_version = || {
        let store = chisel::store::Store::open(&dir.join(".chisel")).unwrap();
        chisel::store::meta_get(&store.doc, "analyzer_version")
    };
    assert_eq!(read_version().as_deref(), Some(env!("CARGO_PKG_VERSION")));
    {
        let mut store = chisel::store::Store::open(&dir.join(".chisel")).unwrap();
        chisel::store::meta_set(&mut store.doc, "analyzer_version", "0.22.0");
        store.save().unwrap();
    }
    // Same HEAD, same files: only the version differs, and the edges are rebuilt.
    let (code, _, err) = run(&dir, &["analyze"], false);
    assert_eq!(code, 0, "{err}");
    assert_eq!(read_version().as_deref(), Some(env!("CARGO_PKG_VERSION")));
}

#[test]
fn every_command_has_help() {
    let dir = scratch("help");
    for (name, _, _) in chisel::cli_text::COMMANDS {
        for args in [vec![*name, "--help"], vec!["help", *name]] {
            let (code, out, err) = run(&dir, &args, false);
            assert_eq!(code, 0, "{args:?}: {out}{err}");
            assert!(out.starts_with(&format!("chisel {name}")), "{args:?}: {out}");
        }
    }
    let (_, out, _) = run(&dir, &["diff-impact", "--help"], false);
    assert!(out.contains("--since") && out.contains("--worktree"), "{out}");
    let (_, out, _) = run(&dir, &["--help"], false);
    for (name, _, _) in chisel::cli_text::COMMANDS {
        assert!(out.contains(name), "{name} missing from --help: {out}");
    }
    let (code, out, _) = run(&dir, &["no-such-command", "--help"], false);
    assert_eq!(code, 2);
    assert!(out.contains("Unknown command"), "{out}");
}

#[test]
fn exit_codes_say_answered_error_or_usage() {
    let dir = repo();
    // Answers exit 0, including the ones that say what to do next.
    for args in [vec!["analyze"], vec!["analyze", "--force"], vec!["update"], vec!["stats"], vec!["diff-impact"], vec!["test-gaps"], vec!["risk-map"], vec!["stale-tests"]] {
        let (code, out, err) = run(&dir, &args, false);
        assert_eq!(code, 0, "{args:?}: {out}{err}");
    }
    let (code, out, _) = run(&dir, &["--json", "analyze", "--force"], false);
    assert_eq!(code, 0);
    assert!(out.contains("code_units_found") && !out.contains("auto_queued"), "the CLI runs a forced analyze itself: {out}");
    let (code, out, _) = run(&dir, &["--json", "start-job", "analyze"], false);
    assert_eq!(code, 0);
    assert!(out.contains("\"completed\""), "the CLI waits for its job: {out}");
    let empty = scratch("empty");
    git(&empty, &["init", "-q"]);
    fs::write(empty.join("a.py"), "def f():\n    return 1\n").unwrap();
    for (k, v) in [("user.email", "a@example.com"), ("user.name", "Ann"), ("commit.gpgsign", "false")] {
        git(&empty, &["config", k, v]);
    }
    git(&empty, &["add", "-A"]);
    git(&empty, &["commit", "-q", "-m", "one"]);
    let (code, out, _) = run(&empty, &["--json", "diff-impact"], false);
    assert_eq!((code, json_of(&out).get("status").and_then(|v| v.as_str()).map(|s| s.to_string())), (0, Some("no_data".to_string())), "{out}");
    // Errors exit 1.
    let plain = scratch("plain");
    fs::write(plain.join("a.py"), "def f():\n    return 1\n").unwrap();
    let (code, _, _) = run(&plain, &["analyze"], false);
    assert_eq!(code, 0);
    let (code, out, _) = run(&plain, &["--json", "diff-impact"], false);
    assert_eq!(code, 1, "not a git repo: {out}");
    // Usage errors exit 2.
    let (code, _, err) = run(&dir, &["no-such-command"], false);
    assert_eq!(code, 2);
    assert!(err.contains("unknown command 'no-such-command'"), "{err}");
    let (code, _, err) = run(&dir, &["stats", "--no-such-flag"], false);
    assert_eq!(code, 2);
    assert!(err.contains("unknown flag"), "{err}");
}
