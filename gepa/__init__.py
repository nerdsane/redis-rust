"""
GEPA - Genetic Evolution for Prompt Artifacts

Skill evaluation harness and DST configuration optimizer.
Uses ground truth from expert reviews to score and evolve
Claude Code skill files (.claude/agents/*.md).
"""

from .scorer import Scorer
from .candidate import SkillCandidate
from .evaluator import OfflineEvaluator
from .evolution import SkillEvolutionEngine
from .dst_candidate import DstCandidate
from .dst_optimizer import DstEvolutionEngine

__version__ = "0.2.0"
__all__ = [
    "Scorer",
    "SkillCandidate",
    "OfflineEvaluator",
    "SkillEvolutionEngine",
    "DstCandidate",
    "DstEvolutionEngine",
]
