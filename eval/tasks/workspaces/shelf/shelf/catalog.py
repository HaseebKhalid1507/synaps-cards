"""The catalog: books by ISBN, and a ranked title search."""

from .text import words


class Catalog:
    def __init__(self):
        self.books = {}

    def add(self, book):
        if book.isbn in self.books:
            raise ValueError(f"book {book.isbn} exists")
        self.books[book.isbn] = book
        return book

    def get(self, isbn):
        try:
            return self.books[isbn]
        except KeyError:
            raise KeyError(f"no book {isbn}") from None

    def search(self, query):
        """Books whose titles share words with `query`, the most shared
        first, then by title."""
        want = set(words(query))
        if not want:
            return []
        scored = []
        for book in self.books.values():
            score = len(want & set(words(book.title)))
            if score:
                scored.append((-score, book.title, book))
        return [b for _, _, b in sorted(scored, key=lambda s: (s[0], s[1]))]
