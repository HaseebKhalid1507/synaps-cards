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
    def test_hyphens_and_spaces_are_dropped(self):
        self.assertEqual(Book("978-0-306-40615-7", "T").isbn, B2)
        self.assertEqual(Book("978 0 306 40615 7", "T").isbn, B2)
        for isbn in (B1, B2, B3, B4):
            self.assertEqual(Book(isbn, "T").isbn, isbn)

    def test_bad_isbns(self):
        for bad in ("9780306406158", "97803064061", "97803064061570", "978030640615X"):
            with self.assertRaises(ValueError, msg=bad):
                Book(bad, "T")


if __name__ == "__main__":
    unittest.main()
