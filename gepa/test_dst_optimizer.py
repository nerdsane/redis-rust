"""Unit tests for dst_optimizer.py"""

import random
import tempfile
import unittest
from pathlib import Path

from .dst_candidate import DstCandidate
from .dst_evaluator import MockDstEvaluator
from .dst_optimizer import DstEvolutionEngine


class TestDstEvolutionEngine(unittest.TestCase):
    def setUp(self):
        DstCandidate.reset_id_counter()
        self.tmpdir = tempfile.mkdtemp()
        self.results_dir = Path(self.tmpdir) / "dst_results"

    def _make_engine(self, **kwargs):
        evaluator = MockDstEvaluator(rng=random.Random(42))
        defaults = dict(
            evaluator=evaluator,
            population_size=6,
            elite_count=2,
            mutation_rate=0.3,
            generations=3,
            results_dir=self.results_dir,
            seed=42,
        )
        defaults.update(kwargs)
        return DstEvolutionEngine(**defaults)

    def test_basic_run(self):
        engine = self._make_engine()
        best = engine.run()
        assert best is not None
        assert best.fitness is not None
        assert best.fitness > 0

    def test_deterministic_with_seed(self):
        engine1 = self._make_engine(seed=99)
        best1 = engine1.run()

        DstCandidate.reset_id_counter()
        engine2 = self._make_engine(seed=99)
        best2 = engine2.run()

        assert abs(best1.fitness - best2.fitness) < 1e-6

    def test_population_size(self):
        engine = self._make_engine(population_size=8, generations=2)
        engine.run()
        for entry in engine.history:
            assert len(entry["population_fitness"]) == 8

    def test_elite_preserved(self):
        engine = self._make_engine(elite_count=2, generations=4)
        engine.run()
        fitnesses = [h["best_fitness"] for h in engine.history]
        for i in range(1, len(fitnesses)):
            # Best should never decrease (elitism)
            assert fitnesses[i] >= fitnesses[i-1] - 0.001

    def test_results_saved(self):
        engine = self._make_engine(generations=2)
        engine.run()
        assert (self.results_dir / "gen_000.json").exists()
        assert (self.results_dir / "gen_001.json").exists()
        assert (self.results_dir / "best_config.env").exists()
        assert (self.results_dir / "evolution_history.json").exists()

    def test_best_config_is_valid(self):
        engine = self._make_engine()
        best = engine.run()
        assert best.is_valid()

    def test_crossover_produces_valid(self):
        engine = self._make_engine()
        p1 = DstCandidate.from_preset("calm")
        p2 = DstCandidate.from_preset("chaos")
        p1.fitness = 0.5
        p2.fitness = 0.5
        child = engine._crossover(p1, p2)
        assert child.is_valid()

    def test_mutation_stays_in_bounds(self):
        engine = self._make_engine()
        for _ in range(100):
            c = DstCandidate.from_preset("moderate")
            c = engine._mutate(c)
            assert c.is_valid(), f"Mutation produced invalid: {c.config}"


if __name__ == "__main__":
    unittest.main()
