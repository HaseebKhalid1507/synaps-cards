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


from ledger import exporter


def same(j):
    return [(e.date, e.memo, e.lines) for e in j.entries]


class Check(unittest.TestCase):
    def test_a_round_trip_gives_the_same_entries(self):
        j = journal(*SAMPLE, ("2026-03-01", 'Rent, "March"', [("5000", 123456), ("1000", -123456)]))
        back = Journal(default_chart())
        importer.import_into(back, exporter.write_csv(j))
        self.assertEqual(same(back), same(j))

    def test_rows_number_the_entries_and_format_the_amounts(self):
        rows = list(csv.reader(io.StringIO(exporter.write_csv(journal(*SAMPLE[:2])))))
        self.assertEqual(rows[0], ["entry", "date", "memo", "account", "amount"])
        self.assertEqual(rows[1], ["1", "2026-01-05", "Owner's capital", "1000", "5,000.00"])
        self.assertEqual(rows[2], ["1", "2026-01-05", "Owner's capital", "3000", "-5,000.00"])
        self.assertEqual([r[0] for r in rows[3:]], ["2", "2"])
        self.assertEqual(len(rows), 5)

    def test_posting_order_not_date_order(self):
        j = journal(SAMPLE[3], SAMPLE[0])
        rows = list(csv.reader(io.StringIO(exporter.write_csv(j))))
        self.assertEqual([(r[0], r[1]) for r in rows[1:]], [("1", "2026-02-03")] * 2 + [("2", "2026-01-05")] * 2)

    def test_the_cli_exports(self):
        _, after = run_cli({"j.csv": sample_csv()}, "export", "j.csv", "new-out.csv")
        back = Journal(default_chart())
        importer.import_into(back, after["new-out.csv"])
        self.assertEqual(same(back), same(journal(*SAMPLE)))


if __name__ == "__main__":
    unittest.main()
