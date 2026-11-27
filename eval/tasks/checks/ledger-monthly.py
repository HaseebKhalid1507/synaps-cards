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
    def test_monthly(self):
        j = journal(*SAMPLE)
        self.assertEqual(report.monthly(j, "5000"), [("2026-01", 120000), ("2026-02", 120000)])
        self.assertEqual(report.monthly(j, "1000"), [("2026-01", 380000), ("2026-02", -40000)])
        self.assertEqual(report.monthly(j, "5100"), [])

    def test_the_cli(self):
        self.assertEqual(run_cli("monthly", "1000", "FILE"), ["2026-01  3,800.00", "2026-02  -400.00"])


if __name__ == "__main__":
    unittest.main()
