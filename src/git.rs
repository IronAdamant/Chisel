//! Git porcelain via the system git binary. Lossy UTF-8. No libgit2.

use std::collections::BTreeMap;
use std::process::{Command, Output};
use std::sync::Arc;

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

    pub fn head_sha(&self) -> Result<String, GitError> {
        Ok(self.run(&["rev-parse", "HEAD"])?.trim().to_string())
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

    /// `changed_functions` for every file that differs from `ref`, from one `git diff`.
    pub fn changed_functions_all(&self, r#ref: &str) -> Result<BTreeMap<String, Vec<String>>, GitError> {
        guard_ref(r#ref)?;
        let raw = self.run(&["diff", "-U0", "--no-color", "--no-ext-diff", "--src-prefix=a/", "--dst-prefix=b/", "--end-of-options", r#ref])?;
        Ok(split_diff_by_file(&raw).into_iter().map(|(path, section)| (path, parse_diff_functions(&section))).collect())
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

    /// Any git command, for callers that build their own arguments.
    pub fn run_args(&self, args: &[String]) -> Result<String, GitError> {
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        self.run(&refs)
    }

    /// The file as committed at HEAD, without eol or textconv conversion.
    pub fn head_blob(&self, path: &str) -> Result<String, GitError> {
        self.run(&["cat-file", "blob", &format!("HEAD:{path}")])
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

pub(crate) fn utc_z(date: &str) -> String {
    date.strip_suffix("+00:00").map(|prefix| format!("{prefix}Z")).unwrap_or_else(|| date.to_string())
}

/// Commits from `--format=COMMIT_START%n%H|%an|%ae|%aI|%s` output with `--numstat` or a patch
/// (`git log -L`). A patch counts its `+` and `-` lines inside hunks; a file's `---`/`+++`
/// header and unchanged context lines are not changes.
pub fn parse_log_output(raw: &str) -> Vec<Commit> {
    let mut blocks: Vec<Vec<&str>> = Vec::new();
    for line in raw.split('\n') {
        if line.trim_end() == "COMMIT_START" {
            blocks.push(Vec::new());
        } else if let Some(block) = blocks.last_mut() {
            block.push(line);
        }
    }
    let mut commits = Vec::new();
    for block in blocks {
        let mut lines = block.into_iter().skip_while(|l| l.trim().is_empty());
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
        let mut in_hunk = false;
        for line in lines {
            let line = line.trim_end_matches('\r');
            if line.starts_with("diff --git ") {
                in_hunk = false;
                continue;
            }
            if line.starts_with("@@") {
                in_hunk = true;
                continue;
            }
            if in_hunk {
                if line.starts_with('+') {
                    diff_ins += 1;
                } else if line.starts_with('-') {
                    diff_del += 1;
                }
                continue;
            }
            let file_parts: Vec<&str> = line.split('\t').collect();
            if file_parts.len() == 3 {
                let (ins, del, path) = (file_parts[0], file_parts[1], file_parts[2]);
                let ins_ok = ins == "-" || (!ins.is_empty() && ins.chars().all(|c| c.is_ascii_digit()));
                let del_ok = del == "-" || (!del.is_empty() && del.chars().all(|c| c.is_ascii_digit()));
                if ins_ok && del_ok {
                    commit.files.push(GitFile {
                        path: path.into(),
                        insertions: if ins == "-" { 0 } else { ins.parse().unwrap_or(0) },
                        deletions: if del == "-" { 0 } else { del.parse().unwrap_or(0) },
                    });
                }
            }
        }
        if diff_ins > 0 || diff_del > 0 {
            commit.files.push(GitFile { path: String::new(), insertions: diff_ins, deletions: diff_del });
        }
        commits.push(commit);
    }
    commits
}

/// One diff split per file, keyed by the new path (the old path for a deletion).
pub fn split_diff_by_file(raw: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut section = String::new();
    let mut old_path: Option<String> = None;
    let mut new_path: Option<String> = None;
    let mut flush = |section: &mut String, old: &mut Option<String>, new: &mut Option<String>| {
        if let Some(path) = new.take().or_else(|| old.take()) {
            out.push((path, std::mem::take(section)));
        }
        section.clear();
        *old = None;
    };
    for line in raw.split('\n') {
        if line.starts_with("diff --git ") {
            flush(&mut section, &mut old_path, &mut new_path);
        } else if let Some(rest) = line.strip_prefix("--- ").filter(|_| old_path.is_none() && new_path.is_none()) {
            old_path = (rest != "/dev/null").then(|| crate::history::unquote(rest).trim_start_matches("a/").to_string());
        } else if let Some(rest) = line.strip_prefix("+++ ").filter(|_| new_path.is_none()) {
            new_path = (rest != "/dev/null").then(|| crate::history::unquote(rest).trim_start_matches("b/").to_string());
        }
        section.push_str(line);
        section.push('\n');
    }
    flush(&mut section, &mut old_path, &mut new_path);
    out
}

pub fn parse_diff_functions(raw: &str) -> Vec<String> {
    let mut functions = Vec::new();
    for line in raw.lines() {
        // A hunk header names the function *before* the hunk, so an edit that starts on a
        // signature line is credited to the previous function. Also read the changed lines.
        if !line.starts_with("+++") && !line.starts_with("---") {
            if let Some(changed) = line.strip_prefix('+').or_else(|| line.strip_prefix('-')) {
                if let Some(name) = defined_name(changed) {
                    if !functions.contains(&name) {
                        functions.push(name);
                    }
                }
                continue;
            }
        }
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

/// The name a line defines, when the line itself starts a function definition
/// (`pub async fn x(`, `def x(`, `func (r *T) X(`, `export function x(`). Calls never match.
fn defined_name(line: &str) -> Option<String> {
    const MODIFIERS: [&str; 10] = ["pub", "async", "export", "default", "static", "unsafe", "const", "private", "public", "extern"];
    let mut rest = line.trim_start();
    loop {
        let word_end = rest.find(|c: char| c.is_whitespace()).unwrap_or(rest.len());
        let word = &rest[..word_end];
        let modifier = MODIFIERS.contains(&word) || (word.starts_with("pub(") && word.ends_with(')')) || (word.len() > 1 && word.starts_with('"') && word.ends_with('"'));
        rest = rest[word_end..].trim_start();
        if modifier {
            continue;
        }
        if !matches!(word, "fn" | "def" | "func" | "fun" | "function") {
            return None;
        }
        break;
    }
    if rest.starts_with('(') {
        // Go method receiver `(r *T)` comes before the name.
        rest = rest[rest.find(')')? + 1..].trim_start();
    }
    let name: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
    if name.is_empty() { None } else { Some(name) }
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

/// HEAD's commit read from `.git` files, without starting git (that costs 70-180 ms on
/// Windows). Follows `gitdir:` files, `commondir` (worktrees), loose refs and packed-refs.
/// `None` when the layout is unusual; callers may fall back to `git rev-parse HEAD`.
pub fn head_commit_fast(project: &std::path::Path) -> Option<String> {
    let mut dir = project.to_path_buf();
    let gitdir = loop {
        let dot = dir.join(".git");
        if dot.is_dir() {
            break dot;
        }
        if dot.is_file() {
            let text = std::fs::read_to_string(&dot).ok()?;
            let target = text.trim().strip_prefix("gitdir:")?.trim();
            let path = std::path::PathBuf::from(target);
            break if path.is_absolute() { path } else { dir.join(path) };
        }
        dir = dir.parent()?.to_path_buf();
    };
    let head = std::fs::read_to_string(gitdir.join("HEAD")).ok()?;
    let head = head.trim();
    let is_sha = |s: &str| s.len() >= 40 && s.chars().all(|c| c.is_ascii_hexdigit());
    let Some(reference) = head.strip_prefix("ref:").map(str::trim) else {
        return is_sha(head).then(|| head.to_string());
    };
    let common = std::fs::read_to_string(gitdir.join("commondir")).ok().map(|c| {
        let p = std::path::PathBuf::from(c.trim());
        if p.is_absolute() { p } else { gitdir.join(p) }
    });
    for base in [Some(gitdir.clone()), common].into_iter().flatten() {
        if let Ok(text) = std::fs::read_to_string(base.join(reference)) {
            let sha = text.trim().to_string();
            if is_sha(&sha) {
                return Some(sha);
            }
        }
        if let Ok(packed) = std::fs::read_to_string(base.join("packed-refs")) {
            for line in packed.lines() {
                if let Some((sha, name)) = line.split_once(' ') {
                    if name.trim() == reference && is_sha(sha) {
                        return Some(sha.to_string());
                    }
                }
            }
        }
    }
    None
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

#[cfg(test)]
mod tests {
    use super::parse_diff_functions;

    #[test]
    fn line_log_output_counts_only_hunk_changes() {
        use super::parse_log_output;
        // A context line that starts with "-" after indentation is not a deletion. A changed
        // line whose text starts with "++" or "--" is still a change.
        let raw = [
            "COMMIT_START",
            "abc|Ann|a@x|2024-01-01T00:00:00+00:00|subject with COMMIT_START inside",
            "",
            "diff --git a/f.py b/f.py",
            "--- a/f.py",
            "+++ b/f.py",
            "@@ -1,3 +1,3 @@",
            " def f():",
            "-    return 1",
            "+    return 2",
            "     - bullet in a docstring",
            "+++counter",
            "--- a lua comment",
            "COMMIT_START",
            "def|Bob|b@x|2024-01-02T00:00:00+00:00|merge",
            "",
        ]
        .join("\n");
        let raw = raw.as_str();
        let commits = parse_log_output(raw);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].date, "2024-01-01T00:00:00Z");
        assert_eq!(commits[0].message, "subject with COMMIT_START inside");
        assert_eq!((commits[0].files[0].insertions, commits[0].files[0].deletions), (2, 2));
        assert!(commits[1].files.is_empty());
    }

    #[test]
    fn one_diff_splits_into_files() {
        use super::split_diff_by_file;
        let raw = [
            "diff --git a/x.py b/x.py",
            "--- a/x.py",
            "+++ b/x.py",
            "@@ -1 +1 @@ def f():",
            "-    return 1",
            "+    return 2",
            "diff --git a/gone.py b/gone.py",
            "deleted file mode 100644",
            "--- a/gone.py",
            "+++ /dev/null",
            "@@ -1,2 +0,0 @@",
            "-def g():",
            "-    pass",
        ]
        .join("\n");
        let files = split_diff_by_file(&raw);
        let names: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(names, vec!["x.py", "gone.py"]);
        assert_eq!(parse_diff_functions(&files[0].1), vec!["f"]);
        assert_eq!(parse_diff_functions(&files[1].1), vec!["g"]);
    }

    #[test]
    fn changed_signature_lines_name_their_function() {
        let raw = "diff --git a/x.rs b/x.rs\n--- a/x.rs\n+++ b/x.rs\n@@ -5 +5 @@ fn before() {\n-fn after(x: i32) {\n+pub(crate) async fn after(x: i64) {\n@@ -9 +9 @@ fn before() {\n-    let y = call(x);\n+    let y = call(x + 1);\n";
        assert_eq!(parse_diff_functions(raw), vec!["before", "after"]);
    }

    #[test]
    fn definition_keywords_across_languages() {
        let raw = "@@ -1 +1 @@\n+func (r *Repo) Load() error {\n+    def method(self):\n+export default function render() {\n+extern \"C\" fn ffi() {}\n+// this function does nothing\n+x = fn(1)\n";
        assert_eq!(parse_diff_functions(raw), vec!["Load", "method", "render", "ffi"]);
    }
}
