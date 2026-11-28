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
    def test_close(self):
        j = journal(*SAMPLE)
        e = j.close_year(2026)
        self.assertEqual((e.date, e.memo), (d("2026-12-31"), "Close 2026"))
        for code in ("4000", "5000", "5100"):
            self.assertEqual(j.balance(code), 0, code)
        self.assertEqual(j.balance("3000"), 340000)
        self.assertEqual(j.balance("1000"), 340000)

    def test_twice_is_refused(self):
        j = journal(*SAMPLE)
        j.close_year(2026)
        with self.assertRaises(ValueError):
            j.close_year(2026)

    def test_nothing_to_close(self):
        self.assertIsNone(journal().close_year(2026))


if __name__ == "__main__":
    unittest.main()
