"""
DST Candidate -- represents a buggify fault configuration as a GA individual.

32 tunable parameters: 31 fault probabilities + global_multiplier.
All floats with min/max/step/default bounds.
"""

from dataclasses import dataclass, field
from typing import Dict, List, Optional


@dataclass
class DstParamBounds:
    """Bounds for a single DST parameter."""
    min: float
    max: float
    step: float
    default: float

    def clamp(self, value: float) -> float:
        return max(self.min, min(self.max, value))


# All 32 tunable parameters with bounds
# Derived from src/buggify/config.rs moderate() preset
DST_PARAM_BOUNDS: Dict[str, DstParamBounds] = {
    # Global
    "global_multiplier": DstParamBounds(0.01, 5.0, 0.1, 1.0),
    # Network faults
    "network.packet_drop": DstParamBounds(0.0, 0.20, 0.005, 0.01),
    "network.packet_corrupt": DstParamBounds(0.0, 0.05, 0.001, 0.001),
    "network.partial_write": DstParamBounds(0.0, 0.10, 0.005, 0.005),
    "network.reorder": DstParamBounds(0.0, 0.20, 0.005, 0.02),
    "network.connection_reset": DstParamBounds(0.0, 0.10, 0.005, 0.005),
    "network.connect_timeout": DstParamBounds(0.0, 0.15, 0.005, 0.01),
    "network.delay": DstParamBounds(0.0, 0.30, 0.01, 0.05),
    "network.duplicate": DstParamBounds(0.0, 0.10, 0.005, 0.005),
    # Timer faults
    "timer.drift_fast": DstParamBounds(0.0, 0.10, 0.005, 0.01),
    "timer.drift_slow": DstParamBounds(0.0, 0.10, 0.005, 0.01),
    "timer.skip": DstParamBounds(0.0, 0.10, 0.005, 0.01),
    "timer.duplicate": DstParamBounds(0.0, 0.05, 0.001, 0.005),
    "timer.jump_forward": DstParamBounds(0.0, 0.05, 0.001, 0.001),
    "timer.jump_backward": DstParamBounds(0.0, 0.02, 0.0005, 0.0005),
    # Process faults
    "process.crash": DstParamBounds(0.0, 0.02, 0.001, 0.001),
    "process.pause": DstParamBounds(0.0, 0.10, 0.005, 0.01),
    "process.slow": DstParamBounds(0.0, 0.15, 0.005, 0.02),
    "process.oom": DstParamBounds(0.0, 0.005, 0.0001, 0.0001),
    "process.cpu_starvation": DstParamBounds(0.0, 0.10, 0.005, 0.01),
    # Disk faults
    "disk.write_fail": DstParamBounds(0.0, 0.02, 0.001, 0.001),
    "disk.partial_write": DstParamBounds(0.0, 0.02, 0.001, 0.001),
    "disk.corruption": DstParamBounds(0.0, 0.005, 0.0001, 0.0001),
    "disk.slow": DstParamBounds(0.0, 0.15, 0.005, 0.02),
    "disk.fsync_fail": DstParamBounds(0.0, 0.01, 0.0005, 0.0005),
    "disk.stale_read": DstParamBounds(0.0, 0.02, 0.001, 0.001),
    "disk.disk_full": DstParamBounds(0.0, 0.005, 0.0001, 0.0001),
    # Replication faults
    "replication.gossip_drop": DstParamBounds(0.0, 0.20, 0.005, 0.02),
    "replication.gossip_delay": DstParamBounds(0.0, 0.30, 0.01, 0.05),
    "replication.gossip_corrupt": DstParamBounds(0.0, 0.02, 0.001, 0.001),
    "replication.split_brain": DstParamBounds(0.0, 0.005, 0.0001, 0.0001),
    "replication.stale_replica": DstParamBounds(0.0, 0.10, 0.005, 0.01),
}

# Preset configurations matching Rust config.rs
PRESETS = {
    "calm": {
        "global_multiplier": 0.1,
        "network.packet_drop": 0.001,
        "network.delay": 0.01,
        "timer.drift_fast": 0.001,
        "timer.drift_slow": 0.001,
    },
    "moderate": {k: v.default for k, v in DST_PARAM_BOUNDS.items()},
    "chaos": {
        "global_multiplier": 3.0,
        "network.packet_drop": 0.05,
        "network.packet_corrupt": 0.01,
        "network.partial_write": 0.02,
        "network.reorder": 0.10,
        "network.connection_reset": 0.02,
        "network.connect_timeout": 0.05,
        "network.delay": 0.15,
        "network.duplicate": 0.02,
        "timer.drift_fast": 0.05,
        "timer.drift_slow": 0.05,
        "timer.skip": 0.05,
        "timer.duplicate": 0.02,
        "timer.jump_forward": 0.01,
        "timer.jump_backward": 0.005,
        "process.crash": 0.005,
        "process.pause": 0.05,
        "process.slow": 0.10,
        "process.oom": 0.001,
        "process.cpu_starvation": 0.05,
        "disk.write_fail": 0.005,
        "disk.partial_write": 0.005,
        "disk.corruption": 0.001,
        "disk.slow": 0.10,
        "disk.fsync_fail": 0.002,
        "disk.stale_read": 0.005,
        "disk.disk_full": 0.001,
        "replication.gossip_drop": 0.10,
        "replication.gossip_delay": 0.15,
        "replication.gossip_corrupt": 0.005,
        "replication.split_brain": 0.001,
        "replication.stale_replica": 0.05,
    },
}


@dataclass
class DstCandidate:
    """A buggify fault configuration as a GA individual."""
    config: Dict[str, float] = field(default_factory=dict)
    fitness: Optional[float] = None
    generation: int = 0
    parent_ids: List[int] = field(default_factory=list)
    _id: int = field(default_factory=lambda: DstCandidate._next_id())

    _id_counter: int = 0

    @classmethod
    def _next_id(cls) -> int:
        cls._id_counter += 1
        return cls._id_counter

    @classmethod
    def reset_id_counter(cls):
        cls._id_counter = 0

    @classmethod
    def from_preset(cls, name: str) -> "DstCandidate":
        """Create from a preset (calm, moderate, chaos)."""
        assert name in PRESETS, f"Unknown preset: {name}. Available: {list(PRESETS.keys())}"
        # Start with moderate defaults, then overlay preset
        config = {k: v.default for k, v in DST_PARAM_BOUNDS.items()}
        config.update(PRESETS[name])
        return cls(config=config)

    @classmethod
    def from_env_string(cls, s: str) -> "DstCandidate":
        """Parse from BUGGIFY_CONFIG env var format."""
        config = {k: v.default for k, v in DST_PARAM_BOUNDS.items()}
        for pair in s.split(","):
            pair = pair.strip()
            if not pair:
                continue
            key, _, val = pair.partition("=")
            key = key.strip()
            val = val.strip()
            if key in DST_PARAM_BOUNDS:
                config[key] = DST_PARAM_BOUNDS[key].clamp(float(val))
        return cls(config=config)

    def to_env_string(self) -> str:
        """Serialize to BUGGIFY_CONFIG env var format."""
        parts = []
        for key in sorted(self.config.keys()):
            val = self.config[key]
            parts.append(f"{key}={val:.6f}")
        return ",".join(parts)

    def to_dict(self) -> Dict:
        """Serialize for JSON storage."""
        return {
            "id": self._id,
            "config": {k: round(v, 6) for k, v in sorted(self.config.items())},
            "fitness": self.fitness,
            "generation": self.generation,
            "parent_ids": self.parent_ids,
        }

    def is_valid(self) -> bool:
        """Check all params are within bounds."""
        for key, bounds in DST_PARAM_BOUNDS.items():
            val = self.config.get(key, bounds.default)
            if val < bounds.min - 1e-9 or val > bounds.max + 1e-9:
                return False
        return True

    def __repr__(self) -> str:
        fitness_str = f"{self.fitness:.4f}" if self.fitness is not None else "?"
        gm = self.config.get("global_multiplier", 1.0)
        return f"DstCandidate(id={self._id}, fitness={fitness_str}, gm={gm:.2f})"
