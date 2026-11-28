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


def late(lib, isbn, member, days):
    """Lend `isbn` to `member` and return it `days` days late: the fine."""
    loan = lib.lend(isbn, member, d("2026-03-01"))
    return lib.give_back(isbn, loan.due + datetime.timedelta(days=days))


class Check(unittest.TestCase):
    def test_fines_are_recorded_and_payments_pay_them(self):
        lib = library()
        self.assertEqual(late(lib, B1, "m1", 4), 100)
        self.assertEqual((lib.loans[0].fine, lib.owed("m1"), lib.owed("m2")), (100, 100, 0))
        lib.pay("m1", 40)
        self.assertEqual(lib.owed("m1"), 60)
        for bad in (0, -5):
            with self.assertRaises(ValueError):
                lib.pay("m1", bad)

    def test_a_member_owing_too_much_cant_borrow(self):
        self.assertEqual(loans_mod.MAX_OWED, 500)
        lib = library()
        late(lib, B1, "m1", 20)
        lib.lend(B2, "m1", d("2026-05-01"))
        lib.give_back(B2, d("2026-05-01"))
        late(lib, B2, "m1", 1)
        self.assertEqual(lib.owed("m1"), 525)
        with self.assertRaises(ValueError):
            lib.lend(B3, "m1", d("2026-06-01"))
        lib.pay("m1", 25)
        lib.lend(B3, "m1", d("2026-06-01"))

    def test_the_store_keeps_fines_and_payments(self):
        lib = library()
        late(lib, B1, "m1", 4)
        lib.pay("m1", 40)
        data = json.loads(json.dumps(store.dump(lib)))
        self.assertEqual(data["payments"], [{"member": "m1", "cents": 40}])
        back = store.load_data(data)
        self.assertEqual((back.loans[0].fine, back.owed("m1")), (100, 60))
        data.pop("payments")
        for l in data["loans"]:
            l.pop("fine")
        self.assertEqual(store.load_data(data).owed("m1"), 0)

    def test_the_cli_takes_payments_and_lists_what_is_owed(self):
        lib = library()
        late(lib, B1, "m1", 10)
        late(lib, B2, "m2", 2)
        lines, after = run_cli(lib, "pay", "FILE", "m1", "1.00")
        self.assertEqual(after.owed("m1"), 150)
        lines, _ = run_cli(after, "owed", "FILE")
        self.assertEqual(lines, ["m1  Ada  1.50", "m2  Grace  0.50"])
        after.pay("m2", 50)
        lines, _ = run_cli(after, "owed", "FILE")
        self.assertEqual(lines, ["m1  Ada  1.50"])


if __name__ == "__main__":
    unittest.main()
