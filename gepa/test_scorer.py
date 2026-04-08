"""Unit tests for scorer.py"""

import unittest
from .scorer import Scorer, _normalize_text, _rank, COMPONENT_WEIGHTS


class TestNormalizeText(unittest.TestCase):
    def test_lowercase(self):
        assert _normalize_text("Hello WORLD") == "hello world"

    def test_whitespace_collapse(self):
        assert _normalize_text("foo  bar\n\nbaz") == "foo bar baz"

    def test_empty(self):
        assert _normalize_text("") == ""


class TestRank(unittest.TestCase):
    def test_distinct_values(self):
        assert _rank([3.0, 1.0, 2.0]) == [3.0, 1.0, 2.0]

    def test_tied_values(self):
        ranks = _rank([1.0, 1.0, 3.0])
        assert ranks == [1.5, 1.5, 3.0]

    def test_all_same(self):
        ranks = _rank([5.0, 5.0, 5.0])
        assert ranks == [2.0, 2.0, 2.0]


class TestScorer(unittest.TestCase):
    def setUp(self):
        self.scorer = Scorer(min_keyword_fraction=0.5)
        self.ground_truth = {
            "expected_findings": [
                {
                    "id": "test-01",
                    "severity": "inaccurate",
                    "claim": "test claim",
                    "ground_truth": "correction",
                    "keywords": ["bounded", "channel", "unbounded"],
                    "required": True,
                    "weight": 2.0,
                    "skill_relevance": ["rust-dev"],
                },
                {
                    "id": "test-02",
                    "severity": "nit",
                    "claim": "minor issue",
                    "ground_truth": "correction",
                    "keywords": ["format", "error", "prefix"],
                    "required": False,
                    "weight": 0.5,
                    "skill_relevance": ["rust-dev"],
                },
            ],
            "known_false_positives": [
                {
                    "id": "fp-01",
                    "claim": "buggify is broken",
                    "why_correct": "intentional",
                    "keywords": ["buggify", "default", "enabled"],
                    "penalty": 1.0,
                },
            ],
        }

    def test_perfect_review(self):
        """Review that hits all keywords should score high."""
        text = (
            "The bounded channel claim is wrong. Channels are unbounded. "
            "Also the error format has wrong prefix."
        )
        result = self.scorer.score(text, self.ground_truth)
        assert result.true_positives == 2
        assert result.false_positives == 0
        assert result.weighted_recall == 1.0
        assert result.composite > 0.8

    def test_empty_review(self):
        """Empty review should score zero recall."""
        result = self.scorer.score("", self.ground_truth)
        assert result.true_positives == 0
        assert result.weighted_recall == 0.0
        # precision=1.0 (no positives) + calibration=0.5 gives ~0.325
        assert result.composite < 0.4

    def test_partial_review(self):
        """Review hitting only some keywords."""
        text = "The channel implementation uses unbounded channels."
        result = self.scorer.score(text, self.ground_truth)
        # Should match test-01 (2/3 keywords = 0.67 > 0.5)
        assert result.true_positives >= 1

    def test_false_positive_detected(self):
        """Review flagging a known false positive should lose precision."""
        text = (
            "The bounded channel claim is wrong. Channels are unbounded. "
            "Also buggify default being enabled is a bug."
        )
        result = self.scorer.score(text, self.ground_truth)
        assert result.false_positives == 1
        assert result.precision < 1.0

    def test_skill_filter(self):
        """Filtering by skill should only score relevant findings."""
        gt = {
            "expected_findings": [
                {
                    "id": "a",
                    "severity": "inaccurate",
                    "keywords": ["foo", "bar"],
                    "required": True,
                    "weight": 1.0,
                    "skill_relevance": ["rust-dev"],
                },
                {
                    "id": "b",
                    "severity": "gap",
                    "keywords": ["baz", "qux"],
                    "required": True,
                    "weight": 1.0,
                    "skill_relevance": ["dst"],
                },
            ],
            "known_false_positives": [],
        }
        result = self.scorer.score("foo bar baz qux", gt, skill_name="rust-dev")
        # Should only evaluate finding "a"
        assert len(result.finding_matches) == 1
        assert result.finding_matches[0].finding_id == "a"

    def test_missed_required(self):
        """Missing a required finding should be tracked."""
        text = "The error format has wrong prefix."
        result = self.scorer.score(text, self.ground_truth)
        assert "test-01" in result.missed_required

    def test_component_weights_sum_to_one(self):
        total = sum(COMPONENT_WEIGHTS.values())
        assert abs(total - 1.0) < 1e-6, f"weights sum to {total}"

    def test_no_findings(self):
        """Ground truth with no findings should return sensible defaults."""
        gt = {"expected_findings": [], "known_false_positives": []}
        result = self.scorer.score("anything here", gt)
        assert result.weighted_recall == 0.0
        assert result.precision == 1.0


class TestScorerMinFraction(unittest.TestCase):
    def test_high_threshold(self):
        """Higher threshold requires more keywords to match."""
        scorer = Scorer(min_keyword_fraction=1.0)
        gt = {
            "expected_findings": [{
                "id": "x",
                "severity": "gap",
                "keywords": ["alpha", "beta", "gamma"],
                "required": True,
                "weight": 1.0,
            }],
            "known_false_positives": [],
        }
        # Only 2/3 keywords present -> should NOT match with threshold=1.0
        result = scorer.score("alpha beta", gt)
        assert result.true_positives == 0

    def test_low_threshold(self):
        """Lower threshold matches with fewer keywords."""
        scorer = Scorer(min_keyword_fraction=0.3)
        gt = {
            "expected_findings": [{
                "id": "x",
                "severity": "gap",
                "keywords": ["alpha", "beta", "gamma"],
                "required": True,
                "weight": 1.0,
            }],
            "known_false_positives": [],
        }
        # 1/3 keywords present -> should match with threshold=0.3
        result = scorer.score("alpha", gt)
        assert result.true_positives == 1


if __name__ == "__main__":
    unittest.main()
