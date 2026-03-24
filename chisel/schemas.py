"""Tool schemas and dispatch tables for Chisel MCP servers.

Contains the JSON Schema definitions for all 16 engine tools and the
dispatch mapping used by both the HTTP and stdio MCP servers.
"""

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
            "test-to-code edges. For incremental changes, use 'update' instead."
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
            "diff, use 'diff_impact' instead."
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
        "description": "Suggest tests to run for a file, ranked by relevance. Use to find existing test coverage for a specific file.",
        "parameters": {
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file to suggest tests for.",
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
        "description": "Get files that frequently change together (co-change coupling). Use when risk_map shows coupling > 0, or before refactoring to find hidden dependencies. Threshold scales with project maturity (see 'stats' for current value).",
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
            "Risk scores for all files combining churn, coupling, coverage gaps, "
            "author concentration, and test instability. Returns {files: [...], "
            "_meta: {effective_components, uniform_components, coupling_threshold, "
            "total_test_edges, total_test_results}}. Check _meta.uniform_components "
            "to identify metrics providing no signal (all files score identically). "
            "Use as first step to prioritize which files need attention."
        ),
        "parameters": {
            "type": "object",
            "properties": {
                "directory": {
                    "type": "string",
                    "description": "Optional subdirectory to scope the risk map.",
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
            "changed files/functions from git diff. On feature branches diffs "
            "against main; on main diffs HEAD. Returns diagnostic with "
            "suggestions when no changes detected."
        ),
        "parameters": {
            "type": "object",
            "properties": {
                "ref": {
                    "type": "string",
                    "description": "Git ref to diff against (default: HEAD for unstaged changes).",
                },
            },
            "required": [],
        },
    },
    "update": {
        "name": "update",
        "description": "Incremental re-analysis — only changed files and new commits. Use instead of full 'analyze' after small edits.",
        "parameters": {
            "type": "object",
            "properties": {},
            "required": [],
        },
    },
    "stats": {
        "name": "stats",
        "description": "Database summary counts plus coupling threshold. Use to verify analysis state or diagnose empty results from other tools.",
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
            "Use to decide which tests to write first. Excludes test files by default."
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
            },
            "required": [],
        },
    },
    "triage": {
        "name": "triage",
        "description": (
            "Combined risk + test gaps + stale tests for top-N files. "
            "Use as the first reconnaissance step before audits or "
            "refactors. Single call replaces risk_map + test_gaps + stale_tests."
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
    "suggest_tests": ("tool_suggest_tests", ["file_path"]),
    "churn": ("tool_churn", ["file_path", "unit_name"]),
    "ownership": ("tool_ownership", ["file_path"]),
    "coupling": ("tool_coupling", ["file_path", "min_count"]),
    "risk_map": ("tool_risk_map", ["directory"]),
    "stale_tests": ("tool_stale_tests", []),
    "history": ("tool_history", ["file_path"]),
    "who_reviews": ("tool_who_reviews", ["file_path"]),
    "diff_impact": ("tool_diff_impact", ["ref"]),
    "update": ("tool_update", []),
    "test_gaps": ("tool_test_gaps", ["file_path", "directory", "exclude_tests"]),
    "record_result": ("tool_record_result", ["test_id", "passed", "duration_ms"]),
    "triage": ("tool_triage", ["directory", "top_n"]),
    "stats": ("tool_stats", []),
}

# Inject 'limit' parameter into all list-returning tool schemas.
_LIMIT_PROP = {
    "type": "integer",
    "description": "Maximum number of results to return.",
}
for _name, _schema in _TOOL_SCHEMAS.items():
    if _name not in ("analyze", "update", "record_result", "stats", "triage"):
        _schema["parameters"]["properties"]["limit"] = dict(_LIMIT_PROP)
del _name, _schema
