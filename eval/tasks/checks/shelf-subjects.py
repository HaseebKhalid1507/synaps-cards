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


from shelf.catalog import Catalog


def shelf():
    c = Catalog()
    c.add(Book(B1, "The Quiet Garden", ["A. Reed"], 1998, ["Gardening", "Poetry"]))
    c.add(Book(B2, "Gardens of Stone", ["M. Ortiz"], 2004, ["History"]))
    c.add(Book(B3, "Émile and the Quiet Sea", ["J. Doré"], 2011, ["Poésie"]))
    c.add(Book(B4, "Introduction to Algorithms", ["T. Cormen"], 2009))
    return c


class Check(unittest.TestCase):
    def test_a_book_has_subjects(self):
        self.assertEqual(Book("1", "T").subjects, [])
        self.assertEqual([b.isbn for b in shelf().by_subject("poetry")], [B1])
        self.assertEqual([b.isbn for b in shelf().by_subject("POESIE")], [B3])
        self.assertEqual(shelf().by_subject("cooking"), [])

    def test_search_filters_by_subject(self):
        c = shelf()
        self.assertEqual([b.isbn for b in c.search("garden")], [B1])
        self.assertEqual([b.isbn for b in c.search("quiet subject:poesie")], [B3])
        self.assertEqual([b.isbn for b in c.search("subject:history")], [B2])
        self.assertEqual([b.isbn for b in c.search("subject:poetry subject:poesie")], [B1, B3])
        self.assertEqual(c.search("garden subject:history"), [])
        # Several subject terms are OR (the prompt says so), and the other
        # words rank the books kept.
        self.assertEqual([b.isbn for b in c.search("quiet sea subject:poetry subject:poesie")], [B3, B1])
        self.assertEqual([b.isbn for b in c.search("garden subject:history subject:poetry")], [B1])

    def test_the_store_keeps_subjects_and_old_files_load(self):
        lib = Loans(shelf(), [Member("m1", "Ada")])
        back = store.load_data(json.loads(json.dumps(store.dump(lib))))
        self.assertEqual(back.catalog.get(B1).subjects, ["Gardening", "Poetry"])
        old = store.dump(lib)
        for b in old["books"]:
            b.pop("subjects")
        self.assertEqual(store.load_data(old).catalog.get(B1).subjects, [])

    def test_the_cli_counts_subjects(self):
        lib = Loans(shelf(), [Member("m1", "Ada")])
        lib.catalog.get(B2).subjects.append("Poetry")
        lines, _ = run_cli(lib, "subjects", "FILE")
        self.assertEqual([l.split() for l in lines], [["Gardening", "1"], ["History", "1"], ["Poetry", "2"], ["Poésie", "1"]])


if __name__ == "__main__":
    unittest.main()
