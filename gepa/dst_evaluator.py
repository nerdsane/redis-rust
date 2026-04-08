"""
DST fitness evaluators for fault config optimization.

DstFitnessEvaluator: runs cargo test with BUGGIFY_CONFIG env var, parses results
MockDstEvaluator: deterministic mock for testing GA machinery
"""

import logging
import os
import re
import subprocess
from abc import ABC, abstractmethod
from dataclasses import dataclass, field
from pathlib import Path
from typing import Dict, List, Optional

from .dst_candidate import DstCandidate

logger = logging.getLogger(__name__)


@dataclass
class TestResult:
    """Result of a single test run."""
    test_name: str
    seed: int
    passed: bool
    invariant_violations: int = 0
    panicked: bool = False
    output: str = ""


@dataclass
class BatchResult:
    """Parsed structured output from test_env_config_batch."""
    total_runs: int = 0
    total_ops: int = 0
    total_crashes: int = 0
    total_recoveries: int = 0
    failures: int = 0
    buggify_checks: int = 0
    buggify_triggers: int = 0
    faults_triggered: int = 0
    global_multiplier: float = 1.0
    test_passed: bool = False
    raw_output: str = ""

    @classmethod
    def parse(cls, output: str, test_passed: bool) -> "BatchResult":
        """Parse GEPA_* structured output from cargo test."""
        result = cls(test_passed=test_passed, raw_output=output[:3000])

        def _extract_int(key: str) -> int:
            m = re.search(rf'{key}=(\d+)', output)
            return int(m.group(1)) if m else 0

        def _extract_float(key: str) -> float:
            m = re.search(rf'{key}=([\d.]+)', output)
            return float(m.group(1)) if m else 0.0

        result.global_multiplier = _extract_float("GEPA_GLOBAL_MULTIPLIER")
        result.total_runs = _extract_int("GEPA_TOTAL_RUNS")
        result.total_ops = _extract_int("GEPA_TOTAL_OPS")
        result.total_crashes = _extract_int("GEPA_TOTAL_CRASHES")
        result.total_recoveries = _extract_int("GEPA_TOTAL_RECOVERIES")
        result.failures = _extract_int("GEPA_FAILURES")
        result.buggify_checks = _extract_int("GEPA_BUGGIFY_CHECKS")
        result.buggify_triggers = _extract_int("GEPA_BUGGIFY_TRIGGERS")
        result.faults_triggered = _extract_int("GEPA_FAULTS_TRIGGERED")

        return result


@dataclass
class StreamingResult:
    """Parsed output from test_env_config_streaming."""
    total_ops: int = 0
    put_failures: int = 0
    get_failures: int = 0
    flushes: int = 0
    crashes: int = 0
    invariant_violations: int = 0
    test_passed: bool = False

    @classmethod
    def parse(cls, output: str, test_passed: bool) -> "StreamingResult":
        result = cls(test_passed=test_passed)

        def _extract_int(key: str) -> int:
            m = re.search(rf'{key}=(\d+)', output)
            return int(m.group(1)) if m else 0

        result.total_ops = _extract_int("GEPA_STREAMING_OPS")
        result.put_failures = _extract_int("GEPA_STREAMING_PUT_FAILURES")
        result.get_failures = _extract_int("GEPA_STREAMING_GET_FAILURES")
        result.flushes = _extract_int("GEPA_STREAMING_FLUSHES")
        result.crashes = _extract_int("GEPA_STREAMING_CRASHES")
        result.invariant_violations = _extract_int("GEPA_STREAMING_INVARIANT_VIOLATIONS")
        return result


@dataclass
class WalResult:
    """Parsed output from test_env_config_wal."""
    total_writes: int = 0
    acknowledged: int = 0
    missing_after_recovery: int = 0
    write_failures: int = 0
    sync_failures: int = 0
    disk_full: int = 0
    invariant_failures: int = 0
    test_passed: bool = False

    @classmethod
    def parse(cls, output: str, test_passed: bool) -> "WalResult":
        result = cls(test_passed=test_passed)

        def _extract_int(key: str) -> int:
            m = re.search(rf'{key}=(\d+)', output)
            return int(m.group(1)) if m else 0

        result.total_writes = _extract_int("GEPA_WAL_TOTAL_WRITES")
        result.acknowledged = _extract_int("GEPA_WAL_ACKNOWLEDGED")
        result.missing_after_recovery = _extract_int("GEPA_WAL_MISSING_AFTER_RECOVERY")
        result.write_failures = _extract_int("GEPA_WAL_WRITE_FAILURES")
        result.sync_failures = _extract_int("GEPA_WAL_SYNC_FAILURES")
        result.disk_full = _extract_int("GEPA_WAL_DISK_FULL")
        result.invariant_failures = _extract_int("GEPA_WAL_INVARIANT_FAILURES")
        return result


@dataclass
class CrdtResult:
    """Parsed output from test_env_config_crdt."""
    total_ops: int = 0
    message_drops: int = 0
    convergence_failures: int = 0
    invariant_violations: int = 0
    test_passed: bool = False

    @classmethod
    def parse(cls, output: str, test_passed: bool) -> "CrdtResult":
        result = cls(test_passed=test_passed)

        def _extract_int(key: str) -> int:
            m = re.search(rf'{key}=(\d+)', output)
            return int(m.group(1)) if m else 0

        result.total_ops = _extract_int("GEPA_CRDT_TOTAL_OPS")
        result.message_drops = _extract_int("GEPA_CRDT_MESSAGE_DROPS")
        result.convergence_failures = _extract_int("GEPA_CRDT_CONVERGENCE_FAILURES")
        result.invariant_violations = _extract_int("GEPA_CRDT_INVARIANT_VIOLATIONS")
        return result


class DstFitnessEvaluator(ABC):
    """Abstract base class for DST fitness evaluation."""

    @abstractmethod
    def evaluate(self, candidate: DstCandidate) -> float:
        """Evaluate a DST candidate. Returns fitness score 0.0-1.0."""
        ...


class MockDstEvaluator(DstFitnessEvaluator):
    """
    Mock evaluator for testing GA machinery.

    Simulates: higher global_multiplier and moderate fault rates find more bugs,
    but too-high rates cause crashes (penalty).
    """

    def __init__(self, rng=None):
        import random
        self._rng = rng or random.Random()

    def evaluate(self, candidate: DstCandidate) -> float:
        gm = candidate.config.get("global_multiplier", 1.0)

        # Simulate: moderate configs find more bugs
        # Sweet spot around gm=1.5-2.5
        bug_discovery = 1.0 - abs(gm - 2.0) / 3.0
        bug_discovery = max(0.0, min(1.0, bug_discovery))

        # Higher fault rates = more coverage but also more crashes
        total_fault_rate = sum(
            v for k, v in candidate.config.items()
            if k != "global_multiplier"
        )
        avg_fault_rate = total_fault_rate / max(1, len(candidate.config) - 1)

        # Coverage increases with fault rate up to a point
        coverage = min(1.0, avg_fault_rate * 20.0)

        # Crash penalty for very high rates
        crash_penalty = 0.0
        if avg_fault_rate > 0.05:
            crash_penalty = (avg_fault_rate - 0.05) * 5.0

        score = (
            0.50 * bug_discovery +
            0.30 * coverage +
            0.20 * (1.0 - min(1.0, crash_penalty))
        )

        noise = self._rng.gauss(0, 0.02)
        return max(0.0, min(1.0, score + noise))


class CargoDstEvaluator(DstFitnessEvaluator):
    """
    Multi-target evaluator: runs all 4 DST subsystem tests with BUGGIFY_CONFIG.

    Targets:
      1. dst_batch_verification::test_env_config_batch  -> crash/recovery metrics
      2. streaming_dst_test::test_env_config_streaming  -> object store fault metrics
      3. wal_dst_test::test_env_config_wal              -> WAL disk fault metrics
      4. crdt_dst_test::test_env_config_crdt            -> replication fault metrics

    Fitness score (0.0-1.0):
      0.25 * crash_score       (process crashes / recovery cycles)
      0.25 * streaming_score   (object store fault coverage + ops surviving)
      0.25 * wal_score         (disk faults triggered, no data loss)
      0.25 * crdt_score        (gossip faults, convergence maintained)
    """

    # Test targets: (test_file, test_name)
    TARGETS = [
        ("dst_batch_verification", "test_env_config_batch"),
        ("streaming_dst_test", "test_env_config_streaming"),
        ("wal_dst_test", "test_env_config_wal"),
        ("crdt_dst_test", "test_env_config_crdt"),
    ]

    def __init__(
        self,
        timeout: int = 300,
        project_dir: Path = None,
    ):
        self.timeout = timeout
        self.project_dir = project_dir or Path(__file__).parent.parent

    def evaluate(self, candidate: DstCandidate) -> float:
        env_string = candidate.to_env_string()

        # Run all 4 targets and collect results
        batch = self._run_target(env_string, "dst_batch_verification", "test_env_config_batch")
        streaming = self._run_target(env_string, "streaming_dst_test", "test_env_config_streaming")
        wal = self._run_target(env_string, "wal_dst_test", "test_env_config_wal")
        crdt = self._run_target(env_string, "crdt_dst_test", "test_env_config_crdt")

        # Parse each target's output
        batch_result = BatchResult.parse(batch[0], batch[1]) if batch else None
        streaming_result = StreamingResult.parse(streaming[0], streaming[1]) if streaming else None
        wal_result = WalResult.parse(wal[0], wal[1]) if wal else None
        crdt_result = CrdtResult.parse(crdt[0], crdt[1]) if crdt else None

        # Compute per-subsystem scores
        crash_score = self._crash_subscore(batch_result, candidate)
        streaming_score = self._streaming_subscore(streaming_result)
        wal_score = self._wal_subscore(wal_result)
        crdt_score = self._crdt_subscore(crdt_result)

        fitness = (
            0.25 * crash_score +
            0.25 * streaming_score +
            0.25 * wal_score +
            0.25 * crdt_score
        )

        logger.info(
            "Fitness=%.4f (crash=%.3f stream=%.3f wal=%.3f crdt=%.3f)",
            fitness, crash_score, streaming_score, wal_score, crdt_score,
        )

        return max(0.0, min(1.0, fitness))

    def _run_target(
        self, env_string: str, test_file: str, test_name: str
    ) -> Optional[tuple]:
        """Run a single test target. Returns (output, passed) or None on timeout."""
        env = os.environ.copy()
        env["BUGGIFY_CONFIG"] = env_string

        cmd = [
            "cargo", "test", "--release",
            "--test", test_file,
            test_name,
            "--", "--nocapture",
        ]

        try:
            result = subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                timeout=self.timeout,
                cwd=str(self.project_dir),
                env=env,
            )
            output = result.stdout + result.stderr
            test_passed = result.returncode == 0
            logger.debug("Target %s::%s passed=%s", test_file, test_name, test_passed)
            return (output, test_passed)

        except subprocess.TimeoutExpired:
            logger.warning("Target %s::%s timed out after %ds", test_file, test_name, self.timeout)
            return None

    def _crash_subscore(self, result: Optional[BatchResult], candidate: DstCandidate) -> float:
        """Score for crash/recovery subsystem (dst_batch_verification)."""
        if result is None:
            return 0.0

        # Crash intensity: more crashes = stressing harder
        if result.total_runs > 0:
            crashes_per_run = result.total_crashes / result.total_runs
            intensity = min(1.0, crashes_per_run / 20.0)
        else:
            intensity = 0.0

        # Fault trigger rate
        if result.buggify_checks > 0:
            trigger_rate = result.buggify_triggers / result.buggify_checks
            trigger_score = min(1.0, trigger_rate * 10.0)
        else:
            trigger_score = 0.0

        # Fault coverage
        configured_faults = sum(
            1 for k, v in candidate.config.items()
            if k != "global_multiplier" and v > 0
        )
        if configured_faults > 0:
            coverage = min(1.0, result.faults_triggered / configured_faults)
        else:
            coverage = 0.0

        # Stability
        stability = 1.0 if result.failures == 0 else 0.0

        return (
            0.30 * intensity +
            0.25 * trigger_score +
            0.25 * coverage +
            0.20 * stability
        )

    def _streaming_subscore(self, result: Optional[StreamingResult]) -> float:
        """Score for object store fault subsystem (streaming_dst_test)."""
        if result is None:
            return 0.0

        # Fault intensity: total store failures / total ops
        total_failures = result.put_failures + result.get_failures
        if result.total_ops > 0:
            fault_rate = total_failures / result.total_ops
            intensity = min(1.0, fault_rate * 10.0)
        else:
            intensity = 0.0

        # Operations surviving: high ops count = system resilient under faults
        ops_score = min(1.0, result.total_ops / 2000.0)

        # Stability: penalize invariant violations
        stability = 1.0 if result.invariant_violations == 0 else 0.0

        return (
            0.40 * intensity +
            0.30 * ops_score +
            0.30 * stability
        )

    def _wal_subscore(self, result: Optional[WalResult]) -> float:
        """Score for WAL disk fault subsystem (wal_dst_test)."""
        if result is None:
            return 0.0

        # Fault intensity: disk faults triggered
        total_faults = result.write_failures + result.sync_failures + result.disk_full
        if result.total_writes > 0:
            fault_rate = total_faults / result.total_writes
            intensity = min(1.0, fault_rate * 10.0)
        else:
            intensity = 0.0

        # Data durability: acknowledged writes with zero missing
        if result.acknowledged > 0:
            durability = 1.0 if result.missing_after_recovery == 0 else 0.0
        else:
            durability = 0.5  # No acked writes = system handled all faults

        # Stability: no invariant failures
        stability = 1.0 if result.invariant_failures == 0 else 0.0

        return (
            0.35 * intensity +
            0.35 * durability +
            0.30 * stability
        )

    def _crdt_subscore(self, result: Optional[CrdtResult]) -> float:
        """Score for CRDT replication fault subsystem (crdt_dst_test)."""
        if result is None:
            return 0.0

        # Fault intensity: message drops / total ops
        if result.total_ops > 0:
            drop_rate = result.message_drops / result.total_ops
            intensity = min(1.0, drop_rate * 5.0)
        else:
            intensity = 0.0

        # Operations completed
        ops_score = min(1.0, result.total_ops / 12000.0)

        # Convergence: penalize convergence failures
        stability = 1.0 if result.convergence_failures == 0 else 0.0

        return (
            0.35 * intensity +
            0.30 * ops_score +
            0.35 * stability
        )
