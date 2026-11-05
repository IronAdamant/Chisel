//! Kill a writer after the temp snapshot exists, and prove readers never see a tear.

use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_chisel")
}

const CHILD_ENV: &str = "CHISEL_DURABILITY_CHILD_DIR";

/// Re-run this test binary so that only `helper` runs, as a separate process.
/// The helpers live here rather than in the shipped binary, so they exist in
/// debug and release test builds alike.
fn spawn_helper(helper: &str, dir: &std::path::Path, stderr: Stdio) -> std::process::Child {
    Command::new(std::env::current_exe().unwrap())
        .args([helper, "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD_ENV, dir)
        .stdout(Stdio::piped())
        .stderr(stderr)
        .spawn()
        .unwrap()
}

fn child_dir() -> Option<PathBuf> {
    std::env::var_os(CHILD_ENV).map(PathBuf::from)
}

/// Child half of `kill_before_rename_keeps_previous_snapshot`. A no-op unless spawned.
#[test]
fn helper_pause_before_rename() {
    let Some(dir) = child_dir() else { return };
    let mut store = chisel::store::Store::open(&dir).unwrap();
    let current = store.doc.get("token").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let next = format!("{current}-pending");
    store.doc.insert("token", chisel::json::Value::String(next.clone()));
    store.doc.insert("token_copy", chisel::json::Value::String(next));
    let _guard = store.hold_exclusive_after_temp().unwrap();
    println!("READY");
    let _ = std::io::stdout().flush();
    std::thread::sleep(std::time::Duration::from_secs(30));
}

/// Child half of `shared_reader_never_sees_a_torn_snapshot`. A no-op unless spawned.
#[test]
fn helper_read_loop() {
    let Some(dir) = child_dir() else { return };
    let start = std::time::Instant::now();
    while start.elapsed() < std::time::Duration::from_millis(800) {
        let store = chisel::store::Store::open(&dir).unwrap();
        let token = store.doc.get("token").and_then(|v| v.as_str()).unwrap_or("");
        let copy = store.doc.get("token_copy").and_then(|v| v.as_str()).unwrap_or("");
        if token != copy {
            println!("TORN {token} {copy}");
            let _ = std::io::stdout().flush();
            return;
        }
        println!("OK");
        let _ = std::io::stdout().flush();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

fn scratch() -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let p = std::env::temp_dir().join(format!("chisel-dur-{t}-{n}"));
    fs::create_dir_all(&p).unwrap();
    p
}

#[test]
fn kill_before_rename_keeps_previous_snapshot() {
    let dir = scratch();
    let mut store = chisel::store::Store::open(&dir).unwrap();
    store.doc.insert("token", chisel::json::Value::String("first".into()));
    store.doc.insert("token_copy", chisel::json::Value::String("first".into()));
    store.save().unwrap();

    let mut child = spawn_helper("helper_pause_before_rename", &dir, Stdio::null());
    let mut stdout = child.stdout.take().unwrap();
    let mut out = String::new();
    let mut buf = [0u8; 32];
    loop {
        let n = stdout.read(&mut buf).unwrap();
        if n == 0 {
            break;
        }
        out.push_str(&String::from_utf8_lossy(&buf[..n]));
        if out.contains("READY") {
            break;
        }
    }
    assert!(dir.join("store.tmp").exists(), "temp snapshot missing before kill: {out}");
    let _ = child.kill();
    let _ = child.wait();
    let loaded = chisel::store::Store::open(&dir).unwrap();
    assert_eq!(loaded.doc.get("token").and_then(|v| v.as_str()), Some("first"));
    assert!(!loaded.snapshot_path().to_string_lossy().contains("chisel.db"));
}

#[test]
fn shared_reader_never_sees_a_torn_snapshot() {
    let dir = scratch();
    let mut store = chisel::store::Store::open(&dir).unwrap();
    store.doc.insert("token", chisel::json::Value::String("0".into()));
    store.doc.insert("token_copy", chisel::json::Value::String("0".into()));
    store.save().unwrap();

    let mut reader = spawn_helper("helper_read_loop", &dir, Stdio::piped());
    std::thread::sleep(std::time::Duration::from_millis(30));
    for i in 1..=30 {
        let token = i.to_string();
        store.doc.insert("token", chisel::json::Value::String(token.clone()));
        store.doc.insert("token_copy", chisel::json::Value::String(token));
        store.save().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    std::thread::sleep(std::time::Duration::from_millis(50));
    let _ = reader.kill();
    let output = reader.wait_with_output().unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!text.contains("TORN"), "{text}");
    assert!(text.contains("OK"), "reader produced no consistent samples: {text:?}");
}

#[test]
fn two_writers_do_not_silently_overwrite() {
    let dir = scratch();
    let mut first = chisel::store::Store::open(&dir).unwrap();
    let mut second = chisel::store::Store::open(&dir).unwrap();
    first.doc.insert("token", chisel::json::Value::String("winner".into()));
    first.save().unwrap();
    second.doc.insert("token", chisel::json::Value::String("loser".into()));
    let err = second.save().unwrap_err();
    assert!(err.to_string().contains("stale"), "{err}");
    let loaded = chisel::store::Store::open(&dir).unwrap();
    assert_eq!(loaded.doc.get("token").and_then(|v| v.as_str()), Some("winner"));
    assert!(!dir.join("store.tmp").exists());
}

fn lock_paths(snapshot: &str) -> Vec<String> {
    let doc = chisel::json::parse(snapshot).unwrap();
    doc.get("file_locks")
        .and_then(|v| v.as_array())
        .unwrap_or(&[])
        .iter()
        .filter_map(|row| row.get("file_path").and_then(|v| v.as_str()).map(|s| s.to_string()))
        .collect()
}

#[test]
fn optimize_storage_drops_expired_locks_from_the_snapshot() {
    let dir = scratch();
    let project = dir.join("proj");
    let storage = dir.join("storage");
    fs::create_dir_all(&project).unwrap();
    let run = |args: &[&str], clock: &str| {
        let out = Command::new(bin())
            .args(args)
            .arg("--json")
            .arg("--project-dir")
            .arg(&project)
            .arg("--storage-dir")
            .arg(&storage)
            .env("CHISEL_NOW", clock)
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?} stderr {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let now = "1700000000";
    run(&["acquire-lock", "src/live.py", "live", "--ttl", "1000"], now);
    run(&["acquire-lock", "src/old.py", "old", "--ttl", "10"], now);
    let before = fs::read_to_string(storage.join("store")).unwrap();
    let mut before_paths = lock_paths(&before);
    before_paths.sort();
    assert_eq!(before_paths, vec!["src/live.py".to_string(), "src/old.py".to_string()], "{before}");
    let optimized = run(&["optimize-storage"], "1700000020");
    let body = chisel::json::parse(optimized.trim()).unwrap();
    assert_eq!(body.get("optimized").and_then(|v| v.as_bool()), Some(true), "{optimized}");
    let after = fs::read_to_string(storage.join("store")).unwrap();
    assert_eq!(lock_paths(&after), vec!["src/live.py".to_string()], "{after}");
    assert!(!after.contains("src/old.py"), "{after}");
}
