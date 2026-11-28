import datetime
import io
import os
import tempfile
import unittest

from ledger import money, report
from ledger.accounts import default_chart
from ledger.journal import Entry, Journal, Line
from tests.helpers import SAMPLE, d, journal


def sample_csv():
    out = ["entry,date,memo,account,amount"]
    for i, (date, memo, lines) in enumerate(SAMPLE, 1):
        for account, cents in lines:
            out.append(f"{i},{date},{memo},{account},{cents / 100:.2f}")
    return "\n".join(out) + "\n"


def run_cli(*args):
    from ledger import cli

    with tempfile.TemporaryDirectory() as tmp:
        path = os.path.join(tmp, "journal.csv")
        with open(path, "w", encoding="utf-8") as f:
            f.write(sample_csv())
        out = io.StringIO()
        cli.main([a if a != "FILE" else path for a in args], out=out)
        return out.getvalue().strip().splitlines()

class Check(unittest.TestCase):
    def test_allocate(self):
        self.assertEqual(money.allocate(100, [1, 1, 1]), [34, 33, 33])
        self.assertEqual(money.allocate(1000, [1, 2, 3]), [167, 333, 500])
        self.assertEqual(money.allocate(5, [0, 1]), [0, 5])
        self.assertEqual(money.allocate(7, [1, 1]), [4, 3])
        self.assertEqual(sum(money.allocate(99999, [7, 3, 5, 11])), 99999)

    def test_bad_weights(self):
        for w in ([], [0, 0], [1, -1]):
            with self.assertRaises(ValueError, msg=str(w)):
                money.allocate(100, w)


if __name__ == "__main__":
    unittest.main()
