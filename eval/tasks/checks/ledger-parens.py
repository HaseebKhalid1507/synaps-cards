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
    def test_parentheses_are_negative(self):
        self.assertEqual(money.parse("(12.50)"), -1250)
        self.assertEqual(money.parse("(1,234.00)"), -123400)
        self.assertEqual(money.parse(" (7) "), -700)

    def test_signs_still_read(self):
        self.assertEqual(money.parse("-3.05"), -305)
        self.assertEqual(money.parse("3.05"), 305)


if __name__ == "__main__":
    unittest.main()
