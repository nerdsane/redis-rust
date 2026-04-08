"""Unit tests for evaluator.py"""

import json
import tempfile
import unittest
from pathlib import Path

from .evaluator import OfflineEvaluator
from .scorer import Scorer


class TestOfflineEvaluator(unittest.TestCase):
    def setUp(self):
        self.tmpdir = tempfile.mkdtemp()
        self.gt_dir = Path(self.tmpdir) / "ground_truth"
        self.gt_dir.mkdir()
        self.reviews_dir = Path(self.tmpdir) / "reviews"
        self.reviews_dir.mkdir()

        # Create a simple ground truth file
        gt = {
            "task": "test_task",
            "description": "Test task",
            "context_files": [],
            "skills_under_test": ["skill-a", "skill-b"],
            "expected_findings": [
                {
                    "id": "f-01",
                    "severity": "inaccurate",
                    "claim": "test",
                    "ground_truth": "correct",
                    "keywords": ["alpha", "beta"],
                    "required": True,
                    "weight": 2.0,
                    "skill_relevance": ["skill-a"],
                },
            ],
            "known_false_positives": [],
        }
        (self.gt_dir / "test_task.json").write_text(json.dumps(gt))

    def test_list_tasks(self):
        evaluator = OfflineEvaluator(
            ground_truth_dir=self.gt_dir,
            reviews_dir=self.reviews_dir,
        )
        tasks = evaluator.list_tasks()
        assert tasks == ["test_task"]

    def test_load_ground_truth(self):
        evaluator = OfflineEvaluator(
            ground_truth_dir=self.gt_dir,
            reviews_dir=self.reviews_dir,
        )
        gt = evaluator.load_ground_truth("test_task")
        assert gt["task"] == "test_task"

    def test_save_and_get_review(self):
        evaluator = OfflineEvaluator(
            ground_truth_dir=self.gt_dir,
            reviews_dir=self.reviews_dir,
        )
        path = evaluator.save_review_text("skill-a", "test_task", "alpha beta review")
        assert path.exists()
        text = evaluator.get_review_text("skill-a", "test_task")
        assert text == "alpha beta review"

    def test_get_missing_review(self):
        evaluator = OfflineEvaluator(
            ground_truth_dir=self.gt_dir,
            reviews_dir=self.reviews_dir,
        )
        assert evaluator.get_review_text("nope", "nope") is None

    def test_evaluate_skill(self):
        evaluator = OfflineEvaluator(
            ground_truth_dir=self.gt_dir,
            reviews_dir=self.reviews_dir,
        )
        evaluator.save_review_text("skill-a", "test_task", "alpha beta found the issue")
        result = evaluator.evaluate_skill("skill-a")
        assert "test_task" in result.task_scores
        assert result.aggregate_score > 0

    def test_evaluate_skill_no_reviews(self):
        evaluator = OfflineEvaluator(
            ground_truth_dir=self.gt_dir,
            reviews_dir=self.reviews_dir,
        )
        result = evaluator.evaluate_skill("no-reviews")
        assert len(result.task_scores) == 0
        assert result.aggregate_score == 0.0

    def test_evaluate_all_skills(self):
        evaluator = OfflineEvaluator(
            ground_truth_dir=self.gt_dir,
            reviews_dir=self.reviews_dir,
        )
        evaluator.save_review_text("skill-a", "test_task", "alpha beta")
        evaluator.save_review_text("skill-b", "test_task", "gamma delta")

        results = evaluator.evaluate_all_skills()
        assert "skill-a" in results
        assert "skill-b" in results
        # skill-a should score higher (has relevant keywords)
        assert results["skill-a"].aggregate_score > results["skill-b"].aggregate_score

    def test_evaluation_result_summary(self):
        evaluator = OfflineEvaluator(
            ground_truth_dir=self.gt_dir,
            reviews_dir=self.reviews_dir,
        )
        evaluator.save_review_text("skill-a", "test_task", "alpha beta")
        result = evaluator.evaluate_skill("skill-a")
        summary = result.summary()
        assert "skill-a" in summary
        assert "aggregate" in summary


if __name__ == "__main__":
    unittest.main()
