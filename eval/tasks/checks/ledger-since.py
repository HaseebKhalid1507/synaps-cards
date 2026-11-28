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
    def test_since(self):
        rows = report.statement(journal(*SAMPLE), "1000", since=d("2026-02-01"))
        self.assertEqual(rows, [
            (d("2026-02-01"), "Opening balance", 0, 380000),
            (d("2026-02-03"), "Rent for February", -120000, 260000),
            (d("2026-02-10"), "Invoice 17 paid", 80000, 340000),
        ])

    def test_without_since_nothing_changes(self):
        rows = report.statement(journal(*SAMPLE), "5000")
        self.assertEqual(rows, [(d("2026-01-06"), "Rent for January", 120000, 120000),
                                (d("2026-02-03"), "Rent for February", 120000, 240000)])

    def test_the_cli_takes_since(self):
        lines = run_cli("statement", "1000", "FILE", "--since", "2026-02-01")
        self.assertEqual(len(lines), 3)
        self.assertIn("Opening balance", lines[0])
        self.assertIn("3,800.00", lines[0])
        self.assertEqual(len(run_cli("statement", "1000", "FILE")), 4)


if __name__ == "__main__":
    unittest.main()
