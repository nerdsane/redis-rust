"""
Text mutation operators for skill evolution.

All operators are pure functions: (content: str, rng: random.Random) -> str.
No external dependencies.
"""

import re
import random as _random_module
from typing import List


def sentence_shuffle(content: str, rng: _random_module.Random) -> str:
    """Shuffle sentences within the content. Skip if content has >=2 code fences."""
    if content.count("```") >= 2:
        return content
    sentences = re.split(r'(?<=[.!?])\s+', content)
    if len(sentences) <= 1:
        return content
    rng.shuffle(sentences)
    return " ".join(sentences)


def sentence_drop(content: str, rng: _random_module.Random) -> str:
    """Remove one random sentence (only if at least 2 sentences)."""
    sentences = re.split(r'(?<=[.!?])\s+', content)
    if len(sentences) < 2:
        return content
    idx = rng.randrange(len(sentences))
    sentences.pop(idx)
    return " ".join(sentences)


def sentence_duplicate(content: str, rng: _random_module.Random) -> str:
    """Duplicate one random sentence."""
    if not content.strip():
        return content
    sentences = re.split(r'(?<=[.!?])\s+', content)
    if not sentences:
        return content
    idx = rng.randrange(len(sentences))
    insert_pos = rng.randrange(len(sentences) + 1)
    sentences.insert(insert_pos, sentences[idx])
    return " ".join(sentences)


def keyword_inject(content: str, rng: _random_module.Random, keywords: List[str] = None) -> str:
    """Insert a templated sentence with keywords from missed findings."""
    if not keywords:
        # Default keyword pairs for common review concerns
        keyword_pairs = [
            ("assertions", "postconditions"),
            ("checked arithmetic", "overflow"),
            ("error handling", "unwrap"),
            ("DST coverage", "fault injection"),
            ("Redis compatibility", "error format"),
            ("TigerStyle", "invariants"),
            ("borrow checker", "lifetime"),
            ("shadow state", "drift"),
        ]
        kw1, kw2 = rng.choice(keyword_pairs)
    else:
        if len(keywords) >= 2:
            kw1, kw2 = rng.sample(keywords, 2)
        elif len(keywords) == 1:
            kw1, kw2 = keywords[0], "correctness"
        else:
            return content

    templates = [
        f"When reviewing, pay special attention to {kw1} vs {kw2} issues.",
        f"Check for {kw1} and {kw2} problems in the implementation.",
        f"Verify that {kw1} is handled correctly, especially regarding {kw2}.",
    ]
    sentence = rng.choice(templates)

    sentences = re.split(r'(?<=[.!?])\s+', content) if content.strip() else []
    if sentences:
        insert_pos = rng.randrange(len(sentences) + 1)
        sentences.insert(insert_pos, sentence)
        return " ".join(sentences)
    return sentence


def section_swap(content: str, rng: _random_module.Random) -> str:
    """Swap two sections at the same heading level within the content."""
    # This operates on multi-section content with ## headings
    parts = re.split(r'^(#{2,3}\s+.+)$', content, flags=re.MULTILINE)

    if len(parts) < 5:  # Need at least 2 sections (preamble + heading + content + heading + content)
        return content

    # Collect section indices (heading is at odd indices in split result)
    section_indices = [i for i in range(1, len(parts), 2)]

    if len(section_indices) < 2:
        return content

    # Pick two random section indices to swap
    i, j = rng.sample(range(len(section_indices)), 2)
    si, sj = section_indices[i], section_indices[j]

    # Swap heading + content pairs
    parts[si], parts[sj] = parts[sj], parts[si]
    # Swap their content blocks too
    if si + 1 < len(parts) and sj + 1 < len(parts):
        parts[si + 1], parts[sj + 1] = parts[sj + 1], parts[si + 1]

    return "".join(parts)


# All mutation operators for convenient iteration
ALL_MUTATIONS = [
    sentence_shuffle,
    sentence_drop,
    sentence_duplicate,
    keyword_inject,
    section_swap,
]
