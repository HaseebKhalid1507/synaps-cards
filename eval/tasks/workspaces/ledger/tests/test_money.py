import unittest

from ledger import money


class Money(unittest.TestCase):
    def test_parse(self):
        self.assertEqual(money.parse("12.50"), 1250)
        self.assertEqual(money.parse("1,234.5"), 123450)
        self.assertEqual(money.parse("7"), 700)
        self.assertEqual(money.parse("-3.05"), -305)

    def test_parse_refuses_what_isnt_an_amount(self):
        for bad in ("", "12.505", "abc", "1,23.00"):
            with self.assertRaises(ValueError, msg=bad):
                money.parse(bad)

    def test_format(self):
        self.assertEqual(money.format(1250), "12.50")
        self.assertEqual(money.format(123400), "1,234.00")
        self.assertEqual(money.format(0), "0.00")

    def test_split(self):
        self.assertEqual(money.split(100, 3), [34, 33, 33])
        self.assertEqual(sum(money.split(1001, 7)), 1001)
