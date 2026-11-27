import csv
import datetime
import io
import os
import tempfile
import unittest

from shelf import store
from shelf.models import Book
from tests.helpers import d, library

B1, B2, B3, B4 = "9780140449136", "9780306406157", "9781861972712", "9780262033848"


def titles(found):
    return [b.title for b in found]


def run_cli(lib, *args):
    from shelf import cli

    with tempfile.TemporaryDirectory() as tmp:
        path = os.path.join(tmp, "library.json")
        store.save(path, lib)
        out = io.StringIO()
        cli.main([a if a != "FILE" else path for a in args], out=out)
        return out.getvalue().strip().splitlines()

class Check(unittest.TestCase):
    def test_years_and_authors_round_trip(self):
        lib = library()
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "library.json")
            store.save(path, lib)
            back = store.load(path)
        for isbn, book in lib.catalog.books.items():
            self.assertEqual((back.catalog.get(isbn).year, back.catalog.get(isbn).authors), (book.year, book.authors))


if __name__ == "__main__":
    unittest.main()
