use chisel::cli;
use std::env;
#[cfg(debug_assertions)]
use chisel::json::Value;
#[cfg(debug_assertions)]
use chisel::store::Store;
#[cfg(debug_assertions)]
use std::io::{self, Write};
#[cfg(debug_assertions)]
use std::path::PathBuf;
#[cfg(debug_assertions)]
use std::thread;
#[cfg(debug_assertions)]
use std::time::Duration;

fn main() {
    let code = match real_main() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {e}");
            1
        }
    };
    std::process::exit(code);
}

fn real_main() -> Result<i32, String> {
    let mut argv: Vec<String> = env::args().skip(1).collect();
    #[cfg(debug_assertions)]
    if argv.first().map(|s| s.as_str()) == Some("internal-pause-before-rename") {
        return pause_before_rename(&argv);
    }
    #[cfg(debug_assertions)]
    if argv.first().map(|s| s.as_str()) == Some("internal-read-loop") {
        return read_loop(&argv);
    }
    let argv0 = env::args().next().unwrap_or_default();
    if cli::argv_is_mcp_binary(&argv0) {
        argv.insert(0, "serve-mcp".into());
    }
    if argv.is_empty() || argv.iter().any(|a| a == "-h" || a == "--help") && argv.len() == 1 {
        cli::help();
        return Ok(0);
    }
    let args = cli::parse(&argv)?;
    cli::run(&args)
}

#[cfg(debug_assertions)]
fn flag_value(argv: &[String], name: &str) -> Option<String> {
    argv.iter().position(|a| a == name).and_then(|i| argv.get(i + 1).cloned())
}

#[cfg(debug_assertions)]
fn pause_before_rename(argv: &[String]) -> Result<i32, String> {
    let dir = PathBuf::from(flag_value(argv, "--storage-dir").ok_or("missing --storage-dir")?);
    let mut store = Store::open(&dir)?;
    let current = store.doc.get("token").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let next = format!("{current}-pending");
    store.doc.insert("token", Value::String(next.clone()));
    store.doc.insert("token_copy", Value::String(next));
    {
        let _guard = store.hold_exclusive_after_temp()?;
        println!("READY");
        let _ = io::stdout().flush();
        thread::sleep(Duration::from_secs(30));
    }
    store.save()?;
    Ok(0)
}

#[cfg(debug_assertions)]
fn read_loop(argv: &[String]) -> Result<i32, String> {
    let dir = PathBuf::from(flag_value(argv, "--storage-dir").ok_or("missing --storage-dir")?);
    let start = std::time::Instant::now();
    let mut samples = 0;
    while start.elapsed() < Duration::from_millis(800) {
        let store = Store::open(&dir)?;
        let token = store.doc.get("token").and_then(|v| v.as_str()).unwrap_or("");
        let copy = store.doc.get("token_copy").and_then(|v| v.as_str()).unwrap_or("");
        if token != copy {
            println!("TORN {token} {copy}");
            let _ = io::stdout().flush();
            return Ok(2);
        }
        samples += 1;
        println!("OK");
        let _ = io::stdout().flush();
        thread::sleep(Duration::from_millis(5));
    }
    println!("DONE samples={samples}");
    let _ = io::stdout().flush();
    Ok(0)
}
