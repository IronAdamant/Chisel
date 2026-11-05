//! `chisel::history` against real `git log -L` and `git blame`.
//!
//! The fixture test always runs. `CHISEL_VERIFY_REPO=/path cargo test --test history_parity -- --ignored`
//! checks every function and file of another repository (read-only).

use chisel::history::{self, History};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git").args(args).current_dir(dir).output().unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn git_ok(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).current_dir(dir).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn load(dir: &Path, heuristic: bool) -> History {
    let args = history::stream_args(heuristic, &history::pathspecs());
    let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let mut h = History::new(history::parse_stream(&git(dir, &refs)));
    let ambiguous = h.ambiguous_merges();
    if !ambiguous.is_empty() {
        let args = history::first_parent_args(&ambiguous, &history::pathspecs());
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        h.resolve_first_parents(&git(dir, &refs));
    }
    h
}

/// (hash, +, -) for each commit `git log -L` prints, counting real diff lines only.
fn real_line_log(dir: &Path, name: &str, path: &str) -> Option<Vec<(String, i64, i64)>> {
    let raw = git_ok(dir, &["log", "--no-color", "--format=COMMIT_START%n%H", &format!("-L:{name}:{path}")])?;
    let mut out: Vec<(String, i64, i64)> = Vec::new();
    let mut in_hunk = false;
    let mut expect_hash = false;
    for line in raw.split('\n') {
        if line == "COMMIT_START" {
            expect_hash = true;
            in_hunk = false;
            continue;
        }
        if expect_hash {
            out.push((line.trim().to_string(), 0, 0));
            expect_hash = false;
            continue;
        }
        if line.starts_with("diff --git") {
            in_hunk = false;
        } else if line.starts_with("@@") {
            in_hunk = true;
        } else if in_hunk {
            if let Some(last) = out.last_mut() {
                if line.starts_with('+') {
                    last.1 += 1;
                } else if line.starts_with('-') {
                    last.2 += 1;
                }
            }
        }
    }
    Some(out)
}

#[derive(Default, Debug)]
struct Tally {
    functions: usize,
    function_mismatch: Vec<String>,
    function_skipped: usize,
    files: usize,
    blame_mismatch: Vec<String>,
}

fn compare(dir: &Path, limit: usize) -> Tally {
    let line_history = load(dir, false);
    let blame_history = load(dir, true);
    let dirty: BTreeSet<String> = git(dir, &["-c", "core.quotePath=false", "diff", "--name-only", "HEAD"]).lines().map(String::from).collect();
    let mut tally = Tally::default();
    let files: Vec<String> = git(dir, &["-c", "core.quotePath=false", "ls-tree", "-r", "--name-only", "HEAD"])
        .lines()
        .filter(|p| chisel::extract::path_has_code_extension(p) && !dirty.contains(*p))
        .map(String::from)
        .collect();
    for path in files.iter().take(limit) {
        let attr = git(dir, &["check-attr", "diff", "--", path]);
        if !(attr.trim_end().ends_with("unspecified") || attr.trim_end().ends_with(": set")) {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(dir.join(path)) else { continue };
        let lines = history::split_lines(&content);
        tally.files += 1;
        let real_blame = chisel::git::parse_blame(&git(dir, &["blame", "--porcelain", "--", path]));
        let ours = blame_history.blame(path, lines.len());
        let same = ours.as_ref().is_some_and(|b| {
            b.len() == real_blame.len()
                && b.iter().zip(&real_blame).all(|(x, y)| (x.line_start, x.line_end, &x.commit_hash, &x.author, &x.author_email) == (y.line_start, y.line_end, &y.commit_hash, &y.author, &y.author_email))
        });
        if !same {
            tally.blame_mismatch.push(format!("{path}: ours {:?} real {:?}", ours.map(|b| b.len()), real_blame.len()));
        }
        let units = chisel::extract::extract_code_units(path, &content);
        let names: BTreeSet<String> = units
            .iter()
            .filter(|u| u.unit_type == "function" || u.unit_type == "async_function")
            .map(|u| u.name.rsplit('.').next().unwrap_or(&u.name).to_string())
            .collect();
        let names: Vec<String> = names.into_iter().collect();
        let ranges: Vec<Option<(usize, usize)>> = names.iter().map(|n| history::funcname_range(&lines, n)).collect();
        let found: Vec<(usize, usize)> = ranges.iter().flatten().copied().collect();
        let logs = line_history.line_logs(path, &found);
        let mut k = 0;
        for (name, range) in names.iter().zip(&ranges) {
            let real = real_line_log(dir, name, path);
            tally.functions += 1;
            let ours: Option<Vec<(String, i64, i64)>> = match range {
                None => None,
                Some(_) => {
                    let shown = logs.as_ref().map(|l| l[k].iter().map(|s| (line_history.commits[s.commit].hash.clone(), s.insertions, s.deletions)).collect());
                    k += 1;
                    if shown.is_none() {
                        tally.function_skipped += 1;
                        continue;
                    }
                    shown
                }
            };
            let real_known = real.filter(|r| !r.is_empty());
            if ours != real_known {
                tally.function_mismatch.push(format!("{path}:{name}: ours {ours:?} real {real_known:?}"));
            }
        }
    }
    tally
}

fn scratch() -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let p = std::env::temp_dir().join(format!("chisel-history-{t}-{}", N.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn run(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_DATE", "2024-03-01T00:00:00+00:00")
        .env("GIT_COMMITTER_DATE", "2024-03-01T00:00:00+00:00")
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

fn commit(dir: &Path, files: &[(&str, &str)], msg: &str) {
    for (path, text) in files {
        let p = dir.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    run(dir, &["add", "-A"]);
    run(dir, &["commit", "-q", "-m", msg]);
}

/// Edits above, inside and below functions, a rename, a deleted line inside a range, and a
/// merge that changes a function on both sides, checked against real git.
#[test]
fn replays_line_log_and_blame_like_git() {
    let dir = scratch();
    run(&dir, &["init", "-q", "-b", "main"]);
    for (k, v) in [("user.email", "a@example.com"), ("user.name", "Ann"), ("commit.gpgsign", "false"), ("core.autocrlf", "false")] {
        run(&dir, &["config", k, v]);
    }
    commit(&dir, &[("src/a.py", "import os\n\ndef alpha(x):\n    return x\n\n\ndef beta(y):\n    y += 1\n    return y\n")], "one");
    commit(&dir, &[("src/a.py", "import os\nimport sys\n\ndef alpha(x):\n    x = abs(x)\n    return x\n\n\ndef beta(y):\n    y += 1\n    return y\n")], "two");
    run(&dir, &["mv", "src/a.py", "src/b.py"]);
    commit(&dir, &[], "rename");
    run(&dir, &["checkout", "-q", "-b", "side"]);
    run(&dir, &["config", "user.name", "Bob"]);
    commit(&dir, &[("src/b.py", "import os\nimport sys\n\ndef alpha(x):\n    x = abs(x)\n    return x\n\n\ndef beta(y):\n    return y + 1\n")], "side edits beta");
    run(&dir, &["checkout", "-q", "main"]);
    run(&dir, &["config", "user.name", "Ann"]);
    commit(&dir, &[("src/b.py", "import os\nimport sys\n\ndef alpha(x):\n    x = abs(x)\n    return x * 1\n\n\ndef beta(y):\n    y += 1\n    return y\n")], "main edits alpha");
    run(&dir, &["merge", "-q", "--no-edit", "side"]);
    commit(&dir, &[("src/b.py", "import os\nimport sys\n\ndef alpha(x):\n    x = abs(x)\n    return x * 1\n\n\ndef beta(y):\n    return y + 1\n\n\ndef gamma():\n    pass\n")], "gamma");
    let tally = compare(&dir, usize::MAX);
    assert_eq!(tally.files, 1, "{tally:?}");
    assert_eq!(tally.functions, 3, "{tally:?}");
    assert!(tally.function_mismatch.is_empty() && tally.blame_mismatch.is_empty() && tally.function_skipped == 0, "{tally:#?}");
}

#[test]
#[ignore]
fn matches_git_on_the_repo_in_chisel_verify_repo() {
    let Ok(repo) = std::env::var("CHISEL_VERIFY_REPO") else { return };
    let limit = std::env::var("CHISEL_VERIFY_LIMIT").ok().and_then(|v| v.parse().ok()).unwrap_or(usize::MAX);
    let tally = compare(Path::new(&repo), limit);
    eprintln!(
        "files {} blame mismatches {} | functions {} mismatches {} skipped {}",
        tally.files,
        tally.blame_mismatch.len(),
        tally.functions,
        tally.function_mismatch.len(),
        tally.function_skipped
    );
    for m in tally.blame_mismatch.iter().chain(&tally.function_mismatch).take(20) {
        eprintln!("  {m}");
    }
    assert!(tally.function_mismatch.is_empty() && tally.blame_mismatch.is_empty(), "see stderr");
}
