# GEPA - Genetic Evolution for Prompt Artifacts

Skill evaluation harness and evolutionary optimizer for Claude Code agent skills (`.claude/agents/*.md`) and DST fault configurations (`src/buggify/config.rs`).

## Quick Start

```bash
# Phase 1: Scoring
python -m gepa.harness --list-tasks                     # Available ground truth tasks
python -m gepa.harness --score-baseline                  # Score all skills
python -m gepa.harness --skill rust-dev --offline        # Score one skill
python -m gepa.harness --detail rust-dev paper_review    # Detailed breakdown
python -m gepa.harness --capture-reviews                 # Capture via Claude CLI

# Phase 2: Skill Evolution
python -m gepa.harness --evolve rust-dev --mock          # Test GA machinery (free)
python -m gepa.harness --evolve rust-dev --live           # Real evolution (~$0.05/eval)
python -m gepa.harness --evolve rust-dev --live --budget 10.0 --generations 15

# Phase 2: DST Config Optimization
python -m gepa.harness --dst-optimize --mock             # Test DST GA (free)
python -m gepa.harness --dst-optimize --test executor_dst_test --seeds 10
python -m gepa.harness --dst-optimize --generations 15 --population 12
```

## Architecture

```
gepa/
    __init__.py             # Package exports
    __main__.py             # python -m gepa entry point
    harness.py              # CLI: argparse + orchestration

    # Phase 1: Scoring
    scorer.py               # Keyword matching + composite scoring
    candidate.py            # SkillCandidate: markdown parsing by section
    evaluator.py            # Offline + live evaluation modes

    # Phase 2: Skill Evolution
    mutations.py            # 5 text mutation operators
    skill_fitness.py        # Mock, Offline, Live fitness evaluators + CostTracker
    evolution.py            # SkillEvolutionEngine: GA loop for skill markdown

    # Phase 2: DST Config Optimization
    dst_candidate.py        # DstCandidate: 32 tunable fault parameters
    dst_evaluator.py        # Mock + cargo test fitness evaluators
    dst_optimizer.py        # DstEvolutionEngine: GA loop for fault configs

    # Data
    ground_truth/
        schema.md           # Ground truth format documentation
        paper_review.json   # Expert paper review findings
        pr13_review.json    # PR #13 review findings (ACL DRYRUN/LOG)
        pr14_review.json    # PR #14 review findings (ACL categories)
    reviews/                # Cached review texts (git-ignored)
    results/                # Skill evolution output (git-ignored)
    dst_results/            # DST optimization output (git-ignored)
```

## Phase 1: Scoring

Each review is scored on four components:

| Component | Weight | Description |
|-----------|--------|-------------|
| Weighted Recall | 0.50 | Found required issues, weighted by severity |
| Precision | 0.25 | True positives / (TP + false positives) |
| Calibration | 0.15 | Correct severity classification |
| Coverage Bonus | 0.10 | Optional findings discovered |

Scoring uses keyword co-occurrence (deterministic, no API calls):
- Each finding has a list of keywords
- A finding is "matched" if >= 50% of keywords appear in the review
- False positives (known-correct things incorrectly flagged) reduce precision

## Phase 2: Skill Evolution

Evolves skill markdown files using genetic algorithms:

- **Crossover**: Section-level uniform (50/50 per section from each parent)
- **Mutation**: 5 text operators (sentence shuffle/drop/duplicate, keyword inject, section swap)
- **Fitness**: Composite score from ground truth evaluation
- **Budget**: CostTracker enforces spending limits for live evaluations

### Evaluator Modes

| Mode | Cost | Usage |
|------|------|-------|
| `--mock` | Free | Tests GA machinery with synthetic fitness |
| `--live` | ~$0.05/task | Calls Claude CLI, scores against ground truth |

### Output

Results saved to `gepa/results/{skill_name}/`:
- `gen_NNN.json` — Population data per generation
- `best_skill.md` — Best evolved skill markdown
- `evolution_history.json` — Full history with fitness curves

## Phase 2: DST Config Optimization

Evolves buggify fault injection probabilities to find configurations that surface the most invariant violations:

- **Parameters**: 32 floats (31 fault probabilities + global_multiplier)
- **Crossover**: Uniform per parameter
- **Mutation**: `current += choice([-1, 0, 1]) * step`, clamped to bounds
- **Initialization**: calm + moderate + chaos presets + random variants
- **Fitness**: 0.5 * violations_found + 0.3 * fault_coverage + 0.2 * (1 - crash_rate)

### Rust Integration

The `BUGGIFY_CONFIG` env var overrides `FaultConfig::moderate()` defaults:

```bash
# Run DST tests with custom fault config
BUGGIFY_CONFIG="global_multiplier=2.0,network.packet_drop=0.05" \
  cargo test --release --test executor_dst_test
```

Format: comma-separated `key=value` pairs. See `src/buggify/faults.rs` for all fault IDs.

### Output

Results saved to `gepa/dst_results/`:
- `gen_NNN.json` — Population data per generation
- `best_config.env` — Best config in `BUGGIFY_CONFIG` format
- `evolution_history.json` — Full history

## Ground Truth

Ground truth files capture expert knowledge from real reviews:
- **paper_review.json**: 13 findings from 6-expert paper review
- **pr13_review.json**: 6 findings from PR #13 (ACL DRYRUN/LOG)
- **pr14_review.json**: 4 findings from PR #14 (ACL categories)

See `ground_truth/schema.md` for the full format specification.

## Testing

```bash
# All Phase 1 tests
python3 -m unittest gepa.test_scorer gepa.test_candidate gepa.test_evaluator -v

# Phase 2: Mutation + Evolution tests
python3 -m unittest gepa.test_mutations gepa.test_evolution -v

# Phase 2: DST optimizer tests
python3 -m unittest gepa.test_dst_candidate gepa.test_dst_optimizer -v

# Rust-side buggify config parsing
cargo test --release -p redis_sim config::tests
```

## Dependencies

None. Python 3.10+ stdlib only (matching `evolve/` pattern).
