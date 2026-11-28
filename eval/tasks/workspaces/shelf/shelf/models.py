"""Books and members."""

from dataclasses import dataclass, field


@dataclass
class Book:
    isbn: str
    title: str
    authors: list = field(default_factory=list)
    year: int | None = None


@dataclass
class Member:
    id: str
    name: str
    limit: int = 3  # books on loan at once
