import unittest

from ledger.importer import read_csv

CSV = """entry,date,memo,account,amount
1,2026-01-05,Rent for January,5000,1200.00

1,2026-01-05,Rent for January,1000,-1200.00
2,2026-01-06,Supplies,5100,45.10
2,2026-01-06,Supplies,1000,-45.10
"""


class Importer(unittest.TestCase):
    def test_rows_group_into_entries(self):
        entries = read_csv(CSV)
        self.assertEqual([e.memo for e in entries], ["Rent for January", "Supplies"])
        self.assertEqual([len(e.lines) for e in entries], [2, 2])
        self.assertEqual(entries[1].lines[0].cents, 4510)

    def test_a_missing_column_is_named(self):
        with self.assertRaisesRegex(ValueError, "amount"):
            read_csv("entry,date,memo,account\n")
