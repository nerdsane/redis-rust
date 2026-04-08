"""Unit tests for mutations.py"""

import random
import unittest
from .mutations import (
    sentence_shuffle, sentence_drop, sentence_duplicate,
    keyword_inject, section_swap, ALL_MUTATIONS,
)


class TestSentenceShuffle(unittest.TestCase):
    def test_shuffles_sentences(self):
        rng = random.Random(42)
        content = "First sentence. Second sentence. Third sentence."
        result = sentence_shuffle(content, rng)
        # Should contain all original sentences
        assert "First sentence." in result
        assert "Second sentence." in result
        assert "Third sentence." in result

    def test_single_sentence_unchanged(self):
        rng = random.Random(42)
        content = "Only one sentence here."
        assert sentence_shuffle(content, rng) == content

    def test_skips_code_fences(self):
        rng = random.Random(42)
        content = "Text. More text.\n```rust\ncode\n```\nEnd."
        assert sentence_shuffle(content, rng) == content

    def test_empty_content(self):
        rng = random.Random(42)
        assert sentence_shuffle("", rng) == ""


class TestSentenceDrop(unittest.TestCase):
    def test_drops_one_sentence(self):
        rng = random.Random(42)
        content = "First. Second. Third."
        result = sentence_drop(content, rng)
        parts = [p.strip() for p in result.split(".") if p.strip()]
        assert len(parts) == 2

    def test_single_sentence_unchanged(self):
        rng = random.Random(42)
        content = "Only one."
        assert sentence_drop(content, rng) == content

    def test_deterministic(self):
        content = "A. B. C. D."
        r1 = sentence_drop(content, random.Random(99))
        r2 = sentence_drop(content, random.Random(99))
        assert r1 == r2


class TestSentenceDuplicate(unittest.TestCase):
    def test_adds_one_sentence(self):
        rng = random.Random(42)
        content = "First. Second."
        result = sentence_duplicate(content, rng)
        # Should have 3 sentence-like chunks now
        assert len(result) > len(content)

    def test_empty_content(self):
        rng = random.Random(42)
        assert sentence_duplicate("", rng) == ""

    def test_deterministic(self):
        content = "A. B. C."
        r1 = sentence_duplicate(content, random.Random(7))
        r2 = sentence_duplicate(content, random.Random(7))
        assert r1 == r2


class TestKeywordInject(unittest.TestCase):
    def test_injects_sentence(self):
        rng = random.Random(42)
        content = "Existing content here."
        result = keyword_inject(content, rng)
        assert len(result) > len(content)

    def test_custom_keywords(self):
        rng = random.Random(42)
        content = "Text."
        result = keyword_inject(content, rng, keywords=["alpha", "beta"])
        assert "alpha" in result or "beta" in result

    def test_empty_content(self):
        rng = random.Random(42)
        result = keyword_inject("", rng)
        assert len(result) > 0

    def test_single_keyword(self):
        rng = random.Random(42)
        result = keyword_inject("Base.", rng, keywords=["only"])
        assert "only" in result


class TestSectionSwap(unittest.TestCase):
    def test_swaps_sections(self):
        rng = random.Random(42)
        content = "## A\n\nContent A.\n\n## B\n\nContent B."
        result = section_swap(content, rng)
        # Should still contain both sections
        assert "Content A." in result
        assert "Content B." in result

    def test_single_section_unchanged(self):
        rng = random.Random(42)
        content = "## Only\n\nOne section."
        assert section_swap(content, rng) == content

    def test_no_headings_unchanged(self):
        rng = random.Random(42)
        content = "Just plain text without sections."
        assert section_swap(content, rng) == content


class TestAllMutations(unittest.TestCase):
    def test_all_mutations_list(self):
        assert len(ALL_MUTATIONS) == 5

    def test_all_return_string(self):
        rng = random.Random(42)
        content = "First sentence. Second sentence. Third sentence."
        for mut in ALL_MUTATIONS:
            if mut == keyword_inject:
                result = mut(content, rng)
            else:
                result = mut(content, rng)
            assert isinstance(result, str), f"{mut.__name__} returned {type(result)}"

    def test_deterministic_with_same_seed(self):
        content = "Alpha. Beta. Gamma. Delta."
        for mut in ALL_MUTATIONS:
            r1 = mut(content, random.Random(123))
            r2 = mut(content, random.Random(123))
            assert r1 == r2, f"{mut.__name__} not deterministic"


if __name__ == "__main__":
    unittest.main()
