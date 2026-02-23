"""
Skill fitness evaluators for evolution.

Three evaluator types:
- MockSkillEvaluator: baseline + gaussian noise (free, for testing GA machinery)
- OfflineSkillEvaluator: wraps existing OfflineEvaluator (free, deterministic)
- LiveSkillEvaluator: calls Claude CLI, scores against ground truth (~$0.05/eval)
"""

import logging
import shutil
import tempfile
from abc import ABC, abstractmethod
from dataclasses import dataclass
from pathlib import Path
from typing import List, Optional

from .candidate import SkillCandidate
from .evaluator import OfflineEvaluator, LiveEvaluator, GROUND_TRUTH_DIR, REVIEWS_DIR, AGENTS_DIR
from .scorer import Scorer

logger = logging.getLogger(__name__)


class SkillFitnessEvaluator(ABC):
    """Abstract base class for skill fitness evaluation."""

    @abstractmethod
    def evaluate(self, candidate: SkillCandidate) -> float:
        """Evaluate a skill candidate. Returns fitness score 0.0-1.0."""
        ...

    @property
    @abstractmethod
    def cost_per_eval(self) -> float:
        """Estimated cost in USD per evaluation."""
        ...


@dataclass
class CostTracker:
    """Track API spending against a budget cap."""
    budget_cap: float = 5.0
    spent: float = 0.0

    def can_afford(self, cost: float) -> bool:
        return (self.spent + cost) <= self.budget_cap

    def charge(self, cost: float) -> None:
        self.spent += cost

    @property
    def remaining(self) -> float:
        return max(0.0, self.budget_cap - self.spent)

    def summary(self) -> str:
        return f"${self.spent:.2f} / ${self.budget_cap:.2f} ({self.remaining:.2f} remaining)"


class MockSkillEvaluator(SkillFitnessEvaluator):
    """
    Mock evaluator for testing GA machinery.

    Score = baseline + gaussian_noise(std=0.05), penalized if word count
    deviates too far from original.
    """

    def __init__(self, baseline: float = 0.5, noise_std: float = 0.05,
                 original_word_count: int = 500, rng=None):
        self.baseline = baseline
        self.noise_std = noise_std
        self.original_word_count = original_word_count
        self._rng = rng  # random.Random instance for determinism

    def evaluate(self, candidate: SkillCandidate) -> float:
        import random as _random
        rng = self._rng or _random.Random()

        noise = rng.gauss(0, self.noise_std)
        score = self.baseline + noise

        # Penalize word count deviation
        wc = candidate.word_count()
        if self.original_word_count > 0:
            ratio = wc / self.original_word_count
            if ratio < 0.5 or ratio > 2.0:
                score -= 0.1 * abs(1.0 - ratio)

        return max(0.0, min(1.0, score))

    @property
    def cost_per_eval(self) -> float:
        return 0.0


class OfflineSkillEvaluator(SkillFitnessEvaluator):
    """
    Wraps existing OfflineEvaluator for scoring stability testing.

    Note: Cannot improve scores since review text is cached and doesn't
    change with the skill content. Useful for verifying GA doesn't degrade.
    """

    def __init__(self, ground_truth_dir: Path = None, reviews_dir: Path = None,
                 tasks: List[str] = None):
        self._evaluator = OfflineEvaluator(
            ground_truth_dir=ground_truth_dir or GROUND_TRUTH_DIR,
            reviews_dir=reviews_dir or REVIEWS_DIR,
        )
        self._tasks = tasks

    def evaluate(self, candidate: SkillCandidate) -> float:
        result = self._evaluator.evaluate_skill(
            candidate.name, tasks=self._tasks
        )
        return result.aggregate_score

    @property
    def cost_per_eval(self) -> float:
        return 0.0


class LiveSkillEvaluator(SkillFitnessEvaluator):
    """
    Live evaluator: writes skill to temp file, calls Claude CLI, scores response.

    Cost: ~$0.05 per task evaluation.
    """

    def __init__(self, ground_truth_dir: Path = None, agents_dir: Path = None,
                 reviews_dir: Path = None, model: str = "sonnet",
                 tasks: List[str] = None, cost_tracker: CostTracker = None):
        self._ground_truth_dir = ground_truth_dir or GROUND_TRUTH_DIR
        self._agents_dir = agents_dir or AGENTS_DIR
        self._reviews_dir = reviews_dir or REVIEWS_DIR
        self._model = model
        self._tasks = tasks
        self._scorer = Scorer()
        self.cost_tracker = cost_tracker or CostTracker()
        self._live_eval = LiveEvaluator(
            ground_truth_dir=self._ground_truth_dir,
            agents_dir=self._agents_dir,
            reviews_dir=self._reviews_dir,
            scorer=self._scorer,
            model=self._model,
        )

    def evaluate(self, candidate: SkillCandidate) -> float:
        tasks = self._tasks or self._list_tasks()
        cost = 0.05 * len(tasks)

        if not self.cost_tracker.can_afford(cost):
            logger.warning("Budget exhausted (spent=%s, cap=%s). Returning 0.0",
                         self.cost_tracker.spent, self.cost_tracker.budget_cap)
            return 0.0

        # Write candidate to temp file
        with tempfile.NamedTemporaryFile(mode='w', suffix='.md', delete=False) as f:
            f.write(candidate.to_markdown())
            temp_path = Path(f.name)

        try:
            # Temporarily point agents dir to temp location
            original_agents = self._live_eval.agents_dir
            temp_agents = temp_path.parent
            # Copy temp file with correct name
            skill_path = temp_agents / f"{candidate.name}.md"
            if skill_path != temp_path:
                shutil.copy2(temp_path, skill_path)

            self._live_eval.agents_dir = temp_agents

            scores = []
            for task_name in tasks:
                result = self._live_eval.evaluate_skill_live(
                    candidate.name, task_name, save_review=False
                )
                if result:
                    scores.append(result.composite)

            self.cost_tracker.charge(cost)
        finally:
            temp_path.unlink(missing_ok=True)
            skill_path_cleanup = temp_path.parent / f"{candidate.name}.md"
            if skill_path_cleanup.exists() and skill_path_cleanup != temp_path:
                skill_path_cleanup.unlink(missing_ok=True)
            self._live_eval.agents_dir = original_agents

        if not scores:
            return 0.0
        return sum(scores) / len(scores)

    def _list_tasks(self) -> List[str]:
        return sorted(
            p.stem for p in self._ground_truth_dir.glob("*.json")
        )

    @property
    def cost_per_eval(self) -> float:
        tasks = self._tasks or self._list_tasks()
        return 0.05 * len(tasks)
