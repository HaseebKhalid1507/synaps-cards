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
    def test_value_is_the_amount(self):
        from ledger.importer import read_csv

        (e,) = read_csv("entry,date,memo,account,value\n1,2026-01-05,Rent,5000,12.50\n1,2026-01-05,Rent,1000,-12.50\n")
        self.assertEqual([line.cents for line in e.lines], [1250, -1250])

    def test_amount_still_works(self):
        from ledger.importer import read_csv

        self.assertEqual(len(read_csv(sample_csv())), 5)


if __name__ == "__main__":
    unittest.main()
