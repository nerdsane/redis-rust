"""Unit tests for candidate.py"""

import unittest
from pathlib import Path
from .candidate import SkillCandidate, SkillSection


SAMPLE_SKILL = """---
name: test-skill
description: A test skill
user_invocable: true
---

# Test Skill

Preamble text before any sections.

## Section One

Content of section one.

## Section Two

Content of section two with **bold** and `code`.

### Subsection 2.1

Nested content here.

## Section Three

Final section.
"""


class TestSkillSection(unittest.TestCase):
    def test_to_markdown(self):
        s = SkillSection(heading="My Section", content="Some content.", level=2)
        assert s.to_markdown() == "## My Section\n\nSome content."

    def test_word_count(self):
        s = SkillSection(heading="X", content="one two three four")
        assert s.word_count() == 4

    def test_level_3(self):
        s = SkillSection(heading="Sub", content="text", level=3)
        assert s.to_markdown().startswith("### Sub")


class TestSkillCandidate(unittest.TestCase):
    def setUp(self):
        SkillCandidate.reset_id_counter()

    def test_parse_sample(self):
        c = SkillCandidate.from_text("test-skill", SAMPLE_SKILL)
        assert c.name == "test-skill"
        assert "---" in c.frontmatter
        # The # heading becomes L1 section; preamble text is in that section's content
        title_section = c.get_section("Test Skill")
        assert title_section is not None
        assert "Preamble" in title_section.content

    def test_section_count(self):
        c = SkillCandidate.from_text("test-skill", SAMPLE_SKILL)
        # # (L1), ## Section One, ## Section Two, ### Subsection 2.1, ## Section Three
        assert len(c.sections) == 5

    def test_section_names(self):
        c = SkillCandidate.from_text("test-skill", SAMPLE_SKILL)
        names = c.section_names()
        assert "Section One" in names
        assert "Section Two" in names
        assert "Section Three" in names
        assert "Subsection 2.1" in names

    def test_get_section(self):
        c = SkillCandidate.from_text("test-skill", SAMPLE_SKILL)
        s = c.get_section("Section One")
        assert s is not None
        assert "Content of section one" in s.content

    def test_get_section_case_insensitive(self):
        c = SkillCandidate.from_text("test-skill", SAMPLE_SKILL)
        s = c.get_section("section one")
        assert s is not None

    def test_get_section_missing(self):
        c = SkillCandidate.from_text("test-skill", SAMPLE_SKILL)
        assert c.get_section("Nonexistent") is None

    def test_replace_section(self):
        c = SkillCandidate.from_text("test-skill", SAMPLE_SKILL)
        result = c.replace_section("Section One", "New content.")
        assert result is True
        assert c.get_section("Section One").content == "New content."

    def test_replace_missing_section(self):
        c = SkillCandidate.from_text("test-skill", SAMPLE_SKILL)
        result = c.replace_section("Nope", "content")
        assert result is False

    def test_roundtrip(self):
        c = SkillCandidate.from_text("test-skill", SAMPLE_SKILL)
        md = c.to_markdown()
        c2 = SkillCandidate.from_text("test-skill", md)
        assert len(c.sections) == len(c2.sections)
        for s1, s2 in zip(c.sections, c2.sections):
            assert s1.heading == s2.heading
            assert s1.level == s2.level

    def test_to_dict(self):
        c = SkillCandidate.from_text("test-skill", SAMPLE_SKILL)
        d = c.to_dict()
        assert d["name"] == "test-skill"
        assert d["section_count"] == len(c.sections)
        assert isinstance(d["word_count"], int)

    def test_auto_id(self):
        c1 = SkillCandidate.from_text("a", SAMPLE_SKILL)
        c2 = SkillCandidate.from_text("b", SAMPLE_SKILL)
        assert c1._id < c2._id

    def test_repr(self):
        c = SkillCandidate.from_text("test-skill", SAMPLE_SKILL)
        r = repr(c)
        assert "test-skill" in r
        assert "sections=" in r

    def test_no_frontmatter(self):
        text = "# Title\n\nContent.\n\n## Section\n\nBody."
        c = SkillCandidate.from_text("bare", text)
        assert c.frontmatter == ""

    def test_word_count(self):
        c = SkillCandidate.from_text("test-skill", SAMPLE_SKILL)
        wc = c.word_count()
        assert wc > 0

    def test_from_real_file(self):
        """Test parsing actual skill files in the repo."""
        agents = Path(__file__).parent.parent / ".claude" / "agents"
        if not agents.exists():
            self.skipTest("agents directory not found")
        for path in agents.glob("*.md"):
            c = SkillCandidate.from_file(path)
            assert c.name == path.stem
            assert len(c.sections) > 0
            # Roundtrip should preserve section count
            md = c.to_markdown()
            c2 = SkillCandidate.from_text(c.name, md)
            assert len(c.sections) == len(c2.sections), (
                f"{c.name}: {len(c.sections)} != {len(c2.sections)}"
            )


if __name__ == "__main__":
    unittest.main()
