"""Unit tests for evolution.py"""

import random
import tempfile
import unittest
from pathlib import Path

from .candidate import SkillCandidate
from .evolution import SkillEvolutionEngine
from .skill_fitness import MockSkillEvaluator


SAMPLE_SKILL = """---
name: test-skill
description: Test
user_invocable: true
---

# Test Skill

Preamble text.

## Section One

First section content. Multiple sentences here. And another one.

## Section Two

Second section content. With more text. And details.

## Section Three

Third section content.
"""


class TestSkillEvolutionEngine(unittest.TestCase):
    def setUp(self):
        SkillCandidate.reset_id_counter()
        self.tmpdir = tempfile.mkdtemp()
        self.results_dir = Path(self.tmpdir) / "results"

        # Create a mock agents dir with the test skill
        self.agents_dir = Path(self.tmpdir) / "agents"
        self.agents_dir.mkdir(parents=True)
        (self.agents_dir / "test-skill.md").write_text(SAMPLE_SKILL)

    def _make_engine(self, **kwargs):
        evaluator = MockSkillEvaluator(
            baseline=0.5, noise_std=0.05,
            rng=random.Random(42),
        )
        defaults = dict(
            skill_name="test-skill",
            evaluator=evaluator,
            population_size=4,
            elite_count=1,
            mutation_rate=0.3,
            generations=3,
            results_dir=self.results_dir,
            budget_cap=100.0,
            seed=42,
        )
        defaults.update(kwargs)

        # Monkey-patch AGENTS_DIR for test
        import gepa.evaluator as eval_mod
        self._orig_agents = eval_mod.AGENTS_DIR
        eval_mod.AGENTS_DIR = self.agents_dir

        return SkillEvolutionEngine(**defaults)

    def tearDown(self):
        import gepa.evaluator as eval_mod
        if hasattr(self, '_orig_agents'):
            eval_mod.AGENTS_DIR = self._orig_agents

    def test_basic_run(self):
        engine = self._make_engine()
        best = engine.run()
        assert best is not None
        assert best.fitness is not None
        assert best.fitness > 0

    def test_deterministic_with_seed(self):
        engine1 = self._make_engine(seed=99)
        best1 = engine1.run()

        SkillCandidate.reset_id_counter()
        engine2 = self._make_engine(seed=99)
        best2 = engine2.run()

        assert best1.fitness == best2.fitness

    def test_population_size_maintained(self):
        engine = self._make_engine(population_size=6, generations=2)
        engine.run()
        assert len(engine.history) == 2
        for entry in engine.history:
            assert len(entry["population_fitness"]) == 6

    def test_elite_preserved(self):
        engine = self._make_engine(elite_count=2, generations=3)
        engine.run()
        # Best fitness should not decrease between generations
        fitnesses = [h["best_fitness"] for h in engine.history]
        for i in range(1, len(fitnesses)):
            assert fitnesses[i] >= fitnesses[i-1] - 0.001  # tiny tolerance for float

    def test_results_saved(self):
        engine = self._make_engine(generations=2)
        engine.run()
        # Check generation files exist
        assert (self.results_dir / "test-skill" / "gen_000.json").exists()
        assert (self.results_dir / "test-skill" / "gen_001.json").exists()
        # Check final results
        assert (self.results_dir / "test-skill" / "best_skill.md").exists()
        assert (self.results_dir / "test-skill" / "evolution_history.json").exists()

    def test_budget_enforcement(self):
        """Engine should stop early if budget is exhausted."""
        # Use a mock that pretends to cost money
        class ExpensiveEval(MockSkillEvaluator):
            @property
            def cost_per_eval(self):
                return 1.0

        engine = self._make_engine(
            evaluator=ExpensiveEval(baseline=0.5, rng=random.Random(42)),
            budget_cap=2.0,
            population_size=4,
            generations=10,
        )
        best = engine.run()
        # Should have stopped early due to budget
        assert len(engine.history) < 10

    def test_crossover_preserves_sections(self):
        engine = self._make_engine()
        from .evaluator import AGENTS_DIR
        parent1 = SkillCandidate.from_file(AGENTS_DIR / "test-skill.md")
        parent2 = SkillCandidate.from_file(AGENTS_DIR / "test-skill.md")
        parent2.replace_section("Section One", "Modified content for crossover test.")

        child = engine._crossover(parent1, parent2)
        # Child should have same number of sections
        assert len(child.sections) == len(parent1.sections)


if __name__ == "__main__":
    unittest.main()
