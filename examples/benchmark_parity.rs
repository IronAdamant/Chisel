//! Compare Chisel Python 0.15.1 with the Rust binary.
//!
//! Five runs per command. Prints median milliseconds and the min-max range,
//! plus peak RSS in kilobytes when /proc can measure it.
//!
//! Usage:
//!   cargo run --release --example benchmark_parity -- \
//!     --rust target/release/chisel \
//!     --python /path/to/venv/bin/chisel \
//!     --small /tmp/chisel-bench-small \
//!     --large /tmp/django

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const RUNS: usize = 5;
const ORDER: [&str; 12] = [
    "startup", "cold_analyze", "warm_analyze", "warm_update", "impact", "suggest_tests",
    "risk_map", "test_gaps", "triage", "churn", "coupling", "stats",
];
const USAGE: &str = "usage: benchmark_parity --rust BIN --python BIN --small DIR --large DIR [--small-file PATH] [--large-file PATH] [--out FILE]";

#[derive(Debug, PartialEq)]
struct Args {
    rust: String,
    python: String,
    small: String,
    large: String,
    small_file: String,
    large_file: String,
    out: String,
}

/// `--name value` or `--name=value`. Errors exit 2 with the usage line.
fn parse_args(argv: &[String]) -> Result<Args, String> {
    let mut found: BTreeMap<&str, String> = BTreeMap::new();
    let names = ["rust", "python", "small", "large", "small-file", "large-file", "out"];
    let mut i = 0;
    while i < argv.len() {
        let arg = &argv[i];
        let (key, inline) = match arg.split_once('=') {
            Some((k, v)) if k.starts_with("--") => (k, Some(v.to_string())),
            _ => (arg.as_str(), None),
        };
        let name = key.strip_prefix("--").and_then(|k| names.iter().find(|n| **n == k)).ok_or_else(|| format!("unknown argument {arg}; expected one of --{}", names.join(", --")))?;
        let value = match inline {
            Some(v) => v,
            None => {
                i += 1;
                argv.get(i).filter(|v| !v.starts_with("--")).cloned().ok_or_else(|| format!("--{name} needs a value"))?
            }
        };
        found.insert(name, value);
        i += 1;
    }
    let missing: Vec<String> = ["rust", "python", "small", "large"].iter().filter(|n| !found.contains_key(**n)).map(|n| format!("--{n}")).collect();
    if !missing.is_empty() {
        return Err(format!("missing {}", missing.join(", ")));
    }
    let mut take = |name: &str, default: &str| found.remove(name).unwrap_or_else(|| default.to_string());
    Ok(Args {
        rust: take("rust", ""),
        python: take("python", ""),
        small: take("small", ""),
        large: take("large", ""),
        small_file: take("small-file", "src/calc.py"),
        large_file: take("large-file", "django/http/request.py"),
        out: take("out", "scripts/benchmark_results.md"),
    })
}

fn capture(cmd: &[&str]) -> String {
    match Command::new(cmd[0]).args(&cmd[1..]).stderr(Stdio::null()).output() {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        _ => "unavailable".into(),
    }
}

fn uname(flag: &str) -> Option<String> {
    let value = capture(&["uname", flag]);
    (value != "unavailable" && !value.is_empty() && value != "unknown").then_some(value)
}

fn machine_spec() -> Vec<(String, String)> {
    let machine = std::env::var("PROCESSOR_ARCHITECTURE").ok().or_else(|| uname("-m")).unwrap_or_else(|| std::env::consts::ARCH.to_string());
    let system = match (uname("-s"), uname("-r")) {
        (Some(s), Some(r)) if !cfg!(windows) => format!("{s}-{r}-{machine}"),
        _ => format!("{}-{}", std::env::consts::OS, machine),
    };
    let python = ["python3", "python"].iter().map(|p| capture(&[p, "--version"])).find(|v| v.starts_with("Python ")).map(|v| v["Python ".len()..].to_string()).unwrap_or_else(|| "unavailable".into());
    let cpu = std::env::var("PROCESSOR_IDENTIFIER").ok().or_else(|| uname("-p")).unwrap_or_else(|| "unknown".into());
    let mut spec = vec![
        ("system".to_string(), system),
        ("python".to_string(), python),
        ("cpu".to_string(), cpu),
        ("machine".to_string(), machine),
    ];
    for (key, cmd) in [
        ("uname", vec!["uname", "-a"]),
        ("cpu_model", vec!["bash", "-lc", "lscpu | awk -F: '/Model name/ {print $2; exit}'"]),
        ("nproc", vec!["nproc"]),
        ("mem_gb", vec!["bash", "-lc", "awk '/MemTotal/ {printf \"%.1f\", $2/1024/1024}' /proc/meminfo"]),
        ("rustc", vec!["rustc", "--version"]),
    ] {
        spec.push((key.to_string(), capture(&cmd)));
    }
    spec
}

fn peak_rss_kb(pid: u32) -> Option<u64> {
    let text = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let line = text.lines().find(|l| l.starts_with("VmHWM:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

#[derive(Clone, Debug)]
struct Sample {
    ms: f64,
    rss_kb: Option<u64>,
    code: i32,
    stderr: String,
}

fn measure(cmd: &[String], cwd: &Path) -> Sample {
    let child = Command::new(&cmd[0]).args(&cmd[1..]).current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn();
    let child = match child {
        Ok(c) => c,
        Err(e) => return Sample { ms: 0.0, rss_kb: None, code: -1, stderr: e.to_string() },
    };
    let pid = child.id();
    let peak = Arc::new(AtomicU64::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let sampler = {
        let (peak, stop) = (Arc::clone(&peak), Arc::clone(&stop));
        std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                if let Some(v) = peak_rss_kb(pid) {
                    peak.fetch_max(v, Ordering::SeqCst);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        })
    };
    let start = Instant::now();
    let out = child.wait_with_output();
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    stop.store(true, Ordering::SeqCst);
    let _ = sampler.join();
    let (code, stderr) = match out {
        Ok(o) => (o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stderr).into_owned()),
        Err(e) => (-1, e.to_string()),
    };
    let tail: String = {
        let chars: Vec<char> = stderr.chars().collect();
        chars[chars.len().saturating_sub(500)..].iter().collect()
    };
    let rss = peak.load(Ordering::SeqCst);
    Sample { ms, rss_kb: (rss > 0).then_some(rss), code, stderr: tail }
}

#[derive(Clone, Debug, PartialEq)]
enum Summary {
    Ok { median_ms: f64, min_ms: f64, max_ms: f64, n: usize, peak_rss_kb: Option<u64> },
    Failed { error: String },
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

fn summarise(samples: &[Sample]) -> Summary {
    let mut values: Vec<f64> = samples.iter().filter(|s| s.code == 0).map(|s| s.ms).collect();
    if values.is_empty() {
        return Summary::Failed { error: samples.last().map(|s| s.stderr.clone()).unwrap_or_else(|| "no runs".into()) };
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let rss = samples.iter().filter(|s| s.code == 0).filter_map(|s| s.rss_kb).max();
    Summary::Ok {
        median_ms: round1(values[values.len() / 2]),
        min_ms: round1(values[0]),
        max_ms: round1(values[values.len() - 1]),
        n: values.len(),
        peak_rss_kb: rss,
    }
}

fn fmt(summary: Option<&Summary>) -> String {
    match summary {
        Some(Summary::Ok { median_ms, min_ms, max_ms, .. }) => format!("{median_ms:.1} ({min_ms:.1}\u{2013}{max_ms:.1})"),
        _ => "not measured".into(),
    }
}

fn git(dir: &Path, args: &[&str], dated: bool) {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(dir);
    if dated {
        cmd.env("GIT_AUTHOR_DATE", "2024-01-01T00:00:00Z").env("GIT_COMMITTER_DATE", "2024-01-01T00:00:00Z");
    }
    let status = cmd.status().unwrap_or_else(|e| fail(&format!("git {}: {e}", args.join(" "))));
    if !status.success() {
        fail(&format!("Command '{}' returned non-zero exit status {}.", ["git"].iter().chain(args).copied().collect::<Vec<_>>().join(" "), status.code().unwrap_or(-1)));
    }
}

fn fail(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(1);
}

fn prepare_small(path: &Path) {
    if path.join(".git").exists() {
        return;
    }
    let write = |rel: &str, text: &str| fs::write(path.join(rel), text).unwrap_or_else(|e| fail(&format!("write {rel}: {e}")));
    fs::create_dir_all(path.join("src")).unwrap_or_else(|e| fail(&e.to_string()));
    fs::create_dir_all(path.join("tests")).unwrap_or_else(|e| fail(&e.to_string()));
    write("src/calc.py", "def add(a, b):\n    return a + b\n\ndef sub(a, b):\n    return a - b\n");
    write("tests/test_calc.py", "from src.calc import add\n\ndef test_add():\n    assert add(1, 1) == 2\n");
    git(path, &["init", "-b", "main"], false);
    git(path, &["config", "user.email", "bench@example.com"], false);
    git(path, &["config", "user.name", "Bench"], false);
    git(path, &["add", "-A"], false);
    git(path, &["commit", "-m", "init"], true);
}

fn run_suite(label: &str, binary: &str, project: &Path, storage: &Path, impact_file: &str) -> BTreeMap<String, Summary> {
    let tail = |cmd: &[&str]| -> Vec<String> {
        let mut v: Vec<String> = std::iter::once(binary).chain(cmd.iter().copied()).map(String::from).collect();
        v.extend(["--json".to_string(), "--project-dir".into(), project.display().to_string(), "--storage-dir".into(), storage.display().to_string()]);
        v
    };
    let commands: Vec<(&str, Vec<String>)> = vec![
        ("startup", vec![binary.to_string(), "--help".into()]),
        ("cold_analyze", tail(&["analyze", "."])),
        ("warm_analyze", tail(&["analyze", "."])),
        ("warm_update", tail(&["update"])),
        ("impact", tail(&["impact", impact_file])),
        ("suggest_tests", tail(&["suggest-tests", impact_file])),
        ("risk_map", tail(&["risk-map"])),
        ("test_gaps", tail(&["test-gaps"])),
        ("triage", tail(&["triage"])),
        ("churn", tail(&["churn", impact_file])),
        ("coupling", tail(&["coupling", impact_file])),
        ("stats", tail(&["stats"])),
    ];
    let here = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut out = BTreeMap::new();
    for (name, cmd) in commands {
        let mut samples = Vec::new();
        for _ in 0..RUNS {
            if name == "cold_analyze" {
                let _ = fs::remove_dir_all(storage);
            } else if name != "startup" && !storage.exists() {
                // Queries need a snapshot. Build one if a previous cold run was skipped.
                let _ = Command::new(binary).args(&tail(&["analyze", "."])[1..]).current_dir(project).output();
            }
            samples.push(measure(&cmd, if name == "startup" { &here } else { project }));
            if samples.last().unwrap().code != 0 && name != "startup" {
                break;
            }
        }
        let summary = summarise(&samples);
        println!("{label} {name}: {}", fmt(Some(&summary)));
        let _ = std::io::stdout().flush();
        out.insert(name.to_string(), summary);
    }
    out
}

fn snapshot_counts(storage: &Path) -> String {
    let snap = storage.join("store");
    if snap.is_file() {
        let text = String::from_utf8_lossy(&fs::read(&snap).unwrap_or_default()).into_owned();
        return format!("rust snapshot bytes {}, test_units mentions {}", text.chars().count(), text.matches("\"framework\"").count());
    }
    let db = storage.join("chisel.db");
    if db.is_file() {
        let count = |t: &str| chisel::sqlite_import::table_row_count(&db, t);
        return match (count("test_units"), count("test_edges")) {
            (Ok(units), Ok(edges)) => format!("python test_units {units}, test_edges {edges}"),
            (Err(e), _) | (_, Err(e)) => format!("python db unreadable: {e}"),
        };
    }
    "no snapshot".into()
}

/// `Path.resolve()`: absolute, symlinks resolved when the path exists, no `\\?\` prefix.
fn resolve(path: &str) -> String {
    let p = Path::new(path);
    let abs = fs::canonicalize(p).unwrap_or_else(|_| std::env::current_dir().map(|d| d.join(p)).unwrap_or_else(|_| p.to_path_buf()));
    let text = abs.display().to_string();
    text.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(text)
}

/// `/tmp/<name>` on Unix. On Windows `/tmp` is relative to the current drive, and the timed
/// commands run from the project's drive, so they would write a different `\tmp` than the
/// one this harness clears and reads. Use the absolute temp directory there.
fn bench_dir(name: &str) -> PathBuf {
    if cfg!(windows) {
        std::env::temp_dir().join(name)
    } else {
        Path::new("/tmp").join(name)
    }
}

type Results = BTreeMap<(&'static str, &'static str), BTreeMap<String, Summary>>;

fn render(spec: &[(String, String)], rust_bin: &str, py_bin: &str, results: &Results, counts: &[String; 4]) -> String {
    let mut lines: Vec<String> = ["# Chisel benchmark results", "", "Generated by `cargo run --release --example benchmark_parity`. Every figure is from a real run.", "", "## Machine", ""].map(String::from).to_vec();
    for (key, value) in spec {
        lines.push(format!("- {key}: {value}"));
    }
    lines.extend(["".into(), format!("Rust binary: `{rust_bin}`"), format!("Python binary: `{py_bin}`"), format!("Runs per command: {RUNS}"), "".into()]);
    lines.push("Times are median milliseconds, with the minimum and maximum in parentheses. Peak RSS is the highest VmHWM sampled from /proc during each run, in KiB.".into());
    lines.push("".into());
    let cells = |name: &str, cell: &dyn Fn(Option<&Summary>) -> String| -> String {
        let row: Vec<String> = [("small", "python"), ("small", "rust"), ("large", "python"), ("large", "rust")].iter().map(|key| cell(results.get(key).and_then(|m| m.get(name)))).collect();
        format!("| {name} | {} |", row.join(" | "))
    };
    lines.push("| Command | Python small | Rust small | Python large | Rust large |".into());
    lines.push("|---|---:|---:|---:|---:|".into());
    for name in ORDER {
        lines.push(cells(name, &fmt));
    }
    lines.extend(["", "## Peak RSS (KiB)", "", "| Command | Python small | Rust small | Python large | Rust large |", "|---|---:|---:|---:|---:|"].map(String::from));
    let rss = |s: Option<&Summary>| match s {
        Some(Summary::Ok { peak_rss_kb: Some(kb), .. }) if *kb > 0 => kb.to_string(),
        _ => "n/a".into(),
    };
    for name in ORDER {
        lines.push(cells(name, &rss));
    }
    lines.extend(["", "## Failures", ""].map(String::from));
    let mut any_fail = false;
    for side in ["small", "large"] {
        for imp in ["python", "rust"] {
            for name in ORDER {
                if let Some(Summary::Failed { error }) = results.get(&(side, imp)).and_then(|m| m.get(name)) {
                    any_fail = true;
                    lines.push(format!("- {imp} {side} {name}: {}", error.trim()));
                }
            }
        }
    }
    if !any_fail {
        lines.push("None.".into());
    }
    lines.extend(["".into(), "## Snapshot counts after the timed runs".into(), "".into()]);
    for (label, count) in ["small rust", "small python", "large rust", "large python"].iter().zip(counts) {
        lines.push(format!("- {label}: {count}"));
    }
    lines.push("".into());
    lines.push("The small repo is a two-file Python sample created by this script. The large repo is the django package from django/django (about 1000 code files) plus 80 of its test modules. History is 38 commits of at most 100 paths each: a shallow clone's boundary commit lists every file, and pairing those files dominates co-change time, so the benchmark uses this bounded history of the same tree. Counts are taken from the snapshot left by the last timed command.".into());
    lines.join("\n") + "\n"
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return;
    }
    let args = parse_args(&argv).unwrap_or_else(|e| {
        eprintln!("benchmark_parity: {e}\n{USAGE}");
        std::process::exit(2);
    });
    let (small, large) = (PathBuf::from(&args.small), PathBuf::from(&args.large));
    prepare_small(&small);
    if !large.is_dir() {
        eprintln!("large repo missing: {}", large.display());
        std::process::exit(1);
    }
    let spec = machine_spec();
    let (rust_bin, py_bin) = (resolve(&args.rust), resolve(&args.python));
    let stores = ["chisel-bench-small-rust", "chisel-bench-small-py", "chisel-bench-large-rust", "chisel-bench-large-py"].map(bench_dir);
    let mut results: Results = BTreeMap::new();
    results.insert(("small", "rust"), run_suite("rust-small", &rust_bin, &small, &stores[0], &args.small_file));
    results.insert(("small", "python"), run_suite("py-small", &py_bin, &small, &stores[1], &args.small_file));
    results.insert(("large", "rust"), run_suite("rust-large", &rust_bin, &large, &stores[2], &args.large_file));
    results.insert(("large", "python"), run_suite("py-large", &py_bin, &large, &stores[3], &args.large_file));
    let counts = stores.each_ref().map(|s| snapshot_counts(s));
    let text = render(&spec, &rust_bin, &py_bin, &results, &counts);
    fs::write(&args.out, &text).unwrap_or_else(|e| fail(&format!("write {}: {e}", args.out)));
    println!("{text}");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(ms: f64, code: i32, rss: Option<u64>) -> Sample {
        Sample { ms, rss_kb: rss, code, stderr: format!("err{ms}") }
    }

    fn argv(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn median_is_the_upper_middle_of_successful_runs() {
        let s = summarise(&[sample(5.0, 0, Some(10)), sample(1.04, 0, None), sample(3.0, 0, Some(30)), sample(99.0, 1, Some(999)), sample(2.0, 0, None)]);
        assert_eq!(s, Summary::Ok { median_ms: 3.0, min_ms: 1.0, max_ms: 5.0, n: 4, peak_rss_kb: Some(30) });
        assert_eq!(fmt(Some(&s)), "3.0 (1.0\u{2013}5.0)");
    }

    #[test]
    fn all_failed_runs_report_the_last_stderr() {
        let s = summarise(&[sample(1.0, 2, None), sample(2.0, 3, None)]);
        assert_eq!(s, Summary::Failed { error: "err2".into() });
        assert_eq!(fmt(Some(&s)), "not measured");
        assert_eq!(fmt(None), "not measured");
        assert_eq!(summarise(&[]), Summary::Failed { error: "no runs".into() });
    }

    #[test]
    fn argument_errors_name_the_fix() {
        let a = parse_args(&argv(&["--rust", "r", "--python=p", "--small", "s", "--large", "l"])).unwrap();
        assert_eq!((a.small_file.as_str(), a.large_file.as_str(), a.out.as_str()), ("src/calc.py", "django/http/request.py", "scripts/benchmark_results.md"));
        assert_eq!(a.python, "p");
        assert_eq!(parse_args(&argv(&["--rust", "r"])).unwrap_err(), "missing --python, --small, --large");
        assert_eq!(parse_args(&argv(&["--rust"])).unwrap_err(), "--rust needs a value");
        assert_eq!(parse_args(&argv(&["--bogus", "x"])).unwrap_err(), "unknown argument --bogus; expected one of --rust, --python, --small, --large, --small-file, --large-file, --out");
    }

    #[test]
    fn snapshot_dirs_are_absolute() {
        let dir = bench_dir("chisel-bench-small-rust");
        assert!(dir.is_absolute(), "{}", dir.display());
        if cfg!(unix) {
            assert_eq!(dir, Path::new("/tmp/chisel-bench-small-rust"));
        }
    }

    #[test]
    fn report_lists_failures_and_missing_rss() {
        let mut results: Results = BTreeMap::new();
        let mut ok = BTreeMap::new();
        ok.insert("stats".to_string(), Summary::Ok { median_ms: 1.0, min_ms: 0.5, max_ms: 2.0, n: 5, peak_rss_kb: None });
        results.insert(("small", "rust"), ok);
        let mut bad = BTreeMap::new();
        bad.insert("stats".to_string(), Summary::Failed { error: " boom \n".into() });
        results.insert(("large", "python"), bad);
        let counts = ["a".to_string(), "b".into(), "c".into(), "d".into()];
        let text = render(&[("nproc".into(), "4".into())], "/r", "/p", &results, &counts);
        assert!(text.contains("| stats | not measured | 1.0 (0.5\u{2013}2.0) | not measured | not measured |"), "{text}");
        assert!(text.contains("| stats | n/a | n/a | n/a | n/a |"), "{text}");
        assert!(text.contains("- python large stats: boom\n"), "{text}");
        assert!(text.contains("- nproc: 4\n") && text.contains("- large python: d\n"), "{text}");
    }
}
