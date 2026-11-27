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
    def test_export(self):
        lib = library()
        lib.catalog.add(Book("9780000000002", "Zebra Stripes", [], None))
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "catalog.csv")
            store.export_csv(lib, path)
            with open(path, encoding="utf-8", newline="") as f:
                rows = list(csv.reader(f))
        self.assertEqual(rows[0], ["isbn", "title", "authors", "year"])
        self.assertEqual([r[0] for r in rows[1:]], [B3, B2, B4, B1, "9780000000002"])
        self.assertEqual(rows[3], [B4, "Introduction to Algorithms", "T. Cormen; C. Leiserson", "2009"])
        self.assertEqual(rows[5][2:], ["", ""])


if __name__ == "__main__":
    unittest.main()
