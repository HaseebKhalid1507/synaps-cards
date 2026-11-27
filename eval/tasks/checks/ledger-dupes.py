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
    def test_a_second_import_skips_everything(self):
        from ledger.importer import import_into

        j = Journal(default_chart())
        self.assertEqual(import_into(j, sample_csv()), (5, 0))
        self.assertEqual(import_into(j, sample_csv()), (0, 5))
        self.assertEqual(len(j.entries), 5)

    def test_a_changed_entry_is_new(self):
        from ledger.importer import import_into

        j = Journal(default_chart())
        import_into(j, sample_csv())
        changed = sample_csv().replace("Invoice 17 paid", "Invoice 17 paid late")
        self.assertEqual(import_into(j, changed), (1, 4))


if __name__ == "__main__":
    unittest.main()
