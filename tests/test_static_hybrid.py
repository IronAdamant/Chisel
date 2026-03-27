"""Hybrid DB + static merge."""

from chisel.impact import _HYBRID_STATIC_BONUS, _merge_impacted_and_static


class TestMergeImpactedAndStatic:
    def test_static_only(self):
        static = [
            {
                "test_id": "t/a.js:x",
                "file_path": "t/a.js",
                "name": "x",
                "score": 0.8,
                "reason": "static",
                "source": "static_require",
            },
        ]
        out = _merge_impacted_and_static([], static)
        assert len(out) == 1
        assert out[0]["source"] == "static_require"

    def test_hybrid_boosts_when_both(self):
        db = [
            {
                "test_id": "t/a.js:x",
                "file_path": "t/a.js",
                "name": "x",
                "score": 0.5,
                "reason": "direct edge",
                "source": "direct",
            },
        ]
        static = [
            {
                "test_id": "t/a.js:x",
                "file_path": "t/a.js",
                "name": "x",
                "score": 0.9,
                "reason": "static import → src/foo.js",
                "source": "static_require",
            },
        ]
        out = _merge_impacted_and_static(db, static)
        assert len(out) == 1
        assert out[0]["source"] == "hybrid"
        expected = min(1.0, 0.5 + 0.9 * _HYBRID_STATIC_BONUS)
        assert abs(out[0]["score"] - expected) < 1e-9
