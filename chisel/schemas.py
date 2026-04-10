"""Tool schemas and dispatch tables for Chisel MCP servers.

Contains the JSON Schema definitions for all 16 engine tools and the
dispatch mapping used by both the HTTP and stdio MCP servers.

Agent-facing vocabulary and trust rules: ``chisel.llm_contract`` and
``docs/LLM_CONTRACT.md``.
"""

from chisel.llm_contract import HEURISTIC_TRUST_NOTE

# ------------------------------------------------------------------ #
# Tool schemas — JSON Schema definitions for all 16 engine tools
# ------------------------------------------------------------------ #

_TOOL_SCHEMAS = {
    "analyze": {
        "name": "analyze",
        "description": (
            "Full project analysis. Use on first run or after major structural "
            "changes (new files, renames, deleted code). Scans source files, "
            "extracts code units, discovers tests, parses git history, builds "
            "test-to-code edges. For incremental changes, use 'update' instead. "
            "Large repos: run `chisel analyze --force` in a terminal (not only via MCP) "
            "so long runs are not mistaken for a hung tool. "
            + HEURISTIC_TRUST_NOTE
        ),
        "parameters": {
            "type": "object",
            "properties": {
                "directory": {
                    "type": "string",
                    "description": "Subdirectory to analyze (default: entire project).",
                },
                "force": {
                    "type": "boolean",
                    "description": "Force re-analysis of all files even if unchanged.",
                },
            },
            "required": [],
        },
    },
    "impact": {
        "name": "impact",
        "description": (
            "Get impacted tests for specific changed files/functions. Use when "
            "you know exactly which files changed. For auto-detection from git "
            "diff, use 'diff_impact' instead. "
            + HEURISTIC_TRUST_NOTE
        ),
        "parameters": {
            "type": "object",
            "properties": {
                "files": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "List of changed file paths.",
                },
                "functions": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Optional list of changed function names.",
                },
            },
            "required": ["files"],
        },
    },
    "suggest_tests": {
        "name": "suggest_tests",
        "description": (
            "Suggest tests to run for a file, ranked by relevance. Combines direct "
            "test edges, git co-change, and static import-graph reachability (e.g. "
            "facade tests covering inner modules). Each item may include "
            "source: direct | co_change | import_graph | static_require | hybrid | "
            "fallback | working_tree. Merges DB impact with a static scan of test "
            "files (require/import → source path); hybrid = both agree. With "
            "working_tree=true, also indexes untracked test files and git-untracked "
            "source paths for resolution. Returns empty for untracked/new files "
            "unless fallback_to_all or working_tree is set. "
            + HEURISTIC_TRUST_NOTE
        ),
        "parameters": {
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": (
                        "Path to the file to suggest tests for. "
                        "Trust: prefer items with source direct or hybrid over "
                        "static_require or fallback alone."
                    ),
                },
                "fallback_to_all": {
                    "type": "boolean",
                    "description": (
                        "If true and no test edges exist for the file, return all known "
                        "test files ranked by name similarity instead of empty. "
                        "Useful for new/untracked files that have no analysis data yet."
                    ),
                },
                "working_tree": {
                    "type": "boolean",
                    "description": (
                        "If true, also scan untracked (uncommitted) files on disk and use "
                        "stem-matching to find relevant tests when the file has no DB edges. "
                        "useful during active development before files are committed."
                    ),
                },
            },
            "required": ["file_path"],
        },
    },
    "churn": {
        "name": "churn",
        "description": "Get change frequency for a file or specific function. High churn = frequently modified = higher risk. Use to understand why a file has a high risk score.",
        "parameters": {
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file.",
                },
                "unit_name": {
                    "type": "string",
                    "description": "Optional name of a specific code unit.",
                },
            },
            "required": ["file_path"],
        },
    },
    "ownership": {
        "name": "ownership",
        "description": "Get original authors of a file via git blame. Returns role='original_author'. For active maintainers/reviewers, use 'who_reviews' instead.",
        "parameters": {
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file.",
                },
            },
            "required": ["file_path"],
        },
    },
    "coupling": {
        "name": "coupling",
        "description": (
            "Get files that frequently change together (co-change coupling) or share "
            "static import edges (structural coupling). Returns co_change_partners, "
            "import_partners, and numeric import_coupling / effective_coupling scores. "
            "Import coupling is reliable in all projects; co-change coupling requires "
            "multi-author commit history with many small commits — solo projects or "
            "bulk-commit workflows will show 0.0 co-change (use import_partners instead). "
            "Threshold scales with project maturity (see 'stats' for current value). "
            + HEURISTIC_TRUST_NOTE
        ),
        "parameters": {
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file.",
                },
                "min_count": {
                    "type": "integer",
                    "description": "Minimum co-commit count threshold (default: 3).",
                },
            },
            "required": ["file_path"],
        },
    },
    "risk_map": {
        "name": "risk_map",
        "description": (
            "Risk scores for all files combining churn, coupling (git co-change "
            "and/or static import graph), coverage gaps, author concentration, "
            "and test instability (failure rate + duration variance). Returns "
            "{files: [...], _meta: {effective_components, uniform_components, "
            "reweighted, effective_weights, coverage_gap_mode, coupling_threshold, "
            "total_test_edges, total_test_results}}. Check _meta.uniform_components "
            "to identify metrics providing no signal (all files score identically). "
            "Use as first step to prioritize which files need attention. "
            + HEURISTIC_TRUST_NOTE
        ),
        "parameters": {
            "type": "object",
            "properties": {
                "directory": {
                    "type": "string",
                    "description": "Optional subdirectory to scope the risk map.",
                },
                "exclude_tests": {
                    "type": "boolean",
                    "description": (
                        "Exclude test files (default: true). Test files always "
                        "score coverage_gap=1.0, adding noise."
                    ),
                },
                "proximity_adjustment": {
                    "type": "boolean",
                    "description": (
                        "If true, slightly reduce coverage_gap for files a few "
                        "import hops from code covered by tests (see _meta.coverage_gap_mode)."
                    ),
                },
                "coverage_mode": {
                    "type": "string",
                    "enum": ["unit", "line"],
                    "description": (
                        "'unit' (default) weights each code unit equally when "
                        "computing coverage_gap; 'line' weights by line count "
                        "so large untested units have proportionally higher gap."
                    ),
                },
            },
            "required": [],
        },
    },
    "stale_tests": {
        "name": "stale_tests",
        "description": "Find tests whose source targets have changed since last analysis. Use to identify tests that may need updating.",
        "parameters": {
            "type": "object",
            "properties": {},
            "required": [],
        },
    },
    "history": {
        "name": "history",
        "description": "Commit history for a specific file. Use for understanding change patterns or finding when a bug was introduced.",
        "parameters": {
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file.",
                },
            },
            "required": ["file_path"],
        },
    },
    "who_reviews": {
        "name": "who_reviews",
        "description": "Suggest reviewers based on recent commit activity. Returns role='suggested_reviewer' — active maintainers, not original authors. For original authors, use 'ownership' instead.",
        "parameters": {
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file.",
                },
            },
            "required": ["file_path"],
        },
    },
    "record_result": {
        "name": "record_result",
        "description": "Record a test pass/fail outcome. Feeds into suggest_tests (failure rate) and risk_map (test instability). Use after running tests.",
        "parameters": {
            "type": "object",
            "properties": {
                "test_id": {
                    "type": "string",
                    "description": "The test ID (e.g. 'tests/test_app.py:test_foo').",
                },
                "passed": {
                    "type": "boolean",
                    "description": "Whether the test passed.",
                },
                "duration_ms": {
                    "type": "integer",
                    "description": "Optional test duration in milliseconds.",
                },
            },
            "required": ["test_id", "passed"],
        },
    },
    "diff_impact": {
        "name": "diff_impact",
        "description": (
            "Use after editing code to find which tests to run. Auto-detects "
            "changed files/functions from git diff and includes untracked code files. "
            "On feature branches diffs against main; on main diffs HEAD. Untracked "
            "files with no DB edges fall back to stem-matching (source=working_tree). "
            "With working_tree=true, performs full static import scanning for "
            "untracked files (matching suggest_tests behavior) to find tests that "
            "import new files by path. "
            "Returns diagnostic when no changes detected. If git fails (wrong cwd, "
            "not a repo), returns status=git_error with error (not_a_git_repo or "
            "git_command_failed), cwd, message, and project_dir — never silent empty "
            "lists. "
            + HEURISTIC_TRUST_NOTE
        ),
        "parameters": {
            "type": "object",
            "properties": {
                "ref": {
                    "type": "string",
                    "description": "Git ref to diff against (default: HEAD for unstaged changes).",
                },
                "working_tree": {
                    "type": "boolean",
                    "description": (
                        "If true, perform full static import scanning for "
                        "untracked files instead of only stem-match fallback. "
                        "Recommended during active development before files "
                        "are committed."
                    ),
                },
            },
            "required": [],
        },
    },
    "update": {
        "name": "update",
        "description": (
            "Incremental re-analysis — only changed files and new commits. Use instead "
            "of full 'analyze' after small edits. For large repos or slow MCP clients, "
            "prefer `chisel update` / `chisel analyze` in a terminal so the run cannot "
            "time out in silence."
        ),
        "parameters": {
            "type": "object",
            "properties": {},
            "required": [],
        },
    },
    "stats": {
        "name": "stats",
        "description": (
            "Database summary counts plus coupling threshold. Use to verify analysis state "
            "or diagnose empty results from other tools. "
            + HEURISTIC_TRUST_NOTE
        ),
        "parameters": {
            "type": "object",
            "properties": {},
            "required": [],
        },
    },
    "test_gaps": {
        "name": "test_gaps",
        "description": (
            "Find code units with no test coverage, sorted by churn risk. "
            "Use to decide which tests to write first. Excludes test files by default. "
            "If the DB has no test edges but a JS/TS relative import or Go import "
            "resolves to the source path, that file is treated as covered. With "
            "working_tree=true, untracked tests/paths are included in that scan. "
            + HEURISTIC_TRUST_NOTE
        ),
        "parameters": {
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Scope to a single file.",
                },
                "directory": {
                    "type": "string",
                    "description": "Scope to a directory (file_path takes precedence).",
                },
                "exclude_tests": {
                    "type": "boolean",
                    "description": "Exclude units from test files (default: true).",
                },
                "working_tree": {
                    "type": "boolean",
                    "description": (
                        "If true, also scan untracked (uncommitted) files from disk and "
                        "include their code units as gaps with churn=0. "
                        "useful for identifying coverage gaps in files that haven't been committed yet."
                    ),
                },
            },
            "required": [],
        },
    },
    "triage": {
        "name": "triage",
        "description": (
            "Combined risk + test gaps + stale tests for top-N files. "
            "Use as the first reconnaissance step before audits or "
            "refactors. Single call replaces risk_map + test_gaps + stale_tests. "
            + HEURISTIC_TRUST_NOTE
        ),
        "parameters": {
            "type": "object",
            "properties": {
                "directory": {
                    "type": "string",
                    "description": "Optional subdirectory to scope the triage.",
                },
                "top_n": {
                    "type": "integer",
                    "description": "Number of top-risk files to include (default: 10).",
                },
                "exclude_tests": {
                    "type": "boolean",
                    "description": (
                        "Exclude test files from risk ranking (default: true)."
                    ),
                },
            },
            "required": [],
        },
    },
    "start_job": {
        "name": "start_job",
        "description": (
            "Run full analyze or incremental update in a background thread; poll "
            "job_status with the returned job_id. Use when MCP clients would time out "
            "on long analyze/update (zero extra dependencies — stdlib threading only)."
        ),
        "parameters": {
            "type": "object",
            "properties": {
                "kind": {
                    "type": "string",
                    "enum": ["analyze", "update"],
                    "description": "analyze = full scan; update = incremental.",
                },
                "directory": {
                    "type": "string",
                    "description": "Optional subdirectory scope (analyze only).",
                },
                "force": {
                    "type": "boolean",
                    "description": "Force full re-analysis (analyze only).",
                },
            },
            "required": ["kind"],
        },
    },
    "job_status": {
        "name": "job_status",
        "description": (
            "Poll a job started with start_job. Returns status running | completed "
            "| failed | not_found; completed includes result dict; failed includes error."
        ),
        "parameters": {
            "type": "object",
            "properties": {
                "job_id": {
                    "type": "string",
                    "description": "Job id returned by start_job.",
                },
            },
            "required": ["job_id"],
        },
    },
    # --- file_locks (advisory locking for multi-agent coordination) ---
    "acquire_file_lock": {
        "name": "acquire_file_lock",
        "description": (
            "Acquire an advisory lock on a file before editing. Other agents "
            "checking this file will see you hold the lock. Use ttl to set how "
            "long the lock persists before auto-expiry (default 300s)."
        ),
        "parameters": {
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "File path to lock (normalized relative path).",
                },
                "agent_id": {
                    "type": "string",
                    "description": "Unique identifier for the requesting agent.",
                },
                "ttl": {
                    "type": "integer",
                    "description": "Lock TTL in seconds (default 300).",
                },
                "purpose": {
                    "type": "string",
                    "description": "Optional purpose or description.",
                },
            },
            "required": ["file_path", "agent_id"],
        },
    },
    "release_file_lock": {
        "name": "release_file_lock",
        "description": (
            "Release an advisory lock held by this agent. "
            "Returns false if the lock is not held by this agent."
        ),
        "parameters": {
            "type": "object",
            "properties": {
                "file_path": {"type": "string"},
                "agent_id": {"type": "string"},
            },
            "required": ["file_path", "agent_id"],
        },
    },
    "refresh_file_lock": {
        "name": "refresh_file_lock",
        "description": (
            "Extend the TTL of a lock held by this agent. "
            "Call periodically while actively editing a locked file."
        ),
        "parameters": {
            "type": "object",
            "properties": {
                "file_path": {"type": "string"},
                "agent_id": {"type": "string"},
                "ttl": {
                    "type": "integer",
                    "description": "New TTL in seconds (default 300).",
                },
            },
            "required": ["file_path", "agent_id"],
        },
    },
    "check_file_lock": {
        "name": "check_file_lock",
        "description": (
            "Check if a file is currently locked. Returns lock holder info "
            "or null if the file is not locked."
        ),
        "parameters": {
            "type": "object",
            "properties": {
                "file_path": {"type": "string"},
            },
            "required": ["file_path"],
        },
    },
    "check_locks": {
        "name": "check_locks",
        "description": (
            "Batch-check lock status for multiple files at once. "
            "Use before starting work to check for collisions."
        ),
        "parameters": {
            "type": "object",
            "properties": {
                "file_paths": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "List of file paths to check.",
                },
            },
            "required": ["file_paths"],
        },
    },
    "list_file_locks": {
        "name": "list_file_locks",
        "description": (
            "List all active file locks, optionally filtered by agent. "
            "Use to audit what other agents are working on."
        ),
        "parameters": {
            "type": "object",
            "properties": {
                "agent_id": {
                    "type": "string",
                    "description": "Filter locks by this agent (optional).",
                },
            },
            "required": [],
        },
    },
}

# ------------------------------------------------------------------ #
# Tool dispatch — map tool name to engine method + argument names
# ------------------------------------------------------------------ #

_TOOL_DISPATCH = {
    "analyze": ("tool_analyze", ["directory", "force"]),
    "impact": ("tool_impact", ["files", "functions"]),
    "suggest_tests": ("tool_suggest_tests", ["file_path", "fallback_to_all", "working_tree"]),
    "churn": ("tool_churn", ["file_path", "unit_name"]),
    "ownership": ("tool_ownership", ["file_path"]),
    "coupling": ("tool_coupling", ["file_path", "min_count"]),
    "risk_map": (
        "tool_risk_map",
        ["directory", "exclude_tests", "proximity_adjustment", "coverage_mode"],
    ),
    "stale_tests": ("tool_stale_tests", []),
    "history": ("tool_history", ["file_path"]),
    "who_reviews": ("tool_who_reviews", ["file_path"]),
    "diff_impact": ("tool_diff_impact", ["ref", "working_tree"]),
    "update": ("tool_update", []),
    "test_gaps": ("tool_test_gaps", ["file_path", "directory", "exclude_tests", "working_tree"]),
    "record_result": ("tool_record_result", ["test_id", "passed", "duration_ms"]),
    "triage": ("tool_triage", ["directory", "top_n", "exclude_tests"]),
    "stats": ("tool_stats", []),
    "start_job": ("tool_start_job", ["kind", "directory", "force"]),
    "job_status": ("tool_job_status", ["job_id"]),
    # --- file_locks ---
    "acquire_file_lock": ("tool_acquire_file_lock", ["file_path", "agent_id", "ttl", "purpose"]),
    "release_file_lock": ("tool_release_file_lock", ["file_path", "agent_id"]),
    "refresh_file_lock": ("tool_refresh_file_lock", ["file_path", "agent_id", "ttl"]),
    "check_file_lock": ("tool_check_file_lock", ["file_path"]),
    "check_locks": ("tool_check_locks", ["file_paths"]),
    "list_file_locks": ("tool_list_file_locks", ["agent_id"]),
}

# Inject 'limit' parameter into all list-returning tool schemas.
_LIMIT_PROP = {
    "type": "integer",
    "description": "Maximum number of results to return.",
}
for _name, _schema in _TOOL_SCHEMAS.items():
    if _name not in (
        "analyze", "update", "record_result", "stats", "triage",
        "start_job", "job_status",
        "acquire_file_lock", "release_file_lock", "refresh_file_lock",
        "check_file_lock",
    ):
        _schema["parameters"]["properties"]["limit"] = dict(_LIMIT_PROP)
del _name, _schema
