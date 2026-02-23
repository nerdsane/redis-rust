"""
Skill Evolution Engine - GA loop for evolving skill markdown files.

Mirrors evolve/evolution.py patterns:
population -> evaluate -> elitism -> tournament -> crossover -> mutate -> repeat

Key differences from config evolution:
- Candidate type: SkillCandidate (sections) not numeric config dict
- Crossover: Section-level uniform (50/50 per section from each parent)
- Mutation: Apply random text operator per section with mutation_rate probability
"""

import copy
import json
import logging
import random
from datetime import datetime
from pathlib import Path
from typing import Dict, List, Optional

from .candidate import SkillCandidate
from .mutations import ALL_MUTATIONS
from .skill_fitness import SkillFitnessEvaluator, CostTracker

logger = logging.getLogger(__name__)


class SkillEvolutionEngine:
    """
    Evolutionary optimization engine for skill markdown files.

    Uses tournament selection, section-level crossover, and text mutations
    to evolve skills towards better review quality scores.
    """

    def __init__(
        self,
        skill_name: str,
        evaluator: SkillFitnessEvaluator,
        population_size: int = 8,
        elite_count: int = 2,
        mutation_rate: float = 0.3,
        generations: int = 10,
        tournament_k: int = 3,
        results_dir: Path = Path("gepa/results"),
        budget_cap: float = 5.0,
        seed: int = None,
    ):
        assert population_size >= 4, "population_size must be >= 4"
        assert elite_count < population_size, "elite_count must be < population_size"
        assert 0.0 <= mutation_rate <= 1.0, "mutation_rate must be in [0, 1]"

        self.skill_name = skill_name
        self.evaluator = evaluator
        self.population_size = population_size
        self.elite_count = elite_count
        self.mutation_rate = mutation_rate
        self.generations = generations
        self.tournament_k = tournament_k
        self.results_dir = Path(results_dir) / skill_name
        self.results_dir.mkdir(parents=True, exist_ok=True)
        self.cost_tracker = CostTracker(budget_cap=budget_cap)
        self.rng = random.Random(seed)

        self._best_ever: Optional[SkillCandidate] = None
        self._history: List[Dict] = []

    def run(self) -> SkillCandidate:
        """Run the evolutionary optimization. Returns the best candidate found."""
        logger.info("Starting skill evolution: %d generations, %d population, skill=%s",
                    self.generations, self.population_size, self.skill_name)

        # Load original skill
        from .evaluator import AGENTS_DIR
        original = SkillCandidate.from_file(AGENTS_DIR / f"{self.skill_name}.md")
        population = self._initialize_population(original)

        for gen in range(self.generations):
            logger.info("=== Generation %d/%d ===", gen + 1, self.generations)

            # Budget check
            eval_cost = self.evaluator.cost_per_eval
            needed = sum(1 for c in population if c.fitness is None) * eval_cost
            if eval_cost > 0 and not self.cost_tracker.can_afford(needed):
                logger.warning("Budget exhausted at generation %d. %s",
                             gen + 1, self.cost_tracker.summary())
                break

            # Evaluate unevaluated candidates
            for candidate in population:
                if candidate.fitness is None:
                    candidate.fitness = self.evaluator.evaluate(candidate)
                    candidate.generation = gen
                    if eval_cost > 0:
                        self.cost_tracker.charge(eval_cost)

            # Sort by fitness descending
            population.sort(key=lambda c: c.fitness or 0.0, reverse=True)

            best = population[0]
            avg_fitness = sum(c.fitness or 0 for c in population) / len(population)

            logger.info("Gen %d: best=%.3f avg=%.3f [%s]",
                       gen + 1, best.fitness or 0, avg_fitness,
                       ", ".join(f"{c.fitness:.3f}" for c in population[:4]))

            # Track best ever
            if self._best_ever is None or (best.fitness or 0) > (self._best_ever.fitness or 0):
                self._best_ever = best
                logger.info("NEW BEST: %.3f", best.fitness)

            self._history.append({
                "generation": gen + 1,
                "best_fitness": best.fitness,
                "avg_fitness": avg_fitness,
                "population_fitness": [c.fitness for c in population],
            })

            self._save_generation(gen, population)

            # Create next generation (unless last)
            if gen < self.generations - 1:
                population = self._evolve(population, original)

        # Save final results
        if self._best_ever:
            self._save_final_results()

        logger.info("Evolution complete. Best: %.3f. %s",
                    self._best_ever.fitness if self._best_ever else 0,
                    self.cost_tracker.summary())

        return self._best_ever or original

    def _initialize_population(self, original: SkillCandidate) -> List[SkillCandidate]:
        """Create initial population: original + mutated variants."""
        population = [self._clone_candidate(original)]

        while len(population) < self.population_size:
            mutant = self._clone_candidate(original)
            mutant = self._mutate(mutant)
            mutant.parent_ids = [original._id]
            population.append(mutant)

        return population

    def _evolve(self, population: List[SkillCandidate], original: SkillCandidate) -> List[SkillCandidate]:
        """Create next generation through selection, crossover, and mutation."""
        new_population = []

        # Elitism: keep top candidates
        for candidate in population[:self.elite_count]:
            elite = self._clone_candidate(candidate)
            elite.fitness = candidate.fitness  # Preserve to avoid re-evaluation
            elite.parent_ids = [candidate._id]
            new_population.append(elite)

        # Fill rest with offspring
        while len(new_population) < self.population_size:
            parent1 = self._tournament_select(population)
            parent2 = self._tournament_select(population)
            child = self._crossover(parent1, parent2)
            child = self._mutate(child)
            child.parent_ids = [parent1._id, parent2._id]
            new_population.append(child)

        return new_population

    def _tournament_select(self, population: List[SkillCandidate]) -> SkillCandidate:
        """Select a candidate using tournament selection."""
        k = min(self.tournament_k, len(population))
        tournament = self.rng.sample(population, k)
        return max(tournament, key=lambda c: c.fitness or 0)

    def _crossover(self, parent1: SkillCandidate, parent2: SkillCandidate) -> SkillCandidate:
        """Section-level uniform crossover: 50/50 per section from each parent."""
        child = self._clone_candidate(parent1)

        # Match sections by heading
        p2_sections = {s.heading.lower(): s for s in parent2.sections}
        for i, section in enumerate(child.sections):
            key = section.heading.lower()
            if key in p2_sections and self.rng.random() < 0.5:
                child.sections[i] = copy.deepcopy(p2_sections[key])

        child.fitness = None  # Must re-evaluate
        return child

    def _mutate(self, candidate: SkillCandidate) -> SkillCandidate:
        """Apply random mutation operator per section with mutation_rate probability."""
        for section in candidate.sections:
            if self.rng.random() < self.mutation_rate:
                # Pick random mutation (excluding section_swap for individual sections)
                mutations = [m for m in ALL_MUTATIONS if m.__name__ != "section_swap"]
                mutation = self.rng.choice(mutations)
                section.content = mutation(section.content, self.rng)

        candidate.fitness = None  # Must re-evaluate
        return candidate

    def _clone_candidate(self, original: SkillCandidate) -> SkillCandidate:
        """Deep clone a candidate."""
        clone = copy.deepcopy(original)
        clone._id = SkillCandidate._next_id()
        clone.fitness = None
        clone.parent_ids = []
        return clone

    def _save_generation(self, gen: int, population: List[SkillCandidate]) -> None:
        """Save generation results to JSON."""
        filename = self.results_dir / f"gen_{gen:03d}.json"
        data = {
            "generation": gen + 1,
            "timestamp": datetime.now().isoformat(),
            "population": [c.to_dict() for c in population],
        }
        filename.write_text(json.dumps(data, indent=2))

    def _save_final_results(self) -> None:
        """Save best skill and evolution history."""
        if self._best_ever:
            best_path = self.results_dir / "best_skill.md"
            self._best_ever.save(best_path)
            logger.info("Best skill saved to: %s", best_path)

        history_file = self.results_dir / "evolution_history.json"
        data = {
            "completed_at": datetime.now().isoformat(),
            "skill_name": self.skill_name,
            "generations": self.generations,
            "population_size": self.population_size,
            "mutation_rate": self.mutation_rate,
            "budget": self.cost_tracker.summary(),
            "best_candidate": self._best_ever.to_dict() if self._best_ever else None,
            "history": self._history,
        }
        history_file.write_text(json.dumps(data, indent=2))

    @property
    def best_ever(self) -> Optional[SkillCandidate]:
        return self._best_ever

    @property
    def history(self) -> List[Dict]:
        return self._history
