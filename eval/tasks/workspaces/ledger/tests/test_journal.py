import unittest

from ledger.journal import Entry, Line
from tests.helpers import SAMPLE, d, journal


class Journal(unittest.TestCase):
    def test_balances_by_kind(self):
        j = journal(*SAMPLE)
        self.assertEqual(j.balance("1000"), 500000 - 240000 + 80000)
        self.assertEqual(j.balance("4000"), 80000)
        self.assertEqual(j.balance("1100"), 0)
        self.assertEqual(j.balance("1000", until=d("2026-01-31")), 380000)

    def test_post_refuses_an_unknown_account_and_one_line(self):
        j = journal()
        with self.assertRaises(KeyError):
            j.post(Entry(d("2026-01-01"), "x", [Line("9999", 1), Line("1000", -1)]))
        with self.assertRaises(ValueError):
            j.post(Entry(d("2026-01-01"), "x", [Line("1000", 0)]))

    def test_post_refuses_an_entry_that_doesnt_balance(self):
        j = journal()
        with self.assertRaises(ValueError):
            j.post(Entry(d("2026-01-01"), "x", [Line("1000", 100), Line("3000", -90)]))
