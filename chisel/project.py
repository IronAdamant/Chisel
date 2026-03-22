"""Project root detection, path normalization, and storage resolution.

Provides utilities for multi-agent safety: ensures all agents (including
those running in git worktrees) resolve to the same canonical project root
and storage location, preventing path divergence and data collision.

Key design decisions:
    - Storage defaults to project-local (<project_root>/.chisel/) so different
      projects never collide in a shared ~/.chisel/ database.
    - For git worktrees, the git common dir is used to find the main repo,
      so all worktrees share one .chisel/ directory and one database.
    - Path normalization ensures ``os.path.relpath`` always uses the
      canonical project root, so agents in different worktrees produce
      identical relative paths for the same file.
    - A cross-process file lock prevents concurrent ``analyze`` / ``update``
      calls from interleaving destructive writes.
"""

import fcntl
import os
import subprocess
from contextlib import contextmanager


def detect_project_root(start_dir=None):
    """Detect the canonical project root directory.

    For regular git repos, returns the repo root.  For git worktrees,
    returns the **main** repository root (parent of the git common dir)
    so all worktrees resolve to the same identity.

    Falls back to *start_dir* (or cwd) if not inside a git repo.

    Args:
        start_dir: Directory to start searching from (default: cwd).

    Returns:
        Absolute path to the canonical project root.
    """
    if start_dir is None:
        start_dir = os.getcwd()
    start_dir = os.path.abspath(start_dir)

    # Try git common dir first (handles worktrees — points to main repo)
    common_dir = _git_common_dir(start_dir)
    if common_dir:
        # common_dir is e.g. /home/user/project/.git — parent is the root
        parent = os.path.dirname(common_dir)
        if os.path.isdir(parent):
            return parent

    # Fall back to git rev-parse --show-toplevel (works for non-worktree repos)
    try:
        result = subprocess.run(
            ["git", "rev-parse", "--show-toplevel"],
            cwd=start_dir, capture_output=True, text=True, timeout=10,
        )
        if result.returncode == 0:
            return result.stdout.strip()
    except (OSError, subprocess.TimeoutExpired):
        pass

    # Not in a git repo — walk up looking for .git
    current = start_dir
    while True:
        if os.path.isdir(os.path.join(current, ".git")):
            return current
        parent = os.path.dirname(current)
        if parent == current:
            break
        current = parent

    return start_dir


def normalize_path(file_path, project_root):
    """Normalize a file path to be relative to the canonical project root.

    Handles absolute paths, relative paths, and paths from different
    worktrees that refer to the same logical file.

    Args:
        file_path: Path to normalize (absolute or relative).
        project_root: The canonical project root from detect_project_root().

    Returns:
        Normalized relative path string (forward slashes, no leading ./).
    """
    if os.path.isabs(file_path):
        try:
            rel = os.path.relpath(file_path, project_root)
        except ValueError:
            return file_path
    else:
        rel = os.path.normpath(file_path)
    # Ensure consistent forward slashes and no leading ./
    rel = rel.replace(os.sep, "/")
    return rel.removeprefix("./")


def resolve_storage_dir(project_dir=None, explicit_dir=None):
    """Resolve the storage directory for Chisel's database.

    Priority (highest to lowest):
        1. explicit_dir — passed directly by the caller
        2. CHISEL_STORAGE_DIR environment variable
        3. Project-local: <canonical_project_root>/.chisel/
        4. Fallback: ~/.chisel/

    For git worktrees, option 3 resolves to the **main** repo's .chisel/
    directory, so all worktrees share one database.

    Args:
        project_dir: The project directory (will be canonicalized).
        explicit_dir: Explicitly provided storage directory.

    Returns:
        Absolute path to the storage directory.
    """
    if explicit_dir is not None:
        return os.path.abspath(explicit_dir)

    env_dir = os.environ.get("CHISEL_STORAGE_DIR")
    if env_dir:
        return os.path.abspath(env_dir)

    if project_dir:
        root = detect_project_root(project_dir)
        return os.path.join(root, ".chisel")

    return os.path.join(os.path.expanduser("~"), ".chisel")


class ProcessLock:
    """Cross-process exclusive file lock for write coordination.

    Prevents multiple processes (e.g., two CLI invocations or two MCP
    servers) from running destructive operations (analyze, update)
    simultaneously on the same database.

    Uses fcntl.flock (Unix) — blocks until the lock is available.
    """

    def __init__(self, lock_dir):
        self._lock_path = os.path.join(lock_dir, "chisel.lock")
        os.makedirs(lock_dir, exist_ok=True)

    @contextmanager
    def _acquire(self, lock_type):
        """Acquire a file lock of the given type, yielding control."""
        fd = open(self._lock_path, "w")
        try:
            fcntl.flock(fd, lock_type)
            try:
                yield
            finally:
                fcntl.flock(fd, fcntl.LOCK_UN)
        finally:
            fd.close()

    @contextmanager
    def exclusive(self):
        """Acquire an exclusive file lock, yielding control to the caller."""
        with self._acquire(fcntl.LOCK_EX):
            yield

    @contextmanager
    def shared(self):
        """Acquire a shared file lock (allows concurrent readers)."""
        with self._acquire(fcntl.LOCK_SH):
            yield


# ------------------------------------------------------------------ #
# Internal helpers
# ------------------------------------------------------------------ #

def _git_common_dir(start_dir):
    """Return the git common directory, or None if not in a git repo.

    For worktrees this points to the main repo's .git dir (shared).
    For regular repos it is the .git dir itself.
    """
    try:
        result = subprocess.run(
            ["git", "rev-parse", "--git-common-dir"],
            cwd=start_dir, capture_output=True, text=True, timeout=10,
        )
        if result.returncode == 0:
            common = result.stdout.strip()
            if not os.path.isabs(common):
                common = os.path.normpath(os.path.join(start_dir, common))
            return common
    except (OSError, subprocess.TimeoutExpired):
        pass
    return None
