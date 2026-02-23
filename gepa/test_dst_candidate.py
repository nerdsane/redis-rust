"""Unit tests for dst_candidate.py"""

import unittest
from .dst_candidate import DstCandidate, DstParamBounds, DST_PARAM_BOUNDS, PRESETS


class TestDstParamBounds(unittest.TestCase):
    def test_clamp_within(self):
        b = DstParamBounds(0.0, 1.0, 0.1, 0.5)
        assert b.clamp(0.5) == 0.5

    def test_clamp_below(self):
        b = DstParamBounds(0.0, 1.0, 0.1, 0.5)
        assert b.clamp(-0.5) == 0.0

    def test_clamp_above(self):
        b = DstParamBounds(0.0, 1.0, 0.1, 0.5)
        assert b.clamp(1.5) == 1.0


class TestDstCandidate(unittest.TestCase):
    def setUp(self):
        DstCandidate.reset_id_counter()

    def test_from_preset_moderate(self):
        c = DstCandidate.from_preset("moderate")
        assert c.config["global_multiplier"] == 1.0
        assert c.config["network.packet_drop"] == 0.01
        assert c.is_valid()

    def test_from_preset_calm(self):
        c = DstCandidate.from_preset("calm")
        assert c.config["global_multiplier"] == 0.1
        assert c.is_valid()

    def test_from_preset_chaos(self):
        c = DstCandidate.from_preset("chaos")
        assert c.config["global_multiplier"] == 3.0
        assert c.is_valid()

    def test_from_preset_invalid(self):
        with self.assertRaises(AssertionError):
            DstCandidate.from_preset("nonexistent")

    def test_env_string_roundtrip(self):
        c = DstCandidate.from_preset("moderate")
        env = c.to_env_string()
        c2 = DstCandidate.from_env_string(env)
        for key in DST_PARAM_BOUNDS:
            assert abs(c.config[key] - c2.config[key]) < 1e-5, (
                f"Mismatch for {key}: {c.config[key]} != {c2.config[key]}"
            )

    def test_env_string_format(self):
        c = DstCandidate.from_preset("moderate")
        env = c.to_env_string()
        assert "global_multiplier=" in env
        assert "network.packet_drop=" in env
        # Should be comma-separated
        parts = env.split(",")
        assert len(parts) == len(DST_PARAM_BOUNDS)

    def test_to_dict(self):
        c = DstCandidate.from_preset("moderate")
        c.fitness = 0.75
        d = c.to_dict()
        assert d["fitness"] == 0.75
        assert "config" in d
        assert "global_multiplier" in d["config"]

    def test_is_valid(self):
        c = DstCandidate.from_preset("moderate")
        assert c.is_valid()

    def test_is_valid_out_of_bounds(self):
        c = DstCandidate.from_preset("moderate")
        c.config["global_multiplier"] = 100.0  # Way above max of 5.0
        assert not c.is_valid()

    def test_auto_id(self):
        c1 = DstCandidate.from_preset("moderate")
        c2 = DstCandidate.from_preset("chaos")
        assert c1._id < c2._id

    def test_repr(self):
        c = DstCandidate.from_preset("moderate")
        c.fitness = 0.42
        r = repr(c)
        assert "0.42" in r
        assert "gm=" in r

    def test_param_count(self):
        """Should have 32 parameters (31 faults + global_multiplier)."""
        assert len(DST_PARAM_BOUNDS) == 32

    def test_all_presets_have_all_params(self):
        """All presets should produce candidates with all params."""
        for name in PRESETS:
            c = DstCandidate.from_preset(name)
            for key in DST_PARAM_BOUNDS:
                assert key in c.config, f"Preset '{name}' missing key '{key}'"

    def test_from_env_string_partial(self):
        """Parsing partial env string should fill defaults."""
        c = DstCandidate.from_env_string("global_multiplier=2.5")
        assert c.config["global_multiplier"] == 2.5
        # Other params should be at defaults
        assert c.config["network.packet_drop"] == DST_PARAM_BOUNDS["network.packet_drop"].default


if __name__ == "__main__":
    unittest.main()
