//! Kill a writer after the temp snapshot exists, and prove readers never see a tear.

use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_chisel")
}

fn scratch() -> PathBuf {
    let n = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let p = std::env::temp_dir().join(format!("chisel-dur-{n}"));
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

    let mut child = Command::new(bin())
        .args(["internal-pause-before-rename", "--storage-dir"])
        .arg(&dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
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

    let mut reader = Command::new(bin())
        .args(["internal-read-loop", "--storage-dir"])
        .arg(&dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
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
