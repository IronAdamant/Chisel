//! Per-function churn and blame for `analyze` and `update`.
//!
//! The fast path reads a few batched git streams and replays them with `crate::history`.
//! Files it cannot replay exactly fall back to one `git log -L` per function and one
//! `git blame` per file, which is also the whole path when `Engine::history_stream` is off.

use super::pool_map;
use crate::git::{self, BlameBlock, Commit, GitAnalyzer, GitFile};
use crate::history::{self, History};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

/// `(file, unit name, bare name)` from `unit_log_tasks`.
pub(crate) type Task = (String, String, String);

pub(crate) struct ChurnAndBlame {
    /// Per task: what `git log -L:<bare>:<file>` reports, or `None` when git has no match.
    pub logs: Vec<Option<Vec<Commit>>>,
    /// Per blame file, in input order.
    pub blames: Vec<Vec<BlameBlock>>,
}

/// The new path of a `--numstat` rename entry (`a/{x => y}/b`, `old => new`).
pub(crate) fn numstat_rename_target(path: &str) -> Option<String> {
    if let (Some(open), Some(close)) = (path.find('{'), path.rfind('}')) {
        let inner = &path[open + 1..close];
        let (_, new) = inner.split_once(" => ")?;
        let joined = format!("{}{}{}", &path[..open], new, &path[close + 1..]);
        return Some(joined.replace("//", "/"));
    }
    path.split_once(" => ").map(|(_, new)| new.to_string())
}

/// Spawn git per task and per file, as before the stream existed.
fn spawned(git: &GitAnalyzer, blame_files: &[String], tasks: &[Task]) -> ChurnAndBlame {
    ChurnAndBlame {
        logs: pool_map(tasks, |t| git.function_log(&t.0, &t.2).ok()),
        blames: pool_map(blame_files, |rel| git.blame(rel).unwrap_or_default()),
    }
}

fn identifier(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn lines_of(raw: &str) -> Vec<String> {
    raw.lines().map(|l| l.trim_end_matches('\r').to_string()).filter(|l| !l.is_empty()).collect()
}

/// Parsed git output that the replay needs. `None` when any of it failed.
pub(crate) struct Prefetched {
    line_history: History,
    blame_history: History,
    dirty_raw: String,
    head_raw: String,
    plain: HashSet<String>,
}

/// Run the batched git calls for `scope`. `whole_repo` streams every code file rather than
/// only the files in scope. `analyze` starts this before it extracts units.
pub(crate) fn prefetch(git: &GitAnalyzer, scope: &[String], whole_repo: bool) -> Option<Prefetched> {
    let pathspecs: Vec<String> = if whole_repo || scope.len() > 200 { history::pathspecs() } else { scope.iter().map(|p| format!(":(literal){p}")).collect() };
    let quote_off = ["-c".to_string(), "core.quotePath=false".to_string()];
    type Raw = Option<String>;
    let (line_raw, blame_raw, dirty_raw, head_raw, attrs): (Raw, Raw, Raw, Raw, Option<HashSet<String>>) = std::thread::scope(|s| {
        let line = s.spawn(|| git.run_args(&history::stream_args(false, &pathspecs)));
        let blame = s.spawn(|| git.run_args(&history::stream_args(true, &pathspecs)));
        let dirty = s.spawn(|| git.run_args(&[&quote_off[..], &["diff".into(), "--name-only".into(), "HEAD".into()]].concat()));
        let in_head = s.spawn(|| git.run_args(&[&quote_off[..], &["ls-tree".into(), "-r".into(), "--name-only".into(), "HEAD".into()]].concat()));
        let attrs = s.spawn(|| {
            let mut plain = HashSet::new();
            for chunk in scope.chunks(100) {
                let mut args = vec!["check-attr".to_string(), "-z".into(), "diff".into(), "--".into()];
                args.extend(chunk.iter().cloned());
                let out = git.run_args(&args).ok()?;
                let fields: Vec<&str> = out.split('\0').collect();
                for row in fields.chunks(3) {
                    if row.len() == 3 && matches!(row[2], "unspecified" | "set") {
                        plain.insert(row[0].to_string());
                    }
                }
            }
            Some(plain)
        });
        let out = |h: std::thread::ScopedJoinHandle<'_, Result<String, git::GitError>>| h.join().ok().and_then(|r| r.ok());
        (out(line), out(blame), out(dirty), out(in_head), attrs.join().ok().flatten())
    });
    let mut line_history = History::new(history::parse_stream(&line_raw?));
    let mut blame_history = History::new(history::parse_stream(&blame_raw?));
    let ambiguous = line_history.ambiguous_merges();
    if !ambiguous.is_empty() {
        if let Ok(raw) = git.run_args(&history::first_parent_args(&ambiguous, &pathspecs)) {
            line_history.resolve_first_parents(&raw);
            blame_history.resolve_first_parents(&raw);
        }
    }
    Some(Prefetched { line_history, blame_history, dirty_raw: dirty_raw?, head_raw: head_raw?, plain: attrs? })
}

/// `renames`: `(commit hash, new path)` of every rename the numstat history saw.
pub(crate) fn churn_and_blame(
    git: &GitAnalyzer,
    project: &Path,
    blame_files: &[String],
    tasks: &[Task],
    renames: &HashSet<(String, String)>,
    stream: bool,
    whole_repo: bool,
) -> ChurnAndBlame {
    if !stream || (blame_files.is_empty() && tasks.is_empty()) {
        return spawned(git, blame_files, tasks);
    }
    let scope: BTreeSet<&str> = blame_files.iter().map(|s| s.as_str()).chain(tasks.iter().map(|t| t.0.as_str())).collect();
    let scope_list: Vec<String> = scope.iter().map(|s| s.to_string()).collect();
    replay(git, project, blame_files, tasks, renames, prefetch(git, &scope_list, whole_repo))
}

/// Replay prefetched history for each task and blame file; spawn git for the rest.
pub(crate) fn replay(git: &GitAnalyzer, project: &Path, blame_files: &[String], tasks: &[Task], renames: &HashSet<(String, String)>, pre: Option<Prefetched>) -> ChurnAndBlame {
    let Some(Prefetched { line_history, blame_history, dirty_raw, head_raw, plain }) = pre else {
        return spawned(git, blame_files, tasks);
    };
    let scope: BTreeSet<&str> = blame_files.iter().map(|s| s.as_str()).chain(tasks.iter().map(|t| t.0.as_str())).collect();
    let dirty: HashSet<String> = lines_of(&dirty_raw).into_iter().collect();
    let in_head: HashSet<String> = lines_of(&head_raw).into_iter().collect();

    // A file the stream shows as added, where numstat saw a rename into it, came from a path
    // outside the pathspec. Only real git follows that, so those files are spawned.
    let mut added_from_outside: HashSet<&str> = HashSet::new();
    for c in &line_history.commits {
        if c.parents.is_empty() {
            continue;
        }
        for f in c.sections.iter().flatten() {
            if let (None, Some(new)) = (&f.old_path, &f.new_path) {
                if renames.contains(&(c.hash.clone(), new.clone())) {
                    added_from_outside.insert(new.as_str());
                }
            }
        }
    }
    let replayable = |f: &str| in_head.contains(f) && plain.contains(f) && !added_from_outside.contains(f);

    // HEAD content: the worktree when it matches HEAD, else the committed blob. Blame of a dirty
    // or unreplayable file needs real `git blame` (worktree lines); those spawns run meanwhile.
    let wanted: Vec<String> = scope.iter().filter(|f| replayable(f)).map(|f| f.to_string()).collect();
    let early_blames: Vec<usize> = (0..blame_files.len()).filter(|&i| in_head.contains(&blame_files[i]) && (dirty.contains(&blame_files[i]) || !replayable(&blame_files[i]))).collect();
    let (committed, early_blocks): (Vec<Option<Vec<String>>>, Vec<Vec<BlameBlock>>) = std::thread::scope(|s| {
        let blames = s.spawn(|| pool_map(&early_blames, |&i| git.blame(&blame_files[i]).unwrap_or_default()));
        let committed = pool_map(&wanted, |f| {
            let text = if dirty.contains(f) { git.head_blob(f).ok()? } else { String::from_utf8_lossy(&std::fs::read(project.join(f)).ok()?).into_owned() };
            Some(history::split_lines(&text))
        });
        (committed, blames.join().unwrap_or_default())
    });
    let content: HashMap<&str, Vec<String>> = wanted.iter().zip(committed).filter_map(|(f, c)| Some((f.as_str(), c?))).collect();

    // `git log -L` and `git blame` both fail on a path that is not in HEAD.
    let mut logs: Vec<Option<Option<Vec<Commit>>>> = tasks.iter().map(|t| if in_head.contains(&t.0) { None } else { Some(None) }).collect();
    let mut by_file: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, t) in tasks.iter().enumerate() {
        by_file.entry(t.0.as_str()).or_default().push(i);
    }
    for (file, idxs) in &by_file {
        let Some(lines) = content.get(file) else { continue };
        let mut names: Vec<&str> = idxs.iter().map(|&i| tasks[i].2.as_str()).filter(|n| identifier(n)).collect();
        names.sort();
        names.dedup();
        let ranges: Vec<Option<(usize, usize)>> = names.iter().map(|n| history::funcname_range(lines, n)).collect();
        let found: Vec<(usize, usize)> = ranges.iter().flatten().copied().collect();
        let Some(shown) = line_history.line_logs(file, &found) else { continue };
        let mut by_name: HashMap<&str, Option<Vec<Commit>>> = HashMap::new();
        let mut k = 0;
        for (name, range) in names.iter().zip(&ranges) {
            let commits = range.map(|_| {
                let list = shown[k]
                    .iter()
                    .map(|s| {
                        let c = &line_history.commits[s.commit];
                        let files = if s.insertions > 0 || s.deletions > 0 { vec![GitFile { path: String::new(), insertions: s.insertions, deletions: s.deletions }] } else { Vec::new() };
                        Commit { hash: c.hash.clone(), author: c.author.clone(), author_email: c.email.clone(), date: git::utc_z(&c.date), message: c.subject.clone(), files }
                    })
                    .collect();
                k += 1;
                list
            });
            by_name.insert(name, commits);
        }
        for &i in idxs {
            if let Some(result) = by_name.get(tasks[i].2.as_str()) {
                logs[i] = Some(result.clone());
            }
        }
    }
    let mut blames: Vec<Option<Vec<BlameBlock>>> = blame_files
        .iter()
        .map(|f| {
            if !in_head.contains(f) {
                return Some(Vec::new());
            }
            if dirty.contains(f) {
                return None;
            }
            let lines = content.get(f.as_str())?;
            blame_history.blame(f, lines.len())
        })
        .collect();
    for (&i, blocks) in early_blames.iter().zip(early_blocks) {
        blames[i] = Some(blocks);
    }

    let spawn_tasks: Vec<usize> = (0..tasks.len()).filter(|&i| logs[i].is_none()).collect();
    let spawn_blames: Vec<usize> = (0..blame_files.len()).filter(|&i| blames[i].is_none()).collect();
    let spawned_logs = pool_map(&spawn_tasks, |&i| git.function_log(&tasks[i].0, &tasks[i].2).ok());
    for (&i, log) in spawn_tasks.iter().zip(spawned_logs) {
        logs[i] = Some(log);
    }
    let spawned_blames = pool_map(&spawn_blames, |&i| git.blame(&blame_files[i]).unwrap_or_default());
    for (&i, b) in spawn_blames.iter().zip(spawned_blames) {
        blames[i] = Some(b);
    }
    ChurnAndBlame { logs: logs.into_iter().map(|l| l.flatten()).collect(), blames: blames.into_iter().map(|b| b.unwrap_or_default()).collect() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numstat_rename_paths() {
        assert_eq!(numstat_rename_target("src/{a.rs => b.rs}"), Some("src/b.rs".into()));
        assert_eq!(numstat_rename_target("{old => new}/x.py"), Some("new/x.py".into()));
        assert_eq!(numstat_rename_target("src/{sub => }/x.py"), Some("src/x.py".into()));
        assert_eq!(numstat_rename_target("a.py => b.py"), Some("b.py".into()));
        assert_eq!(numstat_rename_target("plain.py"), None);
    }
}
