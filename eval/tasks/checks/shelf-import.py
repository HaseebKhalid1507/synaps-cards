import datetime
import io
import json
import os
import tempfile
import unittest

from shelf import store
from shelf.loans import LOAN_DAYS, Loan, Loans, fine
from shelf.models import Book, Member
from tests.helpers import d, library

B1, B2, B3, B4 = "9780140449136", "9780306406157", "9781861972712", "9780262033848"


def run_cli(lib, *args, files=None):
    """`shelf.cli.main(args)` on `lib` saved as FILE in a temporary
    directory (with `files`, name: text, beside it; each of their names in
    `args` replaced by its path): the printed lines, and the library file
    loaded again after the run."""
    from shelf import cli

    with tempfile.TemporaryDirectory() as tmp:
        path = os.path.join(tmp, "library.json")
        store.save(path, lib)
        for name, text in (files or {}).items():
            with open(os.path.join(tmp, name), "w", encoding="utf-8") as f:
                f.write(text)
        out = io.StringIO()
        cli.main([path if a == "FILE" else os.path.join(tmp, a) if a in (files or {}) else a for a in args], out=out)
        return out.getvalue().strip().splitlines(), store.load(path)


CSV = ("isbn,title,authors,year\n"
       "9780000000001,Night Trains, P. Lane ; Q. Ito ,2019\n"
       "9780000000002,Salt,,\n"
       f"{B1},The Quiet Garden (2nd ed.),A. Reed,2020\n")


class Check(unittest.TestCase):
    def test_rows_become_books_and_known_isbns_are_skipped(self):
        lib = library()
        self.assertEqual(store.import_books(lib, CSV), (2, 1))
        b = lib.catalog.get("9780000000001")
        self.assertEqual((b.title, b.authors, b.year), ("Night Trains", ["P. Lane", "Q. Ito"], 2019))
        b = lib.catalog.get("9780000000002")
        self.assertEqual((b.title, b.authors, b.year), ("Salt", [], None))
        self.assertEqual(lib.catalog.get(B1).title, "The Quiet Garden")

    def test_a_bad_file_adds_nothing(self):
        for bad in ("isbn,title,year\n9780000000001,X,2019\n", CSV + "9780000000003,Y,,soon\n"):
            lib = library()
            with self.assertRaises(ValueError, msg=bad):
                store.import_books(lib, bad)
            self.assertEqual(len(lib.catalog.books), 4)

    def test_the_cli_imports_and_saves(self):
        lines, after = run_cli(library(), "import", "FILE", "books.csv", files={"books.csv": CSV})
        self.assertEqual(lines, ["added 2, skipped 1"])
        self.assertEqual(after.catalog.get("9780000000001").authors, ["P. Lane", "Q. Ito"])
        self.assertEqual(len(after.catalog.books), 6)


if __name__ == "__main__":
    unittest.main()
