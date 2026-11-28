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


from ledger import accounts

CHART = "code,name,kind\n2000,Payables,liability\n1000,Cash,asset\n3000,Equity,equity\n9100,Grants,income\n"
JOURNAL = "entry,date,memo,account,amount\n1,2026-01-05,Grant,1000,250.00\n1,2026-01-05,Grant,9100,-250.00\n"


class Check(unittest.TestCase):
    def test_a_chart_from_csv(self):
        c = accounts.load_chart(CHART)
        self.assertEqual([(a.code, a.name, a.kind) for a in c],
                         [("1000", "Cash", "asset"), ("2000", "Payables", "liability"), ("3000", "Equity", "equity"), ("9100", "Grants", "income")])

    def test_a_bad_chart_is_refused(self):
        for bad in ("code,name\n1000,Cash\n", CHART + "1000,Cash again,asset\n", CHART + "4000,Sales,revenue\n", CHART + "40a0,Sales,income\n"):
            with self.assertRaises(ValueError, msg=bad):
                accounts.load_chart(bad)

    def test_the_cli_reads_a_chart_file(self):
        lines, _ = run_cli({"j.csv": JOURNAL, "c.csv": CHART}, "balance", "j.csv", "--chart", "c.csv")
        self.assertTrue(any(l.split()[:2] == ["9100", "Grants"] and "250.00" in l for l in lines), lines)
        self.assertFalse(any(l.startswith("5000") for l in lines), "only the file's accounts")
        lines, _ = run_cli({"j.csv": sample_csv()}, "statement", "1000", "j.csv")
        self.assertEqual(len(lines), 4)

    def test_the_chart_command(self):
        lines, _ = run_cli({"c.csv": CHART}, "chart", "--chart", "c.csv")
        self.assertEqual([l.split() for l in lines], [["1000", "Cash", "asset"], ["2000", "Payables", "liability"],
                                                      ["3000", "Equity", "equity"], ["9100", "Grants", "income"]])
        lines, _ = run_cli({}, "chart")
        self.assertEqual(len(lines), 7)
        self.assertEqual(lines[0].split(), ["1000", "Cash", "asset"])


if __name__ == "__main__":
    unittest.main()
