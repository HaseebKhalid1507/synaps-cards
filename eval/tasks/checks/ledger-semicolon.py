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

SEMI = """entry;date;memo;account;amount
1;2026-01-05;Rent, January;5000;1.200,50
1;2026-01-05;Rent, January;1000;-1.200,50
"""


class Check(unittest.TestCase):
    def test_semicolons_and_decimal_commas(self):
        from ledger.importer import read_csv

        (e,) = read_csv(SEMI)
        self.assertEqual(e.memo, "Rent, January")
        self.assertEqual([line.cents for line in e.lines], [120050, -120050])

    def test_commas_still_work(self):
        from ledger.importer import read_csv

        self.assertEqual(len(read_csv(sample_csv())), 5)


if __name__ == "__main__":
    unittest.main()
