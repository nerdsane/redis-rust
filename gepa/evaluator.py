"""
Evaluator - offline and live evaluation modes for skill quality.

Offline mode: score cached review text against ground truth (free, instant).
Live mode: call Claude CLI with skill as context, score response (costs ~$0.05/eval).
"""

import json
import logging
import subprocess
from dataclasses import dataclass, field
from pathlib import Path
from typing import Dict, List, Optional

from .scorer import Scorer, ScoreResult
from .candidate import SkillCandidate

logger = logging.getLogger(__name__)

# Default locations
GROUND_TRUTH_DIR = Path(__file__).parent / "ground_truth"
AGENTS_DIR = Path(__file__).parent.parent / ".claude" / "agents"
REVIEWS_DIR = Path(__file__).parent / "reviews"


@dataclass
class EvaluationResult:
    """Result of evaluating a skill across multiple ground truth tasks."""
    skill_name: str
    task_scores: Dict[str, ScoreResult]  # task_name -> ScoreResult
    aggregate_score: float  # Mean composite across tasks

    def summary(self) -> str:
        lines = [f"=== {self.skill_name} (aggregate: {self.aggregate_score:.3f}) ==="]
        for task_name, result in sorted(self.task_scores.items()):
            lines.append(f"\n--- {task_name} ---")
            lines.append(result.summary())
        return "\n".join(lines)


class OfflineEvaluator:
    """
    Score skills against ground truth using cached review texts.

    Cached reviews are stored in gepa/reviews/{skill_name}/{task_name}.txt.
    These are captured once (from real Claude reviews) and reused for
    deterministic, zero-cost scoring iterations.
    """

    def __init__(
        self,
        ground_truth_dir: Optional[Path] = None,
        reviews_dir: Optional[Path] = None,
        scorer: Optional[Scorer] = None,
    ):
        self.ground_truth_dir = ground_truth_dir or GROUND_TRUTH_DIR
        self.reviews_dir = reviews_dir or REVIEWS_DIR
        self.scorer = scorer or Scorer()

        assert self.ground_truth_dir.exists(), (
            f"Ground truth directory not found: {self.ground_truth_dir}"
        )

    def load_ground_truth(self, task_name: str) -> Dict:
        """Load a ground truth JSON file by task name."""
        path = self.ground_truth_dir / f"{task_name}.json"
        assert path.exists(), f"Ground truth file not found: {path}"
        return json.loads(path.read_text())

    def list_tasks(self) -> List[str]:
        """List available ground truth tasks."""
        return sorted(
            p.stem for p in self.ground_truth_dir.glob("*.json")
        )

    def get_review_text(self, skill_name: str, task_name: str) -> Optional[str]:
        """Load cached review text for a skill+task combination."""
        path = self.reviews_dir / skill_name / f"{task_name}.txt"
        if path.exists():
            return path.read_text()
        return None

    def save_review_text(self, skill_name: str, task_name: str, text: str) -> Path:
        """Save review text for later offline scoring."""
        dir_path = self.reviews_dir / skill_name
        dir_path.mkdir(parents=True, exist_ok=True)
        path = dir_path / f"{task_name}.txt"
        path.write_text(text)
        return path

    def evaluate_skill(
        self,
        skill_name: str,
        tasks: Optional[List[str]] = None,
    ) -> EvaluationResult:
        """
        Evaluate a skill across all (or specified) ground truth tasks.

        Args:
            skill_name: Name of the skill (e.g., "rust-dev").
            tasks: Specific tasks to evaluate. Defaults to all available.

        Returns:
            EvaluationResult with per-task and aggregate scores.
        """
        if tasks is None:
            tasks = self.list_tasks()

        task_scores = {}
        for task_name in tasks:
            review_text = self.get_review_text(skill_name, task_name)
            if review_text is None:
                logger.warning(
                    "No cached review for %s/%s, skipping", skill_name, task_name
                )
                continue

            ground_truth = self.load_ground_truth(task_name)
            score = self.scorer.score(review_text, ground_truth, skill_name=skill_name)
            task_scores[task_name] = score

        aggregate = 0.0
        if task_scores:
            aggregate = sum(s.composite for s in task_scores.values()) / len(task_scores)

        return EvaluationResult(
            skill_name=skill_name,
            task_scores=task_scores,
            aggregate_score=aggregate,
        )

    def evaluate_all_skills(self) -> Dict[str, EvaluationResult]:
        """Evaluate all skills that have cached reviews."""
        results = {}
        if not self.reviews_dir.exists():
            logger.warning("Reviews directory does not exist: %s", self.reviews_dir)
            return results

        for skill_dir in sorted(self.reviews_dir.iterdir()):
            if skill_dir.is_dir():
                result = self.evaluate_skill(skill_dir.name)
                if result.task_scores:
                    results[skill_dir.name] = result

        return results


class LiveEvaluator:
    """
    Evaluate skills by running Claude CLI with skill context.

    Generates a review by invoking Claude with the skill file as context,
    then scores the response against ground truth. Costs ~$0.05 per eval.
    """

    def __init__(
        self,
        ground_truth_dir: Optional[Path] = None,
        agents_dir: Optional[Path] = None,
        reviews_dir: Optional[Path] = None,
        scorer: Optional[Scorer] = None,
        model: str = "sonnet",
    ):
        self.ground_truth_dir = ground_truth_dir or GROUND_TRUTH_DIR
        self.agents_dir = agents_dir or AGENTS_DIR
        self.reviews_dir = reviews_dir or REVIEWS_DIR
        self.scorer = scorer or Scorer()
        self.model = model

    def evaluate_skill_live(
        self,
        skill_name: str,
        task_name: str,
        save_review: bool = True,
    ) -> Optional[ScoreResult]:
        """
        Run a live evaluation: invoke Claude, score the response.

        Args:
            skill_name: Skill to evaluate (e.g., "rust-dev").
            task_name: Ground truth task (e.g., "paper_review").
            save_review: Whether to cache the review text.

        Returns:
            ScoreResult, or None if the CLI call fails.
        """
        ground_truth = json.loads(
            (self.ground_truth_dir / f"{task_name}.json").read_text()
        )

        # Build the prompt
        context_files = ground_truth.get("context_files", [])
        prompt = self._build_review_prompt(task_name, context_files)

        # Invoke Claude CLI
        skill_path = self.agents_dir / f"{skill_name}.md"
        if not skill_path.exists():
            logger.error("Skill file not found: %s", skill_path)
            return None

        review_text = self._call_claude(prompt, skill_path)
        if review_text is None:
            return None

        # Optionally save the review
        if save_review:
            evaluator = OfflineEvaluator(
                ground_truth_dir=self.ground_truth_dir,
                reviews_dir=self.reviews_dir,
            )
            evaluator.save_review_text(skill_name, task_name, review_text)

        return self.scorer.score(review_text, ground_truth, skill_name=skill_name)

    def _build_review_prompt(self, task_name: str, context_files: List[str]) -> str:
        """Build a review prompt for the given task."""
        if "paper" in task_name:
            return (
                "Review the technical paper (docs/PAPER.md) for accuracy. "
                "Focus on: factual claims about the architecture, benchmark "
                "methodology, and testing coverage. Flag any inaccuracies, "
                "misleading claims, or important gaps. Be specific and cite "
                "the relevant section."
            )
        elif "pr" in task_name:
            files_str = ", ".join(context_files)
            return (
                f"Review this PR's changes to: {files_str}. "
                "Check for: correctness, TigerStyle compliance (assertions, "
                "checked arithmetic), DST coverage gaps, and Redis compatibility. "
                "Flag violations, bugs, and missing test coverage."
            )
        return "Review the code for correctness, style, and test coverage."

    def _call_claude(self, prompt: str, skill_path: Path) -> Optional[str]:
        """Invoke Claude CLI with skill context. Returns response text."""
        cmd = [
            "claude",
            "--print",
            "--model", self.model,
            "--system", skill_path.read_text(),
            prompt,
        ]

        try:
            result = subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                timeout=120,
                cwd=str(Path(__file__).parent.parent),
            )
            if result.returncode != 0:
                logger.error("Claude CLI failed: %s", result.stderr[:500])
                return None
            return result.stdout
        except subprocess.TimeoutExpired:
            logger.error("Claude CLI timed out after 120s")
            return None
        except FileNotFoundError:
            logger.error("Claude CLI not found. Install: npm i -g @anthropic-ai/claude-code")
            return None
