"""Tool schemas and dispatch tables for Chisel MCP servers.

Contains the JSON Schema definitions for all 15 engine tools and the
dispatch mapping used by both the HTTP and stdio MCP servers.
"""

# ------------------------------------------------------------------ #
# Tool schemas — JSON Schema definitions for all 15 engine tools
# ------------------------------------------------------------------ #

_TOOL_SCHEMAS = {
    "analyze": {
        "name": "analyze",
        "description": (
            "Run full code analysis on the project. Scans files, parses code "
            "units, discovers tests, parses git history, and builds test edges."
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
            "Get impacted tests for the given changed files and optional functions."
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
        "description": "Suggest tests to run for a given file.",
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
        "description": "Get churn statistics for a file or a specific unit within it.",
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
        "description": "Get code ownership breakdown showing original authors (blame-based). Each entry has role='original_author'.",
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
        "description": "Get co-change coupling partners for a file.",
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
        "description": "Compute risk scores for all files in the project.",
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
        "description": "Detect stale tests whose source code has changed since last run.",
        "parameters": {
            "type": "object",
            "properties": {},
            "required": [],
        },
    },
    "history": {
        "name": "history",
        "description": "Get commit history for a specific file.",
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
        "description": "Suggest reviewers for a file based on recent commit activity. Each entry has role='suggested_reviewer' — these are not original authors but active maintainers best suited to review changes.",
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
        "description": "Record a test result (pass/fail) for future prioritization.",
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
            "Auto-detect changed files and functions from git diff, "
            "then return impacted tests. No need to specify files manually."
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
        "description": "Incremental re-analysis — only re-process changed files and new commits since last analysis.",
        "parameters": {
            "type": "object",
            "properties": {},
            "required": [],
        },
    },
    "stats": {
        "name": "stats",
        "description": "Get summary counts for the Chisel database (code units, tests, edges, commits, etc.).",
        "parameters": {
            "type": "object",
            "properties": {},
            "required": [],
        },
    },
    "test_gaps": {
        "name": "test_gaps",
        "description": (
            "Find code units (functions, classes) with no test coverage, "
            "prioritized by churn risk. Use after analyze to see what new tests need to be written."
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
    "stats": ("tool_stats", []),
}

# Inject 'limit' parameter into all list-returning tool schemas.
_LIMIT_PROP = {
    "type": "integer",
    "description": "Maximum number of results to return.",
}
for _name, _schema in _TOOL_SCHEMAS.items():
    if _name not in ("analyze", "update", "record_result", "stats"):
        _schema["parameters"]["properties"]["limit"] = dict(_LIMIT_PROP)
del _name, _schema
