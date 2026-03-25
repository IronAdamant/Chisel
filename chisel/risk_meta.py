"""Risk map metadata, diagnostics, and dynamic reweighting of composite scores."""

from chisel.metrics import coupling_threshold

_BASE_RISK_WEIGHTS = {
    "churn": 0.35,
    "coupling": 0.25,
    "coverage_gap": 0.2,
    "author_concentration": 0.1,
    "test_instability": 0.1,
}

_COMPONENTS = tuple(_BASE_RISK_WEIGHTS.keys())


def _diagnose_uniform(comp, value, stats):
    """Return a diagnostic reason for a uniform risk component."""
    if comp == "coupling":
        if value == 0.0:
            thr = coupling_threshold(stats.get("commits", 0))
            return (
                f"no co-changes above threshold ({thr}); "
                "may need more git history or lower threshold"
            )
        return "all files equally coupled"
    if comp == "coverage_gap":
        edges = stats.get("test_edges", 0)
        if value == 1.0 and edges == 0:
            return (
                "no test edges found; edge builder may not match "
                "this project's import/require patterns"
            )
        if value == 1.0:
            return "no code units have test edges despite edges existing"
        if value == 0.0:
            return "all code units have test coverage"
        return f"all files have identical coverage ({value})"
    if comp == "test_instability":
        results = stats.get("test_results", 0)
        if value == 0.0 and results == 0:
            return "no test results recorded; use record_result after running tests"
        if value == 0.0:
            return "all covering tests passing"
        return f"all files have identical instability ({value})"
    if comp == "author_concentration":
        if value == 1.0:
            return "single author per file (common in small teams)"
        return f"all files have identical concentration ({value})"
    if comp == "churn":
        if value == 0.0:
            return "no churn data; run analyze with git history"
        return f"all files have identical churn ({value})"
    return ""


def build_risk_meta(files, stats):
    """Build diagnostic metadata about risk score data quality."""
    if not files:
        return {"total_files": 0}

    commit_count = stats.get("commits", 0)

    uniform = {}
    effective = []

    for comp in _COMPONENTS:
        values = {f["breakdown"][comp] for f in files}
        if len(values) <= 1:
            val = next(iter(values)) if values else 0.0
            uniform[comp] = {
                "value": val,
                "reason": _diagnose_uniform(comp, val, stats),
            }
        else:
            effective.append(comp)

    return {
        "total_files": len(files),
        "coupling_threshold": coupling_threshold(commit_count) if commit_count > 0 else None,
        "total_test_edges": stats.get("test_edges", 0),
        "total_test_results": stats.get("test_results", 0),
        "effective_components": effective,
        "uniform_components": uniform,
    }


def apply_risk_reweighting(risk_map):
    """When 3+ components are uniform, redistribute their weight onto varying ones.

    Only updates ``risk_score``; per-component ``breakdown`` is unchanged.
    Returns ``(risk_map, meta_dict)``.
    """
    if len(risk_map) <= 1:
        return risk_map, {
            "reweighted": False,
            "reweighting_skipped_reason": "insufficient_files",
        }

    uniform = []
    for comp in _COMPONENTS:
        vals = {entry["breakdown"][comp] for entry in risk_map}
        if len(vals) <= 1:
            uniform.append(comp)

    if len(uniform) < 3:
        return risk_map, {"reweighted": False}

    effective = [c for c in _COMPONENTS if c not in uniform]
    if not effective:
        return risk_map, {
            "reweighted": False,
            "reweighting_skipped_reason": "all_components_uniform",
        }

    dead_weight = sum(_BASE_RISK_WEIGHTS[c] for c in uniform)
    sum_eff = sum(_BASE_RISK_WEIGHTS[c] for c in effective)
    new_w = {}
    for c in effective:
        new_w[c] = (
            _BASE_RISK_WEIGHTS[c]
            + dead_weight * (_BASE_RISK_WEIGHTS[c] / sum_eff)
        )

    for entry in risk_map:
        bd = entry["breakdown"]
        risk = sum(new_w[c] * bd[c] for c in effective)
        entry["risk_score"] = round(risk, 4)

    meta = {
        "reweighted": True,
        "effective_weights": {k: round(new_w[k], 4) for k in effective},
    }
    return risk_map, meta
