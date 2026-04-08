"""
DST Evolution Engine - GA loop for evolving buggify fault configurations.

Identical pattern to evolve/evolution.py but with float parameters:
- Crossover: Uniform per parameter (50/50)
- Mutation: current += choice([-1, 0, 1]) * step, clamped to bounds
- Initialization: calm + moderate + chaos + random variants
"""

import copy
import json
import logging
import random
from datetime import datetime
from pathlib import Path
from typing import Dict, List, Optional

from .dst_candidate import DstCandidate, DST_PARAM_BOUNDS
from .dst_evaluator import DstFitnessEvaluator

logger = logging.getLogger(__name__)


class DstEvolutionEngine:
    """
    Evolutionary optimization engine for buggify fault configurations.

    Evolves fault probabilities to find configs that surface the most
    invariant violations in DST tests.
    """

    def __init__(
        self,
        evaluator: DstFitnessEvaluator,
        population_size: int = 8,
        elite_count: int = 2,
        mutation_rate: float = 0.3,
        generations: int = 10,
        tournament_k: int = 3,
        results_dir: Path = Path("gepa/dst_results"),
        seed: int = None,
    ):
        assert population_size >= 4, "population_size must be >= 4"
        assert elite_count < population_size, "elite_count must be < population_size"
        assert 0.0 <= mutation_rate <= 1.0

        self.evaluator = evaluator
        self.population_size = population_size
        self.elite_count = elite_count
        self.mutation_rate = mutation_rate
        self.generations = generations
        self.tournament_k = tournament_k
        self.results_dir = Path(results_dir)
        self.results_dir.mkdir(parents=True, exist_ok=True)
        self.rng = random.Random(seed)

        self._best_ever: Optional[DstCandidate] = None
        self._history: List[Dict] = []

    def run(self) -> DstCandidate:
        """Run the evolutionary optimization. Returns the best candidate."""
        logger.info("Starting DST optimization: %d generations, %d population",
                    self.generations, self.population_size)

        population = self._initialize_population()

        for gen in range(self.generations):
            logger.info("=== Generation %d/%d ===", gen + 1, self.generations)

            # Evaluate
            for candidate in population:
                if candidate.fitness is None:
                    candidate.fitness = self.evaluator.evaluate(candidate)
                    candidate.generation = gen

            # Sort by fitness descending
            population.sort(key=lambda c: c.fitness or 0.0, reverse=True)

            best = population[0]
            avg_fitness = sum(c.fitness or 0 for c in population) / len(population)

            logger.info("Gen %d: best=%.4f avg=%.4f gm=%.2f",
                       gen + 1, best.fitness or 0, avg_fitness,
                       best.config.get("global_multiplier", 1.0))

            if self._best_ever is None or (best.fitness or 0) > (self._best_ever.fitness or 0):
                self._best_ever = best
                logger.info("NEW BEST: %.4f", best.fitness)

            self._history.append({
                "generation": gen + 1,
                "best_fitness": best.fitness,
                "avg_fitness": avg_fitness,
                "best_global_multiplier": best.config.get("global_multiplier", 1.0),
                "population_fitness": [c.fitness for c in population],
            })

            self._save_generation(gen, population)

            # Create next generation (unless last)
            if gen < self.generations - 1:
                population = self._evolve(population)

        if self._best_ever:
            self._save_final_results()

        logger.info("DST optimization complete. Best: %.4f",
                    self._best_ever.fitness if self._best_ever else 0)

        return self._best_ever or DstCandidate.from_preset("moderate")

    def _initialize_population(self) -> List[DstCandidate]:
        """Create initial population: presets + random variants."""
        population = []

        # Include all 3 presets
        for preset in ["calm", "moderate", "chaos"]:
            population.append(DstCandidate.from_preset(preset))

        # Fill with random variants
        while len(population) < self.population_size:
            candidate = self._random_candidate()
            if candidate.is_valid():
                population.append(candidate)

        return population[:self.population_size]

    def _random_candidate(self) -> DstCandidate:
        """Generate a random valid configuration."""
        config = {}
        for key, bounds in DST_PARAM_BOUNDS.items():
            steps = int((bounds.max - bounds.min) / bounds.step)
            step_idx = self.rng.randint(0, max(1, steps))
            config[key] = bounds.clamp(bounds.min + step_idx * bounds.step)
        return DstCandidate(config=config)

    def _evolve(self, population: List[DstCandidate]) -> List[DstCandidate]:
        """Create next generation."""
        new_population = []

        # Elitism
        for candidate in population[:self.elite_count]:
            elite = copy.deepcopy(candidate)
            elite._id = DstCandidate._next_id()
            elite.parent_ids = [candidate._id]
            new_population.append(elite)

        # Offspring
        while len(new_population) < self.population_size:
            parent1 = self._tournament_select(population)
            parent2 = self._tournament_select(population)
            child = self._crossover(parent1, parent2)
            child = self._mutate(child)
            child.parent_ids = [parent1._id, parent2._id]
            if child.is_valid():
                new_population.append(child)

        return new_population

    def _tournament_select(self, population: List[DstCandidate]) -> DstCandidate:
        k = min(self.tournament_k, len(population))
        tournament = self.rng.sample(population, k)
        return max(tournament, key=lambda c: c.fitness or 0)

    def _crossover(self, parent1: DstCandidate, parent2: DstCandidate) -> DstCandidate:
        """Uniform crossover per parameter."""
        config = {}
        for key in DST_PARAM_BOUNDS:
            if self.rng.random() < 0.5:
                config[key] = parent1.config.get(key, DST_PARAM_BOUNDS[key].default)
            else:
                config[key] = parent2.config.get(key, DST_PARAM_BOUNDS[key].default)
        child = DstCandidate(config=config)
        child.fitness = None
        return child

    def _mutate(self, candidate: DstCandidate) -> DstCandidate:
        """Mutate: current += choice([-1, 0, 1]) * step, clamped."""
        for key, bounds in DST_PARAM_BOUNDS.items():
            if self.rng.random() < self.mutation_rate:
                current = candidate.config.get(key, bounds.default)
                delta = self.rng.choice([-1, 0, 1]) * bounds.step
                candidate.config[key] = bounds.clamp(current + delta)
        candidate.fitness = None
        return candidate

    def _save_generation(self, gen: int, population: List[DstCandidate]) -> None:
        filename = self.results_dir / f"gen_{gen:03d}.json"
        data = {
            "generation": gen + 1,
            "timestamp": datetime.now().isoformat(),
            "population": [c.to_dict() for c in population],
        }
        filename.write_text(json.dumps(data, indent=2))

    def _save_final_results(self) -> None:
        if self._best_ever:
            # Save as env string
            env_path = self.results_dir / "best_config.env"
            env_path.write_text(self._best_ever.to_env_string())
            logger.info("Best config saved to: %s", env_path)

        history_file = self.results_dir / "evolution_history.json"
        data = {
            "completed_at": datetime.now().isoformat(),
            "generations": self.generations,
            "population_size": self.population_size,
            "mutation_rate": self.mutation_rate,
            "best_candidate": self._best_ever.to_dict() if self._best_ever else None,
            "history": self._history,
        }
        history_file.write_text(json.dumps(data, indent=2))

    @property
    def best_ever(self) -> Optional[DstCandidate]:
        return self._best_ever

    @property
    def history(self) -> List[Dict]:
        return self._history
