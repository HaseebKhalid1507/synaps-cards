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
    def test_overdue(self):
        lib = library()
        lib.lend(B2, "m1", d("2026-03-01"))
        lib.lend(B1, "m1", d("2026-03-01"))
        lib.lend(B3, "m2", d("2026-03-05"))
        self.assertEqual(run_cli(lib, "overdue", "FILE", "--today", "2026-04-01"), [
            f"{B1}  m1  due 2026-03-22  10 days late",
            f"{B2}  m1  due 2026-03-22  10 days late",
            f"{B3}  m2  due 2026-03-26  6 days late",
        ])

    def test_none_overdue(self):
        lib = library()
        lib.lend(B1, "m1", d("2026-03-01"))
        self.assertEqual(run_cli(lib, "overdue", "FILE", "--today", "2026-03-22"), [])


if __name__ == "__main__":
    unittest.main()
