"""Text normalization for search."""

import re
import unicodedata

_WORD = re.compile(r"[a-z0-9]+")


def fold(text):
    """`text` lowercased, its accents stripped: "Émile" is "emile"."""
    decomposed = unicodedata.normalize("NFKD", text)
    return "".join(c for c in decomposed if not unicodedata.combining(c)).lower()


def words(text):
    return _WORD.findall(fold(text))
