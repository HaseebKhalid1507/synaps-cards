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
    def test_accents_and_case_fold(self):
        c = library().catalog
        self.assertEqual(titles(c.search("emile")), ["Émile and the Quiet Sea"])
        self.assertEqual(titles(c.search("ÉMILE")), ["Émile and the Quiet Sea"])
        self.assertEqual(titles(c.search("Emile quiet"))[0], "Émile and the Quiet Sea")


if __name__ == "__main__":
    unittest.main()
