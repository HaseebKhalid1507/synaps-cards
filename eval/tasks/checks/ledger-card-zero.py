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
    def test_zero_balances_are_left_out(self):
        rows = report.trial_balance(journal(*SAMPLE))
        self.assertEqual([r[0] for r in rows], ["1000", "3000", "4000", "5000"])


if __name__ == "__main__":
    unittest.main()
