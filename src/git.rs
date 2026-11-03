//! Git porcelain via the system git binary. Lossy UTF-8. No libgit2.

use std::process::{Command, Output};
use std::sync::Arc;
use std::time::Duration;

/// `repo_dir` plus the argument list. Tests wrap this to count spawns and still call git.
pub type GitRunner = Arc<dyn Fn(&str, &[&str]) -> Result<Output, std::io::Error> + Send + Sync>;

pub fn real_git_runner() -> GitRunner {
    Arc::new(|repo, args| {
        let mut cmd = Command::new("git");
        cmd.args(args).current_dir(repo);
        cmd.output()
    })
}

#[derive(Clone, Debug)]
pub struct GitFile {
    pub path: String,
    pub insertions: i64,
    pub deletions: i64,
}

#[derive(Clone, Debug)]
pub struct Commit {
    pub hash: String,
    pub author: String,
    pub author_email: String,
    pub date: String,
    pub message: String,
    pub files: Vec<GitFile>,
}

#[derive(Clone, Debug)]
pub struct BlameBlock {
    pub commit_hash: String,
    pub author: String,
    pub author_email: String,
    pub line_start: usize,
    pub line_end: usize,
}

#[derive(Debug)]
pub struct GitError(pub String);

fn guard_ref(r: &str) -> Result<(), GitError> {
    if r.starts_with('-') {
        return Err(GitError("ref must not start with '-'".into()));
    }
    Ok(())
}

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

pub struct GitAnalyzer {
    pub repo_dir: String,
    runner: GitRunner,
}

impl GitAnalyzer {
    pub fn new(repo_dir: impl Into<String>) -> Self {
        Self { repo_dir: repo_dir.into(), runner: real_git_runner() }
    }

    pub fn set_runner(&mut self, runner: GitRunner) {
        self.runner = runner;
    }

    pub fn fork(&self) -> GitAnalyzer {
        GitAnalyzer { repo_dir: self.repo_dir.clone(), runner: Arc::clone(&self.runner) }
    }

    fn run(&self, args: &[&str]) -> Result<String, GitError> {
        let output = (self.runner)(&self.repo_dir, args).map_err(|e| GitError(format!("git {} failed: {e}", args.join(" "))))?;
        finish(args, output)
    }

    pub fn parse_log(&self) -> Result<Vec<Commit>, GitError> {
        self.parse_log_since(None)
    }

    pub fn parse_log_since(&self, since: Option<&str>) -> Result<Vec<Commit>, GitError> {
        let since_arg;
        let mut args = vec!["log", "--format=COMMIT_START%n%H|%an|%ae|%aI|%s", "--numstat"];
        if let Some(since) = since {
            if !since.is_empty() {
                since_arg = format!("--since={since}");
                args.push(&since_arg);
            }
        }
        let raw = self.run(&args)?;
        Ok(parse_log_output(&raw))
    }

    pub fn current_branch(&self) -> Result<String, GitError> {
        Ok(self.run(&["rev-parse", "--abbrev-ref", "HEAD"])?.trim().to_string())
    }

    pub fn branch_exists(&self, name: &str) -> bool {
        if name.starts_with('-') {
            return false;
        }
        self.run(&["rev-parse", "--verify", "--end-of-options", name]).is_ok()
    }

    pub fn merge_base(&self, base: &str) -> Result<String, GitError> {
        guard_ref(base)?;
        Ok(self.run(&["merge-base", "HEAD", "--end-of-options", base])?.trim().to_string())
    }

    pub fn parse_log_range(&self, range: &str) -> Result<Vec<Commit>, GitError> {
        guard_ref(range)?;
        let raw = self.run(&["log", "--format=COMMIT_START%n%H|%an|%ae|%aI|%s", "--numstat", "--end-of-options", range])?;
        Ok(parse_log_output(&raw))
    }

    pub fn changed_files(&self, r#ref: &str) -> Result<Vec<String>, GitError> {
        guard_ref(r#ref)?;
        let raw = self.run(&["diff", "--name-only", "--end-of-options", r#ref])?;
        Ok(raw.lines().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
    }

    pub fn untracked(&self) -> Result<Vec<String>, GitError> {
        let raw = self.run(&["ls-files", "--others", "--exclude-standard"])?;
        Ok(raw.lines().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
    }

    pub fn ls_code_candidates(&self) -> Result<Vec<String>, GitError> {
        let raw = self.run(&["ls-files", "--cached", "--others", "--exclude-standard"])?;
        Ok(raw.lines().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
    }

    pub fn changed_functions(&self, file: &str, r#ref: &str) -> Result<Vec<String>, GitError> {
        guard_ref(r#ref)?;
        let raw = self.run(&["diff", "-U0", "--end-of-options", r#ref, "--", file])?;
        Ok(parse_diff_functions(&raw))
    }

    pub fn function_log(&self, file: &str, func: &str) -> Result<Vec<Commit>, GitError> {
        let spec = format!("-L:{func}:{file}");
        let raw = self.run(&["log", "--format=COMMIT_START%n%H|%an|%ae|%aI|%s", &spec])?;
        Ok(parse_log_output(&raw))
    }

    pub fn blame(&self, file: &str) -> Result<Vec<BlameBlock>, GitError> {
        let raw = self.run(&["blame", "--porcelain", "--", file])?;
        Ok(parse_blame(&raw))
    }

    pub fn toplevel(&self) -> Result<String, GitError> {
        Ok(self.run(&["rev-parse", "--show-toplevel"])?.trim().to_string())
    }
}

fn finish(args: &[&str], output: Output) -> Result<String, GitError> {
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(GitError(format!(
            "git {} failed (rc={}): {}",
            args.join(" "),
            output.status.code().unwrap_or(-1),
            err.trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn utc_z(date: &str) -> String {
    date.strip_suffix("+00:00").map(|prefix| format!("{prefix}Z")).unwrap_or_else(|| date.to_string())
}

pub fn parse_log_output(raw: &str) -> Vec<Commit> {
    let mut commits = Vec::new();
    for block in raw.split("COMMIT_START") {
        let block = block.trim();
        if block.is_empty() {
            continue;
        }
        let mut lines = block.split('\n');
        let Some(header) = lines.next() else { continue };
        let parts: Vec<&str> = header.trim().splitn(5, '|').collect();
        if parts.len() < 5 {
            continue;
        }
        let mut commit = Commit {
            hash: parts[0].into(),
            author: parts[1].into(),
            author_email: parts[2].into(),
            date: utc_z(parts[3]),
            message: parts[4].into(),
            files: Vec::new(),
        };
        let mut diff_ins = 0i64;
        let mut diff_del = 0i64;
        let mut has_numstat = false;
        for line in lines {
            let line = line.trim_end();
            if line.is_empty() {
                continue;
            }
            let file_parts: Vec<&str> = line.split('\t').collect();
            if file_parts.len() == 3 {
                let (ins, del, path) = (file_parts[0], file_parts[1], file_parts[2]);
                let ins_ok = ins == "-" || ins.chars().all(|c| c.is_ascii_digit());
                let del_ok = del == "-" || del.chars().all(|c| c.is_ascii_digit());
                if ins_ok && del_ok {
                    has_numstat = true;
                    commit.files.push(GitFile {
                        path: path.into(),
                        insertions: if ins == "-" { 0 } else { ins.parse().unwrap_or(0) },
                        deletions: if del == "-" { 0 } else { del.parse().unwrap_or(0) },
                    });
                    continue;
                }
            }
            if !has_numstat {
                let trimmed = line.trim();
                if trimmed.starts_with('+') && !trimmed.starts_with("+++") {
                    diff_ins += 1;
                } else if trimmed.starts_with('-') && !trimmed.starts_with("---") {
                    diff_del += 1;
                }
            }
        }
        if !has_numstat && (diff_ins > 0 || diff_del > 0) {
            commit.files.push(GitFile { path: String::new(), insertions: diff_ins, deletions: diff_del });
        }
        commits.push(commit);
    }
    let _ = Duration::from_secs(0);
    commits
}

pub fn parse_diff_functions(raw: &str) -> Vec<String> {
    let mut functions = Vec::new();
    for line in raw.lines() {
        let Some(rest) = line.strip_prefix("@@") else { continue };
        let Some(idx) = rest.find("@@") else { continue };
        let context = rest[idx + 2..].trim();
        if context.is_empty() {
            continue;
        }
        if let Some(name) = func_name_from_context(context) {
            if !functions.contains(&name) {
                functions.push(name);
            }
        }
    }
    functions
}

fn func_name_from_context(context: &str) -> Option<String> {
    for key in ["async def ", "def ", "func ", "fn ", "fun ", "function ", "async function "] {
        if let Some(i) = context.find(key) {
            let rest = &context[i + key.len()..];
            let name: String = rest.chars().skip_while(|c| *c == '(' || c.is_whitespace()).take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    // C-like: last word before '('
    let paren = context.find('(')?;
    let head = context[..paren].trim();
    let name = head.split_whitespace().next_back()?;
    let name = name.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_');
    if name.is_empty() { None } else { Some(name.to_string()) }
}

pub fn parse_blame(raw: &str) -> Vec<BlameBlock> {
    let mut per_line: Vec<(String, String, String, usize)> = Vec::new();
    let mut author = String::new();
    let mut email = String::new();
    let mut hash = String::new();
    let mut info: Vec<(String, String, String)> = Vec::new();
    for line in raw.lines() {
        if line.starts_with('\t') {
            if let Some((_, _, _, line_no)) = per_line.last() {
                let _ = line_no;
            }
            continue;
        }
        let mut parts = line.split_whitespace();
        let first = parts.next().unwrap_or("");
        if first.len() == 40 && first.chars().all(|c| c.is_ascii_hexdigit()) {
            hash = first.to_string();
            let _orig = parts.next();
            let final_line: usize = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            if let Some((_, a, e)) = info.iter().find(|(h, _, _)| h == &hash) {
                author = a.clone();
                email = e.clone();
            }
            if final_line > 0 {
                per_line.push((hash.clone(), author.clone(), email.clone(), final_line));
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("author ") {
            author = rest.to_string();
        } else if let Some(rest) = line.strip_prefix("author-mail ") {
            email = rest.trim_matches(|c| c == '<' || c == '>').to_string();
        }
        if !hash.is_empty() && !author.is_empty() {
            if let Some(slot) = info.iter_mut().find(|(h, _, _)| h == &hash) {
                slot.1 = author.clone();
                slot.2 = email.clone();
            } else {
                info.push((hash.clone(), author.clone(), email.clone()));
            }
        }
    }
    // The porcelain header's second number is the final line. We pushed on the hash line
    // before author lines for subsequent groups. Re-parse more carefully below if empty authors.
    let mut blocks: Vec<BlameBlock> = Vec::new();
    for (h, a, e, line_no) in per_line {
        let (author, email) = info
            .iter()
            .find(|(hh, _, _)| hh == &h)
            .map(|(_, a2, e2)| (a2.clone(), e2.clone()))
            .unwrap_or((a, e));
        if let Some(last) = blocks.last_mut() {
            if last.commit_hash == h && last.line_end + 1 == line_no {
                last.line_end = line_no;
                continue;
            }
        }
        blocks.push(BlameBlock {
            commit_hash: h,
            author,
            author_email: email,
            line_start: line_no,
            line_end: line_no,
        });
    }
    blocks
}

pub fn git_tool_error(project_dir: &str, message: &str) -> crate::json::Value {
    use crate::json::Value;
    let low = message.to_lowercase();
    let err = if low.contains("not a git repository") || low.contains("not a git repo") {
        "not_a_git_repo"
    } else {
        "git_command_failed"
    };
    Value::object(vec![
        ("status".into(), Value::String("git_error".into())),
        ("error".into(), Value::String(err.into())),
        ("message".into(), Value::String(message.into())),
        ("project_dir".into(), Value::String(project_dir.into())),
        ("cwd".into(), Value::String(project_dir.into())),
        ("hint".into(), Value::String(
            "Use the repository root as the project directory: `chisel ... --project-dir /path/to/repo` or start the MCP server with cwd set to the git checkout.".into(),
        )),
    ])
}
