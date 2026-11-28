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


INVOICES = (
    ("2026-01-01", "Invoice 1", [("1100", 100000), ("4000", -100000)]),
    ("2026-02-15", "Invoice 2", [("1100", 50000), ("4000", -50000)]),
    ("2026-03-20", "Invoice 3", [("1100", 20000), ("4000", -20000)]),
    ("2026-03-25", "Paid", [("1000", 120000), ("1100", -120000)]),
)


class Check(unittest.TestCase):
    def test_credits_pay_the_oldest_debits_first(self):
        # Invoice 1 is paid, 200.00 of invoice 2 too: 300.00 is 54 days old, 200.00 is 21.
        rows = report.aging(journal(*INVOICES), d("2026-04-10"))
        self.assertEqual(rows, [("0-30", 20000), ("31-60", 30000), ("61-90", 0), ("90+", 0)])

    def test_the_buckets_edges(self):
        j = journal(*INVOICES[:1])
        for day, bucket in (("2026-01-31", "0-30"), ("2026-02-01", "31-60"), ("2026-03-02", "31-60"),
                            ("2026-03-03", "61-90"), ("2026-04-01", "61-90"), ("2026-04-02", "90+")):
            rows = dict(report.aging(j, d(day)))
            self.assertEqual(rows[bucket], 100000, (day, rows))

    def test_another_account_and_nothing_open(self):
        self.assertEqual(report.aging(journal(*SAMPLE), d("2026-03-01")), [("0-30", 0), ("31-60", 0), ("61-90", 0), ("90+", 0)])
        self.assertEqual(dict(report.aging(journal(*INVOICES), d("2026-04-10"), code="4000")), {"0-30": 0, "31-60": 0, "61-90": 0, "90+": 0})

    def test_the_cli_prints_it(self):
        out = ["entry,date,memo,account,amount"]
        for i, (date, memo, lines) in enumerate(INVOICES, 1):
            out += [f"{i},{date},{memo},{a},{c / 100:.2f}" for a, c in lines]
        lines, _ = run_cli({"j.csv": "\n".join(out) + "\n"}, "aging", "j.csv", "--today", "2026-04-10")
        self.assertEqual([l.split() for l in lines], [["0-30", "200.00"], ["31-60", "300.00"], ["61-90", "0.00"], ["90+", "0.00"]])


if __name__ == "__main__":
    unittest.main()
