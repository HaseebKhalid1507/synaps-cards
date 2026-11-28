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


CSV = ("entry,date,memo,account,amount,currency\n"
       "1,2026-01-05,Capital,1000,500.00,EUR\n1,2026-01-05,Capital,3000,-500.00,EUR\n"
       "2,2026-01-06,Capital,1000,100.00,\n2,2026-01-06,Capital,3000,-100.00,\n")


class Check(unittest.TestCase):
    def test_the_column_and_its_default(self):
        es = importer.read_csv(CSV)
        self.assertEqual([l.currency for l in es[0].lines], ["EUR", "EUR"])
        self.assertEqual([l.currency for l in es[1].lines], ["USD", "USD"])
        self.assertEqual(importer.read_csv(sample_csv())[0].lines[0].currency, "USD")
        for bad in ("eur", "EURO", "E1R"):
            with self.assertRaises(ValueError):
                importer.read_csv(CSV.replace("EUR", bad, 1))

    def test_a_line_is_usd_unless_said(self):
        self.assertEqual(Line("1000", 5).currency, "USD")
        self.assertEqual(Line("1000", 5, "EUR").currency, "EUR")

    def test_an_entry_doesnt_mix_currencies(self):
        j = Journal(default_chart())
        with self.assertRaises(ValueError):
            j.post(Entry(d("2026-01-05"), "Mixed", [Line("1000", 100, "EUR"), Line("3000", -100, "USD")]))
        self.assertEqual(j.entries, [])

    def test_balances_and_the_trial_balance_are_per_currency(self):
        j = Journal(default_chart())
        importer.import_into(j, CSV)
        self.assertEqual((j.balance("1000"), j.balance("1000", currency="EUR")), (10000, 50000))
        eur = {c: b for c, _, b in report.trial_balance(j, currency="EUR")}
        self.assertEqual((eur["1000"], eur["3000"], eur["4000"]), (50000, 50000, 0))
        self.assertEqual({c: b for c, _, b in report.trial_balance(j)}["3000"], 10000)

    def test_the_cli_takes_a_currency(self):
        lines, _ = run_cli({"j.csv": CSV}, "balance", "j.csv", "--currency", "EUR")
        self.assertTrue(any(l.startswith("1000") and "500.00" in l for l in lines), lines)
        lines, _ = run_cli({"j.csv": CSV}, "balance", "j.csv")
        self.assertTrue(any(l.startswith("1000") and "100.00" in l for l in lines), lines)


if __name__ == "__main__":
    unittest.main()
