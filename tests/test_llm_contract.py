"""Sanity checks for chisel.llm_contract (importable, non-empty vocabulary)."""

import chisel.llm_contract as lc


def test_constants_are_non_empty_strings():
    assert len(lc.HEURISTIC_TRUST_NOTE) > 20
    assert len(lc.SECURITY_MODEL) > 50
    assert lc.RESPONSE_STATUSES
    assert lc.SUGGEST_SOURCES
    assert lc.READ_FIRST_KEYS


def test_statuses_include_git_error():
    assert "git_error" in lc.RESPONSE_STATUSES


def test_sources_include_hybrid():
    assert "hybrid" in lc.SUGGEST_SOURCES
    assert "static_require" in lc.SUGGEST_SOURCES
