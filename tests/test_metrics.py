"""Tests for chisel.metrics — date parsing and co-change file cap."""

import pytest
from datetime import datetime, timezone

from chisel.metrics import _parse_iso_date, compute_co_changes, _MAX_CO_CHANGE_FILES
from chisel.risk_meta import (
    _BASE_RISK_WEIGHTS,
    _HIDDEN_RISK_SCALE,
    _NEW_FILE_BOOST,
    compose_risk_score,
    hidden_risk_from_dynamic_edges,
)


# ------------------------------------------------------------------ #
# _parse_iso_date
# ------------------------------------------------------------------ #

class TestParseIsoDate:
    """Test ISO 8601 date string parsing edge cases."""

    def test_z_suffix_converted_to_utc(self):
        dt = _parse_iso_date("2026-03-01T12:00:00Z")
        assert dt.tzinfo is not None
        assert dt == datetime(2026, 3, 1, 12, 0, 0, tzinfo=timezone.utc)

    def test_offset_preserved(self):
        dt = _parse_iso_date("2026-03-01T12:00:00+05:30")
        assert dt.tzinfo is not None
        assert dt.utcoffset().total_seconds() == 5 * 3600 + 30 * 60

    def test_naive_datetime_gets_utc(self):
        dt = _parse_iso_date("2026-03-01T12:00:00")
        assert dt.tzinfo == timezone.utc

    def test_invalid_string_raises(self):
        with pytest.raises(ValueError):
            _parse_iso_date("not-a-date")


# ------------------------------------------------------------------ #
# compute_co_changes — file cap behaviour
# ------------------------------------------------------------------ #

class TestComputeCoChangesCap:
    """Test that commits exceeding _MAX_CO_CHANGE_FILES are skipped."""

    def _make_commit(self, num_files, date="2026-01-01"):
        """Build a single commit touching num_files distinct files."""
        return {
            "hash": "abc123",
            "author": "A",
            "author_email": "",
            "date": date,
            "message": "bulk",
            "files": [
                {"path": f"file_{i}.py", "insertions": 1, "deletions": 0}
                for i in range(num_files)
            ],
        }

    def test_commit_above_cap_skipped(self):
        """A commit touching >50 files should produce no co-change pairs."""
        commit = self._make_commit(_MAX_CO_CHANGE_FILES + 1)
        result = compute_co_changes([commit], min_count=1)
        assert result == []

    def test_commit_at_cap_not_skipped(self):
        """A commit touching exactly 50 files should be processed."""
        commit = self._make_commit(_MAX_CO_CHANGE_FILES)
        result = compute_co_changes([commit], min_count=1)
        assert len(result) > 0


# ------------------------------------------------------------------ #
# Risk weight single source (risk_meta.compose_risk_score)
# ------------------------------------------------------------------ #

class TestComposeRiskScore:
    def test_weights_sum_to_one(self):
        assert abs(sum(_BASE_RISK_WEIGHTS.values()) - 1.0) < 1e-9

    def test_full_churn_only(self):
        score = compose_risk_score(1.0, 0.0, 0.0, 0.0, 0.0, 0.0)
        assert score == pytest.approx(_BASE_RISK_WEIGHTS["churn"])

    def test_all_base_components_one(self):
        score = compose_risk_score(1.0, 1.0, 1.0, 1.0, 1.0, 1.0)
        assert score == pytest.approx(1.0)

    def test_hidden_and_new_file_additive(self):
        base = compose_risk_score(0.0, 0.0, 0.0, 0.0, 0.0, 0.0)
        with_extras = compose_risk_score(
            0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
            hidden_risk_factor=_HIDDEN_RISK_SCALE,
            new_file_boost=_NEW_FILE_BOOST,
        )
        assert with_extras == pytest.approx(base + _HIDDEN_RISK_SCALE + _NEW_FILE_BOOST)

    def test_hidden_risk_from_dynamic_edges_caps(self):
        assert hidden_risk_from_dynamic_edges(0) == 0.0
        assert hidden_risk_from_dynamic_edges(20) == pytest.approx(_HIDDEN_RISK_SCALE)
        assert hidden_risk_from_dynamic_edges(100) == pytest.approx(_HIDDEN_RISK_SCALE)
