import unittest

from ledger import report
from tests.helpers import SAMPLE, journal


class Reports(unittest.TestCase):
    def test_trial_balance_rows(self):
        rows = report.trial_balance(journal(*SAMPLE))
        self.assertEqual(rows[0], ("1000", "Cash", 340000))
        self.assertEqual(len(rows), 7)

    def test_statement_runs_a_balance(self):
        rows = report.statement(journal(*SAMPLE), "5000")
        self.assertEqual([r[3] for r in rows], [120000, 240000])

    def test_render(self):
        self.assertEqual(report.render([("1000", "Cash", 1250)]), "1000  Cash  12.50")
