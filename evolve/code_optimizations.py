"""
Code-level optimization definitions and feature flag management.

This module extends the evolve harness to support code-level optimizations
controlled via Cargo feature flags. Each optimization can be toggled
independently to measure its impact.
"""

from dataclasses import dataclass
from typing import Dict, List, Optional, Tuple
from pathlib import Path
import subprocess
import json
import logging
import re
from datetime import datetime

logger = logging.getLogger(__name__)

@dataclass
class CodeOptimization:
    """Defines a code-level optimization."""
    name: str                   # Feature flag name (e.g., "opt-single-key-alloc")
    description: str            # What the optimization does
    expected_gain: float        # Expected performance gain (0.0 - 1.0)
    risk: str                   # Risk level: "low", "medium", "high"
    files_modified: List[str]   # Files that need changes
    priority: int               # Implementation priority (0 = highest)

    def feature_flag(self) -> str:
        """Return the Cargo feature flag name."""
        return self.name


# Registry of all code-level optimizations
CODE_OPTIMIZATIONS: Dict[str, CodeOptimization] = {
    "opt-single-key-alloc": CodeOptimization(
        name="opt-single-key-alloc",
        description="Single allocation in set_direct (eliminate double key.to_string())",
        expected_gain=0.04,  # 3-5% expected
        risk="low",
        files_modified=["src/redis/commands.rs"],
        priority=0,
    ),
    "opt-static-responses": CodeOptimization(
        name="opt-static-responses",
        description="Static OK/PONG responses instead of allocating each time",
        expected_gain=0.015,  # 1-2% expected
        risk="low",
        files_modified=["src/redis/commands.rs"],
        priority=1,
    ),
    "opt-zero-copy-get": CodeOptimization(
        name="opt-zero-copy-get",
        description="Zero-copy GET response using Bytes/Arc instead of Vec clone",
        expected_gain=0.025,  # 2-3% expected
        risk="medium",
        files_modified=["src/redis/commands.rs", "src/redis/data.rs"],
        priority=2,
    ),
    "opt-itoa-encode": CodeOptimization(
        name="opt-itoa-encode",
        description="Use itoa crate for fast integer encoding in RESP responses",
        expected_gain=0.015,  # 1-2% expected
        risk="low",
        files_modified=["src/production/connection_optimized.rs"],
        priority=3,
    ),
    "opt-fxhash-routing": CodeOptimization(
        name="opt-fxhash-routing",
        description="Use FxHash/AHash for faster shard routing",
        expected_gain=0.015,  # 1-2% expected
        risk="low",
        files_modified=["src/production/sharded_actor.rs"],
        priority=4,
    ),
    "opt-atoi-parse": CodeOptimization(
        name="opt-atoi-parse",
        description="Use atoi crate for fast integer parsing from bytes",
        expected_gain=0.03,  # 2-4% expected
        risk="low",
        files_modified=["src/production/connection_optimized.rs"],
        priority=5,
    ),
}


@dataclass
class OptimizationCandidate:
    """A candidate configuration of enabled optimizations."""
    enabled: Dict[str, bool]   # Map of optimization name -> enabled
    fitness: Optional[float] = None
    generation: int = 0

    def feature_flags(self) -> List[str]:
        """Return list of enabled feature flags."""
        return [name for name, enabled in self.enabled.items() if enabled]

    def cargo_features(self) -> str:
        """Return comma-separated feature flags for cargo."""
        flags = self.feature_flags()
        if flags:
            return ",".join(flags)
        return ""

    def expected_total_gain(self) -> float:
        """Calculate expected total gain from enabled optimizations."""
        total = 0.0
        for name, enabled in self.enabled.items():
            if enabled and name in CODE_OPTIMIZATIONS:
                total += CODE_OPTIMIZATIONS[name].expected_gain
        return total

    def summary(self) -> str:
        """Short summary for logging."""
        enabled = [n.replace("opt-", "") for n in self.feature_flags()]
        return f"[{', '.join(enabled) or 'baseline'}]"

    def to_dict(self) -> Dict:
        """Serialize to dict."""
        return {
            "enabled": self.enabled,
            "fitness": self.fitness,
            "generation": self.generation,
            "expected_gain": self.expected_total_gain(),
        }

    @classmethod
    def from_dict(cls, data: Dict) -> "OptimizationCandidate":
        """Deserialize from dict."""
        return cls(
            enabled=data["enabled"],
            fitness=data.get("fitness"),
            generation=data.get("generation", 0),
        )

    @classmethod
    def baseline(cls) -> "OptimizationCandidate":
        """Create baseline with all optimizations disabled."""
        return cls(enabled={name: False for name in CODE_OPTIMIZATIONS})

    @classmethod
    def all_enabled(cls) -> "OptimizationCandidate":
        """Create candidate with all optimizations enabled."""
        return cls(enabled={name: True for name in CODE_OPTIMIZATIONS})


class CriterionEvaluator:
    """
    Fast evaluator using Criterion benchmarks.

    Provides sub-second feedback for code optimizations.
    """

    def __init__(self, project_root: Path):
        self.project_root = project_root
        self._baseline_results: Optional[Dict[str, float]] = None

    def capture_baseline(self) -> Dict[str, float]:
        """Run benchmarks with no optimizations to establish baseline."""
        logger.info("Capturing Criterion baseline...")
        results = self._run_criterion_bench([])
        self._baseline_results = results
        return results

    def evaluate(self, candidate: OptimizationCandidate) -> Dict[str, float]:
        """
        Evaluate candidate using Criterion benchmarks.

        Returns dict of benchmark -> relative performance (1.0 = baseline).
        """
        features = candidate.feature_flags()
        results = self._run_criterion_bench(features)

        if self._baseline_results is None:
            self.capture_baseline()

        # Calculate relative performance
        relative = {}
        for bench, time_ns in results.items():
            if bench in self._baseline_results:
                baseline = self._baseline_results[bench]
                # Lower time is better, so invert the ratio
                relative[bench] = baseline / time_ns if time_ns > 0 else 1.0
            else:
                relative[bench] = 1.0

        return relative

    def _run_criterion_bench(self, features: List[str]) -> Dict[str, float]:
        """Run Criterion benchmarks and parse results."""
        cmd = ["cargo", "bench", "--bench", "hot_paths"]

        if features:
            cmd.extend(["--features", ",".join(features)])

        # Run with JSON output for parsing
        cmd.extend(["--", "--noplot"])

        logger.debug(f"Running: {' '.join(cmd)}")

        result = subprocess.run(
            cmd,
            cwd=self.project_root,
            capture_output=True,
            text=True,
            timeout=300,
        )

        if result.returncode != 0:
            logger.error(f"Benchmark failed: {result.stderr}")
            return {}

        # Parse Criterion output
        return self._parse_criterion_output(result.stdout)

    def _parse_criterion_output(self, output: str) -> Dict[str, float]:
        """Parse Criterion benchmark output to extract timings."""
        results = {}

        # Pattern: "bench_name       time:   [123.45 ns 124.56 ns 125.67 ns]"
        pattern = r"(\w+/\w+)\s+time:\s+\[\d+\.\d+ \w+ (\d+\.\d+) (ns|µs|ms)"

        for match in re.finditer(pattern, output):
            bench_name = match.group(1)
            time_val = float(match.group(2))
            unit = match.group(3)

            # Normalize to nanoseconds
            if unit == "µs":
                time_val *= 1000
            elif unit == "ms":
                time_val *= 1_000_000

            results[bench_name] = time_val

        return results


class CodeOptimizationEvolver:
    """
    Evolution engine for code-level optimizations.

    Unlike config tuning which uses continuous parameters, this uses
    binary feature flags that are enabled/disabled.
    """

    def __init__(
        self,
        project_root: Path,
        results_dir: Path = Path("evolve/results/code_opt"),
        use_criterion: bool = True,
        use_docker: bool = False,
    ):
        self.project_root = project_root
        self.results_dir = results_dir
        self.results_dir.mkdir(parents=True, exist_ok=True)

        self.use_criterion = use_criterion
        self.use_docker = use_docker

        if use_criterion:
            self.criterion_eval = CriterionEvaluator(project_root)

        self._history: List[Dict] = []
        self._best: Optional[OptimizationCandidate] = None

    def run_incremental(self) -> OptimizationCandidate:
        """
        Run incremental optimization: add one optimization at a time.

        This is safer than genetic evolution for code changes since
        we want to understand the impact of each individual change.
        """
        logger.info("Starting incremental code optimization evolution")

        # Sort optimizations by priority
        opts = sorted(
            CODE_OPTIMIZATIONS.values(),
            key=lambda o: o.priority
        )

        # Start with baseline
        current = OptimizationCandidate.baseline()

        if self.use_criterion:
            baseline_results = self.criterion_eval.capture_baseline()
            logger.info(f"Baseline captured: {len(baseline_results)} benchmarks")

        current.fitness = 100.0  # Baseline = 100%
        self._best = current

        for opt in opts:
            logger.info(f"\n{'='*50}")
            logger.info(f"Testing optimization: {opt.name}")
            logger.info(f"  Description: {opt.description}")
            logger.info(f"  Expected gain: {opt.expected_gain*100:.1f}%")
            logger.info(f"{'='*50}")

            # Create candidate with this optimization enabled
            test = OptimizationCandidate(
                enabled={**current.enabled, opt.name: True},
                generation=opt.priority + 1,
            )

            # Evaluate
            try:
                if self.use_criterion:
                    relative = self.criterion_eval.evaluate(test)
                    # Average relative performance across hot path benchmarks
                    hot_paths = ["set_direct", "get_direct"]
                    relevant = [v for k, v in relative.items()
                               if any(hp in k for hp in hot_paths)]
                    avg_improvement = sum(relevant) / len(relevant) if relevant else 1.0
                    test.fitness = current.fitness * avg_improvement
                else:
                    # Mock fitness based on expected gain
                    test.fitness = current.fitness * (1 + opt.expected_gain)

                logger.info(f"  Result: {test.fitness:.1f}% (was {current.fitness:.1f}%)")

                # Accept if improvement
                if test.fitness > current.fitness:
                    improvement = test.fitness - current.fitness
                    logger.info(f"  ACCEPTED: +{improvement:.2f}%")
                    current = test
                    self._best = current
                else:
                    logger.info(f"  REJECTED: no improvement")

                # Record history
                self._history.append({
                    "optimization": opt.name,
                    "tested_fitness": test.fitness,
                    "accepted": test.fitness > current.fitness,
                    "current_best": current.fitness,
                })

            except Exception as e:
                logger.error(f"  FAILED: {e}")
                self._history.append({
                    "optimization": opt.name,
                    "error": str(e),
                })

        # Save results
        self._save_results()

        return self._best

    def run_combinatorial(self, max_combinations: int = 64) -> OptimizationCandidate:
        """
        Test combinations of optimizations.

        For N optimizations, there are 2^N combinations. This tests
        the most promising combinations based on expected gains.
        """
        logger.info("Starting combinatorial code optimization search")

        # Generate all combinations, sorted by expected total gain
        from itertools import combinations

        all_opts = list(CODE_OPTIMIZATIONS.keys())
        candidates = []

        # Generate power set (all combinations)
        for r in range(len(all_opts) + 1):
            for combo in combinations(all_opts, r):
                enabled = {name: name in combo for name in all_opts}
                cand = OptimizationCandidate(enabled=enabled)
                candidates.append(cand)

        # Sort by expected gain (descending)
        candidates.sort(key=lambda c: c.expected_total_gain(), reverse=True)

        # Limit to max_combinations
        candidates = candidates[:max_combinations]

        logger.info(f"Testing {len(candidates)} combinations")

        best = None
        for i, cand in enumerate(candidates):
            logger.info(f"\n[{i+1}/{len(candidates)}] Testing: {cand.summary()}")
            logger.info(f"  Expected gain: {cand.expected_total_gain()*100:.1f}%")

            try:
                if self.use_criterion:
                    relative = self.criterion_eval.evaluate(cand)
                    # Calculate fitness from relevant benchmarks
                    hot_paths = ["set_direct", "get_direct"]
                    relevant = [v for k, v in relative.items()
                               if any(hp in k for hp in hot_paths)]
                    cand.fitness = 100.0 * (sum(relevant) / len(relevant) if relevant else 1.0)
                else:
                    cand.fitness = 100.0 * (1 + cand.expected_total_gain())

                logger.info(f"  Fitness: {cand.fitness:.1f}%")

                if best is None or cand.fitness > best.fitness:
                    best = cand
                    logger.info(f"  NEW BEST!")

                self._history.append({
                    "combination": cand.summary(),
                    "fitness": cand.fitness,
                    "enabled": cand.enabled,
                })

            except Exception as e:
                logger.error(f"  FAILED: {e}")

        self._best = best
        self._save_results()

        return best

    def _save_results(self) -> None:
        """Save evolution results."""
        timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")

        results = {
            "completed_at": datetime.now().isoformat(),
            "best_candidate": self._best.to_dict() if self._best else None,
            "best_features": self._best.feature_flags() if self._best else [],
            "history": self._history,
        }

        output_file = self.results_dir / f"code_opt_{timestamp}.json"
        output_file.write_text(json.dumps(results, indent=2))
        logger.info(f"Results saved to: {output_file}")

        # Also save best features to a shell script for easy use
        if self._best:
            script_file = self.results_dir / "best_features.sh"
            features = self._best.cargo_features()
            script_content = f"""#!/bin/bash
# Best code optimization features discovered by evolution
# Fitness: {self._best.fitness:.1f}%
# Generated: {datetime.now().isoformat()}

FEATURES="{features}"

# Build with optimizations:
# cargo build --release --features "$FEATURES"

# Run benchmarks with optimizations:
# cargo bench --bench hot_paths --features "$FEATURES"

echo "Best features: $FEATURES"
"""
            script_file.write_text(script_content)
            script_file.chmod(0o755)


def main():
    """CLI entry point for code optimization evolution."""
    import argparse

    parser = argparse.ArgumentParser(
        description="Code-level optimization evolution harness"
    )
    parser.add_argument(
        "--mode", choices=["incremental", "combinatorial"],
        default="incremental",
        help="Evolution mode: incremental (one at a time) or combinatorial"
    )
    parser.add_argument(
        "--mock", action="store_true",
        help="Use mock evaluator (no actual benchmarks)"
    )
    parser.add_argument(
        "--output", "-o", type=Path,
        default=Path("evolve/results/code_opt"),
        help="Output directory"
    )
    parser.add_argument("--verbose", "-v", action="store_true")

    args = parser.parse_args()

    logging.basicConfig(
        level=logging.DEBUG if args.verbose else logging.INFO,
        format="%(asctime)s | %(levelname)-8s | %(message)s",
    )

    project_root = Path(__file__).parent.parent

    evolver = CodeOptimizationEvolver(
        project_root=project_root,
        results_dir=args.output,
        use_criterion=not args.mock,
    )

    print()
    print("=" * 60)
    print("  Code-Level Optimization Evolution")
    print("=" * 60)
    print()
    print("Available optimizations:")
    for name, opt in sorted(CODE_OPTIMIZATIONS.items(), key=lambda x: x[1].priority):
        print(f"  P{opt.priority}: {name}")
        print(f"      {opt.description}")
        print(f"      Expected: +{opt.expected_gain*100:.1f}%, Risk: {opt.risk}")
    print()

    if args.mode == "incremental":
        best = evolver.run_incremental()
    else:
        best = evolver.run_combinatorial()

    print()
    print("=" * 60)
    print("  BEST CONFIGURATION")
    print("=" * 60)
    print(f"  Fitness: {best.fitness:.1f}%")
    print(f"  Features: {best.cargo_features() or '(baseline)'}")
    print()
    print("  Build command:")
    if best.cargo_features():
        print(f"    cargo build --release --features \"{best.cargo_features()}\"")
    else:
        print(f"    cargo build --release")
    print()


if __name__ == "__main__":
    main()
