//! Worktree overlays: answer for a git worktree from the main checkout's snapshot.
//!
//! Agents edit in their own `git worktree`, while the MCP server runs at the main checkout. With
//! `worktree_root` (CLI `--worktree`), a read tool answers as if the snapshot had been built
//! from that tree: the base is main's `.chisel/store`, never written to; the overlay is a copy
//! under `.chisel/overlays/<hash of the worktree path>/` that `update` brings in line with the
//! worktree (only files whose content differs are re-indexed; deleted files drop out). Each call
//! re-checks the worktree by mtime and size, so answers are as current as the files. An overlay
//! is rebuilt when main's snapshot changes, and removed once its worktree is gone.

use super::*;
use crate::hash::sha256_hex;

/// Tools that answer for a worktree. Writes that belong to the project (results, locks, jobs,
/// captured issues) always go to the base snapshot.
pub(crate) const OVERLAY_TOOLS: [&str; 15] = [
    "analyze", "update", "diff_impact", "impact", "suggest_tests", "risk_map", "test_gaps", "triage", "stale_tests", "stats",
    "churn", "ownership", "coupling", "history", "who_reviews",
];

const BASE_FILE: &str = "base.txt";
const INDEX_FILE: &str = "git-index";
const INDEX_STAMP: &str = "git-index.stamp";
const WORKTREE_FILE: &str = "worktree.txt";

/// The directory holding `.git` for `dir`, and that `.git` (a directory, or a file pointing at
/// a linked worktree's git dir).
fn find_dotgit(dir: &Path) -> Option<(PathBuf, PathBuf)> {
    let mut cur = fs::canonicalize(dir).ok()?;
    loop {
        let dotgit = cur.join(".git");
        if dotgit.exists() {
            return Some((cur, dotgit));
        }
        if !cur.pop() {
            return None;
        }
    }
}

/// A linked worktree's git dir and common dir, read from its `.git` file.
fn linked_dirs(root: &Path, dotgit: &Path) -> Option<(PathBuf, PathBuf)> {
    let text = fs::read_to_string(dotgit).ok()?;
    let gitdir = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim();
    let gitdir = fs::canonicalize(root.join(gitdir)).ok()?;
    let common = fs::read_to_string(gitdir.join("commondir")).ok()?;
    let common = fs::canonicalize(gitdir.join(common.trim())).ok()?;
    Some((gitdir, common))
}

/// The git common dir of the checkout holding `dir`, without running git.
fn git_common_dir(dir: &Path) -> Option<PathBuf> {
    let (root, dotgit) = find_dotgit(dir)?;
    if dotgit.is_dir() {
        return fs::canonicalize(dotgit).ok();
    }
    linked_dirs(&root, &dotgit).map(|(_, common)| common)
}

/// When `dir` is inside a linked git worktree: `(main checkout root, worktree root)`. Read from
/// the `.git` file, without spawning git. `None` in a main checkout, a submodule or a bare repo.
pub fn linked_worktree(dir: &Path) -> Option<(PathBuf, PathBuf)> {
    let (root, dotgit) = find_dotgit(dir)?;
    if !dotgit.is_file() {
        return None;
    }
    let (_, common) = linked_dirs(&root, &dotgit)?;
    if common.file_name().and_then(|n| n.to_str()) != Some(".git") {
        return None;
    }
    Some((common.parent()?.to_path_buf(), root))
}

/// A private copy of the worktree's git index under the overlay, refreshed with
/// `git update-index --refresh`. A fresh checkout's index entries are "racily clean", so every
/// `git diff` there re-reads files (about 0.5 s on Specula); refreshing the worktree's own index
/// would write it and could collide with the agent's own git commands, so a copy is refreshed
/// instead, and copied again only when the worktree's index file changes. `None` when the
/// worktree's index cannot be found (git then reads the worktree's own index).
fn private_index(wt: &Path, dir: &Path) -> Option<PathBuf> {
    let (root, dotgit) = find_dotgit(wt)?;
    let wt = PathBuf::from(display_path(wt));
    let source = if dotgit.is_dir() { dotgit.join("index") } else { linked_dirs(&root, &dotgit)?.0.join("index") };
    // git cannot open a verbatim (`\?\`) path: hand it the plain form.
    let dir = PathBuf::from(display_path(dir));
    let copy = dir.join(INDEX_FILE);
    let source_stamp = stamp(&source);
    if source_stamp.is_empty() {
        return None;
    }
    if copy.is_file() && fs::read_to_string(dir.join(INDEX_STAMP)).ok().as_deref() == Some(source_stamp.as_str()) {
        return Some(copy);
    }
    let tmp = dir.join(format!("{INDEX_FILE}.tmp"));
    fs::copy(&source, &tmp).ok()?;
    // Exit code 1 only means some files differ from the index; the copy is refreshed either way.
    let _ = std::process::Command::new("git").args(["update-index", "-q", "--refresh"]).current_dir(&wt).env("GIT_INDEX_FILE", &tmp).output();
    fs::rename(&tmp, &copy).ok()?;
    let _ = fs::write(dir.join(INDEX_STAMP), source_stamp);
    Some(copy)
}

/// A path as an agent would type it (no `\\?\` prefix on Windows).
pub(crate) fn display_path(p: &Path) -> String {
    let s = p.to_string_lossy();
    s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
}

fn stamp(path: &Path) -> String {
    let Ok(meta) = fs::metadata(path) else { return String::new() };
    let nanos = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{}:{nanos}", meta.len())
}

fn error(message: String) -> Value {
    Value::object(vec![
        ("status".into(), Value::String("error".into())),
        ("message".into(), Value::String(message)),
    ])
}

/// Overlay directories whose worktree no longer exists are removed.
fn prune_overlays(base_dir: &Path) {
    let Ok(entries) = fs::read_dir(base_dir.join("overlays")) else { return };
    for entry in entries.flatten() {
        let dir = entry.path();
        let gone = match fs::read_to_string(dir.join(WORKTREE_FILE)) {
            Ok(path) => !Path::new(path.trim()).is_dir(),
            Err(_) => true,
        };
        if gone {
            let _ = fs::remove_dir_all(&dir);
        }
    }
}

/// `args` without `worktree_root`.
fn without_worktree(args: &Value) -> Value {
    match args.as_object() {
        Some(pairs) => Value::Object(pairs.iter().filter(|(k, _)| &**k != "worktree_root").cloned().collect()),
        None => args.clone(),
    }
}

impl Engine {
    /// The worktree a call answers for: `worktree_root`, else the one this process started in.
    /// `Err` is a one-line actionable error reply.
    pub(super) fn overlay_target(&self, name: &str, args: &Value) -> Result<Option<PathBuf>, Value> {
        if !OVERLAY_TOOLS.contains(&name) || self.shard_view.is_some() {
            return Ok(None);
        }
        let given = args.get("worktree_root").and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty()).map(|s| PathBuf::from(s.trim()));
        let Some(raw) = given.or_else(|| self.default_worktree.clone()) else { return Ok(None) };
        let project = display_path(&self.project_dir);
        if !raw.is_absolute() {
            return Err(error(format!("worktree_root must be an absolute path, got '{}'. Pass the full path of your git worktree.", raw.display())));
        }
        let wt = match fs::canonicalize(&raw) {
            Ok(p) if p.is_dir() => p,
            _ => return Err(error(format!("worktree_root '{}' does not exist. Pass the absolute path of a git worktree of {project}.", raw.display()))),
        };
        if wt == self.project_dir {
            return Ok(None);
        }
        if !self.shards.is_empty() {
            return Err(Value::object(vec![
                ("status".into(), Value::String("unsupported".into())),
                ("message".into(), Value::String("worktree_root does not work with shards (CHISEL_SHARDS or .chisel/shards.toml). Run chisel in the worktree with its own --storage-dir instead.".into())),
            ]));
        }
        if let Some(base_common) = git_common_dir(&self.project_dir) {
            if git_common_dir(&wt).as_ref() != Some(&base_common) {
                return Err(error(format!("worktree_root '{}' is not a checkout of {project} (its git common dir differs). Pass a worktree made by `git worktree add` from this repository, or omit worktree_root.", display_path(&wt))));
            }
        }
        Ok(Some(wt))
    }

    /// Run `name` on the overlay for `wt`, refreshed first.
    pub(super) fn dispatch_overlay(&mut self, name: &str, args: &Value, wt: PathBuf) -> Result<Value, String> {
        self.store.reload()?;
        if !self.has_data() {
            let mut reply = Self::no_data();
            reply.insert("hint", Value::String(format!("Run analyze on the main checkout ({}) first, without worktree_root: a worktree overlays main's snapshot.", display_path(&self.project_dir))));
            return Ok(reply);
        }
        let base_dir = self.store.dir().to_path_buf();
        prune_overlays(&base_dir);
        let key = sha256_hex(display_path(&wt).as_bytes());
        let dir = base_dir.join("overlays").join(&key[..16]);
        let base_stamp = stamp(&self.store.snapshot_path());
        let recorded = fs::read_to_string(dir.join(BASE_FILE)).unwrap_or_default();
        let fresh = recorded.lines().next() == Some(base_stamp.as_str()) && dir.join(crate::store::STORE_NAME).is_file();
        let mut overlay = match self.overlays.remove(&wt) {
            Some(o) if fresh && name != "analyze" => o,
            _ => {
                fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
                fs::write(dir.join(WORKTREE_FILE), display_path(&wt)).map_err(|e| e.to_string())?;
                let mut o = Box::new(Engine::open(&wt, Some(&dir))?);
                if !fresh || name == "analyze" {
                    // The refresh below saves it; a fresh copy always has files to restamp.
                    o.store.doc = self.store.doc.clone();
                    o.store.reindex_memory();
                    let mut base = base_stamp.clone();
                    for row in self.store.rows("file_hashes") {
                        if let (Some(p), Some(h)) = (row.get("file_path").and_then(|v| v.as_str()), row.get("hash").and_then(|v| v.as_str())) {
                            let size = row.get("size").and_then(|v| v.as_i64()).unwrap_or(-1);
                            let ns = row.get("mtime_ns").and_then(|v| v.as_i64()).unwrap_or(0);
                            base.push_str(&format!("\n{p}\t{h}\t{size}\t{ns}"));
                        }
                    }
                    fs::write(dir.join(BASE_FILE), base).map_err(|e| e.to_string())?;
                }
                o
            }
        };
        overlay.restamp_unchanged = true;
        overlay.overlay_base = Some(self.project_dir.clone());
        overlay.allow_repo_extractor = self.allow_repo_extractor;
        overlay.include_ignored = self.include_ignored;
        overlay.history_stream = self.history_stream;
        overlay.store.durable = false;
        let index = private_index(&wt, &dir);
        if let Some(index) = &index {
            overlay.set_git_runner(git::index_file_runner(index.clone()));
        }
        if fresh {
            overlay.store.reload()?;
        }
        // Files whose content is main's indexed content are read from main's checkout (already
        // read and cached there); a fresh worktree's first read of every file is slow.
        let same = if fresh { same_by_hash(&overlay, &dir, &self.project_dir) } else { self.same_in_git(&wt, index.as_deref()) };
        if !fresh {
            overlay.known_same = same.clone();
        }
        overlay.source_base = Some(Sources::new(&wt, Some((self.project_dir.clone(), std::sync::Arc::new(same)))));
        let mut refreshed = overlay.update();
        overlay.known_same.clear();
        if refreshed.is_ok() && !overlay.store.snapshot_path().is_file() {
            // Nothing differed and nothing needed a new stamp: publish the copy as it is.
            refreshed = overlay.store.save().map_err(String::from).and(refreshed);
        }
        let result = match refreshed {
            Ok(summary) if matches!(name, "analyze" | "update") => Ok(summary),
            Ok(_) => overlay.dispatch(name, &without_worktree(args)),
            Err(e) => Err(e),
        };
        let changed = overlay_files(&overlay, &dir);
        self.overlays.insert(wt.clone(), overlay);
        let result = result?;
        self.reply_meta = vec![
            ("worktree".into(), Value::String(display_path(&wt))),
            ("overlay_files".into(), Value::Int(changed as i64)),
        ];
        Ok(self.with_reply_meta(result))
    }
}

impl Engine {
    /// Files whose worktree copy is, by git, the content main's snapshot indexed: tracked at the
    /// commit main was analyzed at, changed in neither checkout since (`git diff`), and main's
    /// file still has the stamp the snapshot recorded. Empty when anything cannot be read.
    fn same_in_git(&self, wt: &Path, index: Option<&Path>) -> std::collections::HashSet<String> {
        let mut out = std::collections::HashSet::new();
        let Some(head) = crate::store::meta_get(&self.store.doc, "analyzed_head").filter(|h| !h.is_empty() && !h.starts_with('-')) else { return out };
        let main_git = self.git.fork();
        let mut wt_git = GitAnalyzer::new(wt.to_string_lossy().to_string());
        if let Some(index) = index {
            wt_git.set_runner(git::index_file_runner(index.to_path_buf()));
        }
        let names = |raw: Result<String, git::GitError>| -> Option<std::collections::HashSet<String>> {
            raw.ok().map(|t| t.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
        };
        let (tracked, main_changed, wt_changed) = std::thread::scope(|s| {
            let tracked = s.spawn(|| main_git.run_args(&["ls-tree".into(), "-r".into(), "--name-only".into(), head.clone()]));
            let main_changed = s.spawn(|| main_git.run_args(&["diff".into(), "--name-only".into(), "--relative".into(), "--no-renames".into(), head.clone()]));
            let wt_changed = wt_git.run_args(&["diff".into(), "--name-only".into(), "--relative".into(), "--no-renames".into(), head.clone()]);
            (tracked.join().ok().and_then(names), main_changed.join().ok().and_then(names), names(wt_changed))
        });
        let (Some(tracked), Some(main_changed), Some(wt_changed)) = (tracked, main_changed, wt_changed) else { return out };
        for row in self.store.rows("file_hashes") {
            let Some(path) = row.get("file_path").and_then(|v| v.as_str()) else { continue };
            if !tracked.contains(path) || main_changed.contains(path) || wt_changed.contains(path) {
                continue;
            }
            let Ok(meta) = fs::metadata(self.project_dir.join(path)) else { continue };
            let size = row.get("size").and_then(|v| v.as_i64()).unwrap_or(-1);
            let mtime_ns = row.get("mtime_ns").and_then(|v| v.as_i64()).unwrap_or(0);
            let disk_ns = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos() as i64).unwrap_or(-1);
            if meta.len() as i64 == size && mtime_ns > 0 && disk_ns == mtime_ns {
                out.insert(path.to_string());
            }
        }
        out
    }
}

/// Files the overlay holds with main's indexed hash whose main copy still has the stamp main's
/// snapshot recorded: their main copy is the same content.
fn same_by_hash(overlay: &Engine, dir: &Path, main: &Path) -> std::collections::HashSet<String> {
    let text = fs::read_to_string(dir.join(BASE_FILE)).unwrap_or_default();
    let mut out = std::collections::HashSet::new();
    for line in text.lines().skip(1) {
        let mut parts = line.split('\t');
        let (Some(p), Some(h), Some(size), Some(ns)) = (parts.next(), parts.next(), parts.next(), parts.next()) else { continue };
        if overlay.store.hash_for_file(p) != h {
            continue;
        }
        let Ok(meta) = fs::metadata(main.join(p)) else { continue };
        let disk_ns = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos() as i64).unwrap_or(-1);
        if size.parse::<i64>().ok() == Some(meta.len() as i64) && ns.parse::<i64>().ok() == Some(disk_ns) && disk_ns > 0 {
            out.insert(p.to_string());
        }
    }
    out
}

/// Files whose content in the worktree differs from main's snapshot: changed, added, deleted.
fn overlay_files(overlay: &Engine, dir: &Path) -> usize {
    let text = fs::read_to_string(dir.join(BASE_FILE)).unwrap_or_default();
    let base: HashMap<&str, &str> = text.lines().skip(1).filter_map(|l| {
        let mut parts = l.split('\t');
        Some((parts.next()?, parts.next()?))
    }).collect();
    let mut seen = 0usize;
    let mut differ = 0usize;
    for row in overlay.store.rows("file_hashes") {
        let (Some(p), Some(h)) = (row.get("file_path").and_then(|v| v.as_str()), row.get("hash").and_then(|v| v.as_str())) else { continue };
        match base.get(p) {
            Some(b) => {
                seen += 1;
                if *b != h {
                    differ += 1;
                }
            }
            None => differ += 1,
        }
    }
    differ + (base.len() - seen.min(base.len()))
}
