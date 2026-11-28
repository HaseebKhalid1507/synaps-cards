import csv
import datetime
import io
import os
import tempfile
import unittest

from ledger import importer, money, report
from ledger.accounts import default_chart
from ledger.journal import Entry, Journal, Line
from tests.helpers import SAMPLE, d, journal


def run_cli(files, *args):
    """`ledger.cli.main(args)` in a temporary directory holding `files`
    (name: text), each of their names in `args`, and each name starting
    `new-`, replaced by its path: the printed lines, and every file of the
    directory after the run (name: text)."""
    from ledger import cli

    with tempfile.TemporaryDirectory() as tmp:
        for name, text in files.items():
            with open(os.path.join(tmp, name), "w", encoding="utf-8") as f:
                f.write(text)
        out = io.StringIO()
        cli.main([os.path.join(tmp, a) if a in files or a.startswith("new-") else a for a in args], out=out)
        after = {}
        for name in os.listdir(tmp):
            with open(os.path.join(tmp, name), encoding="utf-8") as f:
                after[name] = f.read()
        return out.getvalue().strip().splitlines(), after


def sample_csv(entries=SAMPLE):
    out = ["entry,date,memo,account,amount"]
    for i, (date, memo, lines) in enumerate(entries, 1):
        for account, cents in lines:
            out.append(f"{i},{date},{memo},{account},{cents / 100:.2f}")
    return "\n".join(out) + "\n"


from ledger import budget

BUDGET = "account,month,amount\n5000,2026-01,1000.00\n4000,2026-01,500.00\n5000,2026-02,1200.00\n"


class Check(unittest.TestCase):
    def test_a_budget_is_planned_cents_by_account_and_month(self):
        self.assertEqual(budget.load_csv(BUDGET), {("5000", "2026-01"): 100000, ("4000", "2026-01"): 50000, ("5000", "2026-02"): 120000})

    def test_a_bad_budget_is_refused(self):
        for bad in ("account,amount\n5000,1.00\n", "account,month,amount\n5000,2026-1,1.00\n",
                    "account,month,amount\n5000,2026-13,1.00\n", BUDGET + "5000,2026-01,3.00\n"):
            with self.assertRaises(ValueError, msg=bad):
                budget.load_csv(bad)

    def test_the_variance_of_a_month(self):
        rows = report.variance(journal(*SAMPLE), budget.load_csv(BUDGET), "2026-01")
        self.assertEqual(rows, [("4000", "Sales", 50000, 80000, 30000), ("5000", "Rent", 100000, 120000, 20000)])
        self.assertEqual(report.variance(journal(*SAMPLE), budget.load_csv(BUDGET), "2026-02"), [("5000", "Rent", 120000, 120000, 0)])
        self.assertEqual(report.variance(journal(*SAMPLE), budget.load_csv(BUDGET), "2026-03"), [])

    def test_the_cli_prints_it(self):
        lines, _ = run_cli({"j.csv": sample_csv(), "b.csv": BUDGET}, "variance", "j.csv", "b.csv", "--month", "2026-01")
        self.assertEqual(len(lines), 2, lines)
        self.assertEqual(lines[0].split(), ["4000", "Sales", "500.00", "800.00", "300.00"])


if __name__ == "__main__":
    unittest.main()
