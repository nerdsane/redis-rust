"""
GEPA Harness - CLI entry point for skill evaluation and evolution.

Usage:
    python -m gepa.harness --score-baseline          # Score all skills with cached reviews
    python -m gepa.harness --skill rust-dev --offline # Score one skill offline
    python -m gepa.harness --capture-reviews          # Record baseline reviews via Claude CLI
    python -m gepa.harness --list-tasks               # Show available ground truth tasks
    python -m gepa.harness --detail rust-dev paper_review  # Detailed score for one skill+task

    # Phase 2: Evolution
    python -m gepa.harness --evolve rust-dev --mock           # Test GA with mock evaluator
    python -m gepa.harness --evolve rust-dev --live            # Real evolution via Claude CLI
    python -m gepa.harness --evolve rust-dev --live --budget 10 --generations 15

    # Phase 2: DST Config Optimization
    python -m gepa.harness --dst-optimize --mock               # Test DST GA with mock evaluator
    python -m gepa.harness --dst-optimize --test executor_dst_test --seeds 10
"""

import argparse
import json
import logging
import sys
from pathlib import Path

from .evaluator import OfflineEvaluator, LiveEvaluator, GROUND_TRUTH_DIR, REVIEWS_DIR, AGENTS_DIR

logger = logging.getLogger(__name__)

SKILL_NAMES = [
    "actor-model",
    "distributed-systems",
    "dst",
    "formal-verification",
    "rust-dev",
    "tigerstyle",
]


def score_baseline(evaluator: OfflineEvaluator) -> None:
    """Score all skills against all available ground truth tasks."""
    results = evaluator.evaluate_all_skills()

    if not results:
        print("No cached reviews found. Run --capture-reviews first.")
        print(f"  Expected reviews in: {evaluator.reviews_dir}/")
        print(f"  Structure: reviews/{{skill_name}}/{{task_name}}.txt")
        return

    print("=" * 60)
    print("GEPA Baseline Scores")
    print("=" * 60)

    # Sort by aggregate score descending
    ranked = sorted(results.items(), key=lambda x: x[1].aggregate_score, reverse=True)
    for rank, (name, result) in enumerate(ranked, 1):
        print(f"\n{rank}. {result.summary()}")

    # Leaderboard summary
    print("\n" + "=" * 60)
    print("Leaderboard")
    print("-" * 60)
    print(f"{'Rank':<6}{'Skill':<25}{'Score':<10}{'Tasks':<6}")
    print("-" * 60)
    for rank, (name, result) in enumerate(ranked, 1):
        n_tasks = len(result.task_scores)
        print(f"{rank:<6}{name:<25}{result.aggregate_score:<10.3f}{n_tasks:<6}")


def score_skill(evaluator: OfflineEvaluator, skill_name: str, tasks=None) -> None:
    """Score a single skill."""
    result = evaluator.evaluate_skill(skill_name, tasks=tasks)
    if not result.task_scores:
        print(f"No cached reviews for skill '{skill_name}'.")
        print(f"  Expected in: {evaluator.reviews_dir}/{skill_name}/")
        return
    print(result.summary())


def show_detail(evaluator: OfflineEvaluator, skill_name: str, task_name: str) -> None:
    """Show detailed scoring for one skill + task combination."""
    review_text = evaluator.get_review_text(skill_name, task_name)
    if review_text is None:
        print(f"No cached review for {skill_name}/{task_name}")
        return

    ground_truth = evaluator.load_ground_truth(task_name)
    result = evaluator.scorer.score(review_text, ground_truth, skill_name=skill_name)

    print(f"=== {skill_name} on {task_name} ===\n")
    print(result.summary())

    print("\nFinding Details:")
    print("-" * 60)
    for match in result.finding_matches:
        status = "HIT" if match.matched else "MISS"
        print(
            f"  [{status}] {match.finding_id}: "
            f"{match.keyword_hits}/{match.keyword_total} keywords "
            f"(confidence={match.confidence:.2f}, weight={match.weight:.1f})"
        )

    if result.false_positive_matches:
        print("\nFalse Positive Checks:")
        for fp in result.false_positive_matches:
            status = "TRIGGERED" if fp.triggered else "ok"
            penalty_str = f" (penalty={fp.penalty:.1f})" if fp.triggered else ""
            print(f"  [{status}] {fp.fp_id}{penalty_str}")


def capture_reviews(live_evaluator: LiveEvaluator) -> None:
    """Capture baseline reviews for all skills by running Claude CLI."""
    tasks = sorted(
        p.stem for p in GROUND_TRUTH_DIR.glob("*.json")
    )

    print(f"Capturing reviews for {len(SKILL_NAMES)} skills x {len(tasks)} tasks")
    print(f"Estimated cost: ~${len(SKILL_NAMES) * len(tasks) * 0.05:.2f}")
    print()

    for skill_name in SKILL_NAMES:
        for task_name in tasks:
            print(f"  {skill_name}/{task_name}...", end=" ", flush=True)
            result = live_evaluator.evaluate_skill_live(
                skill_name, task_name, save_review=True
            )
            if result:
                print(f"score={result.composite:.3f}")
            else:
                print("FAILED")


def list_tasks() -> None:
    """List available ground truth tasks with details."""
    tasks = sorted(GROUND_TRUTH_DIR.glob("*.json"))
    if not tasks:
        print("No ground truth files found.")
        return

    print("Available ground truth tasks:")
    print("-" * 60)
    for path in tasks:
        data = json.loads(path.read_text())
        n_findings = len(data.get("expected_findings", []))
        n_fp = len(data.get("known_false_positives", []))
        skills = ", ".join(data.get("skills_under_test", []))
        print(f"  {path.stem}")
        print(f"    {data.get('description', 'No description')}")
        print(f"    Findings: {n_findings}, False positives: {n_fp}")
        print(f"    Skills: {skills}")
        print()


def run_evolution(args) -> None:
    """Run skill evolution with GA."""
    from .evolution import SkillEvolutionEngine
    from .skill_fitness import MockSkillEvaluator, LiveSkillEvaluator, CostTracker

    skill_name = args.evolve

    if skill_name not in SKILL_NAMES:
        print(f"Unknown skill: {skill_name}")
        print(f"Available: {', '.join(SKILL_NAMES)}")
        return

    if args.mock:
        import random
        evaluator = MockSkillEvaluator(
            baseline=0.5, noise_std=0.05,
            rng=random.Random(42),
        )
        print(f"Running mock evolution for '{skill_name}'...")
    elif args.live:
        gt_dir = args.ground_truth_dir or GROUND_TRUTH_DIR
        evaluator = LiveSkillEvaluator(
            ground_truth_dir=gt_dir,
            model=args.model,
            cost_tracker=CostTracker(budget_cap=args.budget),
        )
        print(f"Running live evolution for '{skill_name}' (budget=${args.budget:.2f})...")
    else:
        print("Specify --mock or --live for evolution mode.")
        return

    engine = SkillEvolutionEngine(
        skill_name=skill_name,
        evaluator=evaluator,
        population_size=args.population,
        generations=args.generations,
        budget_cap=args.budget,
        seed=args.seed,
    )
    best = engine.run()

    print(f"\nBest candidate: {best}")
    print(f"Results saved to: {engine.results_dir}/")


def run_dst_optimize(args) -> None:
    """Run DST config optimization with GA."""
    from .dst_optimizer import DstEvolutionEngine
    from .dst_evaluator import MockDstEvaluator, CargoDstEvaluator

    if args.mock:
        import random
        evaluator = MockDstEvaluator(rng=random.Random(42))
        print("Running mock DST optimization...")
    else:
        evaluator = CargoDstEvaluator(
            test_targets=[args.test] if args.test else ["executor_dst_test"],
            seeds=range(0, args.seeds),
            timeout=args.timeout,
        )
        print(f"Running DST optimization (test={args.test or 'executor_dst_test'}, "
              f"seeds={args.seeds})...")

    engine = DstEvolutionEngine(
        evaluator=evaluator,
        population_size=args.population,
        generations=args.generations,
        seed=args.seed,
    )
    best = engine.run()

    print(f"\nBest candidate: {best}")
    print(f"Best config: {best.to_env_string()[:100]}...")
    print(f"Results saved to: {engine.results_dir}/")


def main() -> None:
    parser = argparse.ArgumentParser(
        description="GEPA - Genetic Evolution for Prompt Artifacts",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
  # Phase 1: Scoring
  python -m gepa.harness --score-baseline
  python -m gepa.harness --skill rust-dev --offline
  python -m gepa.harness --detail rust-dev paper_review
  python -m gepa.harness --capture-reviews --model sonnet
  python -m gepa.harness --list-tasks

  # Phase 2: Skill Evolution
  python -m gepa.harness --evolve rust-dev --mock
  python -m gepa.harness --evolve rust-dev --live --budget 10.0 --generations 15

  # Phase 2: DST Config Optimization
  python -m gepa.harness --dst-optimize --mock
  python -m gepa.harness --dst-optimize --test executor_dst_test --seeds 10
""",
    )

    # Phase 1: Scoring
    parser.add_argument(
        "--score-baseline", action="store_true",
        help="Score all skills with cached reviews",
    )
    parser.add_argument(
        "--skill", type=str,
        help="Evaluate a specific skill",
    )
    parser.add_argument(
        "--offline", action="store_true",
        help="Use offline scoring (cached reviews only)",
    )
    parser.add_argument(
        "--detail", nargs=2, metavar=("SKILL", "TASK"),
        help="Show detailed score for skill+task",
    )
    parser.add_argument(
        "--capture-reviews", action="store_true",
        help="Capture baseline reviews via Claude CLI",
    )
    parser.add_argument(
        "--list-tasks", action="store_true",
        help="List available ground truth tasks",
    )
    parser.add_argument(
        "--model", type=str, default="sonnet",
        help="Model for live evaluation (default: sonnet)",
    )
    parser.add_argument(
        "--ground-truth-dir", type=Path, default=None,
        help="Override ground truth directory",
    )
    parser.add_argument(
        "--reviews-dir", type=Path, default=None,
        help="Override reviews directory",
    )

    # Phase 2: Skill Evolution
    parser.add_argument(
        "--evolve", type=str, metavar="SKILL",
        help="Evolve a skill using GA (e.g., --evolve rust-dev)",
    )
    parser.add_argument(
        "--mock", action="store_true",
        help="Use mock evaluator (free, for testing GA machinery)",
    )
    parser.add_argument(
        "--live", action="store_true",
        help="Use live evaluator (calls Claude CLI, costs ~$0.05/task)",
    )
    parser.add_argument(
        "--budget", type=float, default=5.0,
        help="Budget cap in USD for live evolution (default: 5.0)",
    )
    parser.add_argument(
        "--generations", type=int, default=10,
        help="Number of GA generations (default: 10)",
    )
    parser.add_argument(
        "--population", type=int, default=8,
        help="Population size per generation (default: 8)",
    )
    parser.add_argument(
        "--seed", type=int, default=None,
        help="Random seed for reproducibility",
    )

    # Phase 2: DST Config Optimization
    parser.add_argument(
        "--dst-optimize", action="store_true",
        help="Optimize DST buggify config using GA",
    )
    parser.add_argument(
        "--test", type=str, default=None,
        help="Cargo test target for DST optimization (default: executor_dst_test)",
    )
    parser.add_argument(
        "--seeds", type=int, default=10,
        help="Number of DST seeds to run per evaluation (default: 10)",
    )
    parser.add_argument(
        "--timeout", type=int, default=300,
        help="Timeout in seconds per cargo test run (default: 300)",
    )

    parser.add_argument(
        "-v", "--verbose", action="store_true",
        help="Enable debug logging",
    )

    args = parser.parse_args()

    logging.basicConfig(
        level=logging.DEBUG if args.verbose else logging.INFO,
        format="%(levelname)s: %(message)s",
    )

    gt_dir = args.ground_truth_dir or GROUND_TRUTH_DIR
    rev_dir = args.reviews_dir or REVIEWS_DIR

    if args.list_tasks:
        list_tasks()
        return

    # Phase 2: Evolution
    if args.evolve:
        run_evolution(args)
        return

    if args.dst_optimize:
        run_dst_optimize(args)
        return

    # Phase 1: Scoring
    offline_eval = OfflineEvaluator(ground_truth_dir=gt_dir, reviews_dir=rev_dir)

    if args.score_baseline:
        score_baseline(offline_eval)
    elif args.detail:
        show_detail(offline_eval, args.detail[0], args.detail[1])
    elif args.skill and args.offline:
        score_skill(offline_eval, args.skill)
    elif args.capture_reviews:
        live_eval = LiveEvaluator(
            ground_truth_dir=gt_dir,
            reviews_dir=rev_dir,
            model=args.model,
        )
        capture_reviews(live_eval)
    elif args.skill:
        # Default to offline for a single skill
        score_skill(offline_eval, args.skill)
    else:
        parser.print_help()


if __name__ == "__main__":
    main()
