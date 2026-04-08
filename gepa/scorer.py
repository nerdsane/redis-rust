"""
Scorer - deterministic keyword-matching scorer for review quality.

Scores a review text against ground truth findings using keyword
co-occurrence. No external dependencies, no API calls.
"""

import re
from dataclasses import dataclass, field
from typing import Dict, List, Optional


# Severity → default weight multiplier (used when finding has no explicit weight)
SEVERITY_WEIGHTS = {
    "inaccurate": 2.0,
    "verified": 0.5,
    "misleading": 1.5,
    "gap": 1.0,
    "violation": 1.5,
    "nit": 0.5,
}

# Score component weights (must sum to 1.0)
COMPONENT_WEIGHTS = {
    "weighted_recall": 0.50,
    "precision": 0.25,
    "calibration": 0.15,
    "coverage_bonus": 0.10,
}


@dataclass
class FindingMatch:
    """Result of matching a single finding against review text."""
    finding_id: str
    matched: bool
    keyword_hits: int
    keyword_total: int
    confidence: float  # 0.0-1.0, fraction of keywords found
    weight: float


@dataclass
class FalsePositiveMatch:
    """Result of checking for a known false positive in review text."""
    fp_id: str
    triggered: bool
    penalty: float


@dataclass
class ScoreResult:
    """Complete scoring result for a review."""
    weighted_recall: float  # 0.0-1.0
    precision: float  # 0.0-1.0
    calibration: float  # 0.0-1.0
    coverage_bonus: float  # 0.0-1.0
    composite: float  # weighted combination
    finding_matches: List[FindingMatch] = field(default_factory=list)
    false_positive_matches: List[FalsePositiveMatch] = field(default_factory=list)
    true_positives: int = 0
    false_positives: int = 0
    missed_required: List[str] = field(default_factory=list)

    def summary(self) -> str:
        lines = [
            f"Composite: {self.composite:.3f}",
            f"  Recall:    {self.weighted_recall:.3f} (weight {COMPONENT_WEIGHTS['weighted_recall']})",
            f"  Precision: {self.precision:.3f} (weight {COMPONENT_WEIGHTS['precision']})",
            f"  Calibration: {self.calibration:.3f} (weight {COMPONENT_WEIGHTS['calibration']})",
            f"  Coverage:  {self.coverage_bonus:.3f} (weight {COMPONENT_WEIGHTS['coverage_bonus']})",
            f"  TP={self.true_positives} FP={self.false_positives}",
        ]
        if self.missed_required:
            lines.append(f"  Missed required: {', '.join(self.missed_required)}")
        return "\n".join(lines)


class Scorer:
    """
    Scores review text against ground truth using keyword co-occurrence.

    A finding is considered "matched" if at least `min_keyword_fraction`
    of its keywords appear in the review text (case-insensitive).
    """

    def __init__(self, min_keyword_fraction: float = 0.5):
        assert 0.0 < min_keyword_fraction <= 1.0, (
            f"min_keyword_fraction must be in (0, 1], got {min_keyword_fraction}"
        )
        self.min_keyword_fraction = min_keyword_fraction

    def score(
        self,
        review_text: str,
        ground_truth: Dict,
        skill_name: Optional[str] = None,
    ) -> ScoreResult:
        """
        Score a review against ground truth.

        Args:
            review_text: The review text to score.
            ground_truth: Parsed ground truth dict (from JSON).
            skill_name: If provided, only score findings relevant to this skill.

        Returns:
            ScoreResult with component scores and details.
        """
        assert isinstance(review_text, str), "review_text must be a string"
        assert isinstance(ground_truth, dict), "ground_truth must be a dict"
        assert "expected_findings" in ground_truth, (
            "ground_truth must have 'expected_findings'"
        )

        normalized = _normalize_text(review_text)

        findings = ground_truth["expected_findings"]
        false_positives = ground_truth.get("known_false_positives", [])

        # Filter by skill relevance if specified
        if skill_name:
            findings = [
                f for f in findings
                if skill_name in f.get("skill_relevance", [])
            ]

        # Match expected findings
        finding_matches = []
        for finding in findings:
            match = self._match_finding(normalized, finding)
            finding_matches.append(match)

        # Check for false positives
        fp_matches = []
        for fp in false_positives:
            fp_match = self._match_false_positive(normalized, fp)
            fp_matches.append(fp_match)

        # Compute component scores
        weighted_recall = self._compute_weighted_recall(finding_matches)
        true_positives = sum(1 for m in finding_matches if m.matched)
        false_positive_count = sum(1 for m in fp_matches if m.triggered)
        precision = self._compute_precision(true_positives, false_positive_count)
        calibration = self._compute_calibration(finding_matches, findings)
        coverage_bonus = self._compute_coverage_bonus(finding_matches, findings)
        missed_required = [
            m.finding_id for m, f in zip(finding_matches, findings)
            if not m.matched and f.get("required", False)
        ]

        composite = (
            COMPONENT_WEIGHTS["weighted_recall"] * weighted_recall
            + COMPONENT_WEIGHTS["precision"] * precision
            + COMPONENT_WEIGHTS["calibration"] * calibration
            + COMPONENT_WEIGHTS["coverage_bonus"] * coverage_bonus
        )

        return ScoreResult(
            weighted_recall=weighted_recall,
            precision=precision,
            calibration=calibration,
            coverage_bonus=coverage_bonus,
            composite=composite,
            finding_matches=finding_matches,
            false_positive_matches=fp_matches,
            true_positives=true_positives,
            false_positives=false_positive_count,
            missed_required=missed_required,
        )

    def _match_finding(self, normalized: str, finding: Dict) -> FindingMatch:
        """Check if a finding's keywords appear in the normalized text."""
        keywords = finding.get("keywords", [])
        if not keywords:
            return FindingMatch(
                finding_id=finding["id"],
                matched=False,
                keyword_hits=0,
                keyword_total=0,
                confidence=0.0,
                weight=finding.get("weight", 1.0),
            )

        hits = sum(1 for kw in keywords if kw.lower() in normalized)
        fraction = hits / len(keywords)
        matched = fraction >= self.min_keyword_fraction

        return FindingMatch(
            finding_id=finding["id"],
            matched=matched,
            keyword_hits=hits,
            keyword_total=len(keywords),
            confidence=fraction,
            weight=finding.get("weight", SEVERITY_WEIGHTS.get(finding.get("severity", "nit"), 1.0)),
        )

    def _match_false_positive(self, normalized: str, fp: Dict) -> FalsePositiveMatch:
        """Check if a known false positive was flagged in the review."""
        keywords = fp.get("keywords", [])
        if not keywords:
            return FalsePositiveMatch(fp_id=fp["id"], triggered=False, penalty=0.0)

        hits = sum(1 for kw in keywords if kw.lower() in normalized)
        fraction = hits / len(keywords) if keywords else 0.0
        triggered = fraction >= self.min_keyword_fraction

        return FalsePositiveMatch(
            fp_id=fp["id"],
            triggered=triggered,
            penalty=fp.get("penalty", 1.0) if triggered else 0.0,
        )

    def _compute_weighted_recall(self, matches: List[FindingMatch]) -> float:
        """Weighted recall: fraction of weighted findings matched."""
        if not matches:
            return 0.0
        total_weight = sum(m.weight for m in matches)
        if total_weight == 0.0:
            return 0.0
        matched_weight = sum(m.weight for m in matches if m.matched)
        return matched_weight / total_weight

    def _compute_precision(self, true_positives: int, false_positives: int) -> float:
        """Precision: TP / (TP + FP). Returns 1.0 if no positives."""
        total = true_positives + false_positives
        if total == 0:
            return 1.0
        return true_positives / total

    def _compute_calibration(
        self, matches: List[FindingMatch], findings: List[Dict]
    ) -> float:
        """
        Calibration: how well the review distinguishes severity levels.

        Higher-severity findings should have higher confidence scores.
        Score is 1.0 - mean_absolute_deviation of severity-vs-confidence ranking.
        """
        if len(matches) < 2:
            return 0.5  # Not enough data to calibrate

        severity_order = {
            "inaccurate": 5, "violation": 4, "misleading": 3,
            "gap": 2, "nit": 1, "verified": 0,
        }

        pairs = []
        for match, finding in zip(matches, findings):
            sev = severity_order.get(finding.get("severity", "nit"), 1)
            pairs.append((sev, match.confidence))

        if not pairs:
            return 0.5

        # Compute rank correlation (simplified Spearman)
        n = len(pairs)
        sev_ranked = _rank([p[0] for p in pairs])
        conf_ranked = _rank([p[1] for p in pairs])

        # Spearman's rho
        d_sq_sum = sum((s - c) ** 2 for s, c in zip(sev_ranked, conf_ranked))
        if n <= 1:
            return 0.5
        rho = 1 - (6 * d_sq_sum) / (n * (n * n - 1))

        # Map rho from [-1, 1] to [0, 1]
        return max(0.0, min(1.0, (rho + 1) / 2))

    def _compute_coverage_bonus(
        self, matches: List[FindingMatch], findings: List[Dict]
    ) -> float:
        """Bonus for finding optional (non-required) issues."""
        optional_matches = [
            m for m, f in zip(matches, findings)
            if not f.get("required", False)
        ]
        if not optional_matches:
            return 0.0
        found = sum(1 for m in optional_matches if m.matched)
        return found / len(optional_matches)


def _normalize_text(text: str) -> str:
    """Lowercase and normalize whitespace for keyword matching."""
    text = text.lower()
    text = re.sub(r'\s+', ' ', text)
    return text


def _rank(values: List[float]) -> List[float]:
    """Assign fractional ranks to values (for Spearman correlation)."""
    n = len(values)
    indexed = sorted(enumerate(values), key=lambda x: x[1])
    ranks = [0.0] * n

    i = 0
    while i < n:
        j = i
        while j < n and indexed[j][1] == indexed[i][1]:
            j += 1
        avg_rank = (i + j - 1) / 2.0 + 1
        for k in range(i, j):
            ranks[indexed[k][0]] = avg_rank
        i = j

    return ranks
