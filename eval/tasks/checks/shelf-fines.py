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
    def fine(self, back):
        lib = library()
        lib.lend(B1, "m1", d("2026-03-01"))  # due 2026-03-22
        return lib.give_back(B1, d(back))

    def test_grace(self):
        self.assertEqual(self.fine("2026-03-22"), 0)
        self.assertEqual(self.fine("2026-03-23"), 0)
        self.assertEqual(self.fine("2026-03-24"), 0)

    def test_after_the_grace(self):
        self.assertEqual(self.fine("2026-03-25"), 25)
        self.assertEqual(self.fine("2026-04-01"), 200)

    def test_the_cap(self):
        self.assertEqual(self.fine("2026-07-01"), 1000)


if __name__ == "__main__":
    unittest.main()
