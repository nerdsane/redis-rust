"""
SkillCandidate - represents a skill markdown file as a mutable artifact.

Parses skill markdown into sections by ## headings, enabling
section-level mutation and crossover for GEPA evolution.
"""

import re
from dataclasses import dataclass, field
from pathlib import Path
from typing import Dict, List, Optional


@dataclass
class SkillSection:
    """A single ## section within a skill markdown file."""
    heading: str  # The ## heading text (without ##)
    content: str  # Everything between this heading and the next
    level: int = 2  # Heading level (## = 2, ### = 3, etc.)

    def to_markdown(self) -> str:
        prefix = "#" * self.level
        return f"{prefix} {self.heading}\n\n{self.content}"

    def word_count(self) -> int:
        return len(self.content.split())


@dataclass
class SkillCandidate:
    """
    A skill markdown file parsed into sections for mutation.

    Mirrors evolve/candidate.py pattern: serializable, identifiable,
    and fitness-trackable.
    """
    name: str  # Skill name (e.g., "rust-dev")
    frontmatter: str  # YAML frontmatter (---\n...\n---)
    preamble: str  # Content before first ## heading
    sections: List[SkillSection] = field(default_factory=list)
    fitness: Optional[float] = None
    generation: int = 0
    parent_ids: List[int] = field(default_factory=list)
    _id: int = field(default_factory=lambda: SkillCandidate._next_id())

    _id_counter: int = 0

    @classmethod
    def _next_id(cls) -> int:
        cls._id_counter += 1
        return cls._id_counter

    @classmethod
    def reset_id_counter(cls):
        """Reset ID counter (useful for testing)."""
        cls._id_counter = 0

    @classmethod
    def from_file(cls, path: Path) -> "SkillCandidate":
        """Parse a skill markdown file into sections."""
        assert path.exists(), f"Skill file not found: {path}"
        text = path.read_text()
        name = path.stem
        return cls.from_text(name, text)

    @classmethod
    def from_text(cls, name: str, text: str) -> "SkillCandidate":
        """Parse skill markdown text into sections."""
        assert isinstance(text, str) and len(text) > 0, "text must be non-empty"

        frontmatter = ""
        body = text

        # Extract YAML frontmatter
        fm_match = re.match(r'^---\s*\n(.*?)\n---\s*\n', text, re.DOTALL)
        if fm_match:
            frontmatter = fm_match.group(0)
            body = text[fm_match.end():]

        # Split on ## headings (capture the heading line)
        parts = re.split(r'^(#{1,6})\s+(.+)$', body, flags=re.MULTILINE)

        preamble = parts[0].strip()
        sections = []

        # parts[0] = preamble, then groups of 3: (hashes, heading, content)
        i = 1
        while i < len(parts) - 2:
            hashes = parts[i]
            heading = parts[i + 1].strip()
            content = parts[i + 2].strip() if i + 2 < len(parts) else ""
            level = len(hashes)
            sections.append(SkillSection(
                heading=heading,
                content=content,
                level=level,
            ))
            i += 3

        return cls(
            name=name,
            frontmatter=frontmatter,
            preamble=preamble,
            sections=sections,
        )

    def to_markdown(self) -> str:
        """Reconstruct the full markdown from sections."""
        parts = []
        if self.frontmatter:
            parts.append(self.frontmatter.rstrip())
        if self.preamble:
            parts.append(self.preamble)
        for section in self.sections:
            parts.append(section.to_markdown())
        return "\n\n".join(parts) + "\n"

    def save(self, path: Path) -> None:
        """Save skill markdown to file."""
        path.write_text(self.to_markdown())

    def section_names(self) -> List[str]:
        """List all section headings."""
        return [s.heading for s in self.sections]

    def get_section(self, heading: str) -> Optional[SkillSection]:
        """Find a section by heading (case-insensitive)."""
        heading_lower = heading.lower()
        for s in self.sections:
            if s.heading.lower() == heading_lower:
                return s
        return None

    def replace_section(self, heading: str, new_content: str) -> bool:
        """Replace a section's content by heading. Returns True if found."""
        section = self.get_section(heading)
        if section is None:
            return False
        section.content = new_content
        return True

    def word_count(self) -> int:
        """Total word count across all sections."""
        total = len(self.preamble.split())
        total += sum(s.word_count() for s in self.sections)
        return total

    def to_dict(self) -> Dict:
        """Serialize to dict for JSON storage."""
        return {
            "id": self._id,
            "name": self.name,
            "fitness": self.fitness,
            "generation": self.generation,
            "parent_ids": self.parent_ids,
            "section_count": len(self.sections),
            "word_count": self.word_count(),
            "sections": [s.heading for s in self.sections],
        }

    @classmethod
    def from_dict(cls, data: Dict, agents_dir: Path) -> "SkillCandidate":
        """Load from dict by re-reading the skill file."""
        name = data["name"]
        path = agents_dir / f"{name}.md"
        candidate = cls.from_file(path)
        candidate.fitness = data.get("fitness")
        candidate.generation = data.get("generation", 0)
        candidate.parent_ids = data.get("parent_ids", [])
        return candidate

    def __repr__(self) -> str:
        fitness_str = f"{self.fitness:.3f}" if self.fitness is not None else "?"
        return (
            f"SkillCandidate(name={self.name!r}, fitness={fitness_str}, "
            f"sections={len(self.sections)}, words={self.word_count()})"
        )
