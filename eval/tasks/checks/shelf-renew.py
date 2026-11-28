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


from shelf import loans as loans_mod


class Check(unittest.TestCase):
    def test_a_renewal_moves_the_due_date_from_the_old_one(self):
        lib = library()
        loan = lib.lend(B1, "m1", d("2026-03-01"))
        self.assertEqual(loan.renewals, 0)
        self.assertIs(lib.renew(B1, d("2026-03-10")), loan)
        self.assertEqual((loan.due, loan.renewals), (d("2026-04-12"), 1))
        lib.renew(B1, d("2026-04-12"))
        self.assertEqual((loan.due, loan.renewals), (d("2026-05-03"), 2))

    def test_what_cant_be_renewed(self):
        self.assertEqual(loans_mod.MAX_RENEWALS, 2)
        lib = library()
        with self.assertRaises(ValueError):
            lib.renew(B1, d("2026-03-01"))
        lib.lend(B1, "m1", d("2026-03-01"))
        with self.assertRaises(ValueError):
            lib.renew(B1, d("2026-03-23"))
        lib.renew(B1, d("2026-03-22"))
        lib.renew(B1, d("2026-03-23"))
        with self.assertRaises(ValueError):
            lib.renew(B1, d("2026-03-24"))
        self.assertEqual(lib.on_loan(B1).renewals, 2)

    def test_the_store_keeps_the_count_and_old_files_load(self):
        lib = library()
        lib.lend(B1, "m1", d("2026-03-01"))
        lib.renew(B1, d("2026-03-02"))
        back = store.load_data(json.loads(json.dumps(store.dump(lib))))
        self.assertEqual((back.loans[0].renewals, back.loans[0].due), (1, d("2026-04-12")))
        old = store.dump(lib)
        for l in old["loans"]:
            l.pop("renewals", None)
        self.assertEqual(store.load_data(old).loans[0].renewals, 0)

    def test_the_cli_renews_and_saves(self):
        lib = library()
        lib.lend(B1, "m1", d("2026-03-01"))
        lines, after = run_cli(lib, "renew", "FILE", B1, "--today", "2026-03-05")
        self.assertEqual(lines, [f"{B1}  due 2026-04-12"])
        self.assertEqual((after.loans[0].due, after.loans[0].renewals), (d("2026-04-12"), 1))


if __name__ == "__main__":
    unittest.main()
