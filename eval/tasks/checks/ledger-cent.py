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
    def test_one_cent_off_is_refused(self):
        j = journal()
        with self.assertRaises(ValueError):
            j.post(Entry(d("2026-01-01"), "x", [Line("1000", 100), Line("3000", -99)]))
        with self.assertRaises(ValueError):
            j.post(Entry(d("2026-01-01"), "x", [Line("1000", -100), Line("3000", 101)]))

    def test_a_balanced_entry_posts(self):
        j = journal()
        j.post(Entry(d("2026-01-01"), "x", [Line("1000", 100), Line("3000", -100)]))
        self.assertEqual(len(j.entries), 1)


if __name__ == "__main__":
    unittest.main()
