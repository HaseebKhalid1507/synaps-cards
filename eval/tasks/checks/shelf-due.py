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
    def test_due_in_three_weeks(self):
        lib = library()
        self.assertEqual(lib.lend(B1, "m1", d("2026-03-01")).due, d("2026-03-22"))
        self.assertEqual(lib.lend(B2, "m1", d("2026-02-20")).due, d("2026-03-13"))

    def test_overdue_after_the_due_day(self):
        lib = library()
        lib.lend(B1, "m1", d("2026-03-01"))
        self.assertEqual(lib.overdue(d("2026-03-22")), [])
        self.assertEqual(len(lib.overdue(d("2026-03-23"))), 1)


if __name__ == "__main__":
    unittest.main()
